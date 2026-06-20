use crate::types::{Action, ActionEdgeType, ActionStatus};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::io;
use std::path::Path;

/// Result of a two-phase claim attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimResult {
    /// Successfully claimed.
    Claimed,
    /// Action not found.
    NotFound,
    /// Action is not in a claimable state.
    NotClaimable { current_status: ActionStatus },
    /// Action is blocked by dependencies.
    Blocked { by: String },
    /// Race condition: status changed between check and update.
    RaceCondition { current_status: ActionStatus },
}

pub struct ActionStore {
    conn: Connection,
}

impl ActionStore {
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
            CREATE TABLE IF NOT EXISTS actions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                priority INTEGER NOT NULL DEFAULT 2,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                created_by TEXT,
                assigned_to TEXT,
                project TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '[]',
                source_observation_ids TEXT NOT NULL DEFAULT '[]',
                source_memory_ids TEXT NOT NULL DEFAULT '[]',
                result TEXT,
                parent_id TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                sketch_id TEXT,
                crystallized_into TEXT
            );
            CREATE TABLE IF NOT EXISTS action_edges (
                id TEXT PRIMARY KEY,
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                FOREIGN KEY (from_id) REFERENCES actions(id),
                FOREIGN KEY (to_id) REFERENCES actions(id)
            );
            CREATE INDEX IF NOT EXISTS idx_actions_status ON actions(status);
            CREATE INDEX IF NOT EXISTS idx_actions_priority ON actions(priority);
            CREATE INDEX IF NOT EXISTS idx_edges_from ON action_edges(from_id);
            CREATE INDEX IF NOT EXISTS idx_edges_to ON action_edges(to_id);
            ",
        )?;
        Ok(())
    }

    pub fn create_action(&self, action: &Action) -> Result<()> {
        let tags = serde_json::to_string(&action.tags)?;
        let obs_ids = serde_json::to_string(&action.source_observation_ids)?;
        let mem_ids = serde_json::to_string(&action.source_memory_ids)?;
        let metadata = serde_json::to_string(&action.metadata)?;
        self.conn.execute(
            "INSERT INTO actions (id, title, description, status, priority, created_at, updated_at, created_by, assigned_to, project, tags, source_observation_ids, source_memory_ids, result, parent_id, metadata, sketch_id, crystallized_into) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                action.id, action.title, action.description, action.status.to_string(),
                action.priority, action.created_at.to_rfc3339(), action.updated_at.to_rfc3339(),
                action.created_by, action.assigned_to, action.project, tags, obs_ids, mem_ids,
                action.result, action.parent_id, metadata, action.sketch_id, action.crystallized_into
            ],
        )?;
        Ok(())
    }

    pub fn get_action(&self, id: &str) -> Result<Option<Action>> {
        let mut stmt = self.conn.prepare("SELECT * FROM actions WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_action(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_actions(
        &self,
        project: Option<&str>,
        status: Option<ActionStatus>,
    ) -> Result<Vec<Action>> {
        let mut sql = "SELECT * FROM actions WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(p) = project {
            sql.push_str(" AND project = ?");
            params_vec.push(Box::new(p.to_string()));
        }
        if let Some(s) = status {
            sql.push_str(" AND status = ?");
            params_vec.push(Box::new(format!("{:?}", s)));
        }
        sql.push_str(" ORDER BY priority ASC, created_at DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs.iter()), |row| {
            self.row_to_action(row)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_action_status(&self, id: &str, status: ActionStatus) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE actions SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.to_string(), now, id],
        )?;
        self.propagate_completion(id)?;
        Ok(())
    }

    pub fn add_edge(&self, from_id: &str, to_id: &str, edge_type: ActionEdgeType) -> Result<()> {
        let id = format!(
            "ae_{}_{}_{}",
            from_id,
            to_id,
            format!("{:?}", edge_type).to_lowercase()
        );
        self.conn.execute(
            "INSERT INTO action_edges (id, from_id, to_id, edge_type) VALUES (?1, ?2, ?3, ?4)",
            params![id, from_id, to_id, format!("{:?}", edge_type)],
        )?;

        if edge_type == ActionEdgeType::DependsOn || edge_type == ActionEdgeType::Blocks {
            self.conn.execute(
                "UPDATE actions SET status = 'blocked' WHERE id = ?1 AND status = 'pending'",
                params![to_id],
            )?;
        }
        Ok(())
    }

    pub fn get_dependencies(&self, action_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_id FROM action_edges WHERE to_id = ?1 AND edge_type IN ('DependsOn', 'Blocks')",
        )?;
        let rows = stmt.query_map(params![action_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_dependents(&self, action_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT to_id FROM action_edges WHERE from_id = ?1 AND edge_type IN ('DependsOn', 'Blocks')",
        )?;
        let rows = stmt.query_map(params![action_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn propagate_completion(&self, action_id: &str) -> Result<()> {
        let dependents = self.get_dependents(action_id)?;
        for dep_id in dependents {
            let deps = self.get_dependencies(&dep_id)?;
            let mut all_done = true;
            for dep in &deps {
                let status: String = self.conn.query_row(
                    "SELECT status FROM actions WHERE id = ?1",
                    params![dep],
                    |row| row.get(0),
                )?;
                if status != "completed" && status != "cancelled" {
                    all_done = false;
                    break;
                }
            }
            if all_done && !deps.is_empty() {
                let now = Utc::now().to_rfc3339();
                self.conn.execute(
                    "UPDATE actions SET status = 'pending', updated_at = ?1 WHERE id = ?2 AND status = 'blocked'",
                    params![now, dep_id],
                )?;
            }
        }
        Ok(())
    }

    /// Two-phase claim: check -> lock -> re-check -> update.
    /// Prevents race conditions when multiple agents try to claim the same action.
    pub fn claim_action(&self, action_id: &str, agent_id: &str) -> Result<ClaimResult> {
        // Phase 1: Check if claimable
        let action = match self.get_action(action_id)? {
            Some(a) => a,
            None => return Ok(ClaimResult::NotFound),
        };

        if action.status != ActionStatus::Pending && action.status != ActionStatus::Failed {
            return Ok(ClaimResult::NotClaimable {
                current_status: action.status,
            });
        }

        // Check dependencies are met
        let deps = self.get_dependencies(action_id)?;
        for dep_id in &deps {
            let dep = self.get_action(dep_id)?;
            if let Some(dep) = dep {
                if dep.status != ActionStatus::Completed && dep.status != ActionStatus::Cancelled {
                    return Ok(ClaimResult::Blocked { by: dep_id.clone() });
                }
            }
        }

        // Phase 2: Atomic update with status check
        let now = Utc::now().to_rfc3339();
        let updated = self.conn.execute(
            "UPDATE actions SET status = 'in_progress', assigned_to = ?1, updated_at = ?2 WHERE id = ?3 AND (status = 'pending' OR status = 'failed')",
            params![agent_id, now, action_id],
        )?;

        if updated == 0 {
            // Status changed between check and update (race condition)
            let current = self.get_action(action_id)?;
            let current_status = current.map(|a| a.status).unwrap_or(ActionStatus::Pending);
            return Ok(ClaimResult::RaceCondition { current_status });
        }

        Ok(ClaimResult::Claimed)
    }

    fn row_to_action(&self, row: &rusqlite::Row) -> rusqlite::Result<Action> {
        let status: ActionStatus =
            row.get::<_, String>("status")?
                .parse()
                .map_err(|e: String| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(io::Error::new(
                        io::ErrorKind::InvalidData,
                        e,
                    )))
                })?;
        let created_at_str: String = row.get("created_at")?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let updated_at_str: String = row.get("updated_at")?;
        let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let tags_str: String = row.get("tags")?;
        let source_obs_str: String = row.get("source_observation_ids")?;
        let source_mem_str: String = row.get("source_memory_ids")?;
        let metadata_str: String = row.get("metadata")?;
        Ok(Action {
            id: row.get("id")?,
            title: row.get("title")?,
            description: row.get("description")?,
            status,
            priority: row.get("priority")?,
            created_at,
            updated_at,
            created_by: row.get("created_by")?,
            assigned_to: row.get("assigned_to")?,
            project: row.get("project")?,
            tags: serde_json::from_str(&tags_str)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            source_observation_ids: serde_json::from_str(&source_obs_str)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            source_memory_ids: serde_json::from_str(&source_mem_str)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            result: row.get("result")?,
            parent_id: row.get("parent_id")?,
            metadata: serde_json::from_str(&metadata_str)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
            sketch_id: row.get("sketch_id")?,
            crystallized_into: row.get("crystallized_into")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_store() -> ActionStore {
        ActionStore::open(Path::new(":memory:")).unwrap()
    }

    fn test_action(id: &str) -> Action {
        Action {
            id: id.to_string(),
            title: format!("Action {}", id),
            description: "Test action".to_string(),
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
    fn test_create_and_get_action() {
        let store = test_store();
        let action = test_action("a-1");
        store.create_action(&action).unwrap();
        let retrieved = store.get_action("a-1").unwrap().unwrap();
        assert_eq!(retrieved.id, "a-1");
        assert_eq!(retrieved.title, "Action a-1");
    }

    #[test]
    fn test_list_actions() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        store.create_action(&test_action("a-2")).unwrap();
        let actions = store.list_actions(Some("test"), None).unwrap();
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn test_update_action_status() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        store
            .update_action_status("a-1", ActionStatus::InProgress)
            .unwrap();
        let action = store.get_action("a-1").unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::InProgress);
    }

    #[test]
    fn test_add_edge_and_dependency() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        store.create_action(&test_action("a-2")).unwrap();
        store
            .add_edge("a-1", "a-2", ActionEdgeType::DependsOn)
            .unwrap();

        let deps = store.get_dependencies("a-2").unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], "a-1");
    }

    #[test]
    fn test_propagate_completion() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        let mut a2 = test_action("a-2");
        a2.status = ActionStatus::Blocked;
        store.create_action(&a2).unwrap();
        store
            .add_edge("a-1", "a-2", ActionEdgeType::DependsOn)
            .unwrap();

        store
            .update_action_status("a-1", ActionStatus::Completed)
            .unwrap();
        let a2 = store.get_action("a-2").unwrap().unwrap();
        assert_eq!(a2.status, ActionStatus::Pending);
    }

    #[test]
    fn test_get_dependents() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        store.create_action(&test_action("a-2")).unwrap();
        store
            .add_edge("a-1", "a-2", ActionEdgeType::DependsOn)
            .unwrap();

        let dependents = store.get_dependents("a-1").unwrap();
        assert_eq!(dependents.len(), 1);
        assert_eq!(dependents[0], "a-2");
    }

    #[test]
    fn test_claim_action_success() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();

        let result = store.claim_action("a-1", "agent-1").unwrap();
        assert_eq!(result, ClaimResult::Claimed);

        let action = store.get_action("a-1").unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::InProgress);
        assert_eq!(action.assigned_to, Some("agent-1".to_string()));
    }

    #[test]
    fn test_claim_action_not_found() {
        let store = test_store();

        let result = store.claim_action("nonexistent", "agent-1").unwrap();
        assert_eq!(result, ClaimResult::NotFound);
    }

    #[test]
    fn test_claim_action_not_claimable() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        store
            .update_action_status("a-1", ActionStatus::Completed)
            .unwrap();

        let result = store.claim_action("a-1", "agent-1").unwrap();
        assert!(matches!(
            result,
            ClaimResult::NotClaimable {
                current_status: ActionStatus::Completed
            }
        ));
    }

    #[test]
    fn test_claim_action_blocked() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        let mut a2 = test_action("a-2");
        a2.status = ActionStatus::Pending; // Explicitly set to pending
        store.create_action(&a2).unwrap();
        store
            .add_edge("a-1", "a-2", ActionEdgeType::DependsOn)
            .unwrap();

        // a-2 is blocked because a-1 is not completed
        // But the status was changed to 'blocked' by add_edge
        // So claim_action returns NotClaimable, not Blocked
        let result = store.claim_action("a-2", "agent-1").unwrap();
        assert!(matches!(
            result,
            ClaimResult::NotClaimable {
                current_status: ActionStatus::Blocked
            }
        ));
    }

    #[test]
    fn test_claim_action_with_met_dependencies() {
        let store = test_store();
        store.create_action(&test_action("a-1")).unwrap();
        store.create_action(&test_action("a-2")).unwrap();
        store
            .add_edge("a-1", "a-2", ActionEdgeType::DependsOn)
            .unwrap();

        // Complete dependency first
        store
            .update_action_status("a-1", ActionStatus::Completed)
            .unwrap();

        // Now claim should succeed
        let result = store.claim_action("a-2", "agent-1").unwrap();
        assert_eq!(result, ClaimResult::Claimed);
    }
}
