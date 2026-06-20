use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use std::path::Path;

pub const DEFAULT_TTL_MINUTES: i64 = 10;
pub const MAX_TTL_MINUTES: i64 = 60;

#[derive(Debug, Clone)]
pub struct Lease {
    pub id: String,
    pub action_id: String,
    pub agent_id: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub result: Option<String>,
}

pub struct LeaseStore {
    conn: Connection,
}

impl LeaseStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.init_db()?;
        Ok(store)
    }

    fn init_db(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS leases (
                id TEXT PRIMARY KEY,
                action_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                acquired_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                result TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_leases_action ON leases(action_id);
            CREATE INDEX IF NOT EXISTS idx_leases_agent ON leases(agent_id);
            CREATE INDEX IF NOT EXISTS idx_leases_expires ON leases(expires_at);
            ",
        )?;
        Ok(())
    }

    pub fn acquire(
        &self,
        action_id: &str,
        agent_id: &str,
        ttl_minutes: Option<i64>,
    ) -> Result<Lease> {
        let existing = self.get_active_lease(action_id)?;
        if let Some(lease) = existing {
            if lease.agent_id != agent_id {
                return Err(anyhow!(
                    "Action {} is leased by {}",
                    action_id,
                    lease.agent_id
                ));
            }
            return self.renew(
                &lease.id,
                Duration::minutes(ttl_minutes.unwrap_or(DEFAULT_TTL_MINUTES)),
            );
        }

        let ttl = ttl_minutes
            .unwrap_or(DEFAULT_TTL_MINUTES)
            .min(MAX_TTL_MINUTES);
        let now = Utc::now();
        let expires = now + Duration::minutes(ttl);
        let id = format!("lease_{}_{}", action_id, agent_id);

        self.conn.execute(
            "INSERT INTO leases (id, action_id, agent_id, acquired_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, action_id, agent_id, now.to_rfc3339(), expires.to_rfc3339()],
        )?;

        Ok(Lease {
            id,
            action_id: action_id.to_string(),
            agent_id: agent_id.to_string(),
            acquired_at: now,
            expires_at: expires,
            result: None,
        })
    }

    pub fn release(&self, lease_id: &str, result: Option<&str>) -> Result<()> {
        if let Some(r) = result {
            self.conn.execute(
                "UPDATE leases SET result = ?1 WHERE id = ?2",
                params![r, lease_id],
            )?;
        }
        self.conn
            .execute("DELETE FROM leases WHERE id = ?1", params![lease_id])?;
        Ok(())
    }

    pub fn renew(&self, lease_id: &str, extend: Duration) -> Result<Lease> {
        let lease = self
            .get_lease(lease_id)?
            .ok_or_else(|| anyhow!("Lease {} not found", lease_id))?;

        let now = Utc::now();
        let current_expiry = lease.expires_at;
        let base = if current_expiry > now {
            current_expiry
        } else {
            now
        };
        let new_expiry = base + extend;

        self.conn.execute(
            "UPDATE leases SET expires_at = ?1 WHERE id = ?2",
            params![new_expiry.to_rfc3339(), lease_id],
        )?;

        Ok(Lease {
            expires_at: new_expiry,
            ..lease
        })
    }

    pub fn cleanup(&self) -> Result<usize> {
        let now = Utc::now().to_rfc3339();
        let changed = self
            .conn
            .execute("DELETE FROM leases WHERE expires_at < ?1", params![now])?;
        Ok(changed)
    }

    pub fn get_active_lease(&self, action_id: &str) -> Result<Option<Lease>> {
        let now = Utc::now().to_rfc3339();
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM leases WHERE action_id = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![action_id, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_lease(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_lease(&self, lease_id: &str) -> Result<Option<Lease>> {
        let mut stmt = self.conn.prepare("SELECT * FROM leases WHERE id = ?1")?;
        let mut rows = stmt.query(params![lease_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_lease(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_agent_leases(&self, agent_id: &str) -> Result<Vec<Lease>> {
        let now = Utc::now().to_rfc3339();
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM leases WHERE agent_id = ?1 AND expires_at > ?2")?;
        let rows = stmt.query_map(params![agent_id, now], |row| self.row_to_lease(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn row_to_lease(&self, row: &rusqlite::Row) -> rusqlite::Result<Lease> {
        let acquired_at_str: String = row.get("acquired_at")?;
        let expires_at_str: String = row.get("expires_at")?;
        Ok(Lease {
            id: row.get("id")?,
            action_id: row.get("action_id")?,
            agent_id: row.get("agent_id")?,
            acquired_at: chrono::DateTime::parse_from_rfc3339(&acquired_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
                })?,
            expires_at: chrono::DateTime::parse_from_rfc3339(&expires_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
                })?,
            result: row.get("result")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> LeaseStore {
        LeaseStore::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn test_acquire_lease() {
        let store = test_store();
        let lease = store.acquire("a-1", "agent-1", None).unwrap();
        assert_eq!(lease.action_id, "a-1");
        assert_eq!(lease.agent_id, "agent-1");
    }

    #[test]
    fn test_acquire_conflict() {
        let store = test_store();
        store.acquire("a-1", "agent-1", None).unwrap();
        let result = store.acquire("a-1", "agent-2", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_acquire_same_agent_renews() {
        let store = test_store();
        let lease1 = store.acquire("a-1", "agent-1", Some(5)).unwrap();
        let lease2 = store.acquire("a-1", "agent-1", Some(20)).unwrap();
        assert_eq!(lease1.id, lease2.id);
    }

    #[test]
    fn test_release_lease() {
        let store = test_store();
        let lease = store.acquire("a-1", "agent-1", None).unwrap();
        store.release(&lease.id, Some("completed")).unwrap();
        let existing = store.get_active_lease("a-1").unwrap();
        assert!(existing.is_none());
    }

    #[test]
    fn test_renew_lease() {
        let store = test_store();
        let lease = store.acquire("a-1", "agent-1", Some(5)).unwrap();
        let renewed = store.renew(&lease.id, Duration::minutes(30)).unwrap();
        assert!(renewed.expires_at > lease.expires_at);
    }

    #[test]
    fn test_cleanup_expired() {
        let store = test_store();
        store.acquire("a-1", "agent-1", Some(1)).unwrap();

        let expired = Utc::now() - Duration::minutes(5);
        store
            .conn
            .execute(
                "UPDATE leases SET expires_at = ?1 WHERE action_id = ?2",
                params![expired.to_rfc3339(), "a-1"],
            )
            .unwrap();

        let cleaned = store.cleanup().unwrap();
        assert_eq!(cleaned, 1);
    }

    #[test]
    fn test_get_agent_leases() {
        let store = test_store();
        store.acquire("a-1", "agent-1", None).unwrap();
        store.acquire("a-2", "agent-1", None).unwrap();
        store.acquire("a-3", "agent-2", None).unwrap();

        let leases = store.get_agent_leases("agent-1").unwrap();
        assert_eq!(leases.len(), 2);
    }

    #[test]
    fn test_max_ttl_cap() {
        let store = test_store();
        let lease = store.acquire("a-1", "agent-1", Some(120)).unwrap();
        let expected = lease.acquired_at + Duration::minutes(MAX_TTL_MINUTES);
        assert!((lease.expires_at - expected).num_seconds().abs() < 2);
    }
}
