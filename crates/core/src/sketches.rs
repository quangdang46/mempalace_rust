use crate::types::{Action, ActionEdge, Sketch};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashSet;

pub struct SketchStore {
    conn: Connection,
}

impl SketchStore {
    pub fn new(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sketches (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'active',
                action_ids TEXT NOT NULL DEFAULT '[]',
                project TEXT,
                created_at TEXT NOT NULL,
                expires_at TEXT,
                promoted_at TEXT,
                discarded_at TEXT
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn create(&self, title: &str, description: &str, expires_in_ms: Option<i64>, project: Option<&str>) -> Result<Sketch> {
        let now = Utc::now();
        let sketch = Sketch {
            id: format!("sk-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()),
            title: title.trim().to_string(),
            description: description.trim().to_string(),
            status: "active".to_string(),
            action_ids: vec![],
            project: project.map(String::from),
            created_at: now,
            expires_at: expires_in_ms.map(|ms| now + chrono::Duration::milliseconds(ms)),
            promoted_at: None,
            discarded_at: None,
        };
        self.insert(&sketch)?;
        Ok(sketch)
    }

    pub fn list(&self, status: Option<&str>, project: Option<&str>) -> Result<Vec<Sketch>> {
        let mut sketches = self.load_all()?;
        if let Some(s) = status {
            sketches.retain(|sk| sk.status == s);
        }
        if let Some(p) = project {
            sketches.retain(|sk| sk.project.as_deref() == Some(p));
        }
        sketches.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sketches)
    }

    pub fn get(&self, id: &str) -> Result<Option<Sketch>> {
        let mut stmt = self.conn.prepare("SELECT * FROM sketches WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_sketch(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn promote(&self, id: &str, project: Option<&str>) -> Result<Vec<String>> {
        let mut sketch = self.get(id)?.ok_or_else(|| anyhow::anyhow!("Sketch not found"))?;
        if sketch.status != "active" {
            return Err(anyhow::anyhow!("Sketch is not active"));
        }

        let promoted_ids = sketch.action_ids.clone();
        sketch.status = "promoted".to_string();
        sketch.promoted_at = Some(Utc::now());
        self.update(&sketch)?;

        if let Some(proj) = project {
            for action_id in &promoted_ids {
                self.conn.execute(
                    "UPDATE actions SET project = ?1, sketch_id = NULL WHERE id = ?2",
                    params![proj, action_id],
                )?;
            }
        }

        Ok(promoted_ids)
    }

    pub fn discard(&self, id: &str) -> Result<usize> {
        let mut sketch = self.get(id)?.ok_or_else(|| anyhow::anyhow!("Sketch not found"))?;
        if sketch.status != "active" {
            return Err(anyhow::anyhow!("Sketch is not active"));
        }

        let count = sketch.action_ids.len();
        sketch.status = "discarded".to_string();
        sketch.discarded_at = Some(Utc::now());
        self.update(&sketch)?;
        Ok(count)
    }

    pub fn gc(&self) -> Result<usize> {
        let sketches = self.load_all()?;
        let now = Utc::now();
        let mut collected = 0;

        for sketch in sketches {
            if sketch.status != "active" { continue; }
            if let Some(expires) = sketch.expires_at {
                if expires > now { continue; }
            } else {
                continue;
            }

            let mut s = sketch;
            s.status = "discarded".to_string();
            s.discarded_at = Some(now);
            self.update(&s)?;
            collected += 1;
        }

        Ok(collected)
    }

    fn load_all(&self) -> Result<Vec<Sketch>> {
        let mut stmt = self.conn.prepare("SELECT * FROM sketches")?;
        let rows: Vec<rusqlite::Result<Sketch>> = stmt.query_map([], |row| row_to_sketch(row))?.collect();
        rows.into_iter().map(|r| r.map_err(|e| anyhow::anyhow!(e))).collect()
    }

    fn insert(&self, sketch: &Sketch) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sketches (id, title, description, status, action_ids, project,
                                   created_at, expires_at, promoted_at, discarded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                sketch.id, sketch.title, sketch.description, sketch.status,
                serde_json::to_string(&sketch.action_ids)?, sketch.project,
                sketch.created_at.to_rfc3339(), sketch.expires_at.map(|d| d.to_rfc3339()),
                sketch.promoted_at.map(|d| d.to_rfc3339()),
                sketch.discarded_at.map(|d| d.to_rfc3339())
            ],
        )?;
        Ok(())
    }

    fn update(&self, sketch: &Sketch) -> Result<()> {
        self.conn.execute(
            "UPDATE sketches SET title=?2, description=?3, status=?4, action_ids=?5,
                                project=?6, created_at=?7, expires_at=?8,
                                promoted_at=?9, discarded_at=?10 WHERE id=?1",
            params![
                sketch.id, sketch.title, sketch.description, sketch.status,
                serde_json::to_string(&sketch.action_ids)?, sketch.project,
                sketch.created_at.to_rfc3339(), sketch.expires_at.map(|d| d.to_rfc3339()),
                sketch.promoted_at.map(|d| d.to_rfc3339()),
                sketch.discarded_at.map(|d| d.to_rfc3339())
            ],
        )?;
        Ok(())
    }
}

fn row_to_sketch(row: &rusqlite::Row<'_>) -> rusqlite::Result<Sketch> {
    let action_ids: String = row.get(4)?;
    Ok(Sketch {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        status: row.get(3)?,
        action_ids: serde_json::from_str(&action_ids).unwrap_or_default(),
        project: row.get(5)?,
        created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?).unwrap().with_timezone(&Utc),
        expires_at: row.get::<_, Option<String>>(7)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        promoted_at: row.get::<_, Option<String>>(8)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        discarded_at: row.get::<_, Option<String>>(9)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SketchStore {
        SketchStore::new(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn test_create_sketch() {
        let store = test_store();
        let sketch = store.create("Test sketch", "Description", Some(3600000), None).unwrap();
        assert!(sketch.id.starts_with("sk-"));
        assert_eq!(sketch.status, "active");
        assert!(sketch.expires_at.is_some());
    }

    #[test]
    fn test_list_sketches() {
        let store = test_store();
        store.create("Sketch A", "", None, Some("proj-a")).unwrap();
        store.create("Sketch B", "", None, Some("proj-b")).unwrap();
        let all = store.list(None, None).unwrap();
        assert_eq!(all.len(), 2);
        let proj_a = store.list(None, Some("proj-a")).unwrap();
        assert_eq!(proj_a.len(), 1);
    }

    #[test]
    fn test_promote_sketch() {
        let store = test_store();
        let sketch = store.create("Test", "", None, None).unwrap();
        let promoted = store.promote(&sketch.id, None).unwrap();
        assert!(promoted.is_empty());
        let updated = store.get(&sketch.id).unwrap().unwrap();
        assert_eq!(updated.status, "promoted");
    }

    #[test]
    fn test_discard_sketch() {
        let store = test_store();
        let sketch = store.create("Test", "", None, None).unwrap();
        let count = store.discard(&sketch.id).unwrap();
        assert_eq!(count, 0);
        let updated = store.get(&sketch.id).unwrap().unwrap();
        assert_eq!(updated.status, "discarded");
    }

    #[test]
    fn test_gc_expired_sketches() {
        let store = test_store();
        store.create("Expired", "", Some(-1000), None).unwrap();
        store.create("Active", "", Some(999999999), None).unwrap();
        let collected = store.gc().unwrap();
        assert_eq!(collected, 1);
    }

    #[test]
    fn test_list_filters_by_status() {
        let store = test_store();
        let s1 = store.create("Active", "", None, None).unwrap();
        let s2 = store.create("To promote", "", None, None).unwrap();
        store.promote(&s2.id, None).unwrap();
        let active = store.list(Some("active"), None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, s1.id);
    }

    #[test]
    fn test_promote_non_active_fails() {
        let store = test_store();
        let sketch = store.create("Test", "", None, None).unwrap();
        store.promote(&sketch.id, None).unwrap();
        let result = store.promote(&sketch.id, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_nonexistent() {
        let store = test_store();
        let result = store.get("nonexistent").unwrap();
        assert!(result.is_none());
    }
}
