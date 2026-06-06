/// Consolidation prompt templates.
/// 1:1 port from mempalace `src/prompts/consolidation.ts`.
use crate::types::CompressedObservation;

/// System prompt for semantic memory merging.
pub const SEMANTIC_MERGE_SYSTEM: &str = r#"You are a memory consolidation engine. Given overlapping episodic memories (session summaries), extract stable factual knowledge.

Output format (XML):
<facts>
  <fact confidence="0.0-1.0">Concise factual statement</fact>
</facts>

Rules:
- Extract only facts that appear in 2+ episodes or are highly confident
- Confidence reflects how well-supported the fact is across episodes
- Combine overlapping information into single concise facts
- Skip ephemeral details (specific error messages, temporary states)"#;

/// Build a semantic merge prompt from a list of episodes.
pub fn build_semantic_merge_prompt(episodes: &[CompressedObservation]) -> String {
    let items: Vec<String> = episodes
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let concepts = e.concepts.join(", ");
            format!(
                "[Episode {}]\nTitle: {}\nNarrative: {}\nConcepts: {concepts}",
                i + 1,
                e.title,
                e.narrative
            )
        })
        .collect();

    format!(
        "Consolidate these episodic memories into stable facts:\n\n{}",
        items.join("\n\n")
    )
}

/// System prompt for procedural memory extraction.
pub const PROCEDURAL_EXTRACTION_SYSTEM: &str = r#"You are a procedural memory extractor. Given repeated patterns and workflows observed across sessions, extract reusable procedures.

Output format (XML):
<procedures>
  <procedure name="short descriptive name" trigger="when to use this procedure">
    <step>Step 1 description</step>
    <step>Step 2 description</step>
  </procedure>
</procedures>

Rules:
- Only extract procedures observed 2+ times
- Steps should be concrete and actionable
- Trigger condition should be specific enough to match automatically"#;

/// Build a procedural extraction prompt from recurring patterns.
pub fn build_procedural_extraction_prompt(patterns: &[(String, usize)]) -> String {
    let items: Vec<String> = patterns
        .iter()
        .enumerate()
        .map(|(i, (content, frequency))| {
            format!("[Pattern {}] (seen {frequency}x)\n{content}", i + 1)
        })
        .collect();

    format!(
        "Extract reusable procedures from these recurring patterns:\n\n{}",
        items.join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ObservationType;
    use chrono::Utc;

    fn make_compressed_obs(
        id: &str,
        title: &str,
        narrative: &str,
        concepts: Vec<&str>,
    ) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "sess-1".to_string(),
            timestamp: Utc::now(),
            observation_type: ObservationType::Other,
            title: title.to_string(),
            subtitle: None,
            facts: vec![],
            narrative: narrative.to_string(),
            concepts: concepts.into_iter().map(String::from).collect(),
            files: vec![],
            importance: 5,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        }
    }

    #[test]
    fn test_semantic_merge_system_content() {
        assert!(SEMANTIC_MERGE_SYSTEM.contains("memory consolidation engine"));
        assert!(SEMANTIC_MERGE_SYSTEM.contains("<facts>"));
        assert!(SEMANTIC_MERGE_SYSTEM.contains("confidence"));
    }

    #[test]
    fn test_procedural_extraction_system_content() {
        assert!(PROCEDURAL_EXTRACTION_SYSTEM.contains("procedural memory extractor"));
        assert!(PROCEDURAL_EXTRACTION_SYSTEM.contains("<procedures>"));
        assert!(PROCEDURAL_EXTRACTION_SYSTEM.contains("observed 2+ times"));
    }

    #[test]
    fn test_build_semantic_merge_prompt() {
        let episodes = vec![
            make_compressed_obs(
                "obs-1",
                "Architectural decision",
                "We decided to use SQLite for persistence.",
                vec!["SQLite", "persistence"],
            ),
            make_compressed_obs(
                "obs-2",
                "Database setup",
                "Discovered that SQLite is an embedded database.",
                vec!["SQLite"],
            ),
        ];

        let prompt = build_semantic_merge_prompt(&episodes);
        assert!(prompt.contains("Consolidate these episodic memories"));
        assert!(prompt.contains("[Episode 1]"));
        assert!(prompt.contains("[Episode 2]"));
        assert!(prompt.contains("Architectural decision"));
        assert!(prompt.contains("SQLite, persistence"));
    }

    #[test]
    fn test_build_procedural_extraction_prompt() {
        let patterns = vec![
            ("Run tests before commit".to_string(), 5),
            ("Update docs after API change".to_string(), 3),
        ];

        let prompt = build_procedural_extraction_prompt(&patterns);
        assert!(prompt.contains("Extract reusable procedures"));
        assert!(prompt.contains("[Pattern 1] (seen 5x)"));
        assert!(prompt.contains("[Pattern 2] (seen 3x)"));
    }

    #[test]
    fn test_build_semantic_merge_empty() {
        let prompt = build_semantic_merge_prompt(&[]);
        assert!(prompt.contains("Consolidate these episodic memories"));
    }
}
