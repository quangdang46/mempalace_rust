use crate::types::{
    AuditEntry, FrequencyEntry, ItemVisibility, SharedItemType, TeamConfig, TeamMode, TeamProfile,
    TeamSharedItem,
};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;

pub struct Team {
    config: TeamConfig,
    items: Vec<TeamSharedItem>,
    audit_log: Vec<AuditEntry>,
}

impl Team {
    pub fn new(config: TeamConfig) -> Self {
        Self {
            config,
            items: Vec::new(),
            audit_log: Vec::new(),
        }
    }

    pub fn share(
        &mut self,
        item_id: &str,
        item_type: SharedItemType,
        content: serde_json::Value,
        project: &str,
    ) -> Result<TeamSharedItem> {
        let shared = TeamSharedItem {
            id: format!("ts-{}", uuid::Uuid::new_v4()),
            shared_by: self.config.user_id.clone(),
            shared_at: Utc::now(),
            item_type,
            content,
            project: project.to_string(),
            visibility: ItemVisibility::Shared,
        };

        self.record_audit(
            "share",
            "mem::team-share",
            vec![item_id.to_string()],
            serde_json::json!({
                "teamId": self.config.team_id,
                "userId": self.config.user_id,
                "itemType": format!("{:?}", item_type),
            }),
        );

        self.items.push(shared.clone());
        Ok(shared)
    }

    pub fn feed(&self, limit: usize) -> Vec<&TeamSharedItem> {
        let mut filtered: Vec<_> = self
            .items
            .iter()
            .filter(|i| matches!(i.visibility, ItemVisibility::Shared))
            .collect();
        filtered.sort_by(|a, b| b.shared_at.cmp(&a.shared_at));
        filtered.into_iter().take(limit).collect()
    }

    pub fn profile(&mut self) -> TeamProfile {
        let members: Vec<String> = self
            .items
            .iter()
            .map(|i| i.shared_by.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let mut concept_counts: HashMap<String, usize> = HashMap::new();
        let mut file_counts: HashMap<String, usize> = HashMap::new();
        let mut patterns: Vec<String> = Vec::new();

        for item in &self.items {
            if matches!(item.item_type, SharedItemType::Memory | SharedItemType::Pattern) {
                if let Some(concepts) = item.content.get("concepts").and_then(|c| c.as_array()) {
                    for c in concepts {
                        if let Some(s) = c.as_str() {
                            *concept_counts.entry(s.to_string()).or_insert(0) += 1;
                        }
                    }
                }
                if let Some(files) = item.content.get("files").and_then(|c| c.as_array()) {
                    for f in files {
                        if let Some(s) = f.as_str() {
                            *file_counts.entry(s.to_string()).or_insert(0) += 1;
                        }
                    }
                }
                if matches!(item.item_type, SharedItemType::Pattern) {
                    if let Some(content_str) = item.content.get("content").and_then(|c| c.as_str()) {
                        patterns.push(content_str.chars().take(100).collect());
                    }
                }
            }
        }

        let mut top_concepts: Vec<_> = concept_counts.into_iter().collect();
        top_concepts.sort_by(|a, b| b.1.cmp(&a.1));
        let top_concepts: Vec<FrequencyEntry> = top_concepts
            .into_iter()
            .take(10)
            .map(|(key, frequency)| FrequencyEntry { key, frequency })
            .collect();

        let mut top_files: Vec<_> = file_counts.into_iter().collect();
        top_files.sort_by(|a, b| b.1.cmp(&a.1));
        let top_files: Vec<FrequencyEntry> = top_files
            .into_iter()
            .take(10)
            .map(|(key, frequency)| FrequencyEntry { key, frequency })
            .collect();

        patterns.truncate(10);

        let profile = TeamProfile {
            team_id: self.config.team_id.clone(),
            members,
            top_concepts,
            top_files,
            shared_patterns: patterns,
            total_shared_items: self.items.len(),
            updated_at: Utc::now(),
        };

        self.record_audit(
            "share",
            "mem::team-profile",
            vec!["profile".to_string()],
            serde_json::json!({
                "teamId": self.config.team_id,
                "members": profile.members.len(),
                "totalSharedItems": profile.total_shared_items,
            }),
        );

        profile
    }

    pub fn set_mode(&mut self, mode: TeamMode) {
        self.config.mode = mode;
    }

    pub fn config(&self) -> &TeamConfig {
        &self.config
    }

    pub fn items(&self) -> &[TeamSharedItem] {
        &self.items
    }

    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
    }

    fn record_audit(
        &mut self,
        operation: &str,
        function_id: &str,
        target_ids: Vec<String>,
        details: serde_json::Value,
    ) {
        let entry = AuditEntry {
            id: format!("aud-{}", uuid::Uuid::new_v4()),
            timestamp: Utc::now(),
            operation: operation.to_string(),
            user_id: Some(self.config.user_id.clone()),
            function_id: function_id.to_string(),
            target_ids,
            details: details
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v))
                .collect(),
            quality_score: None,
        };
        self.audit_log.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TeamConfig {
        TeamConfig {
            team_id: "team-1".to_string(),
            user_id: "agent-1".to_string(),
            mode: TeamMode::Cooperative,
        }
    }

    #[test]
    fn test_share_memory_item() {
        let mut team = Team::new(test_config());
        let item = team
            .share(
                "mem-1",
                SharedItemType::Memory,
                serde_json::json!({"concepts": ["auth"], "files": ["src/auth.rs"]}),
                "my-project",
            )
            .unwrap();

        assert_eq!(item.shared_by, "agent-1");
        assert_eq!(item.item_type, SharedItemType::Memory);
        assert_eq!(item.project, "my-project");
        assert!(matches!(item.visibility, ItemVisibility::Shared));
        assert_eq!(team.items().len(), 1);
    }

    #[test]
    fn test_feed_returns_shared_items() {
        let mut team = Team::new(test_config());
        team.share(
            "mem-1",
            SharedItemType::Memory,
            serde_json::json!({}),
            "proj",
        )
        .unwrap();
        team.share(
            "mem-2",
            SharedItemType::Memory,
            serde_json::json!({}),
            "proj",
        )
        .unwrap();

        let feed = team.feed(10);
        assert_eq!(feed.len(), 2);
    }

    #[test]
    fn test_feed_respects_limit() {
        let mut team = Team::new(test_config());
        for i in 0..5 {
            team.share(
                &format!("mem-{}", i),
                SharedItemType::Memory,
                serde_json::json!({}),
                "proj",
            )
            .unwrap();
        }

        let feed = team.feed(2);
        assert_eq!(feed.len(), 2);
    }

    #[test]
    fn test_profile_aggregates_concepts_and_files() {
        let mut team = Team::new(test_config());
        team.share(
            "mem-1",
            SharedItemType::Memory,
            serde_json::json!({"concepts": ["auth", "api"], "files": ["src/auth.rs"]}),
            "proj",
        )
        .unwrap();
        team.share(
            "mem-2",
            SharedItemType::Memory,
            serde_json::json!({"concepts": ["auth", "testing"], "files": ["src/auth.rs", "src/test.rs"]}),
            "proj",
        )
        .unwrap();

        let profile = team.profile();
        assert!(profile.top_concepts.iter().any(|e| e.key == "auth" && e.frequency == 2));
        assert!(profile.top_files.iter().any(|e| e.key == "src/auth.rs" && e.frequency == 2));
        assert_eq!(profile.total_shared_items, 2);
    }

    #[test]
    fn test_profile_collects_patterns() {
        let mut team = Team::new(test_config());
        team.share(
            "pat-1",
            SharedItemType::Pattern,
            serde_json::json!({"content": "Always use Result for error handling in Rust"}),
            "proj",
        )
        .unwrap();

        let profile = team.profile();
        assert_eq!(profile.shared_patterns.len(), 1);
        assert!(profile.shared_patterns[0].contains("Result"));
    }

    #[test]
    fn test_set_mode() {
        let mut team = Team::new(test_config());
        team.set_mode(TeamMode::Competitive);
        assert!(matches!(team.config().mode, TeamMode::Competitive));
    }

    #[test]
    fn test_share_records_audit() {
        let mut team = Team::new(test_config());
        team.share(
            "mem-1",
            SharedItemType::Memory,
            serde_json::json!({}),
            "proj",
        )
        .unwrap();

        assert_eq!(team.audit_log().len(), 1);
        assert_eq!(team.audit_log()[0].operation, "share");
        assert_eq!(team.audit_log()[0].function_id, "mem::team-share");
    }

    #[test]
    fn test_profile_records_audit() {
        let mut team = Team::new(test_config());
        team.profile();

        assert_eq!(team.audit_log().len(), 1);
        assert_eq!(team.audit_log()[0].function_id, "mem::team-profile");
    }
}
