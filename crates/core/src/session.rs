//! Session store — SQLite-backed session tracking with observation storage.

use crate::types::{HookType, RawObservation, Session};
use rusqlite::{params, OptionalExtension};
use std::path::Path;
use tokio::sync::Mutex;

/// SQLite-backed session store. Thread-safe via tokio::sync::Mutex.
pub struct SessionStore {
    conn: Mutex<rusqlite::Connection>,
}

impl SessionStore {
    pub fn open(db_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = rusqlite::Connection::open(db_path)?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn in_memory() -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY, project TEXT NOT NULL, cwd TEXT NOT NULL,
                started_at TEXT NOT NULL, ended_at TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                observation_count INTEGER NOT NULL DEFAULT 0,
                model TEXT, tags TEXT NOT NULL DEFAULT '[]',
                first_prompt TEXT, summary TEXT,
                commit_shas TEXT NOT NULL DEFAULT '[]', agent_id TEXT
            );
            CREATE TABLE IF NOT EXISTS observations (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
                timestamp TEXT NOT NULL, hook_type TEXT NOT NULL,
                tool_name TEXT, tool_input TEXT, tool_output TEXT,
                user_prompt TEXT, assistant_response TEXT, raw TEXT,
                modality TEXT NOT NULL DEFAULT 'text', image_data TEXT, agent_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project);",
        )?;
        Ok(())
    }

    pub fn create_session(&self, id: &str, project: &str, cwd: &str) -> anyhow::Result<Session> {
        let session = Session::new(id, project, cwd);
        let conn = self.conn.blocking_lock();
        conn.execute(
            "INSERT INTO sessions (id, project, cwd, started_at, status, observation_count, tags, commit_shas)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session.id, session.project, session.cwd,
                session.started_at.to_rfc3339(), session.status,
                session.observation_count,
                serde_json::to_string(&session.tags)?,
                serde_json::to_string(&session.commit_shas)?,
            ],
        )?;
        Ok(session)
    }

    /// mr-kqrs: ensure_session — idempotent session creation. If a
    /// session with this id already exists, return it; otherwise
    /// create a stub session with the given project/cwd.
    ///
    /// Uses INSERT OR IGNORE to avoid a TOCTOU race between the
    /// existence check and the insert.
    ///
    /// Used by OpenCode normalizer to satisfy the observations.session_id
    /// FK without requiring an explicit session bootstrap step.
    pub fn ensure_session(&self, id: &str, project: &str, cwd: &str) -> anyhow::Result<Session> {
        let conn = self.conn.blocking_lock();
        conn.execute(
            "INSERT OR IGNORE INTO sessions
                (id, project, cwd, started_at, status, observation_count,
                 tags, commit_shas)
             VALUES (?1, ?2, ?3, ?4, 'active', 0, '[]', '[]')",
            rusqlite::params![id, project, cwd, chrono::Utc::now().to_rfc3339()],
        )?;
        drop(conn);
        self.get_session(id)?
            .ok_or_else(|| anyhow::anyhow!("session {} not found after ensure", id))
    }

    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<Session>> {
        let conn = self.conn.blocking_lock();
        conn.query_row(
            "SELECT id, project, cwd, started_at, ended_at, status,
                    observation_count, model, tags, first_prompt, summary,
                    commit_shas, agent_id FROM sessions WHERE id = ?1",
            params![id],
            parse_session_row,
        )
        .optional()
        .map_err(|e| anyhow::anyhow!("failed to fetch session: {e}"))
    }

    pub fn list_sessions(&self, project: Option<&str>) -> anyhow::Result<Vec<Session>> {
        let conn = self.conn.blocking_lock();
        let query = match project {
            Some(_) => {
                "SELECT id, project, cwd, started_at, ended_at, status,
                               observation_count, model, tags, first_prompt, summary,
                               commit_shas, agent_id
                        FROM sessions WHERE project = ?1 ORDER BY started_at DESC"
            }
            None => {
                "SELECT id, project, cwd, started_at, ended_at, status,
                            observation_count, model, tags, first_prompt, summary,
                            commit_shas, agent_id
                     FROM sessions ORDER BY started_at DESC"
            }
        };
        let mut stmt = conn.prepare(query)?;
        let rows = if let Some(p) = project {
            stmt.query_map(params![p], parse_session_row)?
        } else {
            stmt.query_map([], parse_session_row)?
        };
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn end_session(&self, id: &str, summary: Option<&str>) -> anyhow::Result<()> {
        let conn = self.conn.blocking_lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE sessions SET ended_at = ?1, status = 'completed', summary = ?2 WHERE id = ?3",
            params![now, summary, id],
        )?;
        Ok(())
    }

    pub fn add_observation(&self, obs: &RawObservation) -> anyhow::Result<()> {
        // mr-kqrs (B15): ensure the parent session row exists before
        // the observation insert. Delegates to ensure_session so there
        // is a single session auto-creation path rather than duplicating
        // the INSERT OR IGNORE logic here.
        //
        // We populate a minimal session row from observation metadata
        // when one is absent. The fields are best-effort: project and
        // cwd fall back to "unknown"/"unknown" if not annotated.
        let project = obs
            .agent_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let cwd = "unknown";
        self.ensure_session(&obs.session_id, &project, cwd)?;

        let conn = self.conn.blocking_lock();
        let image_data = obs
            .image_data
            .as_ref()
            .map(|img| serde_json::to_string(img))
            .transpose()?;
        conn.execute(
            "INSERT INTO observations (
                id, session_id, timestamp, hook_type, tool_name, tool_input,
                tool_output, user_prompt, assistant_response, raw, modality,
                image_data, agent_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                obs.id,
                obs.session_id,
                obs.timestamp.to_rfc3339(),
                obs.hook_type.to_string(),
                obs.tool_name,
                obs.tool_input,
                obs.tool_output,
                obs.user_prompt,
                obs.assistant_response,
                obs.raw,
                obs.modality,
                image_data,
                obs.agent_id,
            ],
        )?;
        conn.execute(
            "UPDATE sessions SET observation_count = observation_count + 1 WHERE id = ?1",
            params![obs.session_id],
        )?;
        Ok(())
    }

    pub fn get_observations(&self, session_id: &str) -> anyhow::Result<Vec<RawObservation>> {
        let conn = self.conn.blocking_lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, timestamp, hook_type, tool_name, tool_input,
                    tool_output, user_prompt, assistant_response, raw, modality,
                    image_data, agent_id
             FROM observations WHERE session_id = ?1 ORDER BY timestamp ASC",
        )?;
        let rows = stmt.query_map(params![session_id], parse_observation_row)?;
        let mut observations = Vec::new();
        for row in rows {
            observations.push(row?);
        }
        Ok(observations)
    }

    pub fn list_all_observations(
        &self,
        project: Option<&str>,
    ) -> anyhow::Result<Vec<RawObservation>> {
        let conn = self.conn.blocking_lock();
        let query = match project {
            Some(_) => "SELECT o.id, o.session_id, o.timestamp, o.hook_type, o.tool_name, o.tool_input,
                               o.tool_output, o.user_prompt, o.assistant_response, o.raw, o.modality,
                               o.image_data, o.agent_id
                        FROM observations o JOIN sessions s ON o.session_id = s.id
                        WHERE s.project = ?1 ORDER BY o.timestamp ASC",
            None => "SELECT id, session_id, timestamp, hook_type, tool_name, tool_input,
                            tool_output, user_prompt, assistant_response, raw, modality,
                            image_data, agent_id
                     FROM observations ORDER BY timestamp ASC",
        };
        let mut stmt = conn.prepare(query)?;
        let rows = if let Some(p) = project {
            stmt.query_map(params![p], parse_observation_row)?
        } else {
            stmt.query_map([], parse_observation_row)?
        };
        let mut observations = Vec::new();
        for row in rows {
            observations.push(row?);
        }
        Ok(observations)
    }
}

fn parse_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let tags: String = row.get(8)?;
    let commit_shas: String = row.get(11)?;
    Ok(Session {
        id: row.get(0)?,
        project: row.get(1)?,
        cwd: row.get(2)?,
        started_at: row.get::<_, String>(3).and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
        })?,
        ended_at: row
            .get::<_, Option<String>>(4)?
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        status: row.get(5)?,
        observation_count: row.get(6)?,
        model: row.get(7)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        first_prompt: row.get(9)?,
        summary: row.get(10)?,
        commit_shas: serde_json::from_str(&commit_shas).unwrap_or_default(),
        agent_id: row.get(12)?,
    })
}

fn parse_observation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawObservation> {
    let image_data: Option<String> = row.get(11)?;
    let hook_type_str: String = row.get(3)?;
    let hook_type: HookType = hook_type_str.parse().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown HookType: {hook_type_str}"),
            )),
        )
    })?;
    Ok(RawObservation {
        id: row.get(0)?,
        session_id: row.get(1)?,
        timestamp: row.get::<_, String>(2).and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
        })?,
        hook_type,
        tool_name: row.get(4)?,
        tool_input: row.get(5)?,
        tool_output: row.get(6)?,
        user_prompt: row.get(7)?,
        assistant_response: row.get(8)?,
        raw: row.get(9)?,
        modality: row.get(10)?,
        image_data: image_data.map(|s| serde_json::from_str(&s).unwrap_or_default()),
        agent_id: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_crud() {
        let store = SessionStore::in_memory().unwrap();
        let session = store
            .create_session("s-1", "my-project", "/tmp/proj")
            .unwrap();
        assert_eq!(session.id, "s-1");
        let fetched = store.get_session("s-1").unwrap().unwrap();
        assert_eq!(fetched.id, "s-1");
        let sessions = store.list_sessions(None).unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_end_session() {
        let store = SessionStore::in_memory().unwrap();
        store.create_session("s-1", "proj", "/tmp").unwrap();
        store.end_session("s-1", Some("all done")).unwrap();
        let session = store.get_session("s-1").unwrap().unwrap();
        assert_eq!(session.status, "completed");
        assert!(session.ended_at.is_some());
    }

    #[tokio::test]
    async fn test_observations() {
        tokio::task::spawn_blocking(|| {
            let store = SessionStore::in_memory().unwrap();
            store.create_session("s-1", "proj", "/tmp").unwrap();
            let obs = RawObservation {
                id: "o-1".into(),
                session_id: "s-1".into(),
                timestamp: chrono::Utc::now(),
                hook_type: HookType::PostToolUse,
                tool_name: Some("Read".into()),
                tool_input: Some("file.txt".into()),
                tool_output: Some("content".into()),
                user_prompt: None,
                assistant_response: None,
                raw: None,
                modality: "text".into(),
                image_data: None,
                agent_id: None,
            };
            store.add_observation(&obs).unwrap();
            let observations = store.get_observations("s-1").unwrap();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].id, "o-1");
        })
        .await
        .unwrap();
    }

    // mr-kqrs (B15): add_observation must auto-create the parent
    // session row when none exists. Before this fix, an observation
    // for a never-created session would fail with an FK violation on
    // observations.session_id.
    #[tokio::test]
    async fn test_add_observation_auto_creates_session() {
        tokio::task::spawn_blocking(|| {
            let store = SessionStore::in_memory().unwrap();
            // Pre-condition: no "ghost" session row.
            assert!(store.get_session("ghost").unwrap().is_none());
            let obs = RawObservation {
                id: "o-ghost".into(),
                session_id: "ghost".into(),
                timestamp: chrono::Utc::now(),
                hook_type: HookType::PostToolUse,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                user_prompt: None,
                assistant_response: None,
                raw: None,
                modality: "text".into(),
                image_data: None,
                agent_id: Some("claude".into()),
            };
            // Should succeed even though the session doesn't exist yet.
            store.add_observation(&obs).unwrap();
            // Post-condition: the session was auto-created.
            let session = store
                .get_session("ghost")
                .unwrap()
                .expect("session should be auto-created");
            assert_eq!(session.project, "claude");
            assert_eq!(session.observation_count, 1);
        })
        .await
        .unwrap();
    }

    // mr-kqrs: ensure_session is idempotent.
    #[test]
    fn test_ensure_session_idempotent() {
        let store = SessionStore::in_memory().unwrap();
        let s1 = store.ensure_session("dup", "proj", "/tmp").unwrap();
        let s2 = store.ensure_session("dup", "proj", "/tmp").unwrap();
        assert_eq!(s1.id, s2.id);
        let list = store.list_sessions(None).unwrap();
        assert_eq!(list.len(), 1, "ensure_session must not duplicate");
    }
}
