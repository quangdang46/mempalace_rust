/// Zero-LLM fallback compression: infers observation type and extracts files
/// from raw observation data without calling an LLM.
/// 1:1 port from agentmemory `src/functions/compress-synthetic.ts`.

use chrono::Utc;
use serde_json::Value;

use crate::types::{CompressedObservation, HookType, ObservationType};

/// Infer the observation type from the hook type and tool name.
/// 1:1 port of `inferType()` from agentmemory.
pub fn infer_type(tool_name: Option<&str>, hook_type: &HookType) -> ObservationType {
    // Hook-type-based overrides
    match hook_type {
        HookType::PostToolUseFailure => return ObservationType::Error,
        HookType::UserPromptSubmit => return ObservationType::Conversation,
        HookType::SubagentStop | HookType::TaskCompleted => return ObservationType::Subagent,
        HookType::Notification => return ObservationType::Notification,
        HookType::Stop => return ObservationType::SessionEnd,
        _ => {}
    }

    let Some(name) = tool_name else {
        return ObservationType::Other;
    };

    // Normalize: convert camelCase and kebab-case into word chunks
    let mut normalized = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            normalized.push('_');
        }
        normalized.push(c.to_ascii_lowercase());
    }
    let normalized = normalized.replace(['-', ' '], "_");

    // Strip leading underscore if present
    let normalized = normalized.strip_prefix('_').unwrap_or(&normalized);

    let has_word = |word: &str| -> bool {
        normalized == word
            || normalized.starts_with(&format!("{word}_"))
            || normalized.ends_with(&format!("_{word}"))
            || normalized.contains(&format!("_{word}_"))
    };

    if ["fetch", "http", "web"].iter().any(|w| has_word(w)) {
        return ObservationType::WebFetch;
    }
    if ["grep", "search", "glob", "find"].iter().any(|w| has_word(w)) {
        return ObservationType::Search;
    }
    if ["bash", "shell", "exec", "run"].iter().any(|w| has_word(w)) {
        return ObservationType::CommandRun;
    }
    if ["edit", "update", "patch", "replace"]
        .iter()
        .any(|w| has_word(w))
    {
        return ObservationType::FileEdit;
    }
    if ["write", "create"].iter().any(|w| has_word(w)) {
        return ObservationType::FileWrite;
    }
    if ["read", "view"].iter().any(|w| has_word(w)) {
        return ObservationType::FileRead;
    }
    if ["task", "agent"].iter().any(|w| has_word(w)) {
        return ObservationType::Subagent;
    }

    ObservationType::Other
}

/// Extract file paths from a JSON value by scanning for common file path keys.
/// 1:1 port of `extractFiles()` from agentmemory.
pub fn extract_files(input: &Value) -> Vec<String> {
    let file_keys = [
        "file_path", "filepath", "path", "filePath", "file", "pattern",
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

/// Create a synthetic compressed observation from raw data without using an LLM.
/// 1:1 port of `buildSyntheticCompression()` from agentmemory.
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
        facts: vec![],
        narrative,
        concepts: vec![],
        files,
        importance: 5,
        confidence: 0.3,
        image_ref: image_data.map(String::from),
        image_description: None,
        modality: modality.unwrap_or("text").to_string(),
        agent_id: agent_id.map(String::from),
    }
}

/// Calculate quality score for a compressed observation.
/// Returns a score from 0-100.
/// 1:1 port of `scoreCompression()` from agentmemory.
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
    fn test_quality_score_perfect() {
        let obs = CompressedObservation {
            id: "test".to_string(),
            session_id: "sess".to_string(),
            timestamp: Utc::now(),
            observation_type: ObservationType::FileRead,
            title: "A good title".to_string(),
            subtitle: None,
            facts: vec!["f1".to_string(), "f2".to_string(), "f3".to_string()],
            narrative: "This is a narrative that is long enough to get the full score here".to_string(),
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
        assert_eq!(obs.importance, 5);
        assert_eq!(obs.files, vec!["/src/main.rs"]);
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
