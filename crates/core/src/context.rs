use crate::types::{
    CompressedObservation, ContextBlock, FrequencyEntry, MemorySlot, ProjectProfile, Session,
};
use anyhow::Result;

pub const DEFAULT_TOKEN_BUDGET: usize = 8000;

pub struct ContextBuilder {
    token_budget: usize,
    pinned_slots: Vec<MemorySlot>,
    project_profile: Option<ProjectProfile>,
    lessons: Vec<String>,
    session_summaries: Vec<Session>,
    working_memory: Vec<CompressedObservation>,
}

impl ContextBuilder {
    pub fn new(token_budget: usize) -> Self {
        Self {
            token_budget,
            pinned_slots: Vec::new(),
            project_profile: None,
            lessons: Vec::new(),
            session_summaries: Vec::new(),
            working_memory: Vec::new(),
        }
    }

    pub fn with_pinned_slots(mut self, slots: Vec<MemorySlot>) -> Self {
        self.pinned_slots = slots;
        self
    }

    pub fn with_project_profile(mut self, profile: ProjectProfile) -> Self {
        self.project_profile = Some(profile);
        self
    }

    pub fn with_lessons(mut self, lessons: Vec<String>) -> Self {
        self.lessons = lessons;
        self
    }

    pub fn with_session_summaries(mut self, summaries: Vec<Session>) -> Self {
        self.session_summaries = summaries;
        self
    }

    pub fn with_working_memory(mut self, observations: Vec<CompressedObservation>) -> Self {
        self.working_memory = observations;
        self
    }

    pub fn build(&self) -> Result<Vec<ContextBlock>> {
        let mut blocks = Vec::new();
        let mut used_tokens = 0;

        // 1. Pinned slots (highest priority)
        for slot in &self.pinned_slots {
            let token_cost = slot.token_count;
            if used_tokens + token_cost <= self.token_budget {
                blocks.push(ContextBlock {
                    content: slot.content.clone(),
                    source: format!("slot:{}", slot.name),
                    relevance_score: 1.0,
                    token_count: token_cost,
                    memory_id: Some(slot.id.clone()),
                });
                used_tokens += token_cost;
            }
        }

        // 2. Project profile (top concepts and files)
        if let Some(profile) = &self.project_profile {
            let concepts: Vec<&str> = profile
                .top_concepts
                .iter()
                .map(|e| e.key.as_str())
                .collect();
            let files: Vec<&str> = profile.top_files.iter().map(|e| e.key.as_str()).collect();
            let profile_content = format!(
                "Project: {}\nTop Concepts: {}\nTop Files: {}\nLanguage: {}\nFramework: {}",
                profile.project,
                concepts.join(", "),
                files.join(", "),
                profile.language.as_deref().unwrap_or("unknown"),
                profile.framework.as_deref().unwrap_or("unknown"),
            );
            let token_cost = profile_content.len() / 3;
            if used_tokens + token_cost <= self.token_budget {
                blocks.push(ContextBlock {
                    content: profile_content,
                    source: "project_profile".to_string(),
                    relevance_score: 0.9,
                    token_count: token_cost,
                    memory_id: None,
                });
                used_tokens += token_cost;
            }
        }

        // 3. Lessons
        for lesson in &self.lessons {
            let token_cost = lesson.len() / 3;
            if used_tokens + token_cost <= self.token_budget {
                blocks.push(ContextBlock {
                    content: lesson.clone(),
                    source: "lesson".to_string(),
                    relevance_score: 0.8,
                    token_count: token_cost,
                    memory_id: None,
                });
                used_tokens += token_cost;
            }
        }

        // 4. Session summaries (last 10)
        for session in self.session_summaries.iter().take(10) {
            if let Some(summary) = &session.summary {
                let token_cost = summary.len() / 3;
                if used_tokens + token_cost <= self.token_budget {
                    blocks.push(ContextBlock {
                        content: summary.clone(),
                        source: format!("session:{}", session.id),
                        relevance_score: 0.7,
                        token_count: token_cost,
                        memory_id: Some(session.id.clone()),
                    });
                    used_tokens += token_cost;
                }
            }
        }

        // 5. Working memory (greedy fill)
        for obs in &self.working_memory {
            let content = format!("{}\n{}", obs.title, obs.narrative);
            let token_cost = content.len() / 3;
            if used_tokens + token_cost <= self.token_budget {
                blocks.push(ContextBlock {
                    content,
                    source: format!("observation:{}", obs.id),
                    relevance_score: obs.confidence,
                    token_count: token_cost,
                    memory_id: Some(obs.id.clone()),
                });
                used_tokens += token_cost;
            }
        }

        Ok(blocks)
    }

    pub fn build_xml(&self) -> Result<String> {
        let blocks = self.build()?;
        let mut xml = String::from("<mempalace_context>\n");

        for block in &blocks {
            xml.push_str(&format!(
                "  <block source=\"{}\" relevance=\"{:.2}\" tokens=\"{}\">\n{}\n  </block>\n",
                block.source, block.relevance_score, block.token_count, block.content
            ));
        }

        xml.push_str("</mempalace_context>");
        Ok(xml)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_slot(id: &str, name: &str, content: &str) -> MemorySlot {
        MemorySlot {
            id: id.to_string(),
            name: name.to_string(),
            content: content.to_string(),
            token_count: content.len() / 3,
            priority: 1,
            last_updated: Utc::now(),
        }
    }

    fn test_profile() -> ProjectProfile {
        ProjectProfile {
            project: "test-project".to_string(),
            top_concepts: vec![
                FrequencyEntry {
                    key: "auth".to_string(),
                    frequency: 2,
                },
                FrequencyEntry {
                    key: "api".to_string(),
                    frequency: 1,
                },
            ],
            top_files: vec![FrequencyEntry {
                key: "src/main.rs".to_string(),
                frequency: 1,
            }],
            top_patterns: vec![],
            conventions: vec![],
            common_errors: vec![],
            recent_activity: vec![],
            session_count: 5,
            total_observations: 20,
            language: Some("rust".to_string()),
            framework: Some("actix".to_string()),
            updated_at: Utc::now(),
        }
    }

    fn test_session(id: &str) -> Session {
        Session {
            id: id.to_string(),
            project: "test".to_string(),
            cwd: "/tmp".to_string(),
            started_at: Utc::now(),
            ended_at: None,
            status: "active".to_string(),
            observation_count: 5,
            model: None,
            tags: vec![],
            first_prompt: None,
            summary: Some(format!("Session {} summary", id)),
            commit_shas: vec![],
            agent_id: None,
        }
    }

    fn test_observation(id: &str) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "s-1".to_string(),
            timestamp: Utc::now(),
            observation_type: crate::types::ObservationType::FileEdit,
            title: format!("Edit {}", id),
            subtitle: None,
            facts: vec![],
            narrative: "Test narrative".to_string(),
            concepts: vec![],
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
    fn test_build_with_pinned_slots() {
        let builder = ContextBuilder::new(1000).with_pinned_slots(vec![test_slot(
            "s-1",
            "instructions",
            "Always use Rust",
        )]);
        let blocks = builder.build().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source, "slot:instructions");
    }

    #[test]
    fn test_build_with_project_profile() {
        let builder = ContextBuilder::new(1000).with_project_profile(test_profile());
        let blocks = builder.build().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source, "project_profile");
        assert!(blocks[0].content.contains("test-project"));
    }

    #[test]
    fn test_build_respects_token_budget() {
        let builder = ContextBuilder::new(50).with_pinned_slots(vec![test_slot(
            "s-1",
            "big-slot",
            &"x".repeat(200),
        )]);
        let blocks = builder.build().unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_build_xml_format() {
        let builder =
            ContextBuilder::new(1000).with_pinned_slots(vec![test_slot("s-1", "test", "content")]);
        let xml = builder.build_xml().unwrap();
        assert!(xml.starts_with("<mempalace_context>"));
        assert!(xml.ends_with("</mempalace_context>"));
        assert!(xml.contains("content"));
    }

    #[test]
    fn test_build_multiple_blocks_priority_order() {
        let builder = ContextBuilder::new(1000)
            .with_pinned_slots(vec![test_slot("s-1", "pin", "pinned content")])
            .with_project_profile(test_profile())
            .with_lessons(vec!["Lesson 1".to_string()])
            .with_session_summaries(vec![test_session("s-1")])
            .with_working_memory(vec![test_observation("o-1")]);

        let blocks = builder.build().unwrap();
        assert!(blocks.len() >= 4);
        assert_eq!(blocks[0].source, "slot:pin");
        assert_eq!(blocks[1].source, "project_profile");
    }

    #[test]
    fn test_build_with_lessons() {
        let builder = ContextBuilder::new(1000).with_lessons(vec!["Always test auth".to_string()]);
        let blocks = builder.build().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "Always test auth");
    }

    #[test]
    fn test_build_with_session_summaries() {
        let builder = ContextBuilder::new(1000)
            .with_session_summaries(vec![test_session("s-1"), test_session("s-2")]);
        let blocks = builder.build().unwrap();
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_build_with_working_memory() {
        let builder = ContextBuilder::new(1000)
            .with_working_memory(vec![test_observation("o-1"), test_observation("o-2")]);
        let blocks = builder.build().unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content.contains("Edit o-1"));
    }
}
