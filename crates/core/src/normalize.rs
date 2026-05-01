use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;

const SLACK_PROVENANCE_FOOTER: &str =
    "\n[source: slack-export | multi-party chat — speaker roles are positional, not verified]";

fn strip_noise(text: &str) -> String {
    let mut result = text.to_string();

    let noise_tags = [
        "system-reminder",
        "command-message",
        "command-name",
        "task-notification",
        "user-prompt-submit-hook",
        "hook_output",
    ];
    for tag in &noise_tags {
        let pattern = format!(r"(?m)^[\s]*{}[\s]*$", regex::escape(tag));
        if let Ok(re) = regex::Regex::new(&pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }

    if let Ok(re) = regex::Regex::new(r"(?m)^(?:> )?Ran \d+ (?:Stop|PreCompact|PreToolUse|PostToolUse|UserPromptSubmit|Notification|SessionStart|SessionEnd) hook[s]?.*$") {
        result = re.replace_all(&result, "").to_string();
    }

    let noise_prefixes = [
        "CURRENT TIME:",
        "VERIFIED FACTS",
        "AGENT SPECIALIZATION:",
        "Checking verified facts...",
        "Injecting timestamp...",
        "Starting background pipeline...",
        "Checking emotional weights...",
        "Auto-save reminder...",
        "Checking pipeline...",
        "MemPalace auto-save checkpoint.",
    ];
    for prefix in &noise_prefixes {
        let pattern = format!(r"(?m)^[\s]*{}.*$", regex::escape(prefix));
        if let Ok(re) = regex::Regex::new(&pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }

    if let Ok(re) = regex::Regex::new(r"(?m)^\s*… \+\d+ lines\s*$") {
        result = re.replace_all(&result, "").to_string();
    }

    if let Ok(re) = regex::Regex::new(r"\s*\[(\d+)\s+tokens?\]\s+\(ctrl\+o to (?:open|expand)\)") {
        result = re.replace_all(&result, "").to_string();
    }

    if let Ok(re) = regex::Regex::new(r"(?m)^\s*hook_output\s*$") {
        result = re.replace_all(&result, "").to_string();
    }

    if let Ok(re) = regex::Regex::new(r"\n{3,}") {
        result = re.replace_all(&result, "\n\n").to_string();
    }

    result.trim().to_string()
}

fn format_tool_use(content: &Value) -> String {
    let obj = match content.as_object() {
        Some(o) => o,
        None => return String::new(),
    };

    let tool_name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown");
    let args = obj.get("arguments").or_else(|| obj.get("input")).and_then(|v| v.as_object());

    match tool_name {
        "Bash" => {
            let cmd = args
                .and_then(|a| a.get("command").or_else(|| a.get("cmd")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if cmd.len() > 200 {
                format!("[Bash] {}...", &cmd[..200])
            } else {
                format!("[Bash] {}", cmd)
            }
        }
        "Read" => {
            let path = args
                .and_then(|a| a.get("file_path").or_else(|| a.get("path")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let offset = args.and_then(|a| a.get("offset")).and_then(|v| v.as_u64());
            let limit = args.and_then(|a| a.get("limit")).and_then(|v| v.as_u64());
            if let (Some(off), Some(lim)) = (offset, limit) {
                format!("[Read {}:{}-{}]", path, off, off.saturating_add(lim))
            } else {
                format!("[Read {}]", path)
            }
        }
        "Grep" | "Glob" => {
            let pattern = args
                .and_then(|a| a.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = args
                .and_then(|a| a.get("target").or_else(|| a.get("file_path")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[{}] {} in {}", tool_name, pattern, target)
        }
        "Edit" | "Write" => {
            let path = args
                .and_then(|a| a.get("file_path").or_else(|| a.get("path")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[{}] {}", tool_name, path)
        }
        _ => {
            let args_str = args
                .map(|a| serde_json::to_string(a).unwrap_or_default())
                .unwrap_or_default();
            let summary = if args_str.len() > 200 {
                format!("{}...", &args_str[..200])
            } else {
                args_str
            };
            format!("[{}] {}", tool_name, summary)
        }
    }
}

fn format_tool_result(content: &Value, tool_name: Option<&str>) -> String {
    let obj = match content.as_object() {
        Some(o) => o,
        None => return String::new(),
    };

    let text = obj
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match tool_name {
        Some("Read") | Some("Edit") | Some("Write") => String::new(),
        Some("Bash") => {
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() <= 20 {
                if text.is_empty() {
                    String::new()
                } else {
                    format!("→ {}", text)
                }
            } else if lines.len() <= 40 {
                format!("→ {}", text)
            } else {
                let head: String = lines[..20].join("\n");
                let tail: String = lines[lines.len() - 20..].join("\n");
                format!("→ {}\n… [{} lines truncated] …\n{}", head, lines.len() - 40, tail)
            }
        }
        Some("Grep") | Some("Glob") => {
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() <= 20 {
                if text.is_empty() {
                    String::new()
                } else {
                    format!("→ {}", text)
                }
            } else {
                let kept: String = lines[..20].join("\n");
                format!("→ {}\n… [{} matches truncated] …", kept, lines.len() - 20)
            }
        }
        _ => {
            if text.len() > 2048 {
                format!("→ {}", &text[..2048])
            } else if text.is_empty() {
                String::new()
            } else {
                format!("→ {}", text)
            }
        }
    }
}

fn load_known_names() -> HashSet<String> {
    let Ok(registry_path) = crate::Config::registry_file_path() else {
        return HashSet::new();
    };
    let Ok(registry) = crate::entity_registry::EntityRegistry::load(&registry_path) else {
        return HashSet::new();
    };

    let mut names = HashSet::new();
    for (canonical, entry) in registry.people() {
        names.insert(canonical.to_lowercase());
        if let Some(canonical_name) = &entry.canonical {
            names.insert(canonical_name.to_lowercase());
        }
        for alias in &entry.aliases {
            names.insert(alias.to_lowercase());
        }
    }
    names
}

fn spellcheck_transcript_preserving_known_names(content: &str) -> String {
    let known_names = load_known_names();
    content
        .lines()
        .map(|line| {
            let stripped = line.trim_start();
            if !stripped.starts_with('>') {
                return line.to_string();
            }

            let prefix_len = line.len() - stripped.len() + 2;
            if prefix_len > line.len() {
                return line.to_string();
            }

            let message = &line[prefix_len..];
            if message.trim().is_empty() {
                return line.to_string();
            }

            let corrected = crate::spellcheck::correct_spelling(message, &known_names);
            format!("{}> {}", &line[..prefix_len - 2], corrected)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn normalize(file_path: &std::path::Path, content: &str) -> anyhow::Result<String> {
    use std::fs;

    if content.trim().is_empty() {
        return Ok(content.to_string());
    }

    if let Ok(metadata) = fs::metadata(file_path) {
        if metadata.len() > 500 * 1024 * 1024 {
            anyhow::bail!(
                "Content too large ({} bytes) to normalize: {}",
                metadata.len(),
                file_path.display()
            );
        }
    }

    let lines: Vec<&str> = content.split('\n').collect();
    let quote_count = lines.iter().filter(|l| l.trim().starts_with('>')).count();
    if quote_count >= 3 {
        return Ok(content.to_string());
    }

    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("db") || ext.eq_ignore_ascii_case("sqlite") {
        if let Some(normalized) = normalize_opencode_db(file_path) {
            return Ok(normalized);
        }
    }
    if ext.eq_ignore_ascii_case("json")
        || ext.eq_ignore_ascii_case("jsonl")
        || content.trim().starts_with('{')
        || content.trim().starts_with('[')
    {
        if let Some(normalized) = try_normalize_json(content) {
            return Ok(normalized);
        }
    }

    Ok(content.to_string())
}

fn try_normalize_json(content: &str) -> Option<String> {
    if let Some(normalized) = try_claude_code_jsonl(content) {
        return Some(normalized);
    }
    if let Some(normalized) = try_codex_jsonl(content) {
        return Some(normalized);
    }
    if let Some(normalized) = try_soulforge_jsonl(content) {
        return Some(normalized);
    }
    if let Some(normalized) = try_aider_md(content) {
        return Some(normalized);
    }

    let Ok(data) = serde_json::from_str::<Value>(content) else {
        return None;
    };

    for parser in [try_claude_ai_json, try_chatgpt_json, try_slack_json] {
        if let Some(normalized) = parser(&data) {
            return Some(normalized);
        }
    }

    None
}

fn try_claude_code_jsonl(content: &str) -> Option<String> {
    let lines: Vec<&str> = content
        .trim()
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut tool_use_map: HashMap<String, String> = HashMap::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let entry = entry.as_object()?;
        let msg_type = entry.get("type")?.as_str()?;
        let message = entry.get("message")?.as_object()?;

        match msg_type {
            "assistant" => {
                let content_val = message.get("content")?;
                if let Some(arr) = content_val.as_array() {
                    for block in arr {
                        let obj = match block.as_object() {
                            Some(o) => o,
                            None => continue,
                        };
                        if obj.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                            continue;
                        }
                        let tool_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let tool_name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown");
                        tool_use_map.insert(tool_id.to_string(), tool_name.to_string());
                    }
                }

                let text = extract_claude_code_assistant_text(content_val, &tool_use_map);
                let text = strip_noise(&text);
                if text.is_empty() {
                    continue;
                }
                messages.push(("assistant".to_string(), text));
            }
            "human" | "user" => {
                let content_val = message.get("content")?;
                let (user_text, is_tool_only) = extract_claude_code_user_text(content_val, &tool_use_map);

                if is_tool_only {
                    if let Some(prev) = messages.last_mut() {
                        if prev.0 == "assistant" {
                            if !user_text.is_empty() {
                                prev.1.push_str("\n");
                                prev.1.push_str(&user_text);
                            }
                            continue;
                        }
                    }
                }

                if user_text.is_empty() {
                    continue;
                }
                messages.push(("user".to_string(), user_text));
            }
            _ => continue,
        }
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages));
    }
    None
}

fn try_claude_ai_json(data: &Value) -> Option<String> {
    let messages_data = if data.is_object() {
        data.get("messages")
            .or_else(|| data.get("chat_messages"))
            .unwrap_or(data)
    } else {
        data
    };

    let list = messages_data.as_array()?;
    let mut messages: Vec<(String, String)> = Vec::new();

    for item in list {
        let obj = item.as_object()?;
        let role = obj.get("role")?.as_str()?;
        let text = extract_content_to_string(obj.get("content")?);

        if text.is_empty() {
            continue;
        }

        if role == "user" || role == "human" {
            messages.push(("user".to_string(), text));
        } else if role == "assistant" || role == "ai" {
            messages.push(("assistant".to_string(), text));
        }
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages));
    }
    None
}

fn try_chatgpt_json(data: &Value) -> Option<String> {
    let mapping = data.get("mapping")?.as_object()?;

    let mut root_id: Option<&str> = None;
    let mut fallback_root: Option<&str> = None;

    for (node_id, node) in mapping {
        let node = node.as_object()?;
        let parent = node.get("parent");
        if parent.is_none() || parent?.is_null() {
            let msg = node.get("message");
            if msg.is_none() || msg?.is_null() {
                root_id = Some(node_id);
                break;
            } else if fallback_root.is_none() {
                fallback_root = Some(node_id);
            }
        }
    }

    let root_id = root_id.or(fallback_root)?;
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut current_id: &str = root_id;

    while !current_id.is_empty() && !visited.contains(current_id) {
        visited.insert(current_id);
        let node = mapping.get(current_id)?.as_object()?;
        if let Some(msg_val) = node.get("message") {
            let msg = msg_val.as_object()?;
            let role = msg.get("author")?.as_object()?.get("role")?.as_str()?;
            let content_val = msg.get("content")?;

            let parts: Vec<String> = if content_val.is_array() {
                content_val
                    .as_array()?
                    .iter()
                    .filter_map(|p| p.as_str().map(String::from))
                    .collect()
            } else {
                Vec::new()
            };

            let text: String = parts.join(" ").trim().to_string();

            if text.is_empty() {
                let children = node.get("children")?.as_array()?;
                current_id = children.first()?.as_str().unwrap_or("");
                continue;
            }

            if role == "user" {
                messages.push(("user".to_string(), text));
            } else if role == "assistant" {
                messages.push(("assistant".to_string(), text));
            }
        }

        let children = node.get("children")?.as_array()?;
        current_id = children.first()?.as_str().unwrap_or("");
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages));
    }
    None
}

fn try_slack_json(data: &Value) -> Option<String> {
    let list = data.as_array()?;
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut seen_users: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut last_role: Option<&str> = None;

    for item in list {
        let obj = item.as_object()?;
        if obj.get("type")?.as_str() != Some("message") {
            continue;
        }

        let user_id = obj
            .get("user")
            .or_else(|| obj.get("username"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let text = obj.get("text")?.as_str().unwrap_or("").trim().to_string();

        if text.is_empty() || user_id.is_empty() {
            continue;
        }

        let role = if !seen_users.contains_key(user_id) {
            if seen_users.is_empty() {
                seen_users.insert(user_id, "user");
                "user"
            } else if last_role == Some("user") {
                seen_users.insert(user_id, "assistant");
                "assistant"
            } else {
                seen_users.insert(user_id, "user");
                "user"
            }
        } else {
            *seen_users.get(user_id).unwrap()
        };

        last_role = Some(role);
        messages.push((role.to_string(), text));
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages) + SLACK_PROVENANCE_FOOTER);
    }
    None
}

fn try_codex_jsonl(content: &str) -> Option<String> {
    let lines: Vec<&str> = content
        .trim()
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .collect();

    // Detect Codex format via session_meta presence
    let has_session_meta = lines.iter().any(|l| {
        if let Ok(v) = serde_json::from_str::<Value>(l) {
            if let Some(obj) = v.as_object() {
                if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                    return t == "session_meta";
                }
            }
        }
        false
    });

    if !has_session_meta {
        return None;
    }

    let mut messages: Vec<(String, String)> = Vec::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let entry = entry.as_object()?;

        // Try flat format first: {"type": "event_msg/user_message", "text": "..."}
        let msg_type = entry.get("type").and_then(|v| v.as_str());

        let (text, role) = if let Some(t) = msg_type {
            if t == "event_msg/user_message" || t == "event_msg/agent_message" {
                let text = entry
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if text.is_empty() {
                    continue;
                }
                let role = if t == "event_msg/user_message" {
                    "user"
                } else {
                    "assistant"
                };
                (text.to_string(), role)
            } else if t == "event_msg" {
                // Try nested format: {"type": "event_msg", "payload": {"type": "user_message", "message": "..."}}
                let payload = entry.get("payload")?.as_object()?;
                let nested_type = payload.get("type")?.as_str()?;
                if nested_type != "user_message" && nested_type != "agent_message" {
                    continue;
                }
                let msg_content = payload.get("message")?;
                let text = extract_content_to_string(msg_content);
                if text.is_empty() {
                    continue;
                }
                let role = if nested_type == "user_message" {
                    "user"
                } else {
                    "assistant"
                };
                (text, role)
            } else {
                continue;
            }
        } else {
            continue;
        };

        messages.push((role.to_string(), text));
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages));
    }
    None
}

fn try_soulforge_jsonl(content: &str) -> Option<String> {
    let lines: Vec<&str> = content
        .trim()
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .collect();

    // Detect SoulForge via unique fields: segments, toolCalls, durationMs
    let has_soulforge_marker = lines.iter().any(|l| {
        if let Ok(v) = serde_json::from_str::<Value>(l) {
            if let Some(obj) = v.as_object() {
                // Check for SoulForge-specific fields
                if obj.contains_key("segments")
                    || obj.contains_key("toolCalls")
                    || obj.contains_key("durationMs")
                {
                    return true;
                }
                // Also check message content for segments array or toolCalls
                if let Some(msg) = obj.get("message").and_then(|m| m.as_object()) {
                    if msg.contains_key("segments")
                        || msg.contains_key("toolCalls")
                        || msg.contains_key("durationMs")
                    {
                        return true;
                    }
                }
            }
        }
        false
    });

    if !has_soulforge_marker {
        return None;
    }

    let mut messages: Vec<(String, String)> = Vec::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let entry = entry.as_object()?;

        // Get message content - could be in message.segments or directly in message.text
        let text = if let Some(msg) = entry.get("message").and_then(|m| m.as_object()) {
            if let Some(segments) = msg.get("segments").and_then(|s| s.as_array()) {
                // Extract text from segments array
                let parts: Vec<String> = segments
                    .iter()
                    .filter_map(|seg| seg.as_object()?.get("text")?.as_str().map(String::from))
                    .collect();
                let text = parts.join(" ");
                if !text.is_empty() {
                    Some(text)
                } else {
                    msg.get("text")?.as_str().map(String::from)
                }
            } else {
                msg.get("text")?.as_str().map(String::from)
            }
        } else {
            entry.get("text")?.as_str().map(String::from)
        };

        let Some(text) = text else {
            continue;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }

        // Determine role - SoulForge has user/assistant markers
        let role = entry
            .get("role")
            .and_then(|r| r.as_str())
            .or_else(|| entry.get("type").and_then(|t| t.as_str()))
            .unwrap_or("");

        // Summarize tool calls if present (inside message object)
        let final_text = if role == "assistant" || role == "agent" {
            if let Some(msg) = entry.get("message").and_then(|m| m.as_object()) {
                if let Some(tool_calls) = msg.get("toolCalls").and_then(|tc| tc.as_array()) {
                    if !tool_calls.is_empty() {
                        let tool_names: Vec<String> = tool_calls
                            .iter()
                            .filter_map(|tc| {
                                tc.as_object()?.get("name")?.as_str().map(String::from)
                            })
                            .collect();
                        if !tool_names.is_empty() {
                            format!("{} [tools: {}]", text, tool_names.join(", "))
                        } else {
                            text
                        }
                    } else {
                        text
                    }
                } else {
                    text
                }
            } else {
                text
            }
        } else {
            text
        };

        match role {
            "user" | "human" => messages.push(("user".to_string(), final_text)),
            "assistant" | "ai" | "agent" => messages.push(("assistant".to_string(), final_text)),
            // Skip system messages
            "system" => continue,
            _ => {
                // If role is unknown, alternate based on position
                if messages.is_empty()
                    || messages.last().map(|m| m.0 == "assistant").unwrap_or(false)
                {
                    messages.push(("user".to_string(), final_text));
                } else {
                    messages.push(("assistant".to_string(), final_text));
                }
            }
        }
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages));
    }
    None
}

/// Try to parse Aider .aider.chat.history.md format.
/// Format: Lines starting with "> " are user turns, other lines are assistant responses.
/// Detected by: presence of "# Aider Chat History" header or "> " quoted lines.
/// Try to parse OpenCode SQLite database format.
/// Reads sessions from an OpenCode session SQLite database file.
/// Detected by: file extension is .db or .sqlite and contains OpenCode schema.
pub fn try_opencode_sqlite(content: &str) -> Option<String> {
    // This is a placeholder - actual implementation would need rusqlite
    // For now, return None to indicate this format isn't supported
    let _ = content;
    None
}

/// Try to parse OpenCode SQLite database from file path.
/// Returns transcript format for sessions found.
pub fn normalize_opencode_db(db_path: &std::path::Path) -> Option<String> {
    let conn = rusqlite::Connection::open(db_path).ok()?;

    // Query the session table to get conversation history
    let mut stmt = conn
        .prepare("SELECT id, dir, created_at, updated_at FROM sessions ORDER BY created_at")
        .ok()?;

    let sessions: Vec<(i64, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    if sessions.is_empty() {
        return None;
    }

    let mut messages: Vec<(String, String)> = Vec::new();

    for (session_id, _dir, _created, _updated) in sessions {
        // Try to get messages for this session
        if let Ok(mut msg_stmt) =
            conn.prepare("SELECT role, content FROM messages WHERE session_id = ? ORDER BY id")
        {
            let rows = msg_stmt
                .query_map([session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .ok()?;

            for row in rows.flatten() {
                let (role, content) = row;
                if content.trim().is_empty() {
                    continue;
                }
                match role.as_str() {
                    "user" | "human" => {
                        messages.push(("user".to_string(), content.trim().to_string()));
                    }
                    "assistant" | "ai" | "bot" => {
                        messages.push(("assistant".to_string(), content.trim().to_string()));
                    }
                    _ => {}
                }
            }
        }
    }

    if messages.len() >= 2 {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn try_aider_md(content: &str) -> Option<String> {
    let trimmed = content.trim();

    // Check for Aider format markers
    let has_header =
        trimmed.contains("Aider Chat History") || trimmed.contains("aider.chat.history");
    let has_quoted_lines = trimmed
        .lines()
        .filter(|l| l.trim().starts_with("> "))
        .count()
        >= 2;

    if !has_header && !has_quoted_lines {
        return None;
    }

    let mut messages: Vec<(String, String)> = Vec::new();
    let mut current_assistant = String::new();

    for line in content.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        if trimmed_line.starts_with("> ") {
            // Save previous assistant message if any
            if !current_assistant.is_empty() {
                messages.push((
                    "assistant".to_string(),
                    current_assistant.trim().to_string(),
                ));
                current_assistant.clear();
            }

            // User message (strip the "> " prefix)
            let user_text = trimmed_line
                .strip_prefix("> ")
                .unwrap_or(trimmed_line)
                .trim()
                .to_string();
            if !user_text.is_empty() {
                messages.push(("user".to_string(), user_text));
            }
        } else if trimmed_line.starts_with("#") {
            // Skip markdown headers
            continue;
        } else if trimmed_line.starts_with("```") {
            // Skip code blocks markers
            continue;
        } else {
            // Accumulate as assistant response
            if !current_assistant.is_empty() {
                current_assistant.push('\n');
            }
            current_assistant.push_str(trimmed_line);
        }
    }

    // Don't forget the last assistant message
    if !current_assistant.is_empty() {
        messages.push((
            "assistant".to_string(),
            current_assistant.trim().to_string(),
        ));
    }

    if messages.len() >= 2 {
        Some(messages_to_transcript(&messages))
    } else {
        None
    }
}

fn extract_content_to_string(content: &Value) -> String {
    match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|item| match item {
                    Value::String(s) => Some(s.trim().to_string()),
                    Value::Object(obj) if obj.get("type")?.as_str() == Some("text") => {
                        obj.get("text")?.as_str().map(|s| s.trim().to_string())
                    }
                    _ => None,
                })
                .collect();
            parts.join(" ")
        }
        Value::Object(obj) => obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn extract_claude_code_assistant_text(content: &Value, tool_use_map: &HashMap<String, String>) -> String {
    match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(arr) => {
            let mut parts: Vec<String> = Vec::new();
            for item in arr {
                let obj = match item.as_object() {
                    Some(o) => o,
                    None => continue,
                };
                let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

                if item_type == "text" {
                    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            parts.push(trimmed.to_string());
                        }
                    }
                } else if item_type == "tool_use" {
                    let formatted = format_tool_use(item);
                    if !formatted.is_empty() {
                        parts.push(formatted);
                    }
                }
                // tool_result in assistant messages is rare but possible;
                // format it if we have the tool name
                else if item_type == "tool_result" {
                    let tool_id = obj.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                    let tool_name = tool_use_map.get(tool_id).map(|s| s.as_str());
                    let formatted = format_tool_result(item, tool_name);
                    if !formatted.is_empty() {
                        parts.push(formatted);
                    }
                }
            }
            parts.join(" ")
        }
        Value::Object(obj) => obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn extract_claude_code_user_text(content: &Value, tool_use_map: &HashMap<String, String>) -> (String, bool) {
    if let Value::String(s) = content {
        return (s.trim().to_string(), false);
    }

    let arr = match content.as_array() {
        Some(a) => a,
        None => return (String::new(), false),
    };

    let mut parts: Vec<String> = Vec::new();
    let mut has_tool_result = false;
    let mut has_user_text = false;

    for item in arr {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };
        let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if item_type == "text" {
            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                    has_user_text = true;
                }
            }
        } else if item_type == "tool_result" {
            has_tool_result = true;
            let tool_id = obj.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
            let tool_name = tool_use_map.get(tool_id).map(|s| s.as_str());
            let formatted = format_tool_result(item, tool_name);
            if !formatted.is_empty() {
                parts.push(formatted);
            }
        }
    }

    (parts.join(" "), has_tool_result && !has_user_text)
}

fn messages_to_transcript(messages: &[(String, String)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let (ref role, ref text) = messages[i];

        if role == "user" {
            lines.push(format!("> {}", text));
            if i + 1 < messages.len() && messages[i + 1].0 == "assistant" {
                lines.push(messages[i + 1].1.clone());
                i += 2;
            } else {
                i += 1;
            }
        } else {
            lines.push(text.clone());
            i += 1;
        }
        lines.push(String::new());
    }

    spellcheck_transcript_preserving_known_names(&lines.join("\n"))
}

pub fn detect_format(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let lines: Vec<&str> = content.split('\n').collect();

    // Check for Codex JSONL by scanning all lines for session_meta
    let has_session_meta = lines.iter().any(|l| {
        if let Ok(v) = serde_json::from_str::<Value>(l) {
            if let Some(obj) = v.as_object() {
                if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                    return t == "session_meta";
                }
            }
        }
        false
    });
    if has_session_meta {
        return Some("codex_jsonl".to_string());
    }

    // Check for SoulForge JSONL
    let has_soulforge = lines.iter().any(|l| {
        if let Ok(v) = serde_json::from_str::<Value>(l) {
            if let Some(obj) = v.as_object() {
                // Top-level markers
                if obj.contains_key("segments")
                    || obj.contains_key("toolCalls")
                    || obj.contains_key("durationMs")
                {
                    return true;
                }
                // Also check inside "message" object
                if let Some(msg) = obj.get("message").and_then(|m| m.as_object()) {
                    if msg.contains_key("segments")
                        || msg.contains_key("toolCalls")
                        || msg.contains_key("durationMs")
                    {
                        return true;
                    }
                }
            }
        }
        false
    });
    if has_soulforge {
        return Some("soulforge_jsonl".to_string());
    }

    // Check for Aider markdown format
    let has_aider =
        trimmed.contains("Aider Chat History") || trimmed.contains("aider.chat.history");
    if has_aider {
        return Some("aider_md".to_string());
    }

    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        // Try parsing as single JSON first
        if let Ok(data) = serde_json::from_str::<Value>(trimmed) {
            if let Some(obj) = data.as_object() {
                if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                    if t == "conversation" {
                        return Some("claude_code_jsonl".to_string());
                    }
                }
                if obj.get("messages").is_some() || obj.get("chat_messages").is_some() {
                    return Some("claude_ai_json".to_string());
                }
                if obj.get("mapping").is_some() {
                    return Some("chatgpt_json".to_string());
                }
            }
            if let Some(arr) = data.as_array() {
                if let Some(first) = arr.first() {
                    if first.get("type").and_then(|v| v.as_str()) == Some("message") {
                        return Some("slack_json".to_string());
                    }
                }
            }
        } else if !lines.is_empty() {
            // Try parsing first line as JSON (for JSONL formats)
            if let Ok(first) = serde_json::from_str::<Value>(lines[0].trim()) {
                if let Some(obj) = first.as_object() {
                    if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                        if t == "conversation" {
                            return Some("claude_code_jsonl".to_string());
                        }
                    }
                }
            }
        }
    }

    let quote_count = lines.iter().filter(|l| l.trim().starts_with('>')).count();
    if quote_count >= 3 {
        return Some("transcript".to_string());
    }

    Some("plain_text".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_pass_through() {
        let content = "This is plain text\nwithout any markers";
        let result = normalize(std::path::Path::new("test.txt"), content).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_transcript_pass_through() {
        let content = "> user message\nassistant response\n> another user";
        let result = normalize(std::path::Path::new("test.txt"), content).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_claude_code_jsonl() {
        let content = r#"{"type":"human","message":{"content":"Hello"}}
{"type":"assistant","message":{"content":"Hi there"}}"#;
        let result = normalize(std::path::Path::new("test.jsonl"), content).unwrap();
        assert!(result.contains("> Hello"));
        assert!(result.contains("Hi there"));
    }

    #[test]
    fn test_empty_content() {
        let result = normalize(std::path::Path::new("test.txt"), "").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_codex_jsonl() {
        let content = r#"{"type":"session_meta","sessionId":"abc123","model":"gpt-4"}
{"type":"event_msg/user_message","text":"Hello Codex"}
{"type":"event_msg/agent_message","text":"Hello from Codex agent"}"#;
        let result = normalize(std::path::Path::new("test.jsonl"), content).unwrap();
        assert!(result.contains("> Hello Codex"));
        assert!(result.contains("Hello from Codex agent"));
    }

    #[test]
    fn test_messages_to_transcript_routes_user_lines_through_spellcheck_path() {
        let transcript = messages_to_transcript(&[
            ("user".to_string(), "hello world test message".to_string()),
            (
                "assistant".to_string(),
                "Assistant text should stay untouched.".to_string(),
            ),
        ]);

        assert!(transcript.starts_with("> hello world test message"));
        assert!(transcript.contains("Assistant text should stay untouched."));
    }

    #[test]
    fn test_messages_to_transcript_preserves_assistant_lines() {
        let transcript = messages_to_transcript(&[
            (
                "assistant".to_string(),
                "MemPalace ChromaDB NDCG@10".to_string(),
            ),
            ("user".to_string(), "hello world".to_string()),
        ]);

        assert!(transcript.contains("MemPalace ChromaDB NDCG@10"));
    }

    #[test]
    fn test_codex_jsonl_skips_response_items() {
        // response_item entries should be skipped
        let content = r#"{"type":"session_meta","sessionId":"abc123","model":"gpt-4"}
{"type":"event_msg/user_message","text":"Hello"}
{"type":"response_item","text":"Should be skipped"}
{"type":"event_msg/agent_message","text":"Real response"}"#;
        let result = normalize(std::path::Path::new("test.jsonl"), content).unwrap();
        assert!(result.contains("> Hello"));
        assert!(result.contains("Real response"));
        assert!(!result.contains("Should be skipped"));
    }

    #[test]
    fn test_codex_jsonl_rejects_non_codex() {
        // Other JSONL format should not be detected as Codex
        let content = r#"{"type":"event","data":"something"}"#;
        let result = detect_format(content);
        // Should not be codex (no session_meta)
        assert_ne!(result, Some("codex_jsonl".to_string()));
    }

    #[test]
    fn test_detect_format_codex() {
        let content = r#"{"type":"session_meta","sessionId":"abc123"}
{"type":"event_msg/user_message","text":"Hello"}"#;
        let result = detect_format(content);
        assert_eq!(result, Some("codex_jsonl".to_string()));
    }

    #[test]
    fn test_soulforge_jsonl() {
        let content = r#"{"role":"user","message":{"text":"Hello SoulForge"}}
{"role":"assistant","message":{"text":"Hello from SoulForge"}}"#;
        let result = normalize(std::path::Path::new("test.jsonl"), content);
        assert!(result.is_ok());
    }

    #[test]
    fn test_soulforge_with_segments() {
        let content = r#"{"role":"user","message":{"segments":[{"text":"Hello"}]}}
{"role":"assistant","message":{"segments":[{"text":"Response"}]}}"#;
        let result = normalize(std::path::Path::new("test.jsonl"), content);
        assert!(result.is_ok());
    }

    #[test]
    fn test_soulforge_with_tool_calls() {
        let content = r#"{"role":"user","message":{"text":"Run a command"}}
{"role":"assistant","message":{"text":"Running...","toolCalls":[{"name":"bash","input":"ls"}]}}"#;
        let result = normalize(std::path::Path::new("test.jsonl"), content);
        assert!(result.is_ok());
        let r = result.unwrap();
        // Tool calls should be summarized
        assert!(r.contains("[tools:"));
    }

    #[test]
    fn test_detect_format_soulforge() {
        let content = r#"{"role":"user","message":{"text":"Hello"}}
{"role":"assistant","message":{"segments":[{"text":"Hi"}]}}"#;
        let result = detect_format(content);
        assert_eq!(result, Some("soulforge_jsonl".to_string()));
    }

    #[test]
    fn test_detect_format() {
        assert_eq!(
            detect_format(r#"{"messages": []}"#).unwrap(),
            "claude_ai_json"
        );
        assert_eq!(detect_format(r#"{"mapping": {}}"#).unwrap(), "chatgpt_json");
        assert_eq!(
            detect_format("[{\"type\": \"message\"}]").unwrap(),
            "slack_json"
        );
        assert!(detect_format("plain text").is_some());
    }
}
