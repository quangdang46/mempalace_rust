/// Graph extraction prompt templates.
/// 1:1 port from mempalace `src/prompts/graph-extraction.ts`.
use crate::types::CompressedObservation;

/// System prompt for knowledge graph entity and relationship extraction.
pub const GRAPH_EXTRACTION_SYSTEM: &str = r#"You are a knowledge graph extraction engine. Given a compressed observation from a coding session, extract entities and relationships.

Output format (XML):
<entities>
  <entity type="file|function|concept|error|decision|pattern|library|person" name="exact name">
    <property key="key">value</property>
  </entity>
</entities>
<relationships>
  <relationship type="uses|imports|modifies|causes|fixes|depends_on|related_to" source="entity name" target="entity name" weight="0.1-1.0"/>
</relationships>

Rules:
- Extract concrete entities only (real file paths, function names, library names)
- Use the most specific type available
- Weight relationships by how strong/direct the connection is
- If no entities found, output empty tags"#;

/// Build a graph extraction prompt from a list of observations.
pub fn build_graph_extraction_prompt(observations: &[CompressedObservation]) -> String {
    let items: Vec<String> = observations
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let concepts = o.concepts.join(", ");
            let files = o.files.join(", ");
            format!(
                "[{}] Type: {}\nTitle: {}\nNarrative: {}\nConcepts: {concepts}\nFiles: {files}",
                i + 1,
                o.observation_type,
                o.title,
                o.narrative
            )
        })
        .collect();

    format!(
        "Extract entities and relationships from these observations:\n\n{}",
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
        obs_type: ObservationType,
        title: &str,
        narrative: &str,
        concepts: Vec<&str>,
        files: Vec<&str>,
    ) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "sess-1".to_string(),
            timestamp: Utc::now(),
            observation_type: obs_type,
            title: title.to_string(),
            subtitle: None,
            facts: vec![],
            narrative: narrative.to_string(),
            concepts: concepts.into_iter().map(String::from).collect(),
            files: files.into_iter().map(String::from).collect(),
            importance: 5,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        }
    }

    #[test]
    fn test_graph_extraction_system_content() {
        assert!(GRAPH_EXTRACTION_SYSTEM.contains("knowledge graph extraction engine"));
        assert!(GRAPH_EXTRACTION_SYSTEM.contains("<entities>"));
        assert!(GRAPH_EXTRACTION_SYSTEM.contains("<relationships>"));
        assert!(GRAPH_EXTRACTION_SYSTEM.contains("weight="));
    }

    #[test]
    fn test_build_graph_extraction_prompt() {
        let observations = vec![make_compressed_obs(
            "obs-1",
            ObservationType::FileWrite,
            "Updated auth module",
            "Updated the auth module to add JWT token validation.",
            vec!["JWT", "auth"],
            vec!["src/auth.rs"],
        )];

        let prompt = build_graph_extraction_prompt(&observations);
        assert!(prompt.contains("Extract entities and relationships"));
        assert!(prompt.contains("[1] Type: file_write"));
        assert!(prompt.contains("Title: Updated auth module"));
        assert!(prompt.contains("Concepts: JWT, auth"));
        assert!(prompt.contains("Files: src/auth.rs"));
    }

    #[test]
    fn test_build_graph_extraction_prompt_multiple() {
        let observations = vec![
            make_compressed_obs(
                "obs-1",
                ObservationType::FileRead,
                "Read config",
                "Read the config file.",
                vec!["config"],
                vec!["config.yaml"],
            ),
            make_compressed_obs(
                "obs-2",
                ObservationType::CommandExec,
                "Ran tests",
                "Ran the test suite.",
                vec!["testing"],
                vec![],
            ),
        ];

        let prompt = build_graph_extraction_prompt(&observations);
        assert!(prompt.contains("[1] Type: file_read"));
        assert!(prompt.contains("[2] Type: command_exec"));
    }

    #[test]
    fn test_build_graph_extraction_empty() {
        let prompt = build_graph_extraction_prompt(&[]);
        assert!(prompt.contains("Extract entities and relationships"));
    }
}
