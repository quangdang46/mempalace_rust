//! Replay — port of upstream `replay.ts`.
//!
//! JSONL import from ~/.claude/projects for session replay.

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single JSONL line from Claude Code session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeSessionLine {
    #[serde(rename = "type")]
    pub line_type: String,
    pub timestamp: Option<String>,
    pub message: Option<serde_json::Value>,
    pub session_id: Option<String>,
}

/// Imported session from JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedSession {
    pub id: String,
    pub project: String,
    pub observations: Vec<crate::types::CompressedObservation>,
    pub message_count: usize,
}

/// Parse a JSONL file into session lines.
pub fn parse_jsonl(path: &Path) -> Result<Vec<ClaudeSessionLine>> {
    let content = std::fs::read_to_string(path)?;
    let mut lines = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ClaudeSessionLine>(line) {
            Ok(parsed) => lines.push(parsed),
            Err(e) => {
                // Skip malformed lines
                tracing::warn!("Skipping malformed JSONL line: {}", e);
            }
        }
    }

    Ok(lines)
}

/// Import sessions from a Claude Code JSONL file.
pub fn import_jsonl(
    path: &Path,
    project: &str,
) -> Result<ImportedSession> {
    let lines = parse_jsonl(path)?;

    let session_id = lines.iter()
        .find_map(|l| l.session_id.clone())
        .unwrap_or_else(|| format!("imported-{}", Utc::now().timestamp()));

    let mut observations = Vec::new();
    let mut message_count = 0;

    for (i, line) in lines.iter().enumerate() {
        message_count += 1;

        if line.line_type == "assistant" || line.line_type == "user" {
            if let Some(ref msg) = line.message {
                let content = msg.get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");

                if !content.is_empty() {
                    observations.push(crate::types::CompressedObservation {
                        id: format!("replay-{}-{}", session_id, i),
                        session_id: session_id.clone(),
                        timestamp: line.timestamp.as_ref()
                            .and_then(|t| t.parse().ok())
                            .unwrap_or(Utc::now()),
                        observation_type: if line.line_type == "user" {
                            crate::types::ObservationType::UserPrompt
                        } else {
                            crate::types::ObservationType::AssistantResponse
                        },
                        title: format!("{} message #{}", line.line_type, i),
                        subtitle: None,
                        facts: vec![],
                        narrative: content.to_string(),
                        concepts: vec![],
                        files: vec![],
                        importance: 3,
                        confidence: 0.5,
                        image_ref: None,
                        image_description: None,
                        modality: "text".to_string(),
                        agent_id: None,
                    });
                }
            }
        }
    }

    Ok(ImportedSession {
        id: session_id,
        project: project.to_string(),
        observations,
        message_count,
    })
}

/// Find all JSONL files in ~/.claude/projects.
pub fn find_jsonl_files() -> Result<Vec<std::path::PathBuf>> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());

    let claude_dir = Path::new(&home).join(".claude").join("projects");
    if !claude_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&claude_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            for sub_entry in std::fs::read_dir(&path)? {
                let sub_entry = sub_entry?;
                let sub_path = sub_entry.path();
                if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    files.push(sub_path);
                }
            }
        }
    }

    Ok(files)
}

/// Load all sessions from JSONL files.
pub fn load_all_sessions() -> Result<Vec<ImportedSession>> {
    let files = find_jsonl_files()?;
    let mut sessions = Vec::new();

    for file in files {
        let project = file.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        match import_jsonl(&file, project) {
            Ok(session) => sessions.push(session),
            Err(e) => {
                tracing::warn!("Failed to import {}: {}", file.display(), e);
            }
        }
    }

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jsonl() {
        let dir = std::env::temp_dir().join(format!("replay_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");

        let content = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hello"},"session_id":"s-1"}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":"world"},"session_id":"s-1"}"#;
        std::fs::write(&path, content).unwrap();

        let lines = parse_jsonl(&path).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_type, "user");
        assert_eq!(lines[1].line_type, "assistant");
    }

    #[test]
    fn test_import_jsonl() {
        let dir = std::env::temp_dir().join(format!("import_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");

        let content = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"Fix the auth bug"},"session_id":"s-1"}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":"I'll investigate the auth issue"},"session_id":"s-1"}"#;
        std::fs::write(&path, content).unwrap();

        let session = import_jsonl(&path, "test-project").unwrap();
        assert_eq!(session.id, "s-1");
        assert_eq!(session.project, "test-project");
        assert_eq!(session.message_count, 2);
        assert!(!session.observations.is_empty());
    }

    #[test]
    fn test_find_jsonl_files_no_dir() {
        // ~/.claude/projects likely doesn't exist in test env
        let files = find_jsonl_files();
        assert!(files.is_ok());
    }

    #[test]
    fn test_parse_jsonl_skip_malformed() {
        let dir = std::env::temp_dir().join(format!("malformed_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");

        let content = r#"{"type":"user","message":{"content":"good"},"session_id":"s-1"}
not valid json
{"type":"assistant","message":{"content":"also good"},"session_id":"s-1"}"#;
        std::fs::write(&path, content).unwrap();

        let lines = parse_jsonl(&path).unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_import_empty_jsonl() {
        let dir = std::env::temp_dir().join(format!("empty_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, "").unwrap();

        let session = import_jsonl(&path, "test").unwrap();
        assert!(session.observations.is_empty());
        assert_eq!(session.message_count, 0);
    }

    #[test]
    fn test_claude_session_line_deser() {
        let json = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"test"},"session_id":"s-1"}"#;
        let line: ClaudeSessionLine = serde_json::from_str(json).unwrap();
        assert_eq!(line.line_type, "user");
        assert_eq!(line.session_id, Some("s-1".to_string()));
    }
}
