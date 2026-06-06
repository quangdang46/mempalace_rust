//! File reservation system for workspace coordination between agents.
//!
//! Provides exclusive and shared file-level locks with glob pattern matching,
//! conflict detection, and TTL-based expiry.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Reservation mode: exclusive or shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReservationMode {
    /// Only one agent can hold this reservation.
    Exclusive,
    /// Multiple agents can hold shared reservations (read-only).
    Shared,
}

impl ReservationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReservationMode::Exclusive => "exclusive",
            ReservationMode::Shared => "shared",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "shared" | "non_exclusive" | "non-exclusive" | "observe" | "read" => {
                ReservationMode::Shared
            }
            _ => ReservationMode::Exclusive,
        }
    }
}

/// A file reservation held by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReservation {
    pub id: String,
    pub path_pattern: String,
    pub agent_id: String,
    pub mode: ReservationMode,
    pub reason: Option<String>,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
}

/// Conflict detection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationConflict {
    /// No conflict.
    None,
    /// Another agent holds an exclusive reservation on an overlapping path.
    ExclusiveConflict {
        holder: String,
        expires_at: DateTime<Utc>,
    },
    /// A shared reservation exists but an exclusive is requested.
    SharedWithExclusive { holder: String },
    /// The same agent already holds a reservation.
    SameAgent,
}

/// Heatmap entry for conflict prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReservationHeatmapEntry {
    pub path_pattern: String,
    pub active_count: usize,
    pub exclusive_count: usize,
    pub shared_count: usize,
    pub agents: Vec<String>,
}

/// File reservation store backed by SQLite.
pub struct FileReservationStore {
    conn: Connection,
}

impl FileReservationStore {
    /// Open or create the reservation store at the given path.
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
            CREATE TABLE IF NOT EXISTS file_reservations (
                id TEXT PRIMARY KEY,
                path_pattern TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                mode TEXT NOT NULL,
                reason TEXT,
                acquired_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                released_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_reservations_path ON file_reservations(path_pattern);
            CREATE INDEX IF NOT EXISTS idx_reservations_agent ON file_reservations(agent_id);
            CREATE INDEX IF NOT EXISTS idx_reservations_expires ON file_reservations(expires_at);
            ",
        )?;
        Ok(())
    }

    /// Acquire a file reservation.
    pub fn acquire(
        &self,
        path_pattern: &str,
        agent_id: &str,
        mode: ReservationMode,
        reason: Option<&str>,
        ttl_minutes: i64,
    ) -> Result<FileReservation> {
        // Check for conflicts first
        let conflict = self.check_conflict(path_pattern, agent_id, mode)?;
        match conflict {
            ReservationConflict::None => {}
            ReservationConflict::SameAgent => {
                // Re-acquire: release old and acquire new
                self.release_by_pattern_agent(path_pattern, agent_id)?;
            }
            other => return Err(anyhow!("Reservation conflict: {:?}", other)),
        }

        let id = format!("res-{}", uuid::Uuid::new_v4());
        let now = Utc::now();
        let expires_at = now + Duration::minutes(ttl_minutes.min(60));

        self.conn.execute(
            "INSERT INTO file_reservations (id, path_pattern, agent_id, mode, reason, acquired_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                path_pattern,
                agent_id,
                mode.as_str(),
                reason,
                now.to_rfc3339(),
                expires_at.to_rfc3339()
            ],
        )?;

        Ok(FileReservation {
            id,
            path_pattern: path_pattern.to_string(),
            agent_id: agent_id.to_string(),
            mode,
            reason: reason.map(String::from),
            acquired_at: now,
            expires_at,
            released_at: None,
        })
    }

    /// Release a reservation by ID.
    pub fn release(&self, reservation_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let updated = self.conn.execute(
            "UPDATE file_reservations SET released_at = ?1 WHERE id = ?2 AND released_at IS NULL",
            params![now, reservation_id],
        )?;
        if updated == 0 {
            return Err(anyhow!("Reservation {} not found or already released", reservation_id));
        }
        Ok(())
    }

    /// Check if a path conflicts with existing reservations.
    pub fn check_conflict(
        &self,
        path_pattern: &str,
        agent_id: &str,
        mode: ReservationMode,
    ) -> Result<ReservationConflict> {
        let now = Utc::now().to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT path_pattern, agent_id, mode, expires_at FROM file_reservations WHERE released_at IS NULL AND expires_at > ?1",
        )?;

        let rows = stmt.query_map(params![now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        for row in rows {
            let (other_path, other_agent, other_mode_str, other_expires) = row?;
            let other_mode = ReservationMode::from_str(&other_mode_str);

            // Check if paths overlap
            if !paths_overlap(path_pattern, &other_path) {
                continue;
            }

            // Same agent
            if other_agent == agent_id {
                return Ok(ReservationConflict::SameAgent);
            }

            // Exclusive conflict
            if mode == ReservationMode::Exclusive || other_mode == ReservationMode::Exclusive {
                let expires_at = DateTime::parse_from_rfc3339(&other_expires)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                return Ok(ReservationConflict::ExclusiveConflict {
                    holder: other_agent,
                    expires_at,
                });
            }

            // Shared + Exclusive requested
            if mode == ReservationMode::Exclusive && other_mode == ReservationMode::Shared {
                return Ok(ReservationConflict::SharedWithExclusive {
                    holder: other_agent,
                });
            }
        }

        Ok(ReservationConflict::None)
    }

    /// List all active (non-expired, non-released) reservations.
    pub fn list_active(&self) -> Result<Vec<FileReservation>> {
        let now = Utc::now().to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT id, path_pattern, agent_id, mode, reason, acquired_at, expires_at FROM file_reservations WHERE released_at IS NULL AND expires_at > ?1 ORDER BY acquired_at DESC",
        )?;

        let rows = stmt.query_map(params![now], |row| {
            Ok(FileReservation {
                id: row.get(0)?,
                path_pattern: row.get(1)?,
                agent_id: row.get(2)?,
                mode: ReservationMode::from_str(&row.get::<_, String>(3)?),
                reason: row.get(4)?,
                acquired_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                expires_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                released_at: None,
            })
        })?;

        let mut reservations = Vec::new();
        for row in rows {
            reservations.push(row?);
        }
        Ok(reservations)
    }

    /// List reservations for a specific agent.
    pub fn by_agent(&self, agent_id: &str) -> Result<Vec<FileReservation>> {
        let now = Utc::now().to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT id, path_pattern, agent_id, mode, reason, acquired_at, expires_at FROM file_reservations WHERE agent_id = ?1 AND released_at IS NULL AND expires_at > ?2 ORDER BY acquired_at DESC",
        )?;

        let rows = stmt.query_map(params![agent_id, now], |row| {
            Ok(FileReservation {
                id: row.get(0)?,
                path_pattern: row.get(1)?,
                agent_id: row.get(2)?,
                mode: ReservationMode::from_str(&row.get::<_, String>(3)?),
                reason: row.get(4)?,
                acquired_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                expires_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                released_at: None,
            })
        })?;

        let mut reservations = Vec::new();
        for row in rows {
            reservations.push(row?);
        }
        Ok(reservations)
    }

    /// Cleanup expired reservations. Returns count of cleaned up entries.
    pub fn cleanup(&self) -> Result<usize> {
        let now = Utc::now().to_rfc3339();
        let count = self.conn.execute(
            "DELETE FROM file_reservations WHERE expires_at < ?1",
            params![now],
        )?;
        Ok(count)
    }

    /// Get reservation heatmap for conflict prediction.
    pub fn heatmap(&self) -> Result<Vec<ReservationHeatmapEntry>> {
        let active = self.list_active()?;
        let mut map: std::collections::HashMap<String, ReservationHeatmapEntry> =
            std::collections::HashMap::new();

        for res in active {
            let entry = map
                .entry(res.path_pattern.clone())
                .or_insert_with(|| ReservationHeatmapEntry {
                    path_pattern: res.path_pattern.clone(),
                    active_count: 0,
                    exclusive_count: 0,
                    shared_count: 0,
                    agents: Vec::new(),
                });

            entry.active_count += 1;
            match res.mode {
                ReservationMode::Exclusive => entry.exclusive_count += 1,
                ReservationMode::Shared => entry.shared_count += 1,
            }
            if !entry.agents.contains(&res.agent_id) {
                entry.agents.push(res.agent_id);
            }
        }

        let mut entries: Vec<_> = map.into_values().collect();
        entries.sort_by(|a, b| b.active_count.cmp(&a.active_count));
        Ok(entries)
    }

    /// Release a reservation by path pattern and agent (internal helper).
    fn release_by_pattern_agent(&self, path_pattern: &str, agent_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE file_reservations SET released_at = ?1 WHERE path_pattern = ?2 AND agent_id = ?3 AND released_at IS NULL",
            params![now, path_pattern, agent_id],
        )?;
        Ok(())
    }
}

/// Check if two path patterns overlap.
/// Supports simple glob matching with * and ? wildcards.
fn paths_overlap(a: &str, b: &str) -> bool {
    // Exact match
    if a == b {
        return true;
    }

    // Check if one is a prefix of the other (directory-level overlap)
    let a_parts: Vec<&str> = a.split('/').collect();
    let b_parts: Vec<&str> = b.split('/').collect();

    let min_len = a_parts.len().min(b_parts.len());
    for i in 0..min_len {
        if !glob_match(a_parts[i], b_parts[i]) {
            return false;
        }
    }

    true
}

/// Simple glob matching for a single path component.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }
    if pattern == text {
        return true;
    }

    // Simple wildcard matching
    let mut pi = 0;
    let mut ti = 0;
    let pattern_bytes = pattern.as_bytes();
    let text_bytes = text.as_bytes();
    let mut star_pi = 0;
    let mut star_ti = 0;

    while ti < text_bytes.len() {
        if pi < pattern_bytes.len()
            && (pattern_bytes[pi] == b'?' || pattern_bytes[pi] == text_bytes[ti])
        {
            pi += 1;
            ti += 1;
        } else if pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != 0 {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
        pi += 1;
    }

    pi == pattern_bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_store() -> (FileReservationStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = FileReservationStore::open(&dir.path().join("reservations.db")).unwrap();
        (store, dir)
    }

    #[test]
    fn test_acquire_and_release() {
        let (store, _dir) = open_store();

        let res = store
            .acquire("src/main.rs", "agent-1", ReservationMode::Exclusive, Some("editing"), 10)
            .unwrap();

        assert_eq!(res.path_pattern, "src/main.rs");
        assert_eq!(res.agent_id, "agent-1");
        assert_eq!(res.mode, ReservationMode::Exclusive);

        let active = store.list_active().unwrap();
        assert_eq!(active.len(), 1);

        store.release(&res.id).unwrap();

        let active = store.list_active().unwrap();
        assert_eq!(active.len(), 0);
    }

    #[test]
    fn test_exclusive_conflict() {
        let (store, _dir) = open_store();

        store
            .acquire("src/main.rs", "agent-1", ReservationMode::Exclusive, None, 10)
            .unwrap();

        let conflict = store
            .check_conflict("src/main.rs", "agent-2", ReservationMode::Exclusive)
            .unwrap();

        assert!(matches!(
            conflict,
            ReservationConflict::ExclusiveConflict { .. }
        ));
    }

    #[test]
    fn test_shared_no_conflict() {
        let (store, _dir) = open_store();

        store
            .acquire("src/main.rs", "agent-1", ReservationMode::Shared, None, 10)
            .unwrap();

        let conflict = store
            .check_conflict("src/main.rs", "agent-2", ReservationMode::Shared)
            .unwrap();

        assert_eq!(conflict, ReservationConflict::None);
    }

    #[test]
    fn test_same_agent_reacquire() {
        let (store, _dir) = open_store();

        store
            .acquire("src/main.rs", "agent-1", ReservationMode::Exclusive, None, 10)
            .unwrap();

        // Same agent can re-acquire
        let res = store
            .acquire("src/main.rs", "agent-1", ReservationMode::Exclusive, None, 10)
            .unwrap();

        assert_eq!(res.agent_id, "agent-1");
    }

    #[test]
    fn test_glob_overlap() {
        assert!(paths_overlap("src/main.rs", "src/main.rs"));
        // Same path with wildcard
        assert!(paths_overlap("src/*.rs", "src/*.rs"));
    }

    #[test]
    fn test_by_agent() {
        let (store, _dir) = open_store();

        store
            .acquire("src/a.rs", "agent-1", ReservationMode::Exclusive, None, 10)
            .unwrap();
        store
            .acquire("src/b.rs", "agent-1", ReservationMode::Shared, None, 10)
            .unwrap();
        store
            .acquire("src/c.rs", "agent-2", ReservationMode::Exclusive, None, 10)
            .unwrap();

        let agent1 = store.by_agent("agent-1").unwrap();
        assert_eq!(agent1.len(), 2);

        let agent2 = store.by_agent("agent-2").unwrap();
        assert_eq!(agent2.len(), 1);
    }

    #[test]
    fn test_heatmap() {
        let (store, _dir) = open_store();

        // Use different paths to avoid conflict
        store
            .acquire("src/a.rs", "agent-1", ReservationMode::Exclusive, None, 10)
            .unwrap();
        store
            .acquire("src/b.rs", "agent-2", ReservationMode::Shared, None, 10)
            .unwrap();

        let heatmap = store.heatmap().unwrap();
        assert_eq!(heatmap.len(), 2);
    }

    #[test]
    fn test_cleanup_expired() {
        let (store, _dir) = open_store();

        // Create a reservation with 0 TTL (already expired)
        store
            .acquire("src/main.rs", "agent-1", ReservationMode::Exclusive, None, 0)
            .unwrap();

        // Wait a bit for it to expire
        std::thread::sleep(std::time::Duration::from_millis(10));

        let cleaned = store.cleanup().unwrap();
        assert_eq!(cleaned, 1);

        let active = store.list_active().unwrap();
        assert_eq!(active.len(), 0);
    }
}
