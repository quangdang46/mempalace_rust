use serde_json::Value;

pub fn normalize(file_path: &std::path::Path, content: &str) -> anyhow::Result<String> {
    if content.trim().is_empty() {
        return Ok(content.to_string());
    }

    let lines: Vec<&str> = content.split('\n').collect();
    let quote_count = lines.iter().filter(|l| l.trim().starts_with('>')).count();
    if quote_count >= 3 {
        return Ok(content.to_string());
    }

    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
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
    let mut messages: Vec<(&str, &str)> = Vec::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let entry = entry.as_object()?;
        let msg_type = entry.get("type")?.as_str()?;
        let message = entry.get("message")?.as_object()?;

        let text = extract_content(message.get("content")?);
        if text.is_empty() {
            continue;
        }

        match msg_type {
            "human" => messages.push(("user", text)),
            "assistant" => messages.push(("assistant", text)),
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
    let mut messages: Vec<(&str, &str)> = Vec::new();

    for item in list {
        let obj = item.as_object()?;
        let role = obj.get("role")?.as_str()?;
        let text = extract_content(obj.get("content")?);

        if text.is_empty() {
            continue;
        }

        if (role == "user" || role == "human") {
            messages.push(("user", text));
        } else if role == "assistant" || role == "ai" {
            messages.push(("assistant", text));
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
        if node.get("parent").is_none() || node.get("parent")?.is_null() {
            if node.get("message").is_none() || node.get("message")?.is_null() {
                root_id = Some(node_id);
                break;
            } else if fallback_root.is_none() {
                fallback_root = Some(node_id);
            }
        }
    }

    let root_id = root_id.or(fallback_root)?;
    let mut messages: Vec<(&str, &str)> = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut current_id: &str = root_id;

    while current_id.is_empty() == false && !visited.contains(current_id) {
        visited.insert(current_id);
        let node = mapping.get(current_id)?.as_object()?;
        if let Some(msg) = node.get("message") {
            let msg = msg.as_object()?;
            let role = msg.get("author")?.as_object()?.get("role")?.as_str()?;
            let content = msg.get("content")?;

            let parts = if content.is_object() {
                content.get("parts")?.as_array()?
            } else {
                &Vec::new()
            };

            let text: String = parts
                .iter()
                .filter_map(|p| p.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();

            if text.is_empty() {
                current_id = node.get("children")?.as_array()?.first()?.as_str()?;
                continue;
            }

            if role == "user" {
                messages.push(("user", text));
            } else if role == "assistant" {
                messages.push(("assistant", text));
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
    let mut messages: Vec<(&str, &str)> = Vec::new();
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
        let text = obj.get("text")?.as_str().unwrap_or("").trim();

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
        messages.push((role, text));
    }

    if messages.len() >= 2 {
        return Some(messages_to_transcript(&messages));
    }
    None
}

fn extract_content(content: &Value) -> &str {
    match content {
        Value::String(s) => s.trim(),
        Value::Array(arr) => {
            let parts: Vec<&str> = arr
                .iter()
                .filter_map(|item| match item {
                    Value::String(s) => Some(*s),
                    Value::Object(obj) if obj.get("type")?.as_str() == Some("text") => {
                        obj.get("text")?.as_str()
                    }
                    _ => None,
                })
                .collect();
            let joined = parts.join(" ");
            Box::leak(joined.into_boxed_str()) as &str
        }
        Value::Object(obj) => obj.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        _ => "",
    }
}

fn messages_to_transcript(messages: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let (role, text) = messages[i];

        if role == "user" {
            lines.push(format!("> {}", text));
            if i + 1 < messages.len() && messages[i + 1].0 == "assistant" {
                lines.push(messages[i + 1].1.to_string());
                i += 2;
            } else {
                i += 1;
            }
        } else {
            lines.push(text.to_string());
            i += 1;
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

pub fn detect_format(content: &str) -> Option<String> {
    let trimmed = content.trim();

    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(data) = serde_json::from_str::<Value>(trimmed) {
            if data.get("type").and_then(|v| v.as_str()) == Some("conversation") {
                return Some("claude_code_jsonl".to_string());
            }
            if data.get("messages").is_some() || data.get("chat_messages").is_some() {
                return Some("claude_ai_json".to_string());
            }
            if data.get("mapping").is_some() {
                return Some("chatgpt_json".to_string());
            }
            if data.is_array() {
                let first = data.as_array()?.first()?;
                if first.get("type").and_then(|v| v.as_str()) == Some("message") {
                    return Some("slack_json".to_string());
                }
            }
        }
    }

    let lines: Vec<&str> = content.split('\n').collect();
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
