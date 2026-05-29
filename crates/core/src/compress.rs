/// LLM-powered observation compression engine.
/// 1:1 port from agentmemory `src/functions/compress.ts`.

use chrono::Utc;
use serde_json::Value;

use crate::llm::LlmProvider;
use crate::prompts::compression::{build_compression_prompt, COMPRESSION_SYSTEM, STRICTER_SUFFIX};
use crate::prompts::vision::VISION_DESCRIPTION_PROMPT;
use crate::types::{CompressedObservation, RawObservation};

/// The set of valid observation types from agentmemory.
const VALID_TYPES: &[&str] = &[
    "file_read",
    "file_write",
    "file_edit",
    "command_run",
    "search",
    "web_fetch",
    "conversation",
    "error",
    "decision",
    "discovery",
    "subagent",
    "notification",
    "task",
    "other",
];

/// Compress a raw observation using the LLM provider.
/// 1:1 port of the `mem::compress` function from agentmemory.
///
/// Returns the compressed observation with quality score, or falls back
/// to synthetic compression if the LLM call fails.
pub async fn compress_observation(
    provider: &dyn LlmProvider,
    raw: &RawObservation,
) -> CompressedObservation {
    // Try LLM compression first
    match try_llm_compress(provider, raw).await {
        Ok(obs) => obs,
        Err(e) => {
            // Fall back to synthetic
            crate::compress_synthetic::build_synthetic_compression(
                raw.tool_name.as_deref(),
                &raw.hook_type,
                raw.tool_input
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .as_ref(),
                raw.tool_output
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .as_ref(),
                raw.user_prompt.as_deref(),
                Some(&raw.modality),
                raw.image_data.as_ref().and_then(|img| img.path.as_deref()),
                raw.agent_id.as_deref(),
            )
        }
    }
}

/// Attempt LLM-based compression with retry logic.
async fn try_llm_compress(
    provider: &dyn LlmProvider,
    raw: &RawObservation,
) -> Result<CompressedObservation, String> {
    // Handle image description if applicable
    let mut tool_output = raw.tool_output.clone();
    let mut image_description: Option<String> = None;

    let has_image = raw.modality == "image" || raw.modality == "mixed";
    if has_image {
        if let Some(ref image_data) = raw.image_data {
            if let Some(ref base64) = image_data.base64 {
                match provider
                    .describe_image(base64, &image_data.mime_type, VISION_DESCRIPTION_PROMPT)
                    .await
                {
                    Ok(completion) => {
                        image_description = Some(completion.text.clone());
                        tool_output = Some(format!(
                            "[Image Description]: {}\n\n{}",
                            completion.text,
                            raw.tool_output.as_deref().unwrap_or("")
                        ));
                    }
                    Err(e) => {
                        // Log warning but continue with text-only compression
                        eprintln!(
                            "Vision model call failed, falling back to text-only compression: {e}"
                        );
                    }
                }
            }
        }
    }

    // Build the prompt
    let prompt_obs = RawObservation {
        id: raw.id.clone(),
        session_id: raw.session_id.clone(),
        timestamp: raw.timestamp,
        hook_type: raw.hook_type,
        tool_name: raw.tool_name.clone(),
        tool_input: raw.tool_input.clone(),
        tool_output,
        user_prompt: raw.user_prompt.clone(),
        assistant_response: raw.assistant_response.clone(),
        raw: raw.raw.clone(),
        modality: raw.modality.clone(),
        image_data: raw.image_data.clone(),
        agent_id: raw.agent_id.clone(),
    };
    let prompt = build_compression_prompt(&prompt_obs);

    // Try with retry
    let (response, _retried) = compress_with_retry(provider, &prompt).await?;

    // Parse the XML response
    let mut parsed = parse_compression_xml(&response).ok_or("xml_parse_failed")?;

    // Calculate quality score
    let quality_score = crate::compress_synthetic::score_compression(&parsed);
    parsed.confidence = quality_score as f64 / 100.0;

    // Fill in ID and session info
    parsed.id = raw.id.clone();
    parsed.session_id = raw.session_id.clone();
    parsed.timestamp = raw.timestamp;

    // Attach image metadata if present
    if has_image {
        parsed.modality = raw.modality.clone();
    }
    if let Some(desc) = image_description {
        parsed.image_description = Some(desc);
    }
    if let Some(ref img) = raw.image_data {
        if let Some(ref path) = img.path {
            parsed.image_ref = Some(path.clone());
        } else if img.base64.is_some() {
            parsed.image_ref = Some("inline".to_string());
        }
    }
    if let Some(ref agent_id) = raw.agent_id {
        parsed.agent_id = Some(agent_id.clone());
    }

    Ok(parsed)
}

/// Compress with retry: try once, and if validation fails, retry with stricter prompt.
/// 1:1 port of `compressWithRetry()` from agentmemory.
async fn compress_with_retry(
    provider: &dyn LlmProvider,
    user_prompt: &str,
) -> Result<(String, bool), String> {
    // First attempt
    let first = provider
        .complete(COMPRESSION_SYSTEM, user_prompt)
        .await
        .map_err(|e| e.to_string())?;

    if validate_compression_xml(&first.text) {
        return Ok((first.text, false));
    }

    // Retry with stricter suffix
    let stricter_system = format!("{COMPRESSION_SYSTEM}{STRICTER_SUFFIX}");
    let retry = provider
        .complete(&stricter_system, user_prompt)
        .await
        .map_err(|e| e.to_string())?;

    if validate_compression_xml(&retry.text) {
        return Ok((retry.text, true));
    }

    // Return first attempt even if invalid
    Ok((first.text, true))
}

/// Validate that the XML response has required fields.
fn validate_compression_xml(xml: &str) -> bool {
    let parsed = parse_compression_xml(xml);
    parsed.is_some()
}

/// Parse compression XML output into a CompressedObservation.
/// Validates type against VALID_TYPES and clamps importance [1,10].
pub fn parse_compression_xml(xml: &str) -> Option<CompressedObservation> {
    let raw_type = crate::prompts::xml::get_xml_tag(xml, "type");
    let title = crate::prompts::xml::get_xml_tag(xml, "title");

    if raw_type.is_empty() || title.is_empty() {
        return None;
    }

    // Validate type against VALID_TYPES, default to "other"
    let type_str = if VALID_TYPES.contains(&raw_type.as_str()) {
        raw_type
    } else {
        "other".to_string()
    };

    let subtitle = crate::prompts::xml::get_xml_tag(xml, "subtitle");
    let facts = crate::prompts::xml::get_xml_children(xml, "facts", "fact");
    let narrative = crate::prompts::xml::get_xml_tag(xml, "narrative");
    let concepts = crate::prompts::xml::get_xml_children(xml, "concepts", "concept");
    let files = crate::prompts::xml::get_xml_children(xml, "files", "file");
    let importance_str = crate::prompts::xml::get_xml_tag(xml, "importance");

    // Narrative is required
    if narrative.is_empty() {
        return None;
    }

    // Parse and clamp importance [1, 10]
    let importance: u8 = importance_str
        .parse::<u8>()
        .ok()
        .map(|v| v.clamp(1, 10))
        .unwrap_or(5);

    let observation_type: crate::types::ObservationType =
        type_str.parse().unwrap_or(crate::types::ObservationType::Other);

    Some(CompressedObservation {
        id: String::new(),
        session_id: String::new(),
        timestamp: Utc::now(),
        observation_type,
        title,
        subtitle: if subtitle.is_empty() {
            None
        } else {
            Some(subtitle)
        },
        facts,
        narrative,
        concepts,
        files,
        importance,
        confidence: 0.0,
        image_ref: None,
        image_description: None,
        modality: "text".to_string(),
        agent_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::noop_provider::NoopProvider;
    use crate::types::HookType;

    #[tokio::test]
    async fn test_compress_with_noop_provider() {
        let provider = NoopProvider::default();
        let raw = RawObservation {
            id: "test-1".to_string(),
            session_id: "sess-1".to_string(),
            timestamp: Utc::now(),
            hook_type: HookType::PostToolUse,
            tool_name: Some("read".to_string()),
            tool_input: Some("/src/main.rs".to_string()),
            tool_output: Some("fn main() {}".to_string()),
            user_prompt: Some("read the file".to_string()),
            assistant_response: None,
            raw: None,
            modality: "text".to_string(),
            image_data: None,
            agent_id: None,
        };

        // NoopProvider returns empty string, so XML parse will fail → falls back to synthetic
        let result = compress_observation(&provider, &raw).await;
        assert_eq!(result.id, ""); // Synthetic returns empty id
        assert_eq!(result.session_id, "");
        assert_eq!(result.confidence, 0.3); // Synthetic confidence
        assert_eq!(result.importance, 5); // Synthetic default
    }

    #[test]
    fn test_validate_compression_xml_valid() {
        let xml = r#"<observation>
            <type>file_read</type>
            <title>Read main.rs</title>
            <narrative>Read the main.rs file.</narrative>
            <importance>5</importance>
        </observation>"#;
        assert!(validate_compression_xml(xml));
    }

    #[test]
    fn test_validate_compression_xml_missing_title() {
        let xml = r#"<observation>
            <type>file_read</type>
            <narrative>Read the main.rs file.</narrative>
            <importance>5</importance>
        </observation>"#;
        assert!(!validate_compression_xml(xml));
    }

    #[test]
    fn test_validate_compression_xml_missing_narrative() {
        let xml = r#"<observation>
            <type>file_read</type>
            <title>Read main.rs</title>
            <importance>5</importance>
        </observation>"#;
        assert!(!validate_compression_xml(xml));
    }

    #[test]
    fn test_parse_compression_xml_invalid_type_defaults_to_other() {
        let xml = r#"<observation>
            <type>invalid_foo</type>
            <title>Test</title>
            <narrative>Test narrative here</narrative>
            <importance>5</importance>
        </observation>"#;

        let obs = parse_compression_xml(xml).unwrap();
        assert_eq!(obs.observation_type, crate::types::ObservationType::Other);
    }

    #[test]
    fn test_parse_compression_xml_valid_type() {
        let xml = r#"<observation>
            <type>file_edit</type>
            <title>Edit config</title>
            <narrative>Updated the config file.</narrative>
            <importance>7</importance>
        </observation>"#;

        let obs = parse_compression_xml(xml).unwrap();
        assert_eq!(obs.observation_type, crate::types::ObservationType::FileEdit);
        assert_eq!(obs.importance, 7);
    }

    #[test]
    fn test_parse_compression_xml_with_children() {
        let xml = r#"<observation>
            <type>file_read</type>
            <title>Read files</title>
            <facts><fact>File has 100 lines</fact><fact>Uses Rust</fact></facts>
            <narrative>Read the source files.</narrative>
            <concepts><concept>Rust</concept><concept>async</concept></concepts>
            <files><file>src/main.rs</file><file>src/lib.rs</file></files>
            <importance>5</importance>
        </observation>"#;

        let obs = parse_compression_xml(xml).unwrap();
        assert_eq!(obs.facts.len(), 2);
        assert_eq!(obs.concepts.len(), 2);
        assert_eq!(obs.files.len(), 2);
    }
}
