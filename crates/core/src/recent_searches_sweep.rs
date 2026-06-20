//! Recent Searches Sweep — periodic cleanup of old search entries.
//!
//! Port of upstream `recent-searches-sweep.ts` from agentmemory.
//! Maintains a SQLite table of recent searches and prunes entries
//! older than a configurable TTL.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Default TTL for search entries in days.
const DEFAULT_SEARCH_TTL_DAYS: u64 = 30;

/// A single search record stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchEntry {
    /// Unique ID for this search entry.
    pub id: String,
    /// The raw search query text.
    pub query: String,
    /// Wing/project scope (empty if un-scoped).
    pub wing: String,
    /// Number of results returned.
    pub result_count: usize,
    /// When the search was performed.
    pub created_at: DateTime<Utc>,
}

/// Open or create the search history database.
pub fn open_search_db(palace_path: &Path) -> Result<Connection> {
    let db_dir = palace_path.join("coordination");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("search_history.db");
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS search_history (
            id TEXT PRIMARY KEY,
            query TEXT NOT NULL,
            wing TEXT NOT NULL DEFAULT '',
            result_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_search_history_created
            ON search_history(created_at);",
    )?;
    Ok(conn)
}

/// Record a search entry in the database.
pub fn record_search(
    conn: &Connection,
    id: &str,
    query: &str,
    wing: &str,
    result_count: usize,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO search_history (id, query, wing, result_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            id,
            query,
            wing,
            result_count as i64,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Sweep old search entries from the database.
///
/// Deletes entries older than `ttl_days`. Returns the number of
/// entries removed.
pub fn sweep_search_history(conn: &Connection, ttl_days: u64) -> Result<usize> {
    let ttl = chrono::Duration::days(ttl_days as i64);
    let cutoff = Utc::now() - ttl;
    let cutoff_str = cutoff.to_rfc3339();

    let deleted = conn.execute(
        "DELETE FROM search_history WHERE created_at < ?1",
        rusqlite::params![cutoff_str],
    )?;
    Ok(deleted)
}

/// Count total search entries in the database.
pub fn count_search_entries(conn: &Connection) -> Result<usize> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM search_history", [], |row| row.get(0))?;
    Ok(count as usize)
}

/// List recent search entries, newest first.
pub fn list_recent_searches(conn: &Connection, limit: usize) -> Result<Vec<SearchEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, query, wing, result_count, created_at
         FROM search_history
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
        let created_at_str: String = row.get(4)?;
        Ok(SearchEntry {
            id: row.get(0)?,
            query: row.get(1)?,
            wing: row.get(2)?,
            result_count: row.get::<_, i64>(3)? as usize,
            created_at: created_at_str
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now()),
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

/// Run the full sweep cycle: open DB, prune old entries, return count removed.
///
/// This is the entry point used by background tasks.
pub fn run_sweep(palace_path: &Path, ttl_days: Option<u64>) -> Result<usize> {
    let conn = open_search_db(palace_path)?;
    let ttl = ttl_days.unwrap_or(DEFAULT_SEARCH_TTL_DAYS);
    sweep_search_history(&conn, ttl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_db() -> (TempDir, Connection) {
        let dir = TempDir::new().expect("temp dir");
        let conn = open_search_db(dir.path()).expect("open db");
        (dir, conn)
    }

    #[test]
    fn test_open_search_db_creates_table() {
        let (_dir, conn) = setup_db();
        let count = count_search_entries(&conn).expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn test_record_and_list_searches() {
        let (_dir, conn) = setup_db();
        record_search(&conn, "s1", "rust async", "tech", 5).expect("record s1");
        record_search(&conn, "s2", "tokio spawn", "tech", 3).expect("record s2");

        let entries = list_recent_searches(&conn, 10).expect("list");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].query, "tokio spawn");
        assert_eq!(entries[1].query, "rust async");
    }

    #[test]
    fn test_sweep_removes_old_entries() {
        let (_dir, conn) = setup_db();

        // Insert an entry with a manually old timestamp
        let old_id = "s-old";
        let old_time = (Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        conn.execute(
            "INSERT INTO search_history (id, query, wing, result_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![old_id, "old query", "", 0, old_time],
        )
        .expect("insert old");

        record_search(&conn, "s-new", "recent search", "", 2).expect("record new");

        let removed = sweep_search_history(&conn, 30).expect("sweep");
        assert_eq!(removed, 1);

        let remaining = count_search_entries(&conn).expect("count");
        assert_eq!(remaining, 1);
    }

    #[test]
    fn test_run_sweep_integration() {
        let dir = TempDir::new().expect("temp dir");
        let palace_path = dir.path().join("palace");

        // Pre-populate old entry
        let conn = open_search_db(&palace_path).expect("open db");
        let old_time = (Utc::now() - chrono::Duration::days(60)).to_rfc3339();
        conn.execute(
            "INSERT INTO search_history (id, query, wing, result_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["s-old", "ancient query", "", 0, old_time],
        )
        .expect("insert old");
        drop(conn);

        let removed = run_sweep(&palace_path, Some(30)).expect("run_sweep");
        assert_eq!(removed, 1);
    }
}
