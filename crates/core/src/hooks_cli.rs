//! hooks_cli.rs — Hook runtime for Claude Code integration.
//!
//! Session-start, stop, precompact, and lifecycle hooks that interface with Claude Code.
//!
//! Usage:
//!     mpr hook session-start  — inject context on Claude Code wake-up
//!     mpr hook session-end    — persist session state on termination
//!     mpr hook stop           — block every N messages for auto-save
//!     mpr hook precompact     — context injection before compaction
//!     mpr hook post-tool-use  — log tool usage (fire-and-forget)
//!     mpr hook post-tool-failure — log failure + trigger heal
//!     mpr hook prompt-submit   — capture user prompt as observation
//!     mpr hook notification    — handle system notifications
//!     mpr hook subagent-start  — log subagent spawn
//!     mpr hook subagent-stop   — log subagent completion
//!     mpr hook task-completed  — update action graph

#![doc(hidden)]

use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::thread;

use crate::config::Config;
use crate::mcp::context_inject::{inject_session_context, is_context_injection_enabled};
use crate::types::{HookPayload, HookType};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SAVE_INTERVAL: usize = 15;
const STATE_DIR: &str = ".mempalace/hook_state";

/// Default HTTP port for MemPalace REST API (matches rest_api.rs)
const DEFAULT_HTTP_PORT: u16 = 3111;

// ---------------------------------------------------------------------------
// Hook data parsed from Claude Code JSON input
// ---------------------------------------------------------------------------

/// Hook data parsed from Claude Code JSON input.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
            let path_str = path_str.trim_start_matches('~');
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
    let Some(path) = validate_transcript_path(transcript_path) else {
        return 0;
    };
    if !path.exists() {
        return 0;
    }

    let Ok(content) = fs::read_to_string(&path) else {
        return 0;
    };
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
pub fn hook_session_start(_session_id: &str) {
    ensure_state_dir();
}

/// Run the precompact hook — mine synchronously before compaction.
pub fn hook_precompact(_transcript_path: &str) {
    ensure_state_dir();
}

// ---------------------------------------------------------------------------
// HTTP client for fire-and-forget telemetry
// ---------------------------------------------------------------------------

/// Fire-and-forget HTTP POST to the MemPalace REST API.
/// Uses std::process::Command with curl for fire-and-forget semantics
/// (no blocking, no waiting for response).
fn fire_and_forget(url: &str, body: &serde_json::Value) {
    let body_str = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let url_owned = url.to_string();

    // Use curl in background - spawns process without waiting
    // Both url and body_str are owned by the closure so they don't escape
    thread::spawn(move || {
        let _ = Command::new("curl")
            .args(&[
                "-s",
                "-X", "POST",
                "-H", "Content-Type: application/json",
                "-d", &body_str,
                &url_owned,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    });
}

/// Get the MemPalace HTTP server URL.
fn get_api_base() -> String {
    let port = std::env::var("MEMPALACE_HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_HTTP_PORT);
    format!("http://127.0.0.1:{}", port)
}

// ---------------------------------------------------------------------------
// Context-injecting hook implementations
// ---------------------------------------------------------------------------

/// Build context for session-start: call /observe + /context/build
/// and inject context string to stdout for Claude Code to read.
/// Uses try/catch + timeout pattern from agentmemory.
pub fn hook_session_start_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let _stop_hook_active = data
        .get("stop_hook_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("SESSION START for session {} (harness={})", session_id, harness));
    ensure_state_dir();

    // Build context via inject_session_context (from context_inject.rs)
    let config = Config::load().ok();
    let palace_path = config
        .as_ref()
        .map(|c| c.palace_path.clone())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".mempalace")
                .join("palace")
        });

    // Call /observe with session_start payload (fire-and-forget)
    let observe_payload = build_observe_payload(
        &session_id,
        HookType::SessionStart,
        HashMap::new(),
        &transcript_path,
    );
    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    // Try to inject context with timeout
    let context = try_inject_context(&session_id, &palace_path);

    if !context.is_empty() {
        // Write context to stdout for Claude Code to read
        println!("{}", context);
    }

    Ok(serde_json::json!({}))
}

/// Try to inject session context with timeout (5 second limit).
/// Returns the context string on success, empty string on failure/timeout.
fn try_inject_context(session_id: &str, palace_path: &PathBuf) -> String {
    // Check if context injection is enabled
    if !is_context_injection_enabled() {
        return String::new();
    }

    // Try to inject with timeout using a background thread
    let sid = session_id.to_string();
    let pp = palace_path.clone();

    let result = thread::scope(|s| {
        let handle = s.spawn(|| {
            inject_session_context(&sid, &pp, None)
        });
        // Wait with timeout (5 seconds)
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(5) {
                return String::new();
            }
            if handle.is_finished() {
                return handle.join().unwrap_or_default();
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    result
}

// ---------------------------------------------------------------------------
// Telemetry hook implementations (fire-and-forget)
// ---------------------------------------------------------------------------

/// Session-end hook: call /session/end + /summarize on session termination.
/// Fire-and-forget HTTP calls.
pub fn hook_session_end_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("SESSION END for session {} (harness={})", session_id, harness));

    let api_base = get_api_base();

    // Build session_end payload
    let mut session_end_data = HashMap::new();
    session_end_data.insert("transcript_path".to_string(), serde_json::json!(transcript_path));
    let session_end_payload = build_observe_payload(
        &session_id,
        HookType::SessionEnd,
        session_end_data,
        &transcript_path,
    );
    fire_and_forget(&format!("{}/observe", api_base), &session_end_payload);

    // Fire-and-forget /session/end
    let session_payload = serde_json::json!({
        "session_id": session_id,
        "ended_at": chrono::Utc::now().to_rfc3339(),
    });
    fire_and_forget(&format!("{}/session/end", api_base), &session_payload);

    // Fire-and-forget /summarize
    fire_and_forget(&format!("{}/summarize", api_base), &serde_json::json!({
        "session_id": session_id,
    }));

    Ok(serde_json::json!({}))
}

/// Post-tool-use hook: after every tool call, fire /observe with tool name/input/output.
/// Fire-and-forget.
pub fn hook_post_tool_use_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("POST-TOOL-USE for session {} (harness={})", session_id, harness));

    // Extract tool data from Claude Code format
    let mut hook_data = HashMap::new();

    if let Some(tool_use) = data.get("toolUse").or_else(|| data.get("tool_use")) {
        if let Some(obj) = tool_use.as_object() {
            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                hook_data.insert("toolName".to_string(), serde_json::json!(name));
            }
            if let Some(input) = obj.get("input").or_else(|| obj.get("toolInput")) {
                hook_data.insert("toolInput".to_string(), input.clone());
            }
            if let Some(output) = obj.get("output").or_else(|| obj.get("toolOutput")) {
                hook_data.insert("toolOutput".to_string(), output.clone());
            }
        }
    }

    // Fallback: extract directly from top-level fields
    if let Some(name) = data.get("toolName").and_then(|v| v.as_str()) {
        hook_data.entry("toolName".to_string()).or_insert_with(|| serde_json::json!(name));
    }
    if let Some(input) = data.get("toolInput").or_else(|| data.get("input")) {
        hook_data.entry("toolInput".to_string()).or_insert_with(|| input.clone());
    }
    if let Some(output) = data.get("toolOutput").or_else(|| data.get("output")) {
        hook_data.entry("toolOutput".to_string()).or_insert_with(|| output.clone());
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::PostToolUse,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    Ok(serde_json::json!({}))
}

/// Post-tool-failure hook: log failure, trigger heal check via /heal.
/// Fire-and-forget.
pub fn hook_post_tool_failure_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("POST-TOOL-FAILURE for session {} (harness={})", session_id, harness));

    // Extract error data
    let mut hook_data = HashMap::new();

    if let Some(error) = data.get("error").or_else(|| data.get("toolError")) {
        hook_data.insert("error".to_string(), error.clone());
    } else if let Some(msg) = data.get("error_message").or_else(|| data.get("message")) {
        hook_data.insert("error".to_string(), msg.clone());
    }

    // Include tool info if available
    if let Some(tool_use) = data.get("toolUse").or_else(|| data.get("failed_tool")) {
        if let Some(name) = tool_use.as_str() {
            hook_data.insert("toolName".to_string(), serde_json::json!(name));
        } else if let Some(obj) = tool_use.as_object() {
            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                hook_data.insert("toolName".to_string(), serde_json::json!(name));
            }
        }
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::PostToolUseFailure,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();

    // Fire /observe
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    // Fire /heal to trigger heal check
    fire_and_forget(&format!("{}/heal", api_base), &serde_json::json!({
        "session_id": session_id,
        "trigger": "post_tool_failure",
    }));

    Ok(serde_json::json!({}))
}

/// Prompt-submit hook: capture user prompt as observation via /observe.
pub fn hook_prompt_submit_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("PROMPT-SUBMIT for session {} (harness={})", session_id, harness));

    // Extract user prompt
    let mut hook_data = HashMap::new();

    if let Some(prompt) = data.get("prompt").or_else(|| data.get("userPrompt")) {
        hook_data.insert("userPrompt".to_string(), prompt.clone());
    } else if let Some(message) = data.get("message").and_then(|m| m.get("content")) {
        hook_data.insert("userPrompt".to_string(), message.clone());
    }

    // Include assistant response if available (pair the prompt with response)
    if let Some(response) = data.get("assistantResponse").or_else(|| data.get("response")) {
        hook_data.insert("assistantResponse".to_string(), response.clone());
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::UserPromptSubmit,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    Ok(serde_json::json!({}))
}

/// Notification hook: handle system notifications, call /observe with notification data.
pub fn hook_notification_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("NOTIFICATION for session {} (harness={})", session_id, harness));

    // Extract notification data
    let mut hook_data = HashMap::new();

    if let Some(notification) = data.get("notification").or_else(|| data.get("msg")) {
        if let Some(obj) = notification.as_object() {
            for (k, v) in obj {
                hook_data.insert(k.clone(), v.clone());
            }
        } else {
            hook_data.insert("message".to_string(), notification.clone());
        }
    } else {
        // Forward everything as notification data
        for (k, v) in data.as_object().unwrap_or(&serde_json::Map::new()) {
            if k != "session_id" && k != "transcript_path" && k != "stop_hook_active" {
                hook_data.insert(k.clone(), v.clone());
            }
        }
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::Notification,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    Ok(serde_json::json!({}))
}

/// Subagent-start hook: log subagent spawn via /observe with subagent metadata.
pub fn hook_subagent_start_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("SUBAGENT-START for session {} (harness={})", session_id, harness));

    // Extract subagent data
    let mut hook_data = HashMap::new();

    if let Some(subagent) = data.get("subagent").or_else(|| data.get("agent")) {
        if let Some(obj) = subagent.as_object() {
            for (k, v) in obj {
                hook_data.insert(k.clone(), v.clone());
            }
        } else if let Some(s) = subagent.as_str() {
            hook_data.insert("agentId".to_string(), serde_json::json!(s));
        }
    } else {
        // Forward all data as subagent metadata
        for (k, v) in data.as_object().unwrap_or(&serde_json::Map::new()) {
            if k != "session_id" && k != "transcript_path" && k != "stop_hook_active" {
                hook_data.insert(k.clone(), v.clone());
            }
        }
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::SubagentStart,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    Ok(serde_json::json!({}))
}

/// Subagent-stop hook: log subagent completion via /observe with subagent result.
pub fn hook_subagent_stop_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("SUBAGENT-STOP for session {} (harness={})", session_id, harness));

    // Extract subagent result data
    let mut hook_data = HashMap::new();

    if let Some(result) = data.get("result").or_else(|| data.get("subagentResult")) {
        hook_data.insert("result".to_string(), result.clone());
    }

    // Include exit code/status if available
    if let Some(status) = data.get("exitCode").or_else(|| data.get("exit_code")).or_else(|| data.get("status")) {
        hook_data.insert("exitCode".to_string(), status.clone());
    }

    // Include duration if available
    if let Some(duration) = data.get("duration").or_else(|| data.get("elapsed")) {
        hook_data.insert("duration".to_string(), duration.clone());
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::SubagentStop,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    Ok(serde_json::json!({}))
}

/// Task-completed hook: update action graph via /observe.
pub fn hook_task_completed_response(
    data: &serde_json::Value,
    harness: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_id = sanitize_session_id(
        data.get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
    );
    let transcript_path = data
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    log_hook(&format!("TASK-COMPLETED for session {} (harness={})", session_id, harness));

    // Extract task data
    let mut hook_data = HashMap::new();

    if let Some(task) = data.get("task").or_else(|| data.get("action")) {
        if let Some(obj) = task.as_object() {
            for (k, v) in obj {
                hook_data.insert(k.clone(), v.clone());
            }
        } else {
            hook_data.insert("taskId".to_string(), task.clone());
        }
    } else {
        // Forward all remaining data
        for (k, v) in data.as_object().unwrap_or(&serde_json::Map::new()) {
            if k != "session_id" && k != "transcript_path" && k != "stop_hook_active" {
                hook_data.insert(k.clone(), v.clone());
            }
        }
    }

    let observe_payload = build_observe_payload(
        &session_id,
        HookType::TaskCompleted,
        hook_data,
        &transcript_path,
    );

    let api_base = get_api_base();
    fire_and_forget(&format!("{}/observe", api_base), &observe_payload);

    Ok(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Build an observe payload from hook data.
fn build_observe_payload(
    session_id: &str,
    hook_type: HookType,
    mut data: HashMap<String, serde_json::Value>,
    transcript_path: &str,
) -> serde_json::Value {
    let config = Config::load().ok();
    let project = config
        .as_ref()
        .and_then(|c| c.topic_wings.first().cloned())
        .unwrap_or_else(|| "default".to_string());
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Add transcript_path to data if not empty
    if !transcript_path.is_empty() {
        data.insert("transcript_path".to_string(), serde_json::json!(transcript_path));
    }

    let payload = HookPayload {
        hook_type,
        session_id: session_id.to_string(),
        project,
        cwd,
        timestamp: Utc::now(),
        data,
    };

    serde_json::to_value(payload).unwrap_or_else(|_| serde_json::json!({}))
}

/// Log a hook event to the hook log file.
fn log_hook(message: &str) {
    let state_dir = state_dir();
    if fs::create_dir_all(&state_dir).is_err() {
        return;
    }
    let log_path = state_dir.join("hook.log");
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .and_then(|mut f| {
            writeln!(f, "[{}] {}", timestamp, message)
        });
}

// ---------------------------------------------------------------------------
// HookDecision
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[non_exhaustive]
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
            value
                .get("session_id")
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

    #[test]
    fn test_build_observe_payload() {
        let mut data = HashMap::new();
        data.insert("toolName".to_string(), serde_json::json!("Read"));
        let payload = build_observe_payload(
            "test-session",
            HookType::PostToolUse,
            data,
            "/tmp/transcript.jsonl",
        );
        assert_eq!(payload["session_id"], "test-session");
        // HookType serializes as snake_case
        assert_eq!(payload["hook_type"], "post_tool_use");
    }
}
