use crate::llm::LlmProvider;
use crate::types::{CompressedObservation, Insight};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const REFLECT_SYSTEM_PROMPT: &str = r#"You are a reflection engine that analyzes coding observations and generates insights.

Output format (XML):
<insights>
  <insight>
    <title>Brief insight title</title>
    <content>Detailed insight description</content>
    <confidence>0.0-1.0</confidence>
    <tags>
      <tag>tag1</tag>
      <tag>tag2</tag>
    </tags>
  </insight>
</insights>

Rules:
- Generate 1-3 insights
- Confidence between 0.0 and 1.0
- Tags should be relevant keywords
- Focus on patterns, best practices, and learnings"#;

pub fn build_reflect_prompt(observations: &[CompressedObservation]) -> String {
    let items: Vec<String> = observations
        .iter()
        .map(|o| {
            format!(
                "- {} ({}): {}\n  Files: {}\n  Concepts: {}",
                o.title,
                o.observation_type,
                o.narrative,
                o.files.join(", "),
                o.concepts.join(", ")
            )
        })
        .collect();
    format!("Analyze these observations and generate insights:\n\n{}", items.join("\n"))
}

pub fn parse_insights_xml(xml: &str) -> Result<Vec<Insight>> {
    let insight_re = regex::Regex::new(r#"<insight>([\s\S]*?)</insight>"#)?;
    let title_re = regex::Regex::new(r#"<title>([^<]+)</title>"#)?;
    let content_re = regex::Regex::new(r#"<content>([\s\S]*?)</content>"#)?;
    let confidence_re = regex::Regex::new(r#"<confidence>([\d.]+)</confidence>"#)?;
    let tag_re = regex::Regex::new(r#"<tag>([^<]+)</tag>"#)?;

    let mut insights = Vec::new();

    for cap in insight_re.captures_iter(xml) {
        let block = cap.get(1).map(|m| m.as_str()).unwrap_or("");

        let title = title_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();

        let content = content_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();

        let confidence = confidence_re
            .captures(block)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().trim().parse::<f64>().ok())
            .unwrap_or(0.5);

        let tags: Vec<String> = tag_re
            .captures_iter(block)
            .filter_map(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .collect();

        let now = Utc::now();
        insights.push(Insight {
            id: format!("insight-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()),
            title,
            content,
            confidence,
            reinforcements: 0,
            source_observation_id: None,
            source_concept_cluster: None,
            source_memory_ids: vec![],
            source_lesson_ids: vec![],
            source_crystal_ids: vec![],
            project: None,
            tags,
            decay_rate: 0.1,
            last_reinforced_at: None,
            last_decayed_at: None,
            updated_at: now,
            deleted: false,
            created_at: now,
        });
    }

    Ok(insights)
}

pub async fn reflect(
    llm: &dyn LlmProvider,
    observations: &[CompressedObservation],
) -> Result<Vec<Insight>> {
    if observations.is_empty() {
        return Ok(vec![]);
    }

    let prompt = build_reflect_prompt(observations);
    let response = llm.complete(REFLECT_SYSTEM_PROMPT, &prompt).await?;
    parse_insights_xml(&response.text)
}

pub fn compute_concept_clusters(
    observations: &[CompressedObservation],
) -> Vec<(String, Vec<String>)> {
    let mut concept_to_obs: HashMap<String, Vec<String>> = HashMap::new();

    for obs in observations {
        for concept in &obs.concepts {
            concept_to_obs
                .entry(concept.clone())
                .or_default()
                .push(obs.id.clone());
        }
    }

    concept_to_obs.into_iter().collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayResult {
    pub insight_id: String,
    pub old_confidence: f64,
    pub new_confidence: f64,
    pub decay_rate: f64,
    pub days_since_reinforced: f64,
}

pub fn apply_decay(insights: &mut [Insight], decay_rate: f64) -> Vec<DecayResult> {
    let now = Utc::now();
    let mut results = Vec::new();

    for insight in insights.iter_mut() {
        if insight.deleted {
            continue;
        }

        let days_since = insight
            .last_reinforced_at
            .map(|t| (now - t).num_days() as f64)
            .unwrap_or(0.0);

        if days_since > 0.0 {
            let old_confidence = insight.confidence;
            let decay = decay_rate * days_since;
            insight.confidence = (insight.confidence - decay).max(0.0);
            insight.last_decayed_at = Some(now);

            results.push(DecayResult {
                insight_id: insight.id.clone(),
                old_confidence,
                new_confidence: insight.confidence,
                decay_rate,
                days_since_reinforced: days_since,
            });
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_obs(id: &str, concepts: Vec<&str>, files: Vec<&str>) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "s-1".to_string(),
            timestamp: Utc::now(),
            observation_type: crate::types::ObservationType::FileEdit,
            title: format!("Edit {}", id),
            subtitle: None,
            facts: vec![],
            narrative: format!("Narrative for {}", id),
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
    fn test_parse_insights_xml() {
        let xml = r#"<insights>
  <insight>
    <title>Auth pattern detected</title>
    <content>JWT auth is consistently modified with middleware</content>
    <confidence>0.85</confidence>
    <tags>
      <tag>auth</tag>
      <tag>jwt</tag>
    </tags>
  </insight>
</insights>"#;
        let insights = parse_insights_xml(xml).unwrap();
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].title, "Auth pattern detected");
        assert!((insights[0].confidence - 0.85).abs() < 0.01);
        assert_eq!(insights[0].tags.len(), 2);
    }

    #[test]
    fn test_parse_insights_multiple() {
        let xml = r#"<insights>
  <insight>
    <title>First insight</title>
    <content>Content 1</content>
    <confidence>0.7</confidence>
    <tags><tag>a</tag></tags>
  </insight>
  <insight>
    <title>Second insight</title>
    <content>Content 2</content>
    <confidence>0.9</confidence>
    <tags><tag>b</tag></tags>
  </insight>
</insights>"#;
        let insights = parse_insights_xml(xml).unwrap();
        assert_eq!(insights.len(), 2);
    }

    #[test]
    fn test_build_reflect_prompt() {
        let obs = vec![test_obs("1", vec!["auth"], vec!["src/auth.rs"])];
        let prompt = build_reflect_prompt(&obs);
        assert!(prompt.contains("Edit 1"));
        assert!(prompt.contains("auth"));
    }

    #[test]
    fn test_compute_concept_clusters() {
        let obs = vec![
            test_obs("1", vec!["auth", "jwt"], vec![]),
            test_obs("2", vec!["auth", "api"], vec![]),
            test_obs("3", vec!["jwt"], vec![]),
        ];
        let clusters = compute_concept_clusters(&obs);
        assert!(clusters.len() >= 2);
    }

    #[test]
    fn test_apply_decay() {
        let now = Utc::now();
        let mut insights = vec![
            Insight {
                id: "i-1".into(),
                title: "Test".into(),
                content: "Content".into(),
                confidence: 0.9,
                reinforcements: 0,
                source_observation_id: None,
                source_concept_cluster: None,
                source_memory_ids: vec![],
                source_lesson_ids: vec![],
                source_crystal_ids: vec![],
                project: None,
                tags: vec![],
                decay_rate: 0.05,
                last_reinforced_at: Some(now - chrono::Duration::days(10)),
                last_decayed_at: None,
                updated_at: now,
                deleted: false,
                created_at: now,
            },
        ];
        let results = apply_decay(&mut insights, 0.05);
        assert_eq!(results.len(), 1);
        assert!(results[0].new_confidence < results[0].old_confidence);
    }

    #[test]
    fn test_apply_decay_skips_deleted() {
        let now = Utc::now();
        let mut insights = vec![Insight {
            id: "i-1".into(),
            title: "Test".into(),
            content: "Content".into(),
            confidence: 0.9,
            reinforcements: 0,
            source_observation_id: None,
            source_concept_cluster: None,
            source_memory_ids: vec![],
            source_lesson_ids: vec![],
            source_crystal_ids: vec![],
            project: None,
            tags: vec![],
            decay_rate: 0.05,
            last_reinforced_at: Some(now - chrono::Duration::days(10)),
            last_decayed_at: None,
            updated_at: now,
            deleted: true,
            created_at: now,
        }];
        let results = apply_decay(&mut insights, 0.05);
        assert!(results.is_empty());
    }

    #[test]
    fn test_apply_decay_no_reinforcement() {
        let mut insights = vec![Insight {
            id: "i-1".into(),
            title: "Test".into(),
            content: "Content".into(),
            confidence: 0.9,
            reinforcements: 0,
            source_observation_id: None,
            source_concept_cluster: None,
            source_memory_ids: vec![],
            source_lesson_ids: vec![],
            source_crystal_ids: vec![],
            project: None,
            tags: vec![],
            decay_rate: 0.05,
            last_reinforced_at: None,
            last_decayed_at: None,
            updated_at: Utc::now(),
            deleted: false,
            created_at: Utc::now(),
        }];
        let results = apply_decay(&mut insights, 0.05);
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_reflect_empty_observations() {
        let insights = reflect(&MockLlm, &[]).await.unwrap();
        assert!(insights.is_empty());
    }

    struct MockLlm;

    #[async_trait::async_trait]
    impl LlmProvider for MockLlm {
        fn name(&self) -> &str { "mock" }
        fn model(&self) -> &str { "mock-model" }
        async fn complete(&self, _system: &str, _prompt: &str) -> Result<crate::llm::provider::LlmCompletion, crate::llm::provider::LlmError> {
            Ok(crate::llm::provider::LlmCompletion {
                text: "<insights></insights>".to_string(),
                model: "mock".to_string(),
                provider: "mock".to_string(),
                usage: None,
            })
        }
        async fn check_available(&self) -> Result<(), String> { Ok(()) }
    }
}
