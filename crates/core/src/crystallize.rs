use crate::llm::LlmProvider;
use crate::types::{Action, ActionStatus, Crystal};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

const CRYSTALLIZE_SYSTEM_PROMPT: &str = r#"You are a crystallization engine that converts completed action sequences into concise knowledge crystals.

Output format (JSON):
{
  "narrative": "Summary of what was accomplished",
  "key_outcomes": ["outcome1", "outcome2"],
  "files_affected": ["path1", "path2"],
  "lessons": ["lesson1", "lesson2"]
}

Rules:
- Keep narrative concise (2-3 sentences)
- List 2-5 key outcomes
- List all affected files
- Extract 1-3 reusable lessons"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrystalJson {
    narrative: String,
    key_outcomes: Vec<String>,
    files_affected: Vec<String>,
    lessons: Vec<String>,
}

pub fn build_crystallize_prompt(actions: &[Action]) -> String {
    let items: Vec<String> = actions
        .iter()
        .filter(|a| a.status == ActionStatus::Completed)
        .map(|a| {
            format!("- {}: {}", a.id, a.description)
        })
        .collect();
    format!(
        "Crystallize these completed actions:\n\n{}",
        items.join("\n")
    )
}

pub fn parse_crystal_json(json: &str) -> Result<CrystalJson> {
    let cleaned = json
        .trim()
        .trim_start_matches("```json")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(cleaned).map_err(|e| anyhow::anyhow!("Failed to parse crystal JSON: {}", e))
}

pub async fn crystallize(
    llm: &dyn LlmProvider,
    actions: &[Action],
    project: Option<&str>,
    session_id: Option<&str>,
) -> Result<Crystal> {
    let completed: Vec<&Action> = actions
        .iter()
        .filter(|a| a.status == ActionStatus::Completed)
        .collect();

    if completed.is_empty() {
        return Err(anyhow::anyhow!("No completed actions to crystallize"));
    }

    let prompt = build_crystallize_prompt(actions);
    let response = llm.complete(CRYSTALLIZE_SYSTEM_PROMPT, &prompt).await?;
    let parsed = parse_crystal_json(&response.text)?;

    let now = Utc::now();
    let action_ids: Vec<String> = completed.iter().map(|a| a.id.clone()).collect();

    Ok(Crystal {
        id: format!("crystal-{}", uuid::Uuid::new_v4().to_string()[..8].to_string()),
        action_ids,
        narrative: parsed.narrative,
        key_outcomes: parsed.key_outcomes,
        files_affected: parsed.files_affected,
        lessons: parsed.lessons,
        session_id: session_id.map(String::from),
        project: project.map(String::from),
        created_at: now,
    })
}

pub fn should_auto_crystallize(actions: &[Action], max_age_hours: i64) -> bool {
    let completed: Vec<&Action> = actions
        .iter()
        .filter(|a| a.status == ActionStatus::Completed)
        .collect();

    if completed.is_empty() {
        return false;
    }

    let now = Utc::now();
    let oldest = completed.iter().map(|a| a.updated_at).min();

    if let Some(oldest_time) = oldest {
        let age = now - oldest_time;
        age.num_hours() >= max_age_hours
    } else {
        false
    }
}

pub fn group_actions_for_crystallization(
    actions: &[Action],
    max_age_hours: i64,
) -> Vec<Vec<Action>> {
    let now = Utc::now();
    let cutoff = now - chrono::Duration::hours(max_age_hours);

    let eligible: Vec<Action> = actions
        .iter()
        .filter(|a| {
            a.status == ActionStatus::Completed && a.updated_at < cutoff
        })
        .cloned()
        .collect();

    if eligible.is_empty() {
        return vec![];
    }

    vec![eligible]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_action(id: &str, status: ActionStatus) -> Action {
        Action {
            id: id.to_string(),
            title: format!("Action {}", id),
            description: format!("Description for {}", id),
            status,
            priority: 2,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            created_by: None,
            assigned_to: None,
            project: "test".to_string(),
            tags: vec![],
            source_observation_ids: vec![],
            source_memory_ids: vec![],
            result: None,
            parent_id: None,
            metadata: std::collections::HashMap::new(),
            sketch_id: None,
            crystallized_into: None,
        }
    }

    #[test]
    fn test_parse_crystal_json() {
        let json = r#"{
            "narrative": "Implemented auth system",
            "key_outcomes": ["JWT working", "Middleware added"],
            "files_affected": ["src/auth.rs"],
            "lessons": ["Always test tokens"]
        }"#;
        let parsed = parse_crystal_json(json).unwrap();
        assert_eq!(parsed.narrative, "Implemented auth system");
        assert_eq!(parsed.key_outcomes.len(), 2);
        assert_eq!(parsed.files_affected.len(), 1);
        assert_eq!(parsed.lessons.len(), 1);
    }

    #[test]
    fn test_parse_crystal_json_with_code_block() {
        let json = r#"```json
{"narrative":"test","key_outcomes":[],"files_affected":[],"lessons":[]}
```"#;
        let parsed = parse_crystal_json(json).unwrap();
        assert_eq!(parsed.narrative, "test");
    }

    #[test]
    fn test_build_crystallize_prompt() {
        let actions = vec![
            test_action("a-1", ActionStatus::Completed),
            test_action("a-2", ActionStatus::Failed),
        ];
        let prompt = build_crystallize_prompt(&actions);
        assert!(prompt.contains("a-1"));
        assert!(!prompt.contains("a-2"));
    }

    #[test]
    fn test_should_auto_crystallize_true() {
        let mut action = test_action("a-1", ActionStatus::Completed);
        action.updated_at = Utc::now() - chrono::Duration::hours(5);
        let actions = vec![action];
        assert!(should_auto_crystallize(&actions, 3));
    }

    #[test]
    fn test_should_auto_crystallize_false_recent() {
        let actions = vec![test_action("a-1", ActionStatus::Completed)];
        assert!(!should_auto_crystallize(&actions, 3));
    }

    #[test]
    fn test_should_auto_crystallize_false_no_completed() {
        let actions = vec![test_action("a-1", ActionStatus::Pending)];
        assert!(!should_auto_crystallize(&actions, 3));
    }

    #[test]
    fn test_group_actions_for_crystallization() {
        let mut a1 = test_action("a-1", ActionStatus::Completed);
        a1.updated_at = Utc::now() - chrono::Duration::hours(5);
        let actions = vec![
            a1,
            test_action("a-2", ActionStatus::Completed),
            test_action("a-3", ActionStatus::Pending),
        ];
        let groups = group_actions_for_crystallization(&actions, 3);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].id, "a-1");
    }

    #[test]
    fn test_group_actions_empty() {
        let groups = group_actions_for_crystallization(&[], 3);
        assert!(groups.is_empty());
    }
}
