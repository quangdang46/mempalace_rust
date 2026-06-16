use crate::knowledge_graph::KnowledgeGraph;
use crate::llm::LlmProvider;
use crate::summarize::strip_xml_wrappers;
use crate::types::{CompressedObservation, GraphEdgeType, GraphNodeType};
use anyhow::Result;
use std::collections::HashMap;

const GRAPH_EXTRACTION_SYSTEM: &str = r#"You are a knowledge graph extraction engine. Given a compressed observation from a coding session, extract entities and relationships.

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

pub fn build_graph_extraction_prompt(observations: &[CompressedObservation]) -> String {
    let items: Vec<String> = observations
        .iter()
        .enumerate()
        .map(|(i, o)| {
            format!(
                "[{}] Type: {}\nTitle: {}\nNarrative: {}\nConcepts: {}\nFiles: {}",
                i + 1,
                o.observation_type,
                o.title,
                o.narrative,
                o.concepts.join(", "),
                o.files.join(", ")
            )
        })
        .collect();
    format!(
        "Extract entities and relationships from these observations:\n\n{}",
        items.join("\n\n")
    )
}

fn parse_attrs(raw: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let re = regex::Regex::new(r#"([A-Za-z_][\w:-]*)="([^"]*)""#).unwrap();
    for cap in re.captures_iter(raw) {
        if let (Some(key), Some(val)) = (cap.get(1), cap.get(2)) {
            attrs.insert(key.as_str().to_string(), val.as_str().to_string());
        }
    }
    attrs
}

#[derive(Debug)]
pub struct ExtractedNode {
    pub node_type: String,
    pub name: String,
    pub properties: HashMap<String, String>,
}

#[derive(Debug)]
pub struct ExtractedEdge {
    pub edge_type: String,
    pub source: String,
    pub target: String,
    pub weight: f64,
}

#[derive(Debug)]
pub struct ExtractionResult {
    pub nodes: Vec<ExtractedNode>,
    pub edges: Vec<ExtractedEdge>,
}

fn parse_graph_xml(xml: &str) -> ExtractionResult {
    // Strip markdown code fences / conversational preamble before regex matching
    // (some providers wrap structured XML in ```xml ... ``` blocks).
    let xml = strip_xml_wrappers(xml);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    let entity_self_close = regex::Regex::new(r#"<entity\b([^>]*?)/>"#).unwrap();
    let entity_with_body =
        regex::Regex::new(r#"<entity\b([^>]*[^/])>([\s\S]*?)</entity>"#).unwrap();
    let prop_re = regex::Regex::new(r#"<property\s+key="([^"]+)">([^<]*)</property>"#).unwrap();
    let rel_re = regex::Regex::new(r#"<relationship\b([^>]*?)/>"#).unwrap();

    let mut add_entity = |raw_attrs: &str, props_block: &str| {
        let attrs = parse_attrs(raw_attrs);
        let node_type = attrs.get("type").cloned();
        let name = attrs.get("name").cloned();
        if let (Some(t), Some(n)) = (node_type, name) {
            let mut properties = HashMap::new();
            for cap in prop_re.captures_iter(props_block) {
                if let (Some(k), Some(v)) = (cap.get(1), cap.get(2)) {
                    properties.insert(k.as_str().to_string(), v.as_str().to_string());
                }
            }
            nodes.push(ExtractedNode {
                node_type: t,
                name: n,
                properties,
            });
        }
    };

    for cap in entity_self_close.captures_iter(xml) {
        if let Some(m) = cap.get(1) {
            add_entity(m.as_str(), "");
        }
    }
    for cap in entity_with_body.captures_iter(xml) {
        if let (Some(attrs), Some(body)) = (cap.get(1), cap.get(2)) {
            add_entity(attrs.as_str(), body.as_str());
        }
    }

    for cap in rel_re.captures_iter(xml) {
        if let Some(m) = cap.get(1) {
            let attrs = parse_attrs(m.as_str());
            let edge_type = attrs.get("type").cloned();
            let source = attrs.get("source").cloned();
            let target = attrs.get("target").cloned();
            if let (Some(t), Some(s), Some(tg)) = (edge_type, source, target) {
                let weight = attrs
                    .get("weight")
                    .and_then(|w| w.parse::<f64>().ok())
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0);
                edges.push(ExtractedEdge {
                    edge_type: t,
                    source: s,
                    target: tg,
                    weight,
                });
            }
        }
    }

    ExtractionResult { nodes, edges }
}

fn graph_node_type_from_str(s: &str) -> Option<GraphNodeType> {
    match s {
        "file" => Some(GraphNodeType::File),
        "function" => Some(GraphNodeType::Function),
        "concept" => Some(GraphNodeType::Concept),
        "error" => Some(GraphNodeType::Error),
        "decision" => Some(GraphNodeType::Decision),
        "pattern" => Some(GraphNodeType::Pattern),
        "library" => Some(GraphNodeType::Library),
        "person" => Some(GraphNodeType::Person),
        "project" => Some(GraphNodeType::Project),
        "preference" => Some(GraphNodeType::Preference),
        "location" => Some(GraphNodeType::Location),
        "organization" => Some(GraphNodeType::Organization),
        "event" => Some(GraphNodeType::Event),
        _ => None,
    }
}

fn graph_edge_type_from_str(s: &str) -> Option<GraphEdgeType> {
    match s {
        "uses" => Some(GraphEdgeType::Uses),
        "imports" => Some(GraphEdgeType::Imports),
        "modifies" => Some(GraphEdgeType::Modifies),
        "causes" => Some(GraphEdgeType::Causes),
        "fixes" => Some(GraphEdgeType::Fixes),
        "depends_on" => Some(GraphEdgeType::DependsOn),
        "related_to" => Some(GraphEdgeType::RelatedTo),
        "prefers" => Some(GraphEdgeType::Prefers),
        "blocked_by" => Some(GraphEdgeType::BlockedBy),
        "caused_by" => Some(GraphEdgeType::CausedBy),
        "optimizes_for" => Some(GraphEdgeType::OptimizesFor),
        "rejected" => Some(GraphEdgeType::Rejected),
        "avoids" => Some(GraphEdgeType::Avoids),
        "located_in" => Some(GraphEdgeType::LocatedIn),
        "succeeded_by" => Some(GraphEdgeType::SucceededBy),
        "implements" => Some(GraphEdgeType::Implements),
        _ => None,
    }
}

pub async fn extract_graph(
    kg: &mut KnowledgeGraph,
    llm: &dyn LlmProvider,
    observations: &[CompressedObservation],
) -> Result<ExtractionStats> {
    if observations.is_empty() {
        return Ok(ExtractionStats {
            nodes_added: 0,
            edges_added: 0,
        });
    }

    let prompt = build_graph_extraction_prompt(observations);
    let response = llm.complete(GRAPH_EXTRACTION_SYSTEM, &prompt).await?;
    let extracted = parse_graph_xml(&response.text);

    let obs_ids: Vec<String> = observations.iter().map(|o| o.id.clone()).collect();
    let mut nodes_added = 0;
    let mut edges_added = 0;

    for node in &extracted.nodes {
        if let Some(nt) = graph_node_type_from_str(&node.node_type) {
            let props: HashMap<String, serde_json::Value> = node
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            let eid = kg.add_entity(
                &node.name,
                &format!("{:?}", nt),
                Some(&serde_json::to_value(&props).unwrap_or(serde_json::json!({}))),
            )?;
            nodes_added += 1;

            for prop in &node.properties {
                let _ = kg.add_triple(
                    &node.name, prop.0, prop.1, None, None, None, None, None, None, None,
                );
            }
        }
    }

    for edge in &extracted.edges {
        if let Some(et) = graph_edge_type_from_str(&edge.edge_type) {
            let _ = kg.add_triple(
                &edge.source,
                &format!("{:?}", et),
                &edge.target,
                None,
                None,
                Some(edge.weight),
                None,
                None,
                None,
                None,
            );
            edges_added += 1;
        }
    }

    Ok(ExtractionStats {
        nodes_added,
        edges_added,
    })
}

pub async fn extract_graph_batch(
    kg: &mut KnowledgeGraph,
    llm: &dyn LlmProvider,
    observations: &[CompressedObservation],
    batch_size: usize,
) -> Result<ExtractionStats> {
    let mut total = ExtractionStats {
        nodes_added: 0,
        edges_added: 0,
    };

    for chunk in observations.chunks(batch_size) {
        let stats = extract_graph(kg, llm, chunk).await?;
        total.nodes_added += stats.nodes_added;
        total.edges_added += stats.edges_added;
    }

    Ok(total)
}

#[derive(Debug)]
pub struct ExtractionStats {
    pub nodes_added: usize,
    pub edges_added: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_attrs_basic() {
        let attrs = parse_attrs(r#"type="file" name="src/main.rs""#);
        assert_eq!(attrs.get("type"), Some(&"file".to_string()));
        assert_eq!(attrs.get("name"), Some(&"src/main.rs".to_string()));
    }

    #[test]
    fn test_parse_attrs_empty() {
        let attrs = parse_attrs("");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_graph_xml_entities_and_edges() {
        let xml = r#"<entities>
  <entity type="file" name="src/main.rs"/>
  <entity type="function" name="handle_request">
    <property key="language">rust</property>
  </entity>
</entities>
<relationships>
  <relationship type="uses" source="src/main.rs" target="handle_request" weight="0.8"/>
</relationships>"#;
        let result = parse_graph_xml(xml);
        assert_eq!(result.nodes.len(), 2);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.nodes[0].node_type, "file");
        assert_eq!(result.nodes[0].name, "src/main.rs");
        assert_eq!(
            result.nodes[1].properties.get("language"),
            Some(&"rust".to_string())
        );
        assert_eq!(result.edges[0].source, "src/main.rs");
        assert_eq!(result.edges[0].target, "handle_request");
        assert!((result.edges[0].weight - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_parse_graph_xml_empty() {
        let xml = "<entities></entities><relationships></relationships>";
        let result = parse_graph_xml(xml);
        assert!(result.nodes.is_empty());
        assert!(result.edges.is_empty());
    }

    #[test]
    fn test_parse_graph_xml_invalid_edge_skipped() {
        let xml = r#"<entities>
  <entity type="file" name="a.rs"/>
  <entity type="file" name="b.rs"/>
</entities>
<relationships>
  <relationship type="unknown_type" source="a.rs" target="b.rs" weight="0.5"/>
</relationships>"#;
        let result = parse_graph_xml(xml);
        assert_eq!(result.nodes.len(), 2);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.edges[0].edge_type, "unknown_type");
    }

    #[test]
    fn test_parse_graph_xml_weight_clamped() {
        let xml = r#"<entities>
  <entity type="file" name="a.rs"/>
  <entity type="file" name="b.rs"/>
</entities>
<relationships>
  <relationship type="uses" source="a.rs" target="b.rs" weight="1.5"/>
  <relationship type="imports" source="a.rs" target="b.rs" weight="-0.3"/>
</relationships>"#;
        let result = parse_graph_xml(xml);
        assert_eq!(result.edges.len(), 2);
        assert!((result.edges[0].weight - 1.0).abs() < 0.001);
        assert!((result.edges[1].weight - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_graph_xml_weight_default() {
        let xml = r#"<entities>
  <entity type="file" name="a.rs"/>
  <entity type="file" name="b.rs"/>
</entities>
<relationships>
  <relationship type="uses" source="a.rs" target="b.rs"/>
</relationships>"#;
        let result = parse_graph_xml(xml);
        assert_eq!(result.edges.len(), 1);
        assert!((result.edges[0].weight - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_parse_graph_xml_strips_fenced_xml() {
        // mr-dhbd: some LLM providers (DeepSeek, Qwen) wrap the XML in
        // ```xml ... ``` fences. parse_graph_xml should strip those and parse.
        let xml = "Here is the extraction:\n```xml\n<entities>\n  <entity type=\"file\" name=\"fenced.rs\"/>\n</entities>\n<relationships></relationships>\n```\nDone.";
        let result = parse_graph_xml(xml);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].name, "fenced.rs");
    }

    #[test]
    fn test_build_graph_extraction_prompt() {
        let obs = vec![CompressedObservation {
            id: "o-1".into(),
            session_id: "s-1".into(),
            timestamp: chrono::Utc::now(),
            observation_type: crate::types::ObservationType::FileEdit,
            title: "Edit main.rs".into(),
            subtitle: None,
            facts: vec!["Added auth".into()],
            narrative: "Modified authentication logic".into(),
            concepts: vec!["auth".into()],
            files: vec!["src/main.rs".into()],
            importance: 7,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".into(),
            agent_id: None,
        }];
        let prompt = build_graph_extraction_prompt(&obs);
        assert!(prompt.contains("Edit main.rs"));
        assert!(prompt.contains("src/main.rs"));
        assert!(prompt.contains("Modified authentication logic"));
    }

    #[test]
    fn test_graph_node_type_from_str_all_variants() {
        let variants = [
            "file",
            "function",
            "concept",
            "error",
            "decision",
            "pattern",
            "library",
            "person",
            "project",
            "preference",
            "location",
            "organization",
            "event",
        ];
        for v in variants {
            assert!(graph_node_type_from_str(v).is_some(), "Failed for: {}", v);
        }
        assert!(graph_node_type_from_str("unknown").is_none());
    }

    #[test]
    fn test_graph_edge_type_from_str_all_variants() {
        let variants = [
            "uses",
            "imports",
            "modifies",
            "causes",
            "fixes",
            "depends_on",
            "related_to",
            "prefers",
            "blocked_by",
            "caused_by",
            "optimizes_for",
            "rejected",
            "avoids",
            "located_in",
            "succeeded_by",
            "implements",
        ];
        for v in variants {
            assert!(graph_edge_type_from_str(v).is_some(), "Failed for: {}", v);
        }
        assert!(graph_edge_type_from_str("unknown").is_none());
    }
}
