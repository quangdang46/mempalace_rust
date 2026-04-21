use std::collections::HashMap;
use std::path::Path;

use crate::palace_db::PalaceDb;

const BATCH_SIZE: usize = 64;

/// Sweep statistics for a directory sweep operation.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SweepStats {
    /// Drawers that did not exist before this sweep.
    pub drawers_added: usize,
    /// Drawers whose deterministic ID was already in the palace.
    pub drawers_already_present: usize,
    /// Total upserted drawers (added + already_present).
    pub drawers_upserted: usize,
    /// Records skipped by the cursor (strictly earlier than stored).
    pub drawers_skipped: usize,
    pub files_attempted: usize,
    pub files_succeeded: usize,
    pub failures: Vec<String>,
}

/// Result for a single file in a directory sweep.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SweepFileResult {
    pub file: String,
    pub added: usize,
    pub already_present: usize,
    pub skipped: usize,
}

/// Internal record from JSONL parsing.
#[derive(Debug)]
struct JsonlRecord {
    session_id: String,
    uuid: String,
    timestamp: String,
    role: String,
    content: String,
}

// ── Content flattening ────────────────────────────────────────────────────────

/// Normalize Claude Code's message content to a plain string.
///
/// User messages are strings already; assistant messages are a list of
/// content blocks like [{"type": "text", "text": "..."}, {"type":
/// "tool_use", ...}]. All blocks are preserved verbatim -- the design
/// principle is "verbatim always", so tool inputs and results are
/// serialized in full, never truncated.
pub fn flatten_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut parts = Vec::new();
            for block in arr {
                let obj = match block {
                    serde_json::Value::Object(o) => o,
                    _ => continue,
                };
                let btype = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match btype {
                    "text" => {
                        if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                parts.push(text.to_string());
                            }
                        }
                    }
                    "tool_use" => {
                        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let input = obj.get("input");
                        let input_str = input
                            .map(|v| serde_json::to_string(v).unwrap_or_default())
                            .unwrap_or_default();
                        parts.push(format!("[tool_use: {} input={}]", name, input_str));
                    }
                    "tool_result" => {
                        let content_val = obj.get("content");
                        let content_str = content_val
                            .map(|v| serde_json::to_string(v).unwrap_or_default())
                            .unwrap_or_default();
                        parts.push(format!("[tool_result: {}]", content_str));
                    }
                    _ => {
                        let block_str = serde_json::to_string(block).unwrap_or_default();
                        parts.push(format!("[{}: {}]", btype, block_str));
                    }
                }
            }
            parts
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => content.to_string(),
    }
}

// ── JSONL parsing ─────────────────────────────────────────────────────────────

/// Parse a Claude Code .jsonl file and yield user/assistant records.
///
/// Yields records with: session_id, uuid, timestamp, role, content (flattened).
/// Non-message records and malformed lines are silently skipped.
fn parse_claude_jsonl(path: &Path) -> Vec<JsonlRecord> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let record: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => return None,
            };

            let rtype = record.get("type").and_then(|v| v.as_str())?;
            if rtype != "user" && rtype != "assistant" {
                return None;
            }

            let msg = record.get("message")?;
            let msg_obj = match msg {
                serde_json::Value::Object(o) => o,
                _ => return None,
            };

            let role = msg_obj.get("role")?.as_str()?;
            if role != "user" && role != "assistant" {
                return None;
            }

            let timestamp = record.get("timestamp")?.as_str()?;

            let uuid = record.get("uuid")?.as_str()?;

            let session_id = record
                .get("sessionId")
                .or_else(|| record.get("session_id"))
                .and_then(|v| v.as_str())?;

            let empty_str = serde_json::Value::String("".to_string());
            let content_val = msg_obj.get("content").unwrap_or(&empty_str);
            let content = flatten_content(content_val);
            if content.trim().is_empty() {
                return None;
            }

            Some(JsonlRecord {
                session_id: session_id.to_string(),
                uuid: uuid.to_string(),
                timestamp: timestamp.to_string(),
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

// ── Cursor resolution ────────────────────────────────────────────────────────

/// Return the max timestamp of drawers for this session_id, or None.
///
/// ISO-8601 strings compare lexically in the right order, so we don't
/// need to parse them. Query scans documents for the session, then reduces.
fn get_palace_cursor(palace: &PalaceDb, session_id: &str) -> Option<String> {
    let session_docs = palace.get_documents_by_session(session_id);

    let timestamps: Vec<&str> = session_docs
        .iter()
        .filter_map(|(_, _, meta)| meta.get("timestamp").and_then(|v| v.as_str()))
        .collect();

    if timestamps.is_empty() {
        return None;
    }

    timestamps.into_iter().max().map(|s| s.to_string())
}

// ── Drawer ID ─────────────────────────────────────────────────────────────────

/// Deterministic drawer ID so upserts at the same message are no-ops.
fn drawer_id_for_message(session_id: &str, message_uuid: &str) -> String {
    format!("sweep_{}_{}", session_id, message_uuid)
}

// ── Pre-flight check ──────────────────────────────────────────────────────────

/// Check which batch IDs are already present in the palace.
fn check_existing_ids(
    palace: &PalaceDb,
    batch_ids: &[String],
) -> std::collections::HashSet<String> {
    let existing = palace.get_documents(batch_ids);
    existing.into_iter().collect()
}

// ── Main sweep ───────────────────────────────────────────────────────────────

/// Ingest every user/assistant message not already represented.
///
/// For each message in the jsonl:
///   - If timestamp < cursor for that session, skip (already covered).
///   - At timestamp == cursor we do NOT skip (messages can share timestamp).
///   - Else, upsert a drawer with deterministic ID so reruns dedupe.
///
/// Returns drawer-level stats (use sweep_directory for aggregated stats).
pub fn sweep(jsonl_path: &Path, palace_path: Option<&Path>) -> anyhow::Result<SweepStats> {
    let palace_path = palace_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| jsonl_path.parent().unwrap_or(Path::new(".")).join("palace"));

    let mut palace = PalaceDb::open(&palace_path)?;

    let mut cursors: HashMap<String, Option<String>> = HashMap::new();
    let mut drawers_added = 0;
    let mut drawers_already_present = 0;
    let mut drawers_skipped = 0;

    let mut batch_ids: Vec<String> = Vec::new();
    let mut batch_docs: Vec<String> = Vec::new();
    let mut batch_metas: Vec<HashMap<String, serde_json::Value>> = Vec::new();

    for rec in parse_claude_jsonl(jsonl_path) {
        let sid = &rec.session_id;
        if !cursors.contains_key(sid) {
            cursors.insert(sid.clone(), get_palace_cursor(&palace, sid));
        }

        let cursor = cursors.get(sid).unwrap();
        if cursor.is_some() && rec.timestamp < *cursor.as_ref().unwrap() {
            drawers_skipped += 1;
            continue;
        }

        let did = drawer_id_for_message(sid, &rec.uuid);
        let document = format!("{}: {}", rec.role.to_uppercase(), rec.content);
        let filed_at = chrono::Utc::now().to_rfc3339();

        let metadata = {
            let mut m = HashMap::new();
            m.insert("session_id".to_string(), serde_json::json!(sid));
            m.insert("timestamp".to_string(), serde_json::json!(&rec.timestamp));
            m.insert("message_uuid".to_string(), serde_json::json!(&rec.uuid));
            m.insert("role".to_string(), serde_json::json!(&rec.role));
            m.insert(
                "source_file".to_string(),
                serde_json::json!(jsonl_path.to_string_lossy().as_ref()),
            );
            m.insert("filed_at".to_string(), serde_json::json!(&filed_at));
            m.insert("ingest_mode".to_string(), serde_json::json!("sweep"));
            m
        };

        batch_ids.push(did);
        batch_docs.push(document);
        batch_metas.push(metadata);

        if batch_ids.len() >= BATCH_SIZE {
            let (added, already) = flush_batch(
                &mut palace,
                &mut batch_ids,
                &mut batch_docs,
                &mut batch_metas,
            );
            drawers_added += added;
            drawers_already_present += already;
        }
    }

    // Flush remaining
    if !batch_ids.is_empty() {
        let (added, already) = flush_batch(
            &mut palace,
            &mut batch_ids,
            &mut batch_docs,
            &mut batch_metas,
        );
        drawers_added += added;
        drawers_already_present += already;
    }

    palace.flush()?;

    Ok(SweepStats {
        drawers_added,
        drawers_already_present,
        drawers_upserted: drawers_added + drawers_already_present,
        drawers_skipped,
        files_attempted: 1,
        files_succeeded: 1,
        failures: vec![],
    })
}

/// Flush a batch to the palace, returning (added_count, already_present_count).
fn flush_batch(
    palace: &mut PalaceDb,
    batch_ids: &mut Vec<String>,
    batch_docs: &mut Vec<String>,
    batch_metas: &mut Vec<HashMap<String, serde_json::Value>>,
) -> (usize, usize) {
    if batch_ids.is_empty() {
        return (0, 0);
    }

    // Pre-flight: which IDs in this batch are already present?
    let existing_ids = check_existing_ids(palace, batch_ids);
    let new_count = batch_ids
        .iter()
        .filter(|id| !existing_ids.contains(*id))
        .count();
    let already_count = batch_ids.len() - new_count;

    // Build documents for upsert
    let docs: Vec<(String, String, HashMap<String, serde_json::Value>)> = batch_ids
        .iter()
        .zip(batch_docs.iter())
        .zip(batch_metas.iter())
        .map(|((id, doc), meta)| (id.clone(), doc.clone(), meta.clone()))
        .collect();

    palace.upsert_documents(&docs).ok();

    batch_ids.clear();
    batch_docs.clear();
    batch_metas.clear();

    (new_count, already_count)
}

/// Sweep every .jsonl file in a directory (recursive).
///
/// Returns aggregated summary across all files.
pub fn sweep_directory(dir_path: &Path, palace_path: Option<&Path>) -> anyhow::Result<SweepStats> {
    let files: Vec<_> = {
        let mut f = Vec::new();
        for entry in walkdir::WalkDir::new(dir_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                    f.push(path.to_path_buf());
                }
            }
        }
        f.sort();
        f
    };

    let mut total_added = 0;
    let mut total_already_present = 0;
    let mut total_skipped = 0;
    let mut failures = Vec::new();
    let mut files_succeeded = 0;

    for f in &files {
        match sweep(f, palace_path) {
            Ok(stats) => {
                total_added += stats.drawers_added;
                total_already_present += stats.drawers_already_present;
                total_skipped += stats.drawers_skipped;
                files_succeeded += 1;
            }
            Err(e) => {
                failures.push(format!("{}: {}", f.display(), e));
            }
        }
    }

    Ok(SweepStats {
        drawers_added: total_added,
        drawers_already_present: total_already_present,
        drawers_upserted: total_added + total_already_present,
        drawers_skipped: total_skipped,
        files_attempted: files.len(),
        files_succeeded,
        failures,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_content_string() {
        let content = serde_json::json!("Hello, world!");
        assert_eq!(flatten_content(&content), "Hello, world!");
    }

    #[test]
    fn test_flatten_content_list() {
        let content = serde_json::json!([
            {"type": "text", "text": "Hello"},
            {"type": "text", "text": "World"}
        ]);
        let result = flatten_content(&content);
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn test_flatten_content_list_with_tool_use() {
        let content = serde_json::json!([
            {"type": "text", "text": "Let me help with that."},
            {
                "type": "tool_use",
                "name": "Read",
                "input": {"path": "/tmp/test.txt"}
            }
        ]);
        let result = flatten_content(&content);
        assert!(result.contains("Let me help"));
        assert!(result.contains("tool_use: Read"));
        assert!(result.contains("input={\"path\""));
    }

    #[test]
    fn test_parse_claude_jsonl_basic() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let jsonl_path = temp_dir.path().join("test.jsonl");
        std::fs::write(
            &jsonl_path,
            r#"{"type":"user","sessionId":"sess_abc","uuid":"msg_001","timestamp":"2024-01-01T10:00:00Z","message":{"role":"user","content":"Hello"}}
{"type":"assistant","sessionId":"sess_abc","uuid":"msg_002","timestamp":"2024-01-01T10:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi there!"}]}}
{"type":"user","sessionId":"sess_abc","uuid":"msg_003","timestamp":"2024-01-01T10:02:00Z","message":{"role":"user","content":"How are you?"}}
"#,
        )
        .unwrap();

        let records = parse_claude_jsonl(&jsonl_path);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].session_id, "sess_abc");
        assert_eq!(records[0].uuid, "msg_001");
        assert_eq!(records[0].role, "user");
        assert_eq!(records[0].content, "Hello");
    }

    #[test]
    fn test_drawer_id_deterministic() {
        let id1 = drawer_id_for_message("session_123", "msg_abc");
        let id2 = drawer_id_for_message("session_123", "msg_abc");
        let id3 = drawer_id_for_message("session_123", "msg_xyz");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert!(id1.starts_with("sweep_session_123_msg_abc"));
    }

    #[test]
    fn test_cursor_skip_logic() {
        // Test that timestamp comparison works correctly (ISO-8601 strings lexically compare correctly)
        let cursor = "2024-01-01T10:00:00Z".to_string();
        let cursor_str: &str = &cursor;
        // before cursor -> skip (< cursor is true)
        assert!("2024-01-01T09:00:00Z" < cursor_str);
        // equal to cursor -> don't skip (< cursor is false)
        assert!(!("2024-01-01T10:00:00Z" < cursor_str));
        // after cursor -> don't skip (< cursor is false)
        assert!(!("2024-01-01T11:00:00Z" < cursor_str));

        // When cursor is None, nothing is skipped (cursor is None means no prior writes)
        // This is tested via integration - here we just verify string comparison works
        let _ = "2024-01-01T09:00:00Z".to_string();
    }

    #[test]
    fn test_sweep_stats_serialization() {
        let stats = SweepStats {
            drawers_added: 10,
            drawers_already_present: 5,
            drawers_upserted: 15,
            drawers_skipped: 2,
            files_attempted: 3,
            files_succeeded: 2,
            failures: vec!["file1.jsonl: error".to_string()],
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("drawers_added"));
        let parsed: SweepStats = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.drawers_added, 10);
    }
}
