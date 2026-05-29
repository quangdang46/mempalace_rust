//! Skill extraction — port of upstream `skill-extract.ts`.
//!
//! Extracts reusable skills from completed action sequences via LLM.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Extracted skill from action sequences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<String>,
    pub triggers: Vec<String>,
    pub source_action_ids: Vec<String>,
    pub confidence: f64,
}

/// Build the LLM prompt for skill extraction.
pub fn build_skill_extraction_prompt(
    actions: &[crate::types::Action],
    crystals: &[crate::types::Crystal],
) -> String {
    let action_descriptions: Vec<_> = actions.iter()
        .map(|a| format!("- {} ({}): {}", a.title, a.status, a.description))
        .collect();

    let crystal_narratives: Vec<_> = crystals.iter()
        .map(|c| format!("- {}", c.narrative))
        .collect();

    format!(
        "Analyze these completed actions and crystals to extract reusable skills.\n\n\
         Actions:\n{}\n\n\
         Crystals:\n{}\n\n\
         For each skill, provide:\n\
         1. A concise name\n\
         2. A description of what it does\n\
         3. The steps involved\n\
         4. When to trigger it\n\n\
         Output as JSON array with fields: name, description, steps, triggers, confidence.",
        action_descriptions.join("\n"),
        crystal_narratives.join("\n")
    )
}

/// Parse LLM response into extracted skills.
pub fn parse_skill_extraction(response: &str, source_action_ids: Vec<String>) -> Result<Vec<ExtractedSkill>> {
    // Try to parse as JSON array
    if let Ok(skills) = serde_json::from_str::<Vec<serde_json::Value>>(response) {
        let mut extracted = Vec::new();
        for (i, skill) in skills.iter().enumerate() {
            let name = skill.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unnamed Skill")
                .to_string();

            let description = skill.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let steps = skill.get("steps")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let triggers = skill.get("triggers")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let confidence = skill.get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.5);

            extracted.push(ExtractedSkill {
                id: format!("skill-{}", i),
                name,
                description,
                steps,
                triggers,
                source_action_ids: source_action_ids.clone(),
                confidence,
            });
        }
        return Ok(extracted);
    }

    // Fallback: parse from text
    Ok(vec![ExtractedSkill {
        id: "skill-0".to_string(),
        name: "Extracted Skill".to_string(),
        description: response.chars().take(200).collect(),
        steps: vec![],
        triggers: vec![],
        source_action_ids,
        confidence: 0.3,
    }])
}

/// Extract skills from completed actions and crystals.
pub async fn extract_skills(
    llm: &dyn crate::llm::LlmProvider,
    actions: &[crate::types::Action],
    crystals: &[crate::types::Crystal],
) -> Result<Vec<ExtractedSkill>> {
    let completed_actions: Vec<_> = actions.iter()
        .filter(|a| matches!(a.status, crate::types::ActionStatus::Completed))
        .cloned()
        .collect();

    if completed_actions.is_empty() {
        return Ok(Vec::new());
    }

    let action_ids: Vec<_> = completed_actions.iter().map(|a| a.id.clone()).collect();
    let prompt = build_skill_extraction_prompt(&completed_actions, crystals);

    let response = llm.complete(
        "You are a skill extraction engine. Analyze completed work and extract reusable skills. Output JSON.",
        &prompt
    ).await?;

    parse_skill_extraction(&response.text, action_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_skill_extraction_prompt() {
        use crate::types::{Action, ActionStatus};
        let actions = vec![Action {
            id: "a-1".into(), title: "Fix auth bug".into(),
            description: "Token expiry issue".into(),
            status: ActionStatus::Completed, priority: 1,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            created_by: None, assigned_to: None, project: "test".into(),
            tags: vec![], source_observation_ids: vec![], source_memory_ids: vec![],
            result: Some("Fixed token expiry".to_string()), parent_id: None,
            metadata: std::collections::HashMap::new(),
            sketch_id: None, crystallized_into: None,
        }];
        let crystals = vec![];
        let prompt = build_skill_extraction_prompt(&actions, &crystals);
        assert!(prompt.contains("Fix auth bug"));
        assert!(prompt.contains("Token expiry issue"));
    }

    #[test]
    fn test_parse_skill_extraction_json() {
        let response = r#"[{"name":"Auth Debugging","description":"Systematic approach to auth bugs","steps":["Check token","Verify middleware","Test endpoint"],"triggers":["auth error","401 response"],"confidence":0.8}]"#;
        let skills = parse_skill_extraction(response, vec!["a-1".to_string()]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "Auth Debugging");
        assert_eq!(skills[0].steps.len(), 3);
        assert_eq!(skills[0].confidence, 0.8);
    }

    #[test]
    fn test_parse_skill_extraction_fallback() {
        let response = "This is not valid JSON";
        let skills = parse_skill_extraction(response, vec!["a-1".to_string()]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "Extracted Skill");
    }

    #[test]
    fn test_parse_empty_json() {
        let skills = parse_skill_extraction("[]", vec![]).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn test_extracted_skill_serialization() {
        let skill = ExtractedSkill {
            id: "skill-1".to_string(),
            name: "Test Skill".to_string(),
            description: "A test skill".to_string(),
            steps: vec!["step1".to_string()],
            triggers: vec!["trigger1".to_string()],
            source_action_ids: vec!["a-1".to_string()],
            confidence: 0.7,
        };
        let json = serde_json::to_string(&skill).unwrap();
        let parsed: ExtractedSkill = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "Test Skill");
    }

    #[test]
    fn test_build_prompt_with_crystals() {
        use crate::types::{Action, ActionStatus, Crystal};
        let actions = vec![Action {
            id: "a-1".into(), title: "Setup CI".into(),
            description: "Added GitHub Actions".into(),
            status: ActionStatus::Completed, priority: 1,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            created_by: None, assigned_to: None, project: "test".into(),
            tags: vec![], source_observation_ids: vec![], source_memory_ids: vec![],
            result: None, parent_id: None,
            metadata: std::collections::HashMap::new(),
            sketch_id: None, crystallized_into: None,
        }];
        let crystals = vec![Crystal {
            id: "c-1".into(), action_ids: vec!["a-1".into()],
            narrative: "CI pipeline established".into(),
            key_outcomes: vec!["Automated testing".into()],
            files_affected: vec![".github/workflows/ci.yml".into()],
            lessons: vec!["Use matrix builds".into()],
            session_id: Some("s-1".into()), project: Some("test".into()),
            created_at: chrono::Utc::now(),
        }];
        let prompt = build_skill_extraction_prompt(&actions, &crystals);
        assert!(prompt.contains("CI pipeline established"));
    }
}
