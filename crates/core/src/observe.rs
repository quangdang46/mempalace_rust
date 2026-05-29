//! Event-driven observation capture — port of upstream `observe.ts`.
//!
//! Core entry point for capturing lifecycle hook events, extracting images,
//! deduplication, and triggering auto-compress.

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::types::{CompressedObservation, HookPayload, HookType, ObservationType, RawObservation};

/// Deduplication map using SHA-256 fingerprints.
#[derive(Debug, Clone, Default)]
pub struct DedupMap {
    fingerprints: HashSet<String>,
}

impl DedupMap {
    pub fn new() -> Self {
        Self {
            fingerprints: HashSet::new(),
        }
    }

    pub fn is_duplicate(&self, fingerprint: &str) -> bool {
        self.fingerprints.contains(fingerprint)
    }

    pub fn add(&mut self, fingerprint: String) -> bool {
        self.fingerprints.insert(fingerprint)
    }

    pub fn clear(&mut self) {
        self.fingerprints.clear();
    }
}

/// Extract image data from a hook payload's data fields.
pub fn extract_image(data: &serde_json::Value) -> Option<String> {
    if let Some(obj) = data.as_object() {
        // Check known keys first
        for key in &["image_data", "image_path", "imageBase64", "imagePath"] {
            if let Some(val) = obj.get(*key) {
                if let Some(s) = val.as_str() {
                    if is_image_data(s) || is_image_path(s) {
                        return Some(s.to_string());
                    }
                }
            }
        }
        // Recursive search
        for val in obj.values() {
            if let Some(found) = extract_image(val) {
                return Some(found);
            }
        }
    }
    if let Some(s) = data.as_str() {
        if is_image_data(s) {
            return Some(s.to_string());
        }
    }
    None
}

fn is_image_data(s: &str) -> bool {
    s.starts_with("data:image/") || s.starts_with("iVBORw0KGgo") || s.starts_with("/9j/")
}

fn is_image_path(s: &str) -> bool {
    s.ends_with(".png") || s.ends_with(".jpg") || s.ends_with(".jpeg")
        || s.ends_with(".webp") || s.ends_with(".gif")
}

/// Create a fingerprint for deduplication.
pub fn fingerprint_observation(payload: &HookPayload) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(payload.session_id.as_bytes());
    hasher.update(format!("{:?}", payload.hook_type).as_bytes());
    hasher.update(serde_json::to_string(&payload.data).unwrap_or_default().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Process a hook payload into a raw observation.
pub fn process_observation(payload: &HookPayload) -> Result<RawObservation> {
    let id = format!("obs-{}", short_id(&payload.session_id, &payload.timestamp));
    let image_data = extract_image(&serde_json::Value::Object(payload.data.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
        .map(|s| crate::types::ImageData {
            base64: if is_image_data(&s) { Some(s.clone()) } else { None },
            path: if is_image_path(&s) { Some(s.clone()) } else { None },
            mime_type: "image/png".to_string(),
            description: None,
        });

    Ok(RawObservation {
        id,
        session_id: payload.session_id.clone(),
        timestamp: payload.timestamp,
        hook_type: payload.hook_type,
        tool_name: payload.data.get("toolName").and_then(|v| v.as_str()).map(String::from),
        tool_input: payload.data.get("toolInput").map(|v| v.to_string()),
        tool_output: payload.data.get("toolOutput").map(|v| v.to_string()),
        user_prompt: payload.data.get("userPrompt").and_then(|v| v.as_str()).map(String::from),
        assistant_response: payload.data.get("assistantResponse").and_then(|v| v.as_str()).map(String::from),
        raw: Some(serde_json::to_string(&payload.data).unwrap_or_default()),
        modality: if image_data.is_some() { "mixed".to_string() } else { "text".to_string() },
        image_data,
        agent_id: payload.data.get("agentId").and_then(|v| v.as_str()).map(String::from),
    })
}

fn short_id(session_id: &str, timestamp: &chrono::DateTime<Utc>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(timestamp.to_rfc3339().as_bytes());
    let hex = format!("{:x}", hasher.finalize());
    hex[..8].to_string()
}

/// Store for observations backed by SQLite.
pub struct ObservationStore {
    conn: rusqlite::Connection,
    max_per_session: usize,
}

impl ObservationStore {
    pub fn open(db_path: &std::path::Path, max_per_session: usize) -> Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS observations (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                hook_type TEXT NOT NULL,
                tool_name TEXT,
                tool_input TEXT,
                tool_output TEXT,
                user_prompt TEXT,
                assistant_response TEXT,
                raw TEXT,
                modality TEXT DEFAULT 'text',
                image_data TEXT,
                agent_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id);
            CREATE INDEX IF NOT EXISTS idx_obs_timestamp ON observations(timestamp);",
        )?;
        Ok(Self { conn, max_per_session })
    }

    pub fn save(&self, obs: &RawObservation) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO observations (
                id, session_id, timestamp, hook_type, tool_name, tool_input,
                tool_output, user_prompt, assistant_response, raw, modality,
                image_data, agent_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                obs.id, obs.session_id, obs.timestamp.to_rfc3339(),
                format!("{:?}", obs.hook_type), obs.tool_name, obs.tool_input,
                obs.tool_output, obs.user_prompt, obs.assistant_response,
                obs.raw, obs.modality,
                obs.image_data.as_ref().and_then(|i| i.base64.as_ref().or(i.path.as_ref())).cloned(),
                obs.agent_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_for_session(&self, session_id: &str) -> Result<Vec<RawObservation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, timestamp, hook_type, tool_name, tool_input,
             tool_output, user_prompt, assistant_response, raw, modality,
             image_data, agent_id FROM observations WHERE session_id = ?1
             ORDER BY timestamp DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, self.max_per_session], |row| {
            let ts: String = row.get(2)?;
            Ok(RawObservation {
                id: row.get(0)?,
                session_id: row.get(1)?,
                timestamp: ts.parse().unwrap_or(Utc::now()),
                hook_type: row.get::<_, String>(3)?.parse().unwrap_or(HookType::Notification),
                tool_name: row.get(4)?,
                tool_input: row.get(5)?,
                tool_output: row.get(6)?,
                user_prompt: row.get(7)?,
                assistant_response: row.get(8)?,
                raw: row.get(9)?,
                modality: row.get(10)?,
                image_data: row.get::<_, Option<String>>(11)?.map(|s| crate::types::ImageData {
                    base64: if is_image_data(&s) { Some(s.clone()) } else { None },
                    path: if is_image_path(&s) { Some(s.clone()) } else { None },
                    mime_type: "image/png".to_string(),
                    description: None,
                }),
                agent_id: row.get(12)?,
            })
        })?;
        rows.map(|r| r.map_err(|e| anyhow::anyhow!(e))).collect()
    }

    pub fn count_for_session(&self, session_id: &str) -> Result<usize> {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM observations WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<usize> {
        let changed = self.conn.execute(
            "DELETE FROM observations WHERE session_id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_dedup_map() {
        let mut map = DedupMap::new();
        assert!(!map.is_duplicate("abc"));
        map.add("abc".to_string());
        assert!(map.is_duplicate("abc"));
        assert!(!map.is_duplicate("def"));
    }

    #[test]
    fn test_extract_image_data_uri() {
        let data = serde_json::json!({"image_data": "data:image/png;base64,abc123"});
        let result = extract_image(&data);
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("data:image/"));
    }

    #[test]
    fn test_extract_image_path() {
        let data = serde_json::json!({"image_path": "/tmp/screenshot.png"});
        let result = extract_image(&data);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "/tmp/screenshot.png");
    }

    #[test]
    fn test_extract_image_nested() {
        let data = serde_json::json!({"tool": {"input": {"imageBase64": "iVBORw0KGgo"}}});
        let result = extract_image(&data);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_no_image() {
        let data = serde_json::json!({"query": "hello"});
        let result = extract_image(&data);
        assert!(result.is_none());
    }

    #[test]
    fn test_fingerprint_observation() {
        let mut data = HashMap::new();
        data.insert("toolName".to_string(), serde_json::json!("Read"));
        let payload = HookPayload {
            hook_type: HookType::PostToolUse,
            session_id: "s-1".to_string(),
            project: "test".to_string(),
            cwd: "/tmp".to_string(),
            timestamp: Utc::now(),
            data,
        };
        let fp1 = fingerprint_observation(&payload);
        let fp2 = fingerprint_observation(&payload);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_observation_store_crud() {
        let dir = std::env::temp_dir().join(format!("obs_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("obs.db");

        let store = ObservationStore::open(&db_path, 100).unwrap();

        let mut data = HashMap::new();
        data.insert("toolName".to_string(), serde_json::json!("Read"));
        let payload = HookPayload {
            hook_type: HookType::PostToolUse,
            session_id: "s-1".to_string(),
            project: "test".to_string(),
            cwd: "/tmp".to_string(),
            timestamp: Utc::now(),
            data,
        };
        let obs = process_observation(&payload).unwrap();
        store.save(&obs).unwrap();

        let results = store.list_for_session("s-1").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s-1");

        let count = store.count_for_session("s-1").unwrap();
        assert_eq!(count, 1);

        let deleted = store.delete_session("s-1").unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn test_process_observation_with_image() {
        let mut data = HashMap::new();
        data.insert("image_data".to_string(), serde_json::json!("data:image/png;base64,abc"));
        let payload = HookPayload {
            hook_type: HookType::PostToolUse,
            session_id: "s-1".to_string(),
            project: "test".to_string(),
            cwd: "/tmp".to_string(),
            timestamp: Utc::now(),
            data,
        };
        let obs = process_observation(&payload).unwrap();
        assert_eq!(obs.modality, "mixed");
        assert!(obs.image_data.is_some());
    }
}
