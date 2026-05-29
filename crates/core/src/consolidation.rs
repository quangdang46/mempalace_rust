/// Consolidation engine — groups observations by concept and synthesizes
/// them into long-term memories via LLM.
/// 1:1 port from agentmemory `src/functions/consolidate.ts`.

use std::collections::HashMap;

use crate::llm::LlmProvider;
use crate::prompts::xml::{get_xml_children, get_xml_tag};
use crate::types::{CompressedObservation, Memory, MemoryType};

const CONSOLIDATION_SYSTEM: &str = r#"You are a memory consolidation engine. Given a set of related observations from coding sessions, synthesize them into a single long-term memory.

Output XML:
<memory>
  <type>pattern|preference|architecture|bug|workflow|fact</type>
  <title>Concise memory title (max 80 chars)</title>
  <content>2-4 sentence description of the learned insight</content>
  <concepts>
    <concept>key term</concept>
  </concepts>
  <files>
    <file>relevant/file/path</file>
  </files>
  <strength>1-10 how confident/important this memory is</strength>
</memory>"#;

const VALID_TYPES: &[&str] = &["pattern", "preference", "architecture", "bug", "workflow", "fact"];

const MAX_LLM_CALLS: usize = 10;
const MIN_OBSERVATIONS: usize = 10;
const MIN_GROUP_SIZE: usize = 3;
const MAX_OBS_PER_GROUP: usize = 8;

/// Result of a consolidation run.
#[derive(Debug)]
pub struct ConsolidationResult {
    pub consolidated: usize,
    pub total_observations: usize,
    pub llm_calls: usize,
}

/// Parse memory XML response into a Memory struct (without id/timestamps).
fn parse_memory_xml(xml: &str, session_ids: Vec<String>) -> Option<Memory> {
    let type_str = get_xml_tag(xml, "type");
    let title = get_xml_tag(xml, "title");
    let content = get_xml_tag(xml, "content");

    if type_str.is_empty() || title.is_empty() || content.is_empty() {
        return None;
    }

    let memory_type = if VALID_TYPES.contains(&type_str.as_str()) {
        match type_str.as_str() {
            "pattern" => MemoryType::Procedural,
            "preference" => MemoryType::Semantic,
            "architecture" => MemoryType::Semantic,
            "bug" => MemoryType::Semantic,
            "workflow" => MemoryType::Procedural,
            "fact" => MemoryType::Semantic,
            _ => MemoryType::Semantic,
        }
    } else {
        MemoryType::Semantic
    };

    let strength_str = get_xml_tag(xml, "strength");
    let strength: f64 = strength_str
        .parse::<u8>()
        .ok()
        .map(|v| (v.clamp(1, 10) as f64) / 10.0)
        .unwrap_or(0.5);

    let concepts = get_xml_children(xml, "concepts", "concept");
    let files = get_xml_children(xml, "files", "file");

    Some(Memory {
        id: String::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        memory_type,
        title,
        content,
        concepts,
        files,
        session_ids,
        strength,
        version: 1,
        parent_id: None,
        supersedes: vec![],
        related_ids: vec![],
        source_observation_ids: vec![],
        is_latest: true,
        forget_after: None,
        image_ref: None,
        agent_id: None,
        project: String::new(),
    })
}

/// Build the consolidation prompt for a concept group.
fn build_consolidation_prompt(concept: &str, observations: &[CompressedObservation]) -> String {
    let items: Vec<String> = observations
        .iter()
        .map(|o| {
            format!(
                "[{}] {}\n{}\nFiles: {}\nImportance: {}",
                o.observation_type,
                o.title,
                o.narrative,
                o.files.join(", "),
                o.importance
            )
        })
        .collect();

    format!(
        "Concept: \"{concept}\"\n\nObservations:\n{}",
        items.join("\n\n")
    )
}

/// Run consolidation on a set of compressed observations.
/// Groups by concept, calls LLM for groups with >= 3 observations.
pub async fn consolidate(
    provider: &dyn LlmProvider,
    observations: &[CompressedObservation],
    existing_memories: &[Memory],
) -> ConsolidationResult {
    // Filter: has title AND importance >= 5
    let filtered: Vec<_> = observations
        .iter()
        .filter(|o| !o.title.is_empty() && o.importance >= 5)
        .cloned()
        .collect();

    if filtered.len() < MIN_OBSERVATIONS {
        return ConsolidationResult {
            consolidated: 0,
            total_observations: filtered.len(),
            llm_calls: 0,
        };
    }

    // Build concept groups
    let mut concept_groups: HashMap<String, Vec<CompressedObservation>> = HashMap::new();
    for obs in &filtered {
        for concept in &obs.concepts {
            let key = concept.to_lowercase();
            concept_groups
                .entry(key)
                .or_default()
                .push(obs.clone());
        }
    }

    // Sort by count desc, filter >= MIN_GROUP_SIZE
    let mut sorted_groups: Vec<_> = concept_groups
        .into_iter()
        .filter(|(_, g)| g.len() >= MIN_GROUP_SIZE)
        .collect();
    sorted_groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    // Track existing titles for evolution detection
    let existing_titles: HashMap<String, &Memory> = existing_memories
        .iter()
        .map(|m| (m.title.to_lowercase(), m))
        .collect();

    let mut consolidated = 0;
    let mut llm_calls = 0;

    for (concept, obs_group) in sorted_groups {
        if llm_calls >= MAX_LLM_CALLS {
            break;
        }

        // Take top observations by importance
        let mut sorted = obs_group;
        sorted.sort_by(|a, b| b.importance.cmp(&a.importance));
        let top: Vec<_> = sorted.into_iter().take(MAX_OBS_PER_GROUP).collect();

        let session_ids: Vec<String> = top.iter().map(|o| o.session_id.clone()).collect();
        let obs_ids: Vec<String> = top.iter().map(|o| o.id.clone()).collect();

        let prompt = build_consolidation_prompt(&concept, &top);

        match provider.complete(CONSOLIDATION_SYSTEM, &prompt).await {
            Ok(completion) => {
                llm_calls += 1;
                if let Some(mut memory) = parse_memory_xml(&completion.text, session_ids) {
                    memory.source_observation_ids = obs_ids;

                    // Check for existing memory with same title
                    if let Some(existing) = existing_titles.get(&memory.title.to_lowercase()) {
                        // Evolve: mark old as not latest, create new version
                        memory.version = existing.version + 1;
                        memory.parent_id = Some(existing.id.clone());
                        memory.supersedes = vec![existing.id.clone()];
                        memory.supersedes.extend(existing.supersedes.clone());
                    }

                    consolidated += 1;
                }
            }
            Err(e) => {
                eprintln!("Consolidation failed for concept '{concept}': {e}");
            }
        }
    }

    ConsolidationResult {
        consolidated,
        total_observations: filtered.len(),
        llm_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::noop_provider::NoopProvider;
    use crate::types::ObservationType;
    use chrono::Utc;

    fn make_obs(id: &str, title: &str, importance: u8, concepts: Vec<&str>) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "sess-1".to_string(),
            timestamp: Utc::now(),
            observation_type: ObservationType::FileRead,
            title: title.to_string(),
            subtitle: None,
            facts: vec![],
            narrative: format!("Narrative for {title}"),
            concepts: concepts.into_iter().map(String::from).collect(),
            files: vec![],
            importance,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        }
    }

    #[tokio::test]
    async fn test_consolidate_insufficient_observations() {
        let provider = NoopProvider::default();
        let observations = vec![make_obs("1", "Test", 5, vec!["rust"])];
        let result = consolidate(&provider, &observations, &[]).await;
        assert_eq!(result.consolidated, 0);
        assert_eq!(result.total_observations, 1);
        assert_eq!(result.llm_calls, 0);
    }

    #[tokio::test]
    async fn test_consolidate_with_noop_provider() {
        let provider = NoopProvider::default();
        let observations: Vec<_> = (0..12)
            .map(|i| make_obs(&format!("obs-{i}"), &format!("Test {i}"), 5, vec!["rust", "async"]))
            .collect();
        let result = consolidate(&provider, &observations, &[]).await;
        // NoopProvider returns empty, so XML parse fails -> 0 consolidated
        assert_eq!(result.consolidated, 0);
        assert_eq!(result.total_observations, 12);
    }

    #[test]
    fn test_parse_memory_xml_valid() {
        let xml = r#"<memory>
            <type>architecture</type>
            <title>SQLite for persistence</title>
            <content>The project uses SQLite for embedded persistence.</content>
            <concepts><concept>SQLite</concept><concept>persistence</concept></concepts>
            <files><file>src/db.rs</file></files>
            <strength>8</strength>
        </memory>"#;

        let memory = parse_memory_xml(xml, vec!["sess-1".to_string()]).unwrap();
        assert_eq!(memory.title, "SQLite for persistence");
        assert_eq!(memory.concepts, vec!["SQLite", "persistence"]);
        assert_eq!(memory.files, vec!["src/db.rs"]);
        assert_eq!(memory.strength, 0.8);
        assert!(memory.is_latest);
        assert_eq!(memory.version, 1);
    }

    #[test]
    fn test_parse_memory_xml_invalid_type_defaults_to_semantic() {
        let xml = r#"<memory>
            <type>invalid_type</type>
            <title>Test</title>
            <content>Content</content>
            <strength>5</strength>
        </memory>"#;

        let memory = parse_memory_xml(xml, vec![]).unwrap();
        assert_eq!(memory.memory_type, MemoryType::Semantic);
    }

    #[test]
    fn test_parse_memory_xml_missing_required() {
        let xml = r#"<memory>
            <type>fact</type>
            <title>Test</title>
        </memory>"#;

        assert!(parse_memory_xml(xml, vec![]).is_none());
    }

    #[test]
    fn test_parse_memory_xml_strength_clamped() {
        let xml = r#"<memory>
            <type>fact</type>
            <title>Test</title>
            <content>Content</content>
            <strength>99</strength>
        </memory>"#;

        let memory = parse_memory_xml(xml, vec![]).unwrap();
        assert_eq!(memory.strength, 1.0);
    }

    #[test]
    fn test_build_consolidation_prompt() {
        let obs = vec![
            make_obs("1", "Test 1", 7, vec!["rust"]),
            make_obs("2", "Test 2", 5, vec!["rust"]),
        ];
        let prompt = build_consolidation_prompt("rust", &obs);
        assert!(prompt.contains("Concept: \"rust\""));
        assert!(prompt.contains("Test 1"));
        assert!(prompt.contains("Test 2"));
    }
}
