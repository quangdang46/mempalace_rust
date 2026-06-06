/// Zero-LLM fallback compression: infers observation type and extracts files
/// from raw observation data without calling an LLM.
/// 1:1 port from mempalace `src/functions/compress-synthetic.ts`.
/// Enhanced with heuristic fact extraction, entity extraction, and importance scoring.
use chrono::Utc;
use serde_json::Value;
use std::collections::HashSet;

use crate::types::{CompressedObservation, HookType, ObservationType};

/// Infer the observation type from the hook type and tool name.
/// 1:1 port of `inferType()` from mempalace.
pub fn infer_type(tool_name: Option<&str>, hook_type: &HookType) -> ObservationType {
    // Tool-name-based classification takes precedence when the hook
    // also has a generic override (e.g. `PostToolUseFailure` should
    // still report the *type* of the tool that failed, not a blanket
    // `Error`).
    let tool_based = tool_name.and_then(|name| {
        // Normalize: convert camelCase and kebab-case into word chunks
        let mut normalized = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                normalized.push('_');
            }
            normalized.push(c.to_ascii_lowercase());
        }
        let normalized = normalized.replace(['-', ' '], "_");
        let normalized = normalized.strip_prefix('_').unwrap_or(&normalized);

        let has_word = |word: &str| -> bool {
            normalized == word
                || normalized.starts_with(&format!("{word}_"))
                || normalized.ends_with(&format!("_{word}"))
                || normalized.contains(&format!("_{word}_"))
        };

        if ["fetch", "http", "web"].iter().any(|w| has_word(w)) {
            return Some(ObservationType::WebFetch);
        }
        if ["grep", "search", "glob", "find"]
            .iter()
            .any(|w| has_word(w))
        {
            return Some(ObservationType::Search);
        }
        if ["bash", "shell", "exec", "run"].iter().any(|w| has_word(w)) {
            return Some(ObservationType::CommandRun);
        }
        if ["edit", "update", "patch", "replace"]
            .iter()
            .any(|w| has_word(w))
        {
            return Some(ObservationType::FileEdit);
        }
        if ["write", "create"].iter().any(|w| has_word(w)) {
            return Some(ObservationType::FileWrite);
        }
        if ["read", "view"].iter().any(|w| has_word(w)) {
            return Some(ObservationType::FileRead);
        }
        if ["task", "agent"].iter().any(|w| has_word(w)) {
            return Some(ObservationType::Subagent);
        }
        None
    });
    if let Some(t) = tool_based {
        return t;
    }

    // Hook-type-based overrides (only when no tool name matches)
    match hook_type {
        HookType::PostToolUseFailure => return ObservationType::Error,
        HookType::UserPromptSubmit => return ObservationType::Conversation,
        HookType::SubagentStop | HookType::TaskCompleted => return ObservationType::Subagent,
        HookType::Notification => return ObservationType::Notification,
        HookType::Stop => return ObservationType::SessionEnd,
        _ => {}
    }

    ObservationType::Other
}

/// Extract file paths from a JSON value by scanning for common file path keys.
/// 1:1 port of `extractFiles()` from mempalace.
pub fn extract_files(input: &Value) -> Vec<String> {
    let file_keys = [
        "file_path",
        "filepath",
        "path",
        "filePath",
        "file",
        "pattern",
    ];

    let Value::Object(map) = input else {
        return vec![];
    };

    let mut out = Vec::new();
    for key in file_keys {
        if let Some(Value::String(v)) = map.get(key) {
            if !v.is_empty() && v.len() < 512 {
                out.push(v.clone());
            }
        }
    }

    // Deduplicate
    out.sort();
    out.dedup();
    out
}

fn stringify_for_narrative(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() > n {
        format!("{}…", &s[..n.saturating_sub(1)])
    } else {
        s.to_string()
    }
}

/// Extract key facts from raw observation data using heuristics.
///
/// Scans the input for patterns like:
/// - Error messages and exceptions
/// - Success/failure indicators
/// - File operations (read/write/edit)
/// - Command executions
/// - Key-value pairs that look like facts
pub fn extract_facts(input: &Value) -> Vec<String> {
    let mut facts = Vec::new();

    let text = match input {
        Value::Object(map) => {
            // Collect string values that might contain facts
            let mut parts = Vec::new();
            for (key, val) in map {
                if let Value::String(s) = val {
                    parts.push(format!("{}: {}", key, s));
                } else if let Value::Number(n) = val {
                    parts.push(format!("{}: {}", key, n));
                }
            }
            parts.join("; ")
        }
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => return facts,
    };

    // Extract error patterns
    let error_patterns = [
        r"(?i)error[:\s]+([^\n]+)",
        r"(?i)exception[:\s]+([^\n]+)",
        r"(?i)failed[:\s]+([^\n]+)",
        r"(?i)failure[:\s]+([^\n]+)",
    ];

    for pattern in error_patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            for cap in re.captures_iter(&text) {
                if let Some(m) = cap.get(1) {
                    let fact = format!("Error: {}", m.as_str().trim());
                    if !fact.is_empty() && fact.len() < 200 {
                        facts.push(fact);
                    }
                }
            }
        }
    }

    // Extract success patterns
    if text.to_lowercase().contains("success")
        || text.to_lowercase().contains("completed")
        || text.to_lowercase().contains("done")
    {
        facts.push("Operation completed successfully".to_string());
    }

    // Extract file paths mentioned in text
    let path_regex = regex::Regex::new(r"(/[a-zA-Z0-9_./-]+)").ok();
    if let Some(re) = path_regex {
        let paths: Vec<String> = re
            .captures_iter(&text)
            .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
            .filter(|p| p.len() < 512)
            .collect();
        if !paths.is_empty() {
            facts.push(format!("Files: {}", paths.join(", ")));
        }
    }

    // Extract "key: value" patterns that look like facts
    let kv_regex = regex::Regex::new(r"([a-zA-Z_][a-zA-Z0-9_]*)[:=\s]+([^\n,;]{3,100})").ok();
    if let Some(re) = kv_regex {
        for cap in re.captures_iter(&text) {
            if let (Some(key), Some(val)) = (cap.get(1), cap.get(2)) {
                let key_lower = key.as_str().to_lowercase();
                // Skip common non-factual keys
                if !["content", "data", "text", "body"].contains(&key_lower.as_str()) {
                    let fact = format!("{}: {}", key.as_str(), val.as_str().trim());
                    if !facts.contains(&fact) && fact.len() < 200 {
                        facts.push(fact);
                    }
                }
            }
        }
    }

    // Deduplicate and limit
    facts.sort();
    facts.dedup();
    facts.truncate(10);
    facts
}

/// Extract important entities from raw observation data.
///
/// Returns:
/// - Function names (camelCase, snake_case patterns)
/// - File paths
/// - Error messages
/// - Important keywords and patterns
pub fn extract_entities(input: &Value) -> (Vec<String>, Vec<String>) {
    let mut functions = Vec::new();
    let mut keywords = Vec::new();

    let text = stringify_for_narrative(input);

    // Extract function names (camelCase and snake_case)
    let camel_regex = regex::Regex::new(r"\b([a-z][a-z0-9]+[A-Z][a-zA-Z0-9]+)\b").ok();
    let snake_regex = regex::Regex::new(r"\b([a-z][a-z0-9]+(?:_[a-z][a-z0-9]+)+)\b").ok();

    if let Some(re) = camel_regex {
        for cap in re.captures_iter(&text) {
            if let Some(m) = cap.get(1) {
                let fn_name = m.as_str().to_string();
                if !functions.contains(&fn_name) && fn_name.len() < 100 {
                    functions.push(fn_name);
                }
            }
        }
    }

    if let Some(re) = snake_regex {
        for cap in re.captures_iter(&text) {
            if let Some(m) = cap.get(1) {
                let fn_name = m.as_str().to_string();
                if !functions.contains(&fn_name) && fn_name.len() < 100 {
                    functions.push(fn_name);
                }
            }
        }
    }

    // Extract important keywords
    let important_patterns = [
        "error",
        "exception",
        "warning",
        "failed",
        "success",
        "created",
        "updated",
        "deleted",
        "read",
        "wrote",
        "edited",
        "executed",
        "ran",
        "built",
        "compiled",
        "tested",
        "config",
        "settings",
        "options",
        "parameters",
    ];

    let text_lower = text.to_lowercase();
    for pattern in important_patterns {
        if text_lower.contains(pattern) {
            if !keywords.contains(&pattern.to_string()) {
                keywords.push(pattern.to_string());
            }
        }
    }

    // Extract error messages
    let error_regex =
        regex::Regex::new(r#"(?i)(?:error|exception|failed)[:\s]+([^\n]{5,150})"#).ok();
    if let Some(re) = error_regex {
        for cap in re.captures_iter(&text) {
            if let Some(m) = cap.get(1) {
                let error = m.as_str().trim().to_string();
                if error.len() < 200 && !keywords.contains(&error) {
                    keywords.push(error);
                }
            }
        }
    }

    functions.truncate(5);
    keywords.truncate(10);

    (functions, keywords)
}

/// Score the importance of an observation on a 0.0-1.0 scale based on heuristics.
///
/// Factors:
/// - Error observations are high importance
/// - File edits are medium-high importance
/// - File reads are medium importance
/// - Command runs vary based on content
/// - Observations with many facts are higher importance
/// - Observations with files are higher importance
pub fn score_importance(
    tool_name: Option<&str>,
    hook_type: &HookType,
    tool_input: Option<&Value>,
    tool_output: Option<&Value>,
) -> f64 {
    let mut importance: f64 = 0.5; // Base importance

    // Hook-type adjustments
    match hook_type {
        HookType::PostToolUseFailure => importance += 0.3,
        HookType::SessionEnd => importance -= 0.1,
        HookType::Notification => importance += 0.1,
        _ => {}
    }

    // Tool-name adjustments
    let tool_lower = tool_name.unwrap_or("").to_lowercase();
    if tool_lower.contains("edit") || tool_lower.contains("write") {
        importance += 0.2;
    }
    if tool_lower.contains("delete") || tool_lower.contains("remove") {
        importance += 0.15;
    }
    if tool_lower.contains("error") || tool_lower.contains("fail") {
        importance += 0.2;
    }
    if tool_lower.contains("test") || tool_lower.contains("build") {
        importance += 0.1;
    }

    // Check for error indicators in output
    if let Some(output) = tool_output {
        let output_str = stringify_for_narrative(output).to_lowercase();
        if output_str.contains("error")
            || output_str.contains("exception")
            || output_str.contains("failed")
        {
            importance += 0.2;
        }
        if output_str.contains("success") || output_str.contains("completed") {
            importance += 0.05;
        }
    }

    // File references increase importance
    if let Some(input) = tool_input {
        let files = extract_files(input);
        if !files.is_empty() {
            importance += 0.1;
            // Multiple files = higher importance
            if files.len() > 2 {
                importance += 0.05;
            }
        }

        // Extract facts to see if there's substantial content
        let facts = extract_facts(input);
        if facts.len() >= 3 {
            importance += 0.1;
        }
    }

    importance.clamp(0.0, 1.0)
}

/// Generate a compressed summary that fits in a context window.
///
/// Uses heuristics to extract the most important information:
/// - Tool name and operation type
/// - Key file paths involved
/// - Error/success indicators
/// - First few substantive facts
pub fn generate_summary(
    tool_name: Option<&str>,
    hook_type: &HookType,
    tool_input: Option<&Value>,
    tool_output: Option<&Value>,
    user_prompt: Option<&str>,
    max_len: usize,
) -> String {
    let mut parts = Vec::new();

    // Add tool name
    if let Some(name) = tool_name {
        parts.push(format!("[{}]", name));
    }

    // Add hook type if no tool name
    if tool_name.is_none() {
        parts.push(format!("[{:?}]", hook_type));
    }

    // Add user prompt if provided
    if let Some(prompt) = user_prompt {
        let truncated_prompt = truncate(prompt, 100);
        if !truncated_prompt.is_empty() {
            parts.push(truncated_prompt);
        }
    }

    // Extract and add key facts
    if let Some(input) = tool_input {
        let facts = extract_facts(input);
        for fact in facts.into_iter().take(3) {
            parts.push(truncate(&fact, 80));
        }
    }

    // Add error/success indicator from output
    if let Some(output) = tool_output {
        let output_str = stringify_for_narrative(output);
        let output_lower = output_str.to_lowercase();
        if output_lower.contains("error")
            || output_lower.contains("exception")
            || output_lower.contains("failed")
        {
            parts.push("[FAILED]".to_string());
        } else if output_lower.contains("success") || output_lower.contains("completed") {
            parts.push("[OK]".to_string());
        }

        // Add last part of output if it looks important
        if output_str.len() > 10 {
            let truncated_output = truncate(&output_str, 150);
            if !truncated_output.is_empty() {
                parts.push(truncated_output);
            }
        }
    }

    // Join parts and truncate to max_len
    let summary = parts.join(" | ");
    truncate(&summary, max_len)
}

/// Create a synthetic compressed observation from raw data without using an LLM.
/// 1:1 port of `buildSyntheticCompression()` from mempalace.
pub fn build_synthetic_compression(
    tool_name: Option<&str>,
    hook_type: &HookType,
    tool_input: Option<&Value>,
    tool_output: Option<&Value>,
    user_prompt: Option<&str>,
    modality: Option<&str>,
    image_data: Option<&str>,
    agent_id: Option<&str>,
) -> CompressedObservation {
    let hook_str = hook_type.to_string();
    let name = tool_name.unwrap_or(&hook_str);
    let input_str = tool_input.map(stringify_for_narrative).unwrap_or_default();
    let output_str = tool_output.map(stringify_for_narrative).unwrap_or_default();
    let prompt_str = user_prompt.unwrap_or("");

    let narrative_parts: Vec<&str> = [prompt_str, &input_str, &output_str]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect();

    let narrative = truncate(&narrative_parts.join(" | "), 400);

    let files = tool_input.map(extract_files).unwrap_or_default();

    // Extract facts using heuristics
    let facts = tool_input.map(extract_facts).unwrap_or_default();

    // Extract entities (functions, keywords)
    let (functions, keywords) = tool_input.map(extract_entities).unwrap_or_default();

    // Build concepts from functions and keywords
    let mut concepts: Vec<String> = Vec::new();
    concepts.extend(functions);
    concepts.extend(keywords);
    concepts.truncate(10);

    // Score importance using heuristics
    let importance_score = score_importance(tool_name, hook_type, tool_input, tool_output);
    let importance = (importance_score * 9.0).round().clamp(1.0, 10.0) as u8;

    CompressedObservation {
        id: String::new(),
        session_id: String::new(),
        timestamp: Utc::now(),
        observation_type: infer_type(tool_name, hook_type),
        title: truncate(name, 80),
        subtitle: if input_str.is_empty() {
            None
        } else {
            Some(truncate(&input_str, 120))
        },
        facts,
        narrative,
        concepts,
        files,
        importance,
        confidence: 0.3,
        image_ref: image_data.map(String::from),
        image_description: None,
        modality: modality.unwrap_or("text").to_string(),
        agent_id: agent_id.map(String::from),
    }
}

/// Calculate quality score for a compressed observation.
/// Returns a score from 0-100.
/// 1:1 port of `scoreCompression()` from mempalace.
pub fn score_compression(obs: &CompressedObservation) -> u8 {
    let mut score: u8 = 0;
    if !obs.facts.is_empty() {
        score += 25;
    }
    if obs.facts.len() >= 3 {
        score += 10;
    }
    if obs.narrative.len() >= 20 {
        score += 20;
    }
    if obs.narrative.len() >= 50 {
        score += 5;
    }
    if obs.title.len() >= 5 && obs.title.len() <= 120 {
        score += 15;
    }
    if !obs.concepts.is_empty() {
        score += 15;
    }
    if obs.importance >= 1 && obs.importance <= 10 {
        score += 10;
    }
    score.min(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_type_hook_overrides() {
        assert_eq!(
            infer_type(None, &HookType::PostToolUseFailure),
            ObservationType::Error
        );
        assert_eq!(
            infer_type(None, &HookType::UserPromptSubmit),
            ObservationType::Conversation
        );
        assert_eq!(
            infer_type(None, &HookType::SubagentStop),
            ObservationType::Subagent
        );
        assert_eq!(
            infer_type(None, &HookType::TaskCompleted),
            ObservationType::Subagent
        );
        assert_eq!(
            infer_type(None, &HookType::Notification),
            ObservationType::Notification
        );
        assert_eq!(
            infer_type(None, &HookType::Stop),
            ObservationType::SessionEnd
        );
    }

    #[test]
    fn test_infer_type_tool_patterns() {
        assert_eq!(
            infer_type(Some("WebFetch"), &HookType::PostToolUse),
            ObservationType::WebFetch
        );
        assert_eq!(
            infer_type(Some("http_request"), &HookType::PostToolUse),
            ObservationType::WebFetch
        );
        assert_eq!(
            infer_type(Some("grep"), &HookType::PostToolUse),
            ObservationType::Search
        );
        assert_eq!(
            infer_type(Some("search_files"), &HookType::PostToolUse),
            ObservationType::Search
        );
        assert_eq!(
            infer_type(Some("bash"), &HookType::PostToolUse),
            ObservationType::CommandRun
        );
        assert_eq!(
            infer_type(Some("run_command"), &HookType::PostToolUse),
            ObservationType::CommandRun
        );
        assert_eq!(
            infer_type(Some("edit_file"), &HookType::PostToolUse),
            ObservationType::FileEdit
        );
        assert_eq!(
            infer_type(Some("replace"), &HookType::PostToolUse),
            ObservationType::FileEdit
        );
        assert_eq!(
            infer_type(Some("write_file"), &HookType::PostToolUse),
            ObservationType::FileWrite
        );
        assert_eq!(
            infer_type(Some("create"), &HookType::PostToolUse),
            ObservationType::FileWrite
        );
        assert_eq!(
            infer_type(Some("read_file"), &HookType::PostToolUse),
            ObservationType::FileRead
        );
        assert_eq!(
            infer_type(Some("view"), &HookType::PostToolUse),
            ObservationType::FileRead
        );
        assert_eq!(
            infer_type(Some("task_runner"), &HookType::PostToolUse),
            ObservationType::Subagent
        );
    }

    #[test]
    fn test_infer_type_no_tool() {
        assert_eq!(
            infer_type(None, &HookType::PostToolUse),
            ObservationType::Other
        );
    }

    #[test]
    fn test_extract_files() {
        let input = serde_json::json!({
            "file_path": "/src/main.rs",
            "other_key": "value",
            "path": "/src/lib.rs"
        });
        let files = extract_files(&input);
        assert_eq!(files, vec!["/src/lib.rs", "/src/main.rs"]);
    }

    #[test]
    fn test_extract_files_dedup() {
        let input = serde_json::json!({
            "file_path": "/same.rs",
            "path": "/same.rs"
        });
        let files = extract_files(&input);
        assert_eq!(files, vec!["/same.rs"]);
    }

    #[test]
    fn test_extract_files_long_path_filtered() {
        let input = serde_json::json!({
            "path": "x".repeat(600)
        });
        let files = extract_files(&input);
        assert!(files.is_empty());
    }

    #[test]
    fn test_extract_files_not_object() {
        let input = serde_json::json!("just a string");
        let files = extract_files(&input);
        assert!(files.is_empty());
    }

    #[test]
    fn test_extract_facts_error() {
        let input = serde_json::json!({
            "error": "file not found",
            "path": "/src/main.rs"
        });
        let facts = extract_facts(&input);
        assert!(!facts.is_empty());
        // Should contain an error fact
        assert!(facts.iter().any(|f| f.to_lowercase().contains("error")));
    }

    #[test]
    fn test_extract_facts_key_value() {
        let input = serde_json::json!({
            "status": "completed",
            "count": 42
        });
        let facts = extract_facts(&input);
        assert!(!facts.is_empty());
    }

    #[test]
    fn test_extract_entities() {
        let input = serde_json::json!({
            "function": "processData",
            "message": "Error: timeout occurred"
        });
        let (functions, keywords) = extract_entities(&input);
        assert!(!functions.is_empty() || !keywords.is_empty());
    }

    #[test]
    fn test_score_importance_base() {
        let score = score_importance(
            Some("read_file"),
            &HookType::PostToolUse,
            Some(&serde_json::json!({"path": "/src/main.rs"})),
            None,
        );
        assert!(score >= 0.0 && score <= 1.0);
    }

    #[test]
    fn test_score_importance_error() {
        let score = score_importance(
            Some("bash"),
            &HookType::PostToolUseFailure,
            Some(&serde_json::json!({"command": "ls"})),
            Some(&serde_json::json!("error: command not found")),
        );
        assert!(score > 0.5); // Error should increase importance
    }

    #[test]
    fn test_generate_summary() {
        let summary = generate_summary(
            Some("edit_file"),
            &HookType::PostToolUse,
            Some(&serde_json::json!({"path": "/src/main.rs", "content": "fn main() {}"})),
            Some(&serde_json::json!("File updated successfully")),
            Some("Edit main.rs"),
            200,
        );
        assert!(!summary.is_empty());
        assert!(summary.len() <= 200);
    }

    #[test]
    fn test_quality_score_perfect() {
        let obs = CompressedObservation {
            id: "test".to_string(),
            session_id: "sess".to_string(),
            timestamp: Utc::now(),
            observation_type: ObservationType::FileRead,
            title: "A good title".to_string(),
            subtitle: None,
            facts: vec!["f1".to_string(), "f2".to_string(), "f3".to_string()],
            narrative: "This is a narrative that is long enough to get the full score here"
                .to_string(),
            concepts: vec!["concept1".to_string()],
            files: vec![],
            importance: 7,
            confidence: 0.0,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        };
        assert_eq!(score_compression(&obs), 100);
    }

    #[test]
    fn test_quality_score_empty() {
        let obs = CompressedObservation {
            id: "test".to_string(),
            session_id: "sess".to_string(),
            timestamp: Utc::now(),
            observation_type: ObservationType::FileRead,
            title: "".to_string(),
            subtitle: None,
            facts: vec![],
            narrative: "".to_string(),
            concepts: vec![],
            files: vec![],
            importance: 0,
            confidence: 0.0,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        };
        assert_eq!(score_compression(&obs), 0);
    }

    #[test]
    fn test_quality_score_capped() {
        let obs = CompressedObservation {
            id: "test".to_string(),
            session_id: "sess".to_string(),
            timestamp: Utc::now(),
            observation_type: ObservationType::FileRead,
            title: "A good title here".to_string(),
            subtitle: None,
            facts: vec!["f1".to_string(), "f2".to_string(), "f3".to_string(), "f4".to_string()],
            narrative: "This is a narrative that is long enough to get the full score here and then some more text".to_string(),
            concepts: vec!["c1".to_string(), "c2".to_string()],
            files: vec![],
            importance: 7,
            confidence: 0.0,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        };
        // Would be 25+10+20+5+15+15+10 = 100, even with extra facts/concepts stays capped
        assert_eq!(score_compression(&obs), 100);
    }

    #[test]
    fn test_build_synthetic_compression() {
        let obs = build_synthetic_compression(
            Some("read_file"),
            &HookType::PostToolUse,
            Some(&serde_json::json!({"file_path": "/src/main.rs"})),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(obs.observation_type, ObservationType::FileRead);
        assert_eq!(obs.title, "read_file");
        assert_eq!(obs.confidence, 0.3);
        assert_eq!(obs.files, vec!["/src/main.rs"]);
    }

    #[test]
    fn test_build_synthetic_compression_with_facts() {
        let obs = build_synthetic_compression(
            Some("bash"),
            &HookType::PostToolUseFailure,
            Some(&serde_json::json!({
                "command": "npm install",
                "error": "package not found"
            })),
            Some(&serde_json::json!("error: command failed")),
            None,
            None,
            None,
            None,
        );
        assert_eq!(obs.observation_type, ObservationType::CommandRun);
        // Should have facts extracted
        assert!(!obs.facts.is_empty());
        // Should have higher importance due to error
        assert!(obs.importance >= 5);
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate("hello world this is long", 10);
        assert!(result.ends_with('…'));
        assert!(result.chars().count() <= 10);
    }
}
