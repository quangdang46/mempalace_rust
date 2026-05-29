//! Sliding window memory — port of upstream `sliding-window.ts`.
//!
//! Maintains a sliding window of recent observations for context injection.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Configuration for the sliding window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidingWindowConfig {
    pub max_tokens: usize,
    pub max_observations: usize,
    pub max_age_hours: u64,
    pub importance_threshold: u8,
}

impl Default for SlidingWindowConfig {
    fn default() -> Self {
        Self {
            max_tokens: 4000,
            max_observations: 20,
            max_age_hours: 24,
            importance_threshold: 3,
        }
    }
}

/// A window of recent observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidingWindow {
    pub observations: Vec<crate::types::CompressedObservation>,
    pub total_tokens: usize,
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
}

impl SlidingWindow {
    pub fn new() -> Self {
        Self {
            observations: Vec::new(),
            total_tokens: 0,
            oldest: None,
            newest: None,
        }
    }

    /// Add an observation to the window, evicting old ones if needed.
    pub fn add(&mut self, obs: crate::types::CompressedObservation, config: &SlidingWindowConfig) {
        let obs_tokens = (obs.title.len() + obs.narrative.len()) / 3;

        // Add new observation
        self.observations.push(obs.clone());
        self.total_tokens += obs_tokens;

        // Update time bounds
        self.oldest = self.observations.iter().map(|o| o.timestamp).min();
        self.newest = self.observations.iter().map(|o| o.timestamp).max();

        // Evict if over limits
        self.evict(config);
    }

    /// Evict observations that exceed limits.
    fn evict(&mut self, config: &SlidingWindowConfig) {
        // 1. Evict by age
        let cutoff = Utc::now() - chrono::Duration::hours(config.max_age_hours as i64);
        self.observations.retain(|o| o.timestamp >= cutoff);

        // 2. Evict by importance threshold
        self.observations.retain(|o| o.importance >= config.importance_threshold);

        // 3. Evict by count (oldest first)
        while self.observations.len() > config.max_observations {
            self.observations.remove(0);
        }

        // 4. Evict by token budget (oldest first)
        self.recalculate_tokens();
        while self.total_tokens > config.max_tokens && !self.observations.is_empty() {
            let removed = self.observations.remove(0);
            self.total_tokens -= (removed.title.len() + removed.narrative.len()) / 3;
        }

        // Update time bounds
        self.oldest = self.observations.iter().map(|o| o.timestamp).min();
        self.newest = self.observations.iter().map(|o| o.timestamp).max();
    }

    fn recalculate_tokens(&mut self) {
        self.total_tokens = self.observations.iter()
            .map(|o| (o.title.len() + o.narrative.len()) / 3)
            .sum();
    }

    /// Get observations as context blocks.
    pub fn to_context(&self) -> Vec<crate::types::ContextBlock> {
        self.observations.iter().map(|o| crate::types::ContextBlock {
            content: format!("{}\n{}", o.title, o.narrative),
            source: format!("sliding_window:{}", o.id),
            relevance_score: o.confidence,
            token_count: (o.title.len() + o.narrative.len()) / 3,
            memory_id: Some(o.id.clone()),
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CompressedObservation, ObservationType};

    fn test_obs(id: &str, importance: u8, hours_ago: i64) -> CompressedObservation {
        CompressedObservation {
            id: id.into(),
            session_id: "s-1".into(),
            timestamp: Utc::now() - chrono::Duration::hours(hours_ago),
            observation_type: ObservationType::FileEdit,
            title: format!("Edit {}", id),
            subtitle: None,
            facts: vec![],
            narrative: "test content".into(),
            concepts: vec![],
            files: vec![],
            importance,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".into(),
            agent_id: None,
        }
    }

    #[test]
    fn test_add_observation() {
        let mut window = SlidingWindow::new();
        let config = SlidingWindowConfig::default();
        window.add(test_obs("o-1", 5, 0), &config);
        assert_eq!(window.observations.len(), 1);
        assert!(window.total_tokens > 0);
    }

    #[test]
    fn test_evict_by_age() {
        let mut window = SlidingWindow::new();
        let config = SlidingWindowConfig { max_age_hours: 1, ..Default::default() };
        window.add(test_obs("o-1", 5, 0), &config);
        window.add(test_obs("o-2", 5, 2), &config); // Should be evicted
        assert_eq!(window.observations.len(), 1);
        assert_eq!(window.observations[0].id, "o-1");
    }

    #[test]
    fn test_evict_by_importance() {
        let mut window = SlidingWindow::new();
        let config = SlidingWindowConfig { importance_threshold: 4, ..Default::default() };
        window.add(test_obs("o-1", 5, 0), &config);
        window.add(test_obs("o-2", 2, 0), &config); // Should be evicted
        assert_eq!(window.observations.len(), 1);
    }

    #[test]
    fn test_evict_by_count() {
        let mut window = SlidingWindow::new();
        let config = SlidingWindowConfig { max_observations: 3, ..Default::default() };
        for i in 0..5 {
            window.add(test_obs(&format!("o-{}", i), 5, 0), &config);
        }
        assert_eq!(window.observations.len(), 3);
    }

    #[test]
    fn test_to_context() {
        let mut window = SlidingWindow::new();
        let config = SlidingWindowConfig::default();
        window.add(test_obs("o-1", 5, 0), &config);
        let context = window.to_context();
        assert_eq!(context.len(), 1);
        assert!(context[0].content.contains("Edit o-1"));
    }

    #[test]
    fn test_window_serialization() {
        let window = SlidingWindow::new();
        let json = serde_json::to_string(&window).unwrap();
        let parsed: SlidingWindow = serde_json::from_str(&json).unwrap();
        assert!(parsed.observations.is_empty());
    }
}
