use crate::coordination::actions::ActionStore;
use crate::types::{Action, ActionEdgeType, ActionStatus, Checkpoint, CheckpointStatus, CheckpointType};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

pub struct CheckpointStore {
    conn: Connection,
}

impl CheckpointStore {
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
            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                action_id TEXT NOT NULL,
                checkpoint_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                condition TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                resolved_at TEXT,
                resolved_by TEXT,
                result TEXT,
                expires_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_checkpoints_action ON checkpoints(action_id);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_status ON checkpoints(status);
            ",
        )?;
        Ok(())
    }

    pub fn create_checkpoint(
        &self,
        action_store: &ActionStore,
        checkpoint: &Checkpoint,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO checkpoints (id, action_id, checkpoint_type, status, condition, created_at, resolved_at, resolved_by, result, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                checkpoint.id, checkpoint.action_id, checkpoint.checkpoint_type.to_string(),
                checkpoint.status.to_string(), checkpoint.condition,
                checkpoint.created_at.to_rfc3339(),
                checkpoint.resolved_at.map(|dt| dt.to_rfc3339()),
                Option::<String>::None, Option::<String>::None,
                expires_at.map(|dt| dt.to_rfc3339()),
            ],
        )?;

        action_store.update_action_status(&checkpoint.action_id, ActionStatus::Blocked)?;
        Ok(())
    }

    pub fn resolve_checkpoint(
        &self,
        action_store: &ActionStore,
        checkpoint_id: &str,
        status: CheckpointStatus,
        resolved_by: &str,
        result: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE checkpoints SET status = ?1, resolved_at = ?2, resolved_by = ?3, result = ?4 WHERE id = ?5",
            params![status.to_string(), now, resolved_by, result, checkpoint_id],
        )?;

        if status == CheckpointStatus::Passed {
            let action_id: String = self.conn.query_row(
                "SELECT action_id FROM checkpoints WHERE id = ?1",
                params![checkpoint_id],
                |row| row.get(0),
            )?;

            let all_passed: bool = self.conn.query_row(
                "SELECT COUNT(*) = 0 FROM checkpoints WHERE action_id = ?1 AND status != 'passed'",
                params![action_id],
                |row| row.get(0),
            )?;

            if all_passed {
                action_store.update_action_status(&action_id, ActionStatus::Pending)?;
            }
        }
        Ok(())
    }

    pub fn expire_checkpoints(&self) -> Result<usize> {
        let now = Utc::now().to_rfc3339();
        let changed = self.conn.execute(
            "UPDATE checkpoints SET status = 'failed' WHERE expires_at IS NOT NULL AND expires_at < ?1 AND status = 'pending'",
            params![now],
        )?;
        Ok(changed)
    }

    pub fn get_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        let mut stmt = self.conn.prepare("SELECT * FROM checkpoints WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_checkpoint(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_action_checkpoints(&self, action_id: &str) -> Result<Vec<Checkpoint>> {
        let mut stmt = self.conn.prepare("SELECT * FROM checkpoints WHERE action_id = ?1 ORDER BY created_at")?;
        let rows = stmt.query_map(params![action_id], |row| self.row_to_checkpoint(row))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

impl CheckpointStore {
    fn row_to_checkpoint(&self, row: &rusqlite::Row) -> rusqlite::Result<Checkpoint> {
        let cp_type_str: String = row.get("checkpoint_type")?;
        let cp_type = cp_type_str.parse().unwrap_or(CheckpointType::Manual);
        let status_str: String = row.get("status")?;
        let status = status_str.parse().unwrap_or(CheckpointStatus::Pending);

        Ok(Checkpoint {
            id: row.get("id")?,
            action_id: row.get("action_id")?,
            checkpoint_type: cp_type,
            status,
            condition: row.get("condition")?,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("created_at")?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            resolved_at: row.get::<_, Option<String>>("resolved_at")?
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Action;
    use std::collections::HashMap;

    fn test_stores() -> (ActionStore, CheckpointStore) {
        let action_store = ActionStore::open(Path::new(":memory:")).unwrap();
        let checkpoint_store = CheckpointStore::open(Path::new(":memory:")).unwrap();
        (action_store, checkpoint_store)
    }

    fn test_action(id: &str) -> Action {
        Action {
            id: id.to_string(),
            title: format!("Action {}", id),
            description: "Test".to_string(),
            status: ActionStatus::Pending,
            priority: 2,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            created_by: None,
            assigned_to: None,
            project: "test".to_string(),
            tags: vec![],
            source_observation_ids: vec![],
            source_memory_ids: vec![],
            result: None,
            parent_id: None,
            metadata: HashMap::new(),
            sketch_id: None,
            crystallized_into: None,
        }
    }

    #[test]
    fn test_create_checkpoint_blocks_action() {
        let (action_store, checkpoint_store) = test_stores();
        action_store.create_action(&test_action("a-1")).unwrap();

        let checkpoint = Checkpoint {
            id: "cp-1".to_string(),
            action_id: "a-1".to_string(),
            checkpoint_type: CheckpointType::Ci,
            status: CheckpointStatus::Pending,
            condition: "CI passes".to_string(),
            created_at: Utc::now(),
            resolved_at: None,
        };
        checkpoint_store.create_checkpoint(&action_store, &checkpoint, None).unwrap();

        let action = action_store.get_action("a-1").unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::Blocked);
    }

    #[test]
    fn test_resolve_checkpoint_unblocks_action() {
        let (action_store, checkpoint_store) = test_stores();
        action_store.create_action(&test_action("a-1")).unwrap();

        let checkpoint = Checkpoint {
            id: "cp-1".to_string(),
            action_id: "a-1".to_string(),
            checkpoint_type: CheckpointType::Ci,
            status: CheckpointStatus::Pending,
            condition: "CI passes".to_string(),
            created_at: Utc::now(),
            resolved_at: None,
        };
        checkpoint_store.create_checkpoint(&action_store, &checkpoint, None).unwrap();

        checkpoint_store.resolve_checkpoint(&action_store, "cp-1", CheckpointStatus::Passed, "agent-1", None).unwrap();
        let action = action_store.get_action("a-1").unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::Pending);
    }

    #[test]
    fn test_expire_checkpoints() {
        let (action_store, checkpoint_store) = test_stores();
        action_store.create_action(&test_action("a-1")).unwrap();

        let checkpoint = Checkpoint {
            id: "cp-1".to_string(),
            action_id: "a-1".to_string(),
            checkpoint_type: CheckpointType::Timer,
            status: CheckpointStatus::Pending,
            condition: "timer".to_string(),
            created_at: Utc::now(),
            resolved_at: None,
        };
        let expired = Utc::now() - chrono::Duration::minutes(5);
        checkpoint_store.create_checkpoint(&action_store, &checkpoint, Some(expired)).unwrap();

        let expired_count = checkpoint_store.expire_checkpoints().unwrap();
        assert_eq!(expired_count, 1);

        let cp = checkpoint_store.get_checkpoint("cp-1").unwrap().unwrap();
        assert_eq!(cp.status, CheckpointStatus::Failed);
    }

    #[test]
    fn test_get_action_checkpoints() {
        let (action_store, checkpoint_store) = test_stores();
        action_store.create_action(&test_action("a-1")).unwrap();

        let cp1 = Checkpoint {
            id: "cp-1".to_string(),
            action_id: "a-1".to_string(),
            checkpoint_type: CheckpointType::Ci,
            status: CheckpointStatus::Pending,
            condition: "CI".to_string(),
            created_at: Utc::now(),
            resolved_at: None,
        };
        checkpoint_store.create_checkpoint(&action_store, &cp1, None).unwrap();

        let cps = checkpoint_store.get_action_checkpoints("a-1").unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].id, "cp-1");
    }
}
