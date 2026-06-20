use crate::types::{Signal, SignalType};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

pub struct SignalStore {
    conn: Connection,
}

impl SignalStore {
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
            CREATE TABLE IF NOT EXISTS signals (
                id TEXT PRIMARY KEY,
                from_agent TEXT NOT NULL,
                to_agent TEXT NOT NULL,
                thread_id TEXT,
                reply_to TEXT,
                signal_type TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                read_at TEXT,
                expires_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_signals_to ON signals(to_agent);
            CREATE INDEX IF NOT EXISTS idx_signals_thread ON signals(thread_id);
            CREATE INDEX IF NOT EXISTS idx_signals_type ON signals(signal_type);
            CREATE INDEX IF NOT EXISTS idx_signals_read ON signals(read_at);
            ",
        )?;
        Ok(())
    }

    pub fn send(&self, signal: &Signal) -> Result<()> {
        let metadata = serde_json::to_string(&signal.metadata)?;
        self.conn.execute(
            "INSERT INTO signals (id, from_agent, to_agent, thread_id, reply_to, signal_type, content, metadata, created_at, read_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                signal.id, signal.from, signal.to, signal.thread_id, signal.reply_to,
                signal.signal_type.to_string(), signal.content, metadata,
                signal.created_at.to_rfc3339(),
                signal.read_at.map(|dt| dt.to_rfc3339()),
                signal.expires_at.map(|dt| dt.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn read_signals(
        &self,
        agent_id: &str,
        unread_only: bool,
        thread_id: Option<&str>,
        signal_type: Option<SignalType>,
    ) -> Result<Vec<Signal>> {
        let mut sql = "SELECT * FROM signals WHERE to_agent = ?1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(agent_id.to_string())];
        let mut param_idx = 2;

        if unread_only {
            sql.push_str(&format!(" AND read_at IS NULL"));
        }
        if let Some(tid) = thread_id {
            sql.push_str(&format!(" AND thread_id = ?{}", param_idx));
            params_vec.push(Box::new(tid.to_string()));
            param_idx += 1;
        }
        if let Some(st) = signal_type {
            sql.push_str(&format!(" AND signal_type = ?{}", param_idx));
            params_vec.push(Box::new(st.to_string()));
            param_idx += 1;
        }
        sql.push_str(" ORDER BY created_at DESC");

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs.iter()), |row| {
            self.row_to_signal(row)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn mark_read(&self, signal_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE signals SET read_at = ?1 WHERE id = ?2",
            params![now, signal_id],
        )?;
        Ok(())
    }

    pub fn get_threads(&self, agent_id: &str) -> Result<HashMap<String, ThreadSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT thread_id, COUNT(*) as count, GROUP_CONCAT(DISTINCT from_agent) as participants FROM signals WHERE to_agent = ?1 AND thread_id IS NOT NULL GROUP BY thread_id",
        )?;
        let rows = stmt.query_map(params![agent_id], |row| {
            let thread_id: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let participants: String = row.get(2)?;
            Ok((
                thread_id,
                ThreadSummary {
                    count: count as usize,
                    participants: participants.split(',').map(|s| s.to_string()).collect(),
                },
            ))
        })?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(Into::into)
    }
}

#[derive(Debug)]
pub struct ThreadSummary {
    pub count: usize,
    pub participants: Vec<String>,
}

impl SignalStore {
    fn row_to_signal(&self, row: &rusqlite::Row) -> rusqlite::Result<Signal> {
        let signal_type: crate::types::SignalType = row
            .get::<_, String>("signal_type")?
            .parse()
            .map_err(|e: String| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e,
                )))
            })?;
        let metadata: HashMap<String, serde_json::Value> =
            serde_json::from_str(&row.get::<_, String>("metadata")?)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let created_at_str: String = row.get("created_at")?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok(Signal {
            id: row.get("id")?,
            from: row.get("from_agent")?,
            to: row.get("to_agent")?,
            thread_id: row.get("thread_id")?,
            reply_to: row.get("reply_to")?,
            signal_type,
            content: row.get("content")?,
            metadata,
            created_at,
            read_at: row
                .get::<_, Option<String>>("read_at")?
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
            expires_at: row
                .get::<_, Option<String>>("expires_at")?
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_store() -> SignalStore {
        SignalStore::open(Path::new(":memory:")).unwrap()
    }

    fn test_signal(id: &str, from: &str, to: &str) -> Signal {
        Signal {
            id: id.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            thread_id: None,
            reply_to: None,
            signal_type: SignalType::Info,
            content: "Test signal".to_string(),
            metadata: HashMap::new(),
            created_at: Utc::now(),
            read_at: None,
            expires_at: None,
        }
    }

    #[test]
    fn test_send_and_read_signal() {
        let store = test_store();
        let signal = test_signal("sig-1", "Alice", "Bob");
        store.send(&signal).unwrap();

        let signals = store.read_signals("Bob", true, None, None).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].id, "sig-1");
    }

    #[test]
    fn test_read_signals_filters_by_type() {
        let store = test_store();
        let mut s1 = test_signal("sig-1", "Alice", "Bob");
        s1.signal_type = SignalType::Alert;
        store.send(&s1).unwrap();

        let mut s2 = test_signal("sig-2", "Alice", "Bob");
        s2.signal_type = SignalType::Info;
        store.send(&s2).unwrap();

        let alerts = store
            .read_signals("Bob", true, None, Some(SignalType::Alert))
            .unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].signal_type, SignalType::Alert);
    }

    #[test]
    fn test_read_signals_by_thread() {
        let store = test_store();
        let mut s1 = test_signal("sig-1", "Alice", "Bob");
        s1.thread_id = Some("thread-1".to_string());
        store.send(&s1).unwrap();

        let mut s2 = test_signal("sig-2", "Alice", "Bob");
        s2.thread_id = Some("thread-2".to_string());
        store.send(&s2).unwrap();

        let thread_signals = store
            .read_signals("Bob", true, Some("thread-1"), None)
            .unwrap();
        assert_eq!(thread_signals.len(), 1);
        assert_eq!(thread_signals[0].thread_id, Some("thread-1".to_string()));
    }

    #[test]
    fn test_mark_read() {
        let store = test_store();
        store.send(&test_signal("sig-1", "Alice", "Bob")).unwrap();

        let unread = store.read_signals("Bob", true, None, None).unwrap();
        assert_eq!(unread.len(), 1);

        store.mark_read("sig-1").unwrap();
        let unread_after = store.read_signals("Bob", true, None, None).unwrap();
        assert_eq!(unread_after.len(), 0);
    }

    #[test]
    fn test_get_threads() {
        let store = test_store();
        let mut s1 = test_signal("sig-1", "Alice", "Bob");
        s1.thread_id = Some("thread-1".to_string());
        store.send(&s1).unwrap();

        let mut s2 = test_signal("sig-2", "Charlie", "Bob");
        s2.thread_id = Some("thread-1".to_string());
        store.send(&s2).unwrap();

        let mut s3 = test_signal("sig-3", "Alice", "Bob");
        s3.thread_id = Some("thread-2".to_string());
        store.send(&s3).unwrap();

        let threads = store.get_threads("Bob").unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads["thread-1"].count, 2);
        assert_eq!(threads["thread-2"].count, 1);
    }
}
