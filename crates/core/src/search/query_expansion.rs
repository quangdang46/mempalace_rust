/// LLM-based query expansion for search recall improvement.
///
/// Ported from mempalace's query-expansion.ts:
/// - Generates 3-5 reformulations for diverse recall
/// - Extracts entities (quoted strings, capitalized words excluding stop words)
/// - Generates temporal concretizations for time-related queries
/// - XML output format: <expansion><reformulations><query>...
use crate::llm::LlmProvider;
use crate::prompts;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Expanded query with reformulations, temporal queries, and entities.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryExpansion {
    pub reformulations: Vec<String>,
    pub temporal_concretizations: Vec<String>,
    pub entity_extractions: Vec<String>,
}

/// Stop words to exclude from entity extraction.
const STOP_WORDS: &[&str] = &[
    "The", "This", "That", "What", "When", "Where", "How", "Why", "Who", "Which", "Did", "Does",
    "Do", "Is", "Are", "Was", "Were", "Has", "Have", "Had", "Can", "Could", "Would", "Should",
    "Will", "May", "Might", "If", "And", "But", "Or", "Not", "For", "From", "With", "About",
    "After", "Before", "Between",
];

/// System prompt for query expansion.
const QUERY_EXPANSION_SYSTEM_PROMPT: &str = r#"You are a query expansion engine for a memory retrieval system. Given a user query, generate diverse reformulations to maximize recall.

Output EXACTLY this XML:
<expansion>
  <reformulations>
    <query>semantically diverse rephrasing 1</query>
    <query>semantically diverse rephrasing 2</query>
    <query>semantically diverse rephrasing 3</query>
  </reformulations>
  <temporal>
    <query>time-concretized version if applicable</query>
  </temporal>
  <entities>
    <entity>extracted entity name 1</entity>
    <entity>extracted entity name 2</entity>
  </entities>
</expansion>

Rules:
- Generate 3-5 reformulations capturing different interpretations
- Include paraphrases, domain-specific restatements, and abstract/concrete variants
- Extract any named entities (people, files, projects, libraries, concepts)
- If the query mentions time ("last week", "recently"), generate temporal concretizations
- Each reformulation should capture a distinct facet of intent
- Keep reformulations concise (under 100 chars each)"#;

/// Clamp a value to a range.
fn clamp(value: usize, min: usize, max: usize) -> usize {
    value.max(min).min(max)
}

/// Extract entities from a query string using heuristic rules.
///
/// Extracts:
/// 1. Quoted strings (e.g., "entity name")
/// 2. Capitalized words (excluding stop words)
/// 3. Deduplicates results
pub fn extract_entities_from_query(query: &str) -> Vec<String> {
    let mut entities: Vec<String> = Vec::new();

    // Extract quoted strings
    for cap in regex::Regex::new(r#""([^"]+)""#)
        .unwrap()
        .captures_iter(query)
    {
        if let Some(m) = cap.get(1) {
            entities.push(m.as_str().to_string());
        }
    }

    // Extract capitalized words
    for cap in regex::Regex::new(r"\b[A-Z][a-zA-Z0-9_.-]+\b")
        .unwrap()
        .captures_iter(query)
    {
        let word = cap.get(0).unwrap().as_str();
        if !STOP_WORDS.contains(&word) && !entities.contains(&word.to_string()) {
            entities.push(word.to_string());
        }
    }

    entities
}

/// Parse XML expansion response into a QueryExpansion struct.
pub fn parse_expansion_xml(response: &str) -> Option<QueryExpansion> {
    let mut expansion = QueryExpansion::default();

    // Extract reformulations
    let reformulation_pattern = regex::Regex::new(r"<query>([^<]+)</query>").ok()?;
    let reformulations_section = extract_xml_tag(response, "reformulations")?;
    for cap in reformulation_pattern.captures_iter(&reformulations_section) {
        if let Some(m) = cap.get(1) {
            let q = m.as_str().trim().to_string();
            if !q.is_empty() {
                expansion.reformulations.push(q);
            }
        }
    }

    // Extract temporal queries
    if let Some(temporal_section) = extract_xml_tag(response, "temporal") {
        for cap in reformulation_pattern.captures_iter(&temporal_section) {
            if let Some(m) = cap.get(1) {
                let q = m.as_str().trim().to_string();
                if !q.is_empty() {
                    expansion.temporal_concretizations.push(q);
                }
            }
        }
    }

    // Extract entities
    let entity_pattern = regex::Regex::new(r"<entity>([^<]+)</entity>").ok()?;
    if let Some(entities_section) = extract_xml_tag(response, "entities") {
        for cap in entity_pattern.captures_iter(&entities_section) {
            if let Some(m) = cap.get(1) {
                let e = m.as_str().trim().to_string();
                if !e.is_empty() {
                    expansion.entity_extractions.push(e);
                }
            }
        }
    }

    Some(expansion)
}

/// Extract content between XML tags.
fn extract_xml_tag(content: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = content.find(&open)? + open.len();
    let end = content[start..].find(&close)? + start;
    Some(content[start..end].to_string())
}

/// Synchronous wrapper for expand_query — spins up a temp tokio Runtime
/// when one is not already active, avoiding `async` in the caller.
pub fn expand_query_sync(
    provider: &Arc<dyn crate::llm::LlmProvider>,
    query: &str,
    max_reformulations: Option<usize>,
) -> QueryExpansion {
    let max = max_reformulations;
    let query = query.to_string();
    let p = Arc::clone(provider);
    if tokio::runtime::Handle::try_current().is_ok() {
        // Already inside a runtime — spawn and block.
        let rt = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| rt.block_on(expand_query(&p, &query, max)))
    } else {
        // No runtime active — build a one-shot current-thread runtime.
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(_) => return QueryExpansion::default(),
        };
        rt.block_on(expand_query(&p, &query, max))
    }
}

/// Expand a query using LLM.
///
/// Calls the LLM with the query expansion prompt and parses the XML response.
/// Falls back to empty expansion on failure.
pub async fn expand_query(
    provider: &Arc<dyn LlmProvider>,
    query: &str,
    max_reformulations: Option<usize>,
) -> QueryExpansion {
    let max_reformulations = clamp(max_reformulations.unwrap_or(5), 1, 10);

    let system_prompt = QUERY_EXPANSION_SYSTEM_PROMPT.to_string();
    let user_prompt = format!(
        "Original query: {query}\nGenerate expansion with max {max_reformulations} reformulations."
    );

    match provider.complete(&system_prompt, &user_prompt).await {
        Ok(completion) => {
            if let Some(expansion) = parse_expansion_xml(&completion.text) {
                // Limit reformulations to max
                let mut limited = expansion;
                if limited.reformulations.len() > max_reformulations {
                    limited.reformulations.truncate(max_reformulations);
                }
                return limited;
            }
        }
        Err(_) => {}
    }

    // Fallback: empty expansion
    QueryExpansion::default()
}

/// Build the full set of queries for search (original + reformulations + temporal).
pub fn build_search_queries(original: &str, expansion: &QueryExpansion) -> Vec<String> {
    let mut queries = vec![original.to_string()];
    queries.extend(expansion.reformulations.iter().cloned());
    queries.extend(expansion.temporal_concretizations.iter().cloned());
    queries
}

/// Build the full set of entities for search (extracted + LLM-identified).
pub fn build_search_entities(original: &str, expansion: &QueryExpansion) -> Vec<String> {
    let mut entities = extract_entities_from_query(original);
    for entity in &expansion.entity_extractions {
        if !entities.contains(entity) {
            entities.push(entity.clone());
        }
    }
    entities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_entities_quoted() {
        let entities = extract_entities_from_query("Find the \"ProjectAlpha\" config");
        assert!(entities.contains(&"ProjectAlpha".to_string()));
    }

    #[test]
    fn test_extract_entities_capitalized() {
        let entities = extract_entities_from_query("How does RustTokio work");
        assert!(entities.contains(&"RustTokio".to_string()));
    }

    #[test]
    fn test_extract_entities_excludes_stop_words() {
        let entities = extract_entities_from_query("What is the answer");
        assert!(!entities.contains(&"What".to_string()));
    }

    #[test]
    fn test_extract_entities_deduplicates() {
        let entities = extract_entities_from_query("The RustTokio and RustTokio project");
        let count = entities.iter().filter(|e| *e == "RustTokio").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_parse_expansion_xml() {
        let xml = r#"<expansion>
  <reformulations>
    <query>how to setup rust async</query>
    <query>rust async configuration guide</query>
  </reformulations>
  <temporal>
    <query>rust async setup 2024</query>
  </temporal>
  <entities>
    <entity>RustTokio</entity>
    <entity>async-std</entity>
  </entities>
</expansion>"#;

        let expansion = parse_expansion_xml(xml).unwrap();
        assert_eq!(expansion.reformulations.len(), 2);
        assert_eq!(expansion.reformulations[0], "how to setup rust async");
        assert_eq!(expansion.temporal_concretizations.len(), 1);
        assert_eq!(expansion.entity_extractions.len(), 2);
    }

    #[test]
    fn test_parse_expansion_xml_empty_sections() {
        let xml = r#"<expansion>
  <reformulations>
    <query>rephrased query</query>
  </reformulations>
  <temporal>
  </temporal>
  <entities>
  </entities>
</expansion>"#;

        let expansion = parse_expansion_xml(xml).unwrap();
        assert_eq!(expansion.reformulations.len(), 1);
        assert!(expansion.temporal_concretizations.is_empty());
        assert!(expansion.entity_extractions.is_empty());
    }

    #[test]
    fn test_build_search_queries() {
        let expansion = QueryExpansion {
            reformulations: vec!["alt1".to_string(), "alt2".to_string()],
            temporal_concretizations: vec!["time1".to_string()],
            entity_extractions: vec![],
        };
        let queries = build_search_queries("original", &expansion);
        assert_eq!(queries.len(), 4);
        assert_eq!(queries[0], "original");
    }

    #[test]
    fn test_build_search_entities() {
        let expansion = QueryExpansion {
            reformulations: vec![],
            temporal_concretizations: vec![],
            entity_extractions: vec!["LLM".to_string()],
        };
        let entities = build_search_entities("Find RustTokio config", &expansion);
        assert!(entities.contains(&"RustTokio".to_string()));
        assert!(entities.contains(&"LLM".to_string()));
    }

    #[test]
    fn test_clamp() {
        assert_eq!(clamp(5, 1, 10), 5);
        assert_eq!(clamp(0, 1, 10), 1);
        assert_eq!(clamp(15, 1, 10), 10);
    }
}
