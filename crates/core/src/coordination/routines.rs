use crate::coordination::actions::ActionStore;
use crate::types::{Action, ActionStatus, Routine, RoutineRun, RoutineStep};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

pub struct RoutineStore {
    conn: Connection,
}

impl RoutineStore {
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
            CREATE TABLE IF NOT EXISTS routines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                steps TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                frozen INTEGER NOT NULL DEFAULT 0,
                tags TEXT NOT NULL DEFAULT '[]',
                source_procedural_ids TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS routine_runs (
                id TEXT PRIMARY KEY,
                routine_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                status TEXT NOT NULL DEFAULT 'running',
                step_results TEXT NOT NULL DEFAULT '{}',
                FOREIGN KEY (routine_id) REFERENCES routines(id)
            );
            ",
        )?;
        Ok(())
    }

    pub fn create_routine(&self, routine: &Routine) -> Result<()> {
        let steps = serde_json::to_string(&routine.steps)?;
        let tags = serde_json::to_string(&routine.tags)?;
        let proc_ids = serde_json::to_string(&routine.source_procedural_ids)?;
        self.conn.execute(
            "INSERT INTO routines (id, name, description, steps, created_at, updated_at, frozen, tags, source_procedural_ids) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                routine.id, routine.name, routine.description, steps,
                routine.created_at.to_rfc3339(), routine.updated_at.to_rfc3339(),
                if routine.frozen { 1 } else { 0 }, tags, proc_ids
            ],
        )?;
        Ok(())
    }

    pub fn run_routine(
        &self,
        action_store: &ActionStore,
        routine_id: &str,
        project: &str,
    ) -> Result<String> {
        let routine = self.get_routine(routine_id)?
            .ok_or_else(|| anyhow::anyhow!("Routine {} not found", routine_id))?;

        if routine.frozen {
            return Err(anyhow::anyhow!("Routine {} is frozen", routine_id));
        }

        let run_id = format!("run_{}_{}", routine_id, Utc::now().timestamp_millis());
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO routine_runs (id, routine_id, started_at, status) VALUES (?1, ?2, ?3, 'running')",
            params![run_id, routine_id, now],
        )?;

        let mut prev_action_id: Option<String> = None;
        for step in &routine.steps {
            let action_id = format!("action_{}_{}", step.action_id, &run_id[..8]);
            let action = Action {
                id: action_id.clone(),
                title: format!("[{}] {}", routine.name, step.action_id),
                description: step.condition.clone().unwrap_or_default(),
                status: if prev_action_id.is_some() { ActionStatus::Blocked } else { ActionStatus::Pending },
                priority: 2,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                created_by: Some(format!("routine:{}", routine_id)),
                assigned_to: None,
                project: project.to_string(),
                tags: routine.tags.clone(),
                source_observation_ids: vec![],
                source_memory_ids: vec![],
                result: None,
                parent_id: prev_action_id.clone(),
                metadata: HashMap::new(),
                sketch_id: None,
                crystallized_into: None,
            };
            action_store.create_action(&action)?;

            if let Some(prev_id) = prev_action_id {
                action_store.add_edge(&prev_id, &action_id, crate::types::ActionEdgeType::DependsOn)?;
            }

            prev_action_id = Some(action_id);
        }

        Ok(run_id)
    }

    pub fn get_run_status(&self, action_store: &ActionStore, run_id: &str) -> Result<RoutineRun> {
        let mut stmt = self.conn.prepare("SELECT * FROM routine_runs WHERE id = ?1")?;
        let mut rows = stmt.query(params![run_id])?;
        let row = rows.next()?.ok_or_else(|| anyhow::anyhow!("Run {} not found", run_id))?;

        let routine_id: String = row.get("routine_id")?;
        let started_at = chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("started_at")?)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let completed_at = row.get::<_, Option<String>>("completed_at")?
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let status: String = row.get("status")?;
        let step_results: HashMap<String, String> = serde_json::from_str(&row.get::<_, String>("step_results")?).unwrap_or_default();

        Ok(RoutineRun {
            id: run_id.to_string(),
            routine_id,
            started_at,
            completed_at,
            status,
            step_results,
        })
    }

    pub fn get_routine(&self, id: &str) -> Result<Option<Routine>> {
        let mut stmt = self.conn.prepare("SELECT * FROM routines WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_routine(row)?))
        } else {
            Ok(None)
        }
    }
}

impl RoutineStore {
    fn row_to_routine(&self, row: &rusqlite::Row) -> rusqlite::Result<Routine> {
        Ok(Routine {
            id: row.get("id")?,
            name: row.get("name")?,
            description: row.get("description")?,
            steps: serde_json::from_str(&row.get::<_, String>("steps")?).unwrap_or_default(),
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("created_at")?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("updated_at")?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            frozen: row.get::<_, i64>("frozen")? != 0,
            tags: serde_json::from_str(&row.get::<_, String>("tags")?).unwrap_or_default(),
            source_procedural_ids: serde_json::from_str(&row.get::<_, String>("source_procedural_ids")?).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ActionStatus;

    fn test_stores() -> (ActionStore, RoutineStore) {
        let action_store = ActionStore::open(Path::new(":memory:")).unwrap();
        let routine_store = RoutineStore::open(Path::new(":memory:")).unwrap();
        (action_store, routine_store)
    }

    #[test]
    fn test_create_routine() {
        let (_action_store, routine_store) = test_stores();
        let routine = Routine {
            id: "r-1".to_string(),
            name: "Test Routine".to_string(),
            description: "A test routine".to_string(),
            steps: vec![
                RoutineStep { action_id: "step-1".to_string(), order: 0, condition: None },
                RoutineStep { action_id: "step-2".to_string(), order: 1, condition: Some("if needed".to_string()) },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            frozen: false,
            tags: vec!["test".to_string()],
            source_procedural_ids: vec![],
        };
        routine_store.create_routine(&routine).unwrap();

        let retrieved = routine_store.get_routine("r-1").unwrap().unwrap();
        assert_eq!(retrieved.name, "Test Routine");
        assert_eq!(retrieved.steps.len(), 2);
    }

    #[test]
    fn test_run_routine_creates_actions() {
        let (action_store, routine_store) = test_stores();
        let routine = Routine {
            id: "r-1".to_string(),
            name: "Deploy".to_string(),
            description: "Deploy routine".to_string(),
            steps: vec![
                RoutineStep { action_id: "build".to_string(), order: 0, condition: None },
                RoutineStep { action_id: "test".to_string(), order: 1, condition: None },
                RoutineStep { action_id: "deploy".to_string(), order: 2, condition: None },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            frozen: false,
            tags: vec![],
            source_procedural_ids: vec![],
        };
        routine_store.create_routine(&routine).unwrap();

        let run_id = routine_store.run_routine(&action_store, "r-1", "test-project").unwrap();
        assert!(run_id.starts_with("run_r-1_"));

        let actions = action_store.list_actions(Some("test-project"), None).unwrap();
        assert_eq!(actions.len(), 3);
        assert!(actions.iter().any(|a| a.status == ActionStatus::Pending));
        assert!(actions.iter().filter(|a| a.status == ActionStatus::Blocked).count() >= 2);
    }

    #[test]
    fn test_frozen_routine_cannot_run() {
        let (action_store, routine_store) = test_stores();
        let routine = Routine {
            id: "r-1".to_string(),
            name: "Frozen".to_string(),
            description: "".to_string(),
            steps: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            frozen: true,
            tags: vec![],
            source_procedural_ids: vec![],
        };
        routine_store.create_routine(&routine).unwrap();

        let result = routine_store.run_routine(&action_store, "r-1", "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_run_status() {
        let (action_store, routine_store) = test_stores();
        let routine = Routine {
            id: "r-1".to_string(),
            name: "Test".to_string(),
            description: "".to_string(),
            steps: vec![RoutineStep { action_id: "step-1".to_string(), order: 0, condition: None }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            frozen: false,
            tags: vec![],
            source_procedural_ids: vec![],
        };
        routine_store.create_routine(&routine).unwrap();

        let run_id = routine_store.run_routine(&action_store, "r-1", "test").unwrap();
        let status = routine_store.get_run_status(&action_store, &run_id).unwrap();
        assert_eq!(status.status, "running");
        assert_eq!(status.routine_id, "r-1");
    }
}
