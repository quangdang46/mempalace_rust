//! hooks_cli.rs — Hook runtime for Claude Code integration.
//!
//! Session-start, stop, and precompact hooks that interface with Claude Code.
//!
//! Usage:
//!     mpr hook save     — save hook for Claude Code stop
//!     mpr hook precompact — precompact hook for Claude Code

use crate::config::Config;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const SAVE_INTERVAL: usize = 15;
const STATE_DIR: &str = ".mempalace/hook_state";

/// Hook data parsed from Claude Code JSON input.
#[derive(Debug, Clone)]
pub struct HookData {
    pub session_id: String,
    pub stop_hook_active: bool,
    pub transcript_path: String,
}

fn sanitize_session_id(session_id: &str) -> String {
    let re = Regex::new(r"[^a-zA-Z0-9_-]").unwrap();
    let sanitized = re.replace_all(session_id, "").to_string();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn expand_path(path_str: &str) -> Option<PathBuf> {
    let p = PathBuf::from(path_str);
    if p.to_string_lossy().starts_with("~") {
        if let Ok(home) = std::env::var("HOME") {
            let path_str = path_str.trim_start_matches("~");
            let path_str = if path_str.starts_with('/') || path_str.starts_with('\\') {
                &path_str[1..]
            } else {
                path_str
            };
            Some(PathBuf::from(home).join(path_str))
        } else {
            None
        }
    } else {
        Some(p)
    }
}

fn validate_transcript_path(transcript_path: &str) -> Option<PathBuf> {
    if transcript_path.is_empty() {
        return None;
    }
    let path = expand_path(transcript_path)?;
    let ext = path.extension()?;
    if ext == "jsonl" || ext == "json" {
        Some(path)
    } else {
        None
    }
}

fn count_human_messages(transcript_path: &str) -> usize {
    let Some(path) = validate_transcript_path(transcript_path) else { return 0 };
    if !path.exists() {
        return 0;
    }

    let Ok(content) = fs::read_to_string(&path) else { return 0 };
    let mut count = 0usize;

    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            // Claude Code format: {"message": {"role": "user", "content": "..."}}
            if let Some(msg) = entry.get("message").and_then(|m| m.as_object()) {
                if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                    let content_val = msg.get("content");
                    if let Some(text) = content_val.and_then(|c| c.as_str()) {
                        if !text.contains("<command-message>") {
                            count += 1;
                            continue;
                        }
                    }
                    if let Some(arr) = content_val.and_then(|c| c.as_array()) {
                        let text: String = arr
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if !text.contains("<command-message>") {
                            count += 1;
                        }
                    }
                }
            }
            // Codex CLI format: {"type": "event_msg", "payload": {"type": "user_message", "message": "..."}}
            if entry.get("type") == Some(&serde_json::json!("event_msg")) {
                if let Some(payload) = entry.get("payload").and_then(|p| p.as_object()) {
                    if payload.get("type") == Some(&serde_json::json!("user_message")) {
                        if let Some(text) = payload.get("message").and_then(|m| m.as_str()) {
                            if !text.contains("<command-message>") {
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    count
}

fn state_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(STATE_DIR)
}

fn last_save_file(session_id: &str) -> PathBuf {
    state_dir().join(format!("{}_last_save", session_id))
}

fn ensure_state_dir() {
    let dir = state_dir();
    fs::create_dir_all(&dir).ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).ok();
    }
}

/// Run the stop hook — block every N messages for auto-save.
pub fn hook_stop(session_id: &str, stop_hook_active: bool, transcript_path: &str) -> HookDecision {
    // If already in a save cycle, let through
    if stop_hook_active {
        return HookDecision::Pass;
    }

    let exchange_count = count_human_messages(transcript_path);
    ensure_state_dir();

    let last_save_path = last_save_file(session_id);
    let last_save: usize = fs::read_to_string(&last_save_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    let since_last = exchange_count.saturating_sub(last_save);

    if since_last >= SAVE_INTERVAL && exchange_count > 0 {
        // Update last save point
        fs::write(&last_save_path, exchange_count.to_string()).ok();

        HookDecision::Block {
            reason: STOP_BLOCK_REASON.to_string(),
        }
    } else {
        HookDecision::Pass
    }
}

/// Run the session-start hook — initialize session tracking.
pub fn hook_session_start(session_id: &str) {
    ensure_state_dir();
}

/// Run the precompact hook — mine synchronously before compaction.
pub fn hook_precompact(transcript_path: &str) {
    ensure_state_dir();
}

#[derive(Debug)]
pub enum HookDecision {
    Pass,
    Block { reason: String },
}

const STOP_BLOCK_REASON: &str = "AUTO-SAVE checkpoint (MemPalace). Save this session's key content:
1. mpr_diary_write — AAAK-compressed session summary
2. mpr_add_drawer — verbatim quotes, decisions, code snippets
3. mpr_kg_add — entity relationships (optional)
Continue conversation after saving.";

/// Parse hook JSON data (stdin format).
pub fn parse_hook_json(json_str: &str) -> Option<HookData> {
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;
    Some(HookData {
        session_id: sanitize_session_id(
            value.get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
        ),
        stop_hook_active: value
            .get("stop_hook_active")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        transcript_path: value
            .get("transcript_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_session_id() {
        assert_eq!(sanitize_session_id("abc-123"), "abc-123");
        assert_eq!(sanitize_session_id("abc/.."), "abc");
        assert_eq!(sanitize_session_id("!!!"), "unknown");
    }

    #[test]
    fn test_parse_hook_json() {
        let json = r#"{"session_id": "abc-123", "stop_hook_active": false, "transcript_path": "/tmp/test.jsonl"}"#;
        let data = parse_hook_json(json).unwrap();
        assert_eq!(data.session_id, "abc-123");
        assert!(!data.stop_hook_active);
    }
}
