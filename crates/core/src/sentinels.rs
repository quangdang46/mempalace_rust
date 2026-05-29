use crate::types::Sentinel;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

pub struct SentinelStore {
    conn: Connection,
}

impl SentinelStore {
    pub fn new(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sentinels (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                sentinel_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'watching',
                config TEXT NOT NULL DEFAULT '{}',
                condition TEXT NOT NULL DEFAULT '',
                action TEXT NOT NULL DEFAULT '',
                active INTEGER NOT NULL DEFAULT 1,
                linked_action_ids TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                expires_at TEXT,
                triggered_at TEXT,
                last_triggered TEXT,
                result TEXT
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn create(&self, name: &str, sentinel_type: &str, config: Option<&str>, linked_action_ids: Vec<String>, expires_in_ms: Option<i64>) -> Result<Sentinel> {
        let now = Utc::now();
        let sentinel = Sentinel {
            id: format!("snl-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()),
            name: name.trim().to_string(),
            sentinel_type: crate::types::SentinelType::Custom,
            status: "watching".to_string(),
            config: config.map(|c| serde_json::from_str(c).unwrap_or_default()).unwrap_or_default(),
            condition: String::new(),
            action: String::new(),
            active: true,
            linked_action_ids,
            created_at: now,
            expires_at: expires_in_ms.map(|ms| now + chrono::Duration::milliseconds(ms)),
            triggered_at: None,
            last_triggered: None,
            result: None,
        };
        self.insert(&sentinel)?;
        Ok(sentinel)
    }

    pub fn trigger(&self, id: &str, result: Option<serde_json::Value>) -> Result<Sentinel> {
        let mut sentinel = self.get(id)?.ok_or_else(|| anyhow::anyhow!("Sentinel not found"))?;
        if sentinel.status != "watching" {
            return Err(anyhow::anyhow!("Sentinel already {}", sentinel.status));
        }
        sentinel.status = "triggered".to_string();
        sentinel.triggered_at = Some(Utc::now());
        sentinel.result = result;
        self.update(&sentinel)?;
        Ok(sentinel)
    }

    pub fn cancel(&self, id: &str) -> Result<Sentinel> {
        let mut sentinel = self.get(id)?.ok_or_else(|| anyhow::anyhow!("Sentinel not found"))?;
        if sentinel.status != "watching" {
            return Err(anyhow::anyhow!("Cannot cancel sentinel with status {}", sentinel.status));
        }
        sentinel.status = "cancelled".to_string();
        self.update(&sentinel)?;
        Ok(sentinel)
    }

    pub fn list(&self, status: Option<&str>, sentinel_type: Option<&str>) -> Result<Vec<Sentinel>> {
        let mut sentinels = self.load_all()?;
        if let Some(s) = status {
            let s_owned = s.to_string();
            sentinels.retain(|s| s.status == s_owned);
        }
        if let Some(t) = sentinel_type {
            let t_lower = t.to_lowercase();
            sentinels.retain(|s| format!("{:?}", s.sentinel_type).to_lowercase() == t_lower);
        }
        sentinels.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sentinels)
    }

    pub fn expire(&self) -> Result<usize> {
        let sentinels = self.load_all()?;
        let now = Utc::now();
        let mut expired = 0;

        for mut sentinel in sentinels {
            if sentinel.status != "watching" { continue; }
            if let Some(expires) = sentinel.expires_at {
                if expires <= now {
                    sentinel.status = "expired".to_string();
                    sentinel.triggered_at = Some(now);
                    self.update(&sentinel)?;
                    expired += 1;
                }
            }
        }

        Ok(expired)
    }

    pub fn get(&self, id: &str) -> Result<Option<Sentinel>> {
        let mut stmt = self.conn.prepare("SELECT * FROM sentinels WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_sentinel(row)?))
        } else {
            Ok(None)
        }
    }

    fn load_all(&self) -> Result<Vec<Sentinel>> {
        let mut stmt = self.conn.prepare("SELECT * FROM sentinels")?;
        let rows: Vec<rusqlite::Result<Sentinel>> = stmt.query_map([], |row| row_to_sentinel(row))?.collect();
        rows.into_iter().map(|r| r.map_err(|e| anyhow::anyhow!(e))).collect()
    }

    fn insert(&self, sentinel: &Sentinel) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sentinels (id, name, sentinel_type, status, config, condition, action, active,
                                    linked_action_ids, created_at, expires_at, triggered_at, last_triggered, result)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                sentinel.id, sentinel.name, format!("{:?}", sentinel.sentinel_type), sentinel.status,
                serde_json::to_string(&sentinel.config)?, sentinel.condition, sentinel.action,
                sentinel.active as i32, serde_json::to_string(&sentinel.linked_action_ids)?,
                sentinel.created_at.to_rfc3339(), sentinel.expires_at.map(|d| d.to_rfc3339()),
                sentinel.triggered_at.map(|d| d.to_rfc3339()),
                sentinel.last_triggered.map(|d| d.to_rfc3339()),
                sentinel.result.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default())
            ],
        )?;
        Ok(())
    }

    fn update(&self, sentinel: &Sentinel) -> Result<()> {
        self.conn.execute(
            "UPDATE sentinels SET name=?2, sentinel_type=?3, status=?4, config=?5,
                                condition=?6, action=?7, active=?8, linked_action_ids=?9,
                                created_at=?10, expires_at=?11, triggered_at=?12,
                                last_triggered=?13, result=?14 WHERE id=?1",
            params![
                sentinel.id, sentinel.name, format!("{:?}", sentinel.sentinel_type), sentinel.status,
                serde_json::to_string(&sentinel.config)?, sentinel.condition, sentinel.action,
                sentinel.active as i32, serde_json::to_string(&sentinel.linked_action_ids)?,
                sentinel.created_at.to_rfc3339(), sentinel.expires_at.map(|d| d.to_rfc3339()),
                sentinel.triggered_at.map(|d| d.to_rfc3339()),
                sentinel.last_triggered.map(|d| d.to_rfc3339()),
                sentinel.result.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default())
            ],
        )?;
        Ok(())
    }
}

fn row_to_sentinel(row: &rusqlite::Row<'_>) -> rusqlite::Result<Sentinel> {
    let config: String = row.get(4)?;
    let linked_action_ids: String = row.get(8)?;
    Ok(Sentinel {
        id: row.get(0)?,
        name: row.get(1)?,
        sentinel_type: crate::types::SentinelType::Custom,
        status: row.get(3)?,
        config: serde_json::from_str(&config).unwrap_or_default(),
        condition: row.get(5)?,
        action: row.get(6)?,
        active: row.get::<_, i32>(7)? != 0,
        linked_action_ids: serde_json::from_str(&linked_action_ids).unwrap_or_default(),
        created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(9)?).unwrap().with_timezone(&Utc),
        expires_at: row.get::<_, Option<String>>(10)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        triggered_at: row.get::<_, Option<String>>(11)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        last_triggered: row.get::<_, Option<String>>(12)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        result: row.get::<_, Option<String>>(13)?.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SentinelStore {
        SentinelStore::new(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn test_create_sentinel() {
        let store = test_store();
        let sentinel = store.create("Test sentinel", "timer", Some(r#"{"durationMs": 5000}"#), vec![], Some(60000)).unwrap();
        assert!(sentinel.id.starts_with("snl-"));
        assert_eq!(sentinel.status, "watching");
        assert!(sentinel.expires_at.is_some());
    }

    #[test]
    fn test_trigger_sentinel() {
        let store = test_store();
        let sentinel = store.create("Test", "timer", None, vec![], None).unwrap();
        let triggered = store.trigger(&sentinel.id, Some(serde_json::json!({"reason": "manual"}))).unwrap();
        assert_eq!(triggered.status, "triggered");
        assert!(triggered.triggered_at.is_some());
    }

    #[test]
    fn test_cancel_sentinel() {
        let store = test_store();
        let sentinel = store.create("Test", "timer", None, vec![], None).unwrap();
        let cancelled = store.cancel(&sentinel.id).unwrap();
        assert_eq!(cancelled.status, "cancelled");
    }

    #[test]
    fn test_list_sentinels() {
        let store = test_store();
        store.create("Active", "timer", None, vec![], None).unwrap();
        store.create("Another", "threshold", None, vec![], None).unwrap();
        let all = store.list(None, None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_filters_by_status() {
        let store = test_store();
        let s1 = store.create("Active", "timer", None, vec![], None).unwrap();
        let s2 = store.create("To trigger", "timer", None, vec![], None).unwrap();
        store.trigger(&s2.id, None).unwrap();
        let watching = store.list(Some("watching"), None).unwrap();
        assert_eq!(watching.len(), 1);
        assert_eq!(watching[0].id, s1.id);
    }

    #[test]
    fn test_expire_sentinels() {
        let store = test_store();
        store.create("Expired", "timer", None, vec![], Some(-1000)).unwrap();
        store.create("Active", "timer", None, vec![], Some(999999999)).unwrap();
        let expired = store.expire().unwrap();
        assert_eq!(expired, 1);
    }

    #[test]
    fn test_trigger_non_watching_fails() {
        let store = test_store();
        let sentinel = store.create("Test", "timer", None, vec![], None).unwrap();
        store.trigger(&sentinel.id, None).unwrap();
        let result = store.trigger(&sentinel.id, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_nonexistent() {
        let store = test_store();
        let result = store.get("nonexistent").unwrap();
        assert!(result.is_none());
    }
}
