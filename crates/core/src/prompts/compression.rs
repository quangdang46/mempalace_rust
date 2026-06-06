/// Compression prompt templates.
/// 1:1 port from mempalace `src/prompts/compression.ts`.
use crate::types::RawObservation;

/// System prompt for memory compression.
pub const COMPRESSION_SYSTEM: &str = r#"You are a memory compression engine for an AI coding agent. Your job is to extract the essential information from a tool usage observation and compress it into structured data.

Output EXACTLY this XML format with no additional text:

<observation>
  <type>one of: file_read, file_write, file_edit, command_run, search, web_fetch, conversation, error, decision, discovery, subagent, notification, task, other</type>
  <title>Short descriptive title (max 80 chars)</title>
  <subtitle>One-line context (optional)</subtitle>
  <facts>
    <fact>Specific factual detail 1</fact>
    <fact>Specific factual detail 2</fact>
  </facts>
  <narrative>2-3 sentence summary of what happened and why it matters</narrative>
  <concepts>
    <concept>technical concept or pattern</concept>
  </concepts>
  <files>
    <file>path/to/file</file>
  </files>
  <importance>1-10 scale, 10 being critical architectural decision</importance>
</observation>

Rules:
- Be concise but preserve ALL technically relevant details
- File paths must be exact
- Importance: 1-3 for routine reads, 4-6 for edits/commands, 7-9 for architectural decisions, 10 for breaking changes
- Concepts should be reusable search terms (e.g., "React hooks", "SQL migration", "auth middleware")
- Strip any secrets, tokens, or credentials from the output"#;

/// Suffix appended when retry is needed due to invalid output format.
pub const STRICTER_SUFFIX: &str = r#"

IMPORTANT: Your previous response was invalid. Please ensure your output strictly follows the required XML format. Every required field must be present with valid values."#;

/// Truncate a string to a maximum length.
fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}[...truncated]", &s[..max])
    } else {
        s.to_string()
    }
}

/// Build a compression prompt from a raw observation.
/// 1:1 port of `buildCompressionPrompt()` from mempalace.
pub fn build_compression_prompt(obs: &RawObservation) -> String {
    let mut parts = Vec::new();

    parts.push(format!("Timestamp: {}", obs.timestamp));
    parts.push(format!("Hook: {}", obs.hook_type));

    if let Some(ref tool_name) = obs.tool_name {
        parts.push(format!("Tool: {tool_name}"));
    }

    if let Some(ref tool_input) = obs.tool_input {
        parts.push(format!("Input:\n{}", truncate(tool_input, 4000)));
    }

    if let Some(ref tool_output) = obs.tool_output {
        parts.push(format!("Output:\n{}", truncate(tool_output, 4000)));
    }

    if let Some(ref user_prompt) = obs.user_prompt {
        parts.push(format!("User prompt:\n{}", truncate(user_prompt, 2000)));
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HookType;
    use chrono::Utc;

    fn make_raw_obs() -> RawObservation {
        RawObservation {
            id: "test-1".to_string(),
            session_id: "sess-1".to_string(),
            hook_type: HookType::PostToolUse,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            user_prompt: None,
            assistant_response: None,
            raw: None,
            modality: "text".to_string(),
            image_data: None,
            agent_id: None,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world this is long", 10);
        assert_eq!(result, "hello worl[...truncated]");
    }

    #[test]
    fn test_build_compression_prompt_minimal() {
        let obs = make_raw_obs();

        let prompt = build_compression_prompt(&obs);
        assert!(prompt.contains("Timestamp:"));
        assert!(prompt.contains("Hook: post_tool_use"));
    }

    #[test]
    fn test_build_compression_prompt_with_tool() {
        let mut obs = make_raw_obs();
        obs.tool_name = Some("read".to_string());
        obs.tool_input = Some("/foo/bar.rs".to_string());
        obs.tool_output = Some("fn main() {}".to_string());
        obs.user_prompt = Some("read the file".to_string());

        let prompt = build_compression_prompt(&obs);
        assert!(prompt.contains("Tool: read"));
        assert!(prompt.contains("Input:"));
        assert!(prompt.contains("Output:"));
        assert!(prompt.contains("User prompt:"));
    }

    #[test]
    fn test_build_compression_prompt_truncation() {
        let mut obs = make_raw_obs();
        obs.tool_name = Some("run".to_string());
        obs.tool_input = Some("x".repeat(5000));

        let prompt = build_compression_prompt(&obs);
        assert!(prompt.contains("[...truncated]"));
        let input_section = prompt.split("Input:\n").nth(1).unwrap_or("");
        assert!(input_section.len() <= 4020);
    }

    #[test]
    fn test_compression_system_prompt_content() {
        assert!(COMPRESSION_SYSTEM.contains("memory compression engine"));
        assert!(COMPRESSION_SYSTEM.contains("<observation>"));
        assert!(COMPRESSION_SYSTEM.contains("<facts>"));
        assert!(COMPRESSION_SYSTEM.contains("<importance>"));
    }

    #[test]
    fn test_stricter_suffix_content() {
        assert!(STRICTER_SUFFIX.contains("previous response was invalid"));
        assert!(STRICTER_SUFFIX.contains("XML format"));
    }
}
