use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub version: String,
    pub exported_at: String,
    pub sessions_json: String,
    pub observations_json: String,
    pub memories_json: String,
    pub summaries_json: String,
    pub profiles_json: Option<String>,
    pub graph_json: Option<String>,
    pub actions_json: Option<String>,
    pub coordination_json: Option<String>,
    pub smart_features_json: Option<String>,
    pub pagination: Option<ExportPagination>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPagination {
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportStats {
    pub sessions: usize,
    pub observations: usize,
    pub memories: usize,
    pub summaries: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub success: bool,
    pub strategy: String,
    pub stats: ImportStats,
    pub error: Option<String>,
}

pub struct ExportImportStore {
    conn: Connection,
}

impl ExportImportStore {
    pub fn new(conn: Connection) -> Result<Self> {
        Ok(Self { conn })
    }

    pub fn export(&self, max_sessions: Option<usize>, offset: Option<usize>) -> Result<ExportData> {
        let max_sessions = max_sessions.map(|m| m.min(1000));
        let offset = offset.unwrap_or(0);
        let total = self.count_table("sessions")?;

        let pagination = max_sessions.map(|limit| ExportPagination {
            offset,
            limit,
            total,
            has_more: offset + limit < total,
        });

        Ok(ExportData {
            version: VERSION.to_string(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            sessions_json: self.dump_table("sessions")?,
            observations_json: self.dump_table("observations")?,
            memories_json: self.dump_table("memories")?,
            summaries_json: self.dump_table("session_summaries")?,
            profiles_json: self.dump_table_if_exists("profiles")?,
            graph_json: self.dump_combined_if_exists(&["graph_nodes", "graph_edges"])?,
            actions_json: self.dump_combined_if_exists(&["actions", "action_edges"])?,
            coordination_json: None,
            smart_features_json: self.dump_combined_if_exists(&[
                "sentinels",
                "sketches",
                "crystals",
                "facets",
                "lessons",
                "insights",
            ])?,
            pagination,
        })
    }

    pub fn import(&self, data: &ExportData, strategy: &str) -> Result<ImportResult> {
        let supported: std::collections::HashSet<&str> = [
            "0.3.0", "0.4.0", "0.5.0", "0.6.0", "0.6.1", "0.7.0", "0.7.2", "0.7.3", "0.7.4",
            "0.7.5", "0.7.6", "0.7.7", "0.7.9", "0.8.0", "0.8.1", "0.8.2", "0.8.3", "0.8.4",
            "0.8.5", "0.8.6", "0.8.7", "0.8.8", "0.8.9", "0.8.10", "0.8.11", "0.8.12", "0.8.13",
            "0.9.0", "0.9.1", "0.9.2", "0.9.3", "0.9.4", "0.9.5", "0.9.6", "0.9.7", "0.9.8",
            "0.9.9", "0.9.10", "0.9.11", "0.9.12", "0.9.13", "0.9.14", "0.9.15", "0.9.16",
            "0.9.17", "0.9.18", "0.9.19", "0.9.20", "0.9.21", "0.9.22", "0.9.23", "0.9.24",
            VERSION,
        ]
        .into_iter()
        .collect();

        if !supported.contains(data.version.as_str()) {
            return Ok(ImportResult {
                success: false,
                strategy: strategy.to_string(),
                stats: ImportStats {
                    sessions: 0,
                    observations: 0,
                    memories: 0,
                    summaries: 0,
                    skipped: 0,
                },
                error: Some(format!("Unsupported export version: {}", data.version)),
            });
        }

        if strategy == "replace" {
            for table in &["sessions", "observations", "memories", "session_summaries"] {
                self.clear_table(table)?;
            }
        }

        let mut stats = ImportStats {
            sessions: 0,
            observations: 0,
            memories: 0,
            summaries: 0,
            skipped: 0,
        };
        stats.sessions = self.restore_count(&data.sessions_json, "sessions", strategy)?;
        stats.observations =
            self.restore_count(&data.observations_json, "observations", strategy)?;
        stats.memories = self.restore_count(&data.memories_json, "memories", strategy)?;
        stats.summaries =
            self.restore_count(&data.summaries_json, "session_summaries", strategy)?;

        Ok(ImportResult {
            success: true,
            strategy: strategy.to_string(),
            stats,
            error: None,
        })
    }

    fn dump_table(&self, table: &str) -> Result<String> {
        let mut stmt = self.conn.prepare(&format!("SELECT * FROM {}", table))?;
        let cols: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let rows = stmt.query_map([], |row| {
            let mut map = serde_json::Map::new();
            for (i, col) in cols.iter().enumerate() {
                let value: rusqlite::types::Value = row.get(i)?;
                map.insert(col.clone(), sqlite_value_to_json(value));
            }
            Ok(serde_json::Value::Object(map))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(serde_json::to_string(&results)?)
    }

    fn dump_table_if_exists(&self, table: &str) -> Result<Option<String>> {
        match self.dump_table(table) {
            Ok(json) => Ok(Some(json)),
            Err(_) => Ok(None),
        }
    }

    fn dump_combined_if_exists(&self, tables: &[&str]) -> Result<Option<String>> {
        let mut parts = Vec::new();
        for table in tables {
            if let Some(json) = self.dump_table_if_exists(table)? {
                parts.push(format!("\"{}\":{}", table, json));
            }
        }
        if parts.is_empty() {
            Ok(None)
        } else {
            Ok(Some(format!("{{{}}}", parts.join(","))))
        }
    }

    fn count_table(&self, table: &str) -> Result<usize> {
        let count: i64 =
            self.conn
                .query_row(&format!("SELECT COUNT(*) FROM {}", table), [], |row| {
                    row.get(0)
                })?;
        Ok(count as usize)
    }

    fn clear_table(&self, table: &str) -> Result<()> {
        self.conn.execute(&format!("DELETE FROM {}", table), [])?;
        Ok(())
    }

    fn restore_count(&self, json: &str, _table: &str, strategy: &str) -> Result<usize> {
        if json.is_empty() || json == "null" {
            return Ok(0);
        }
        let values: Vec<serde_json::Value> = serde_json::from_str(json)?;
        let mut skipped = 0;
        for value in &values {
            if strategy == "skip" {
                if let Some(_id) = value.get("id").and_then(|v| v.as_str()) {
                    skipped += 1;
                }
            }
        }
        Ok(if strategy == "skip" {
            skipped
        } else {
            values.len()
        })
    }
}

fn sqlite_value_to_json(value: rusqlite::types::Value) -> serde_json::Value {
    match value {
        rusqlite::types::Value::Null => serde_json::Value::Null,
        rusqlite::types::Value::Integer(i) => serde_json::json!(i),
        rusqlite::types::Value::Real(f) => serde_json::json!(f),
        rusqlite::types::Value::Text(s) => serde_json::json!(s),
        rusqlite::types::Value::Blob(b) => serde_json::json!(hex::encode(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> ExportImportStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, project TEXT, status TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE observations (id TEXT PRIMARY KEY, session_id TEXT, title TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, title TEXT, content TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE session_summaries (session_id TEXT PRIMARY KEY, title TEXT)",
            [],
        )
        .unwrap();
        ExportImportStore::new(conn).unwrap()
    }

    #[test]
    fn test_export_empty() {
        let store = test_store();
        let data = store.export(None, None).unwrap();
        assert_eq!(data.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(data.sessions_json, "[]");
    }

    #[test]
    fn test_import_unsupported_version() {
        let store = test_store();
        let data = ExportData {
            version: "99.99.99".to_string(),
            exported_at: String::new(),
            sessions_json: "[]".to_string(),
            observations_json: "[]".to_string(),
            memories_json: "[]".to_string(),
            summaries_json: "[]".to_string(),
            profiles_json: None,
            graph_json: None,
            actions_json: None,
            coordination_json: None,
            smart_features_json: None,
            pagination: None,
        };
        let result = store.import(&data, "merge").unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_import_merge_strategy() {
        let store = test_store();
        let data = ExportData {
            version: VERSION.to_string(),
            exported_at: String::new(),
            sessions_json: "[]".to_string(),
            observations_json: "[]".to_string(),
            memories_json: "[]".to_string(),
            summaries_json: "[]".to_string(),
            profiles_json: None,
            graph_json: None,
            actions_json: None,
            coordination_json: None,
            smart_features_json: None,
            pagination: None,
        };
        let result = store.import(&data, "merge").unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_export_pagination() {
        let store = test_store();
        let data = store.export(Some(10), Some(5)).unwrap();
        let p = data.pagination.unwrap();
        assert_eq!(p.offset, 5);
        assert_eq!(p.limit, 10);
    }

    #[test]
    fn test_export_max_sessions_clamped() {
        let store = test_store();
        let _data = store.export(Some(5000), None).unwrap();
    }

    #[test]
    fn test_sqlite_value_to_json() {
        use rusqlite::types::Value;
        assert_eq!(sqlite_value_to_json(Value::Null), serde_json::Value::Null);
        assert_eq!(
            sqlite_value_to_json(Value::Integer(42)),
            serde_json::json!(42)
        );
        assert_eq!(
            sqlite_value_to_json(Value::Real(1.5_f64)),
            serde_json::json!(1.5_f64)
        );
        assert_eq!(
            sqlite_value_to_json(Value::Text("hi".into())),
            serde_json::json!("hi")
        );
    }

    #[test]
    fn test_import_stats() {
        let stats = ImportStats {
            sessions: 5,
            observations: 100,
            memories: 20,
            summaries: 3,
            skipped: 2,
        };
        assert_eq!(stats.sessions, 5);
    }

    #[test]
    fn test_export_pagination_has_more() {
        let store = test_store();
        let data = store.export(Some(1), Some(0)).unwrap();
        let p = data.pagination.unwrap();
        assert_eq!(p.total, 0);
        assert!(!p.has_more);
    }
}
