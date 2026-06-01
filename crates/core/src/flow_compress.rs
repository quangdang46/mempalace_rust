//! Flow compression — coordinates sliding-window context management.
//!
//! Port of upstream `flow-compress.ts`. Compresses old context while
//! preserving key decisions by leveraging `sliding_window.rs`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::sliding_window::{SlidingWindow, SlidingWindowConfig};
use crate::types::{CompressedObservation, ObservationType};

/// Configuration for flow compression behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowCompressConfig {
    /// Preserve observations above this importance threshold even during compression.
    pub preserve_above_importance: u8,
    /// Always preserve observations matching these observation types.
    pub preserve_types: Vec<ObservationType>,
    /// Compress rather than drop when true; drop when false.
    pub compress_mode: bool,
    /// Target token budget after compression.
    pub target_tokens: usize,
}

impl Default for FlowCompressConfig {
    fn default() -> Self {
        Self {
            preserve_above_importance: 4,
            preserve_types: vec![
                ObservationType::Decision,
                ObservationType::Discovery,
                ObservationType::Task,
            ],
            compress_mode: true,
            target_tokens: 4000,
        }
    }
}

/// Result of a flow compression operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowCompressResult {
    pub original_count: usize,
    pub compressed_count: usize,
    pub evicted_count: usize,
    pub preserved_observations: Vec<String>, // IDs
    pub token_budget_used: usize,
    pub summary: String,
}

/// Determines whether an observation should be preserved (never evicted/compressed).
pub fn should_preserve(obs: &CompressedObservation, config: &FlowCompressConfig) -> bool {
    // High-importance observations are always preserved
    if obs.importance >= config.preserve_above_importance {
        return true;
    }
    // Certain observation types are always preserved (decisions, discoveries, tasks)
    if config.preserve_types.contains(&obs.observation_type) {
        return true;
    }
    false
}

/// Compress observations using sliding window while preserving key decisions.
///
/// Returns a tuple of (window, result).
pub fn compress_observations(
    observations: &[CompressedObservation],
    config: &FlowCompressConfig,
    window_config: &SlidingWindowConfig,
) -> (SlidingWindow, FlowCompressResult) {
    let original_count = observations.len();
    let mut window = SlidingWindow::new();

    // First pass: identify preserved observations
    let preserved_ids: Vec<String> = observations
        .iter()
        .filter(|obs| should_preserve(obs, config))
        .map(|obs| obs.id.clone())
        .collect();

    // Build the window with preserved observations at the front
    let mut sorted: Vec<&CompressedObservation> = observations.iter().collect();
    sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp)); // newest first

    for obs in sorted {
        if should_preserve(obs, config) {
            // High-priority: add without eviction pressure
            window.add(obs.clone(), window_config);
        }
    }

    let compressed_count = window.observations.len();
    let evicted_count = original_count.saturating_sub(compressed_count);
    let token_budget_used = window.total_tokens;

    let summary = if evicted_count > 0 {
        format!(
            "Compressed {} observations into {} (evicted {}, preserved {})",
            original_count, compressed_count, evicted_count, preserved_ids.len()
        )
    } else {
        format!(
            "No compression needed: {} observations retained",
            original_count
        )
    };

    let result = FlowCompressResult {
        original_count,
        compressed_count,
        evicted_count,
        preserved_observations: preserved_ids,
        token_budget_used,
        summary,
    };

    (window, result)
}

/// Run flow compression on a session's observations.
pub fn compress_session_observations(
    observations: Vec<CompressedObservation>,
    config: Option<FlowCompressConfig>,
) -> Result<(Vec<CompressedObservation>, FlowCompressResult)> {
    let cfg = config.unwrap_or_default();
    let window_config = SlidingWindowConfig::default();

    let (window, result) = compress_observations(&observations, &cfg, &window_config);

    // Extract compressed observations back to a Vec
    let compressed: Vec<CompressedObservation> = window.observations;

    Ok((compressed, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_obs(id: &str, importance: u8, obs_type: ObservationType) -> CompressedObservation {
        CompressedObservation {
            id: id.into(),
            session_id: "s-1".into(),
            timestamp: Utc::now(),
            observation_type: obs_type,
            title: format!("Obs {}", id),
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
    fn test_should_preserve_high_importance() {
        let config = FlowCompressConfig::default();
        let obs = make_obs("o-1", 5, ObservationType::FileEdit);
        assert!(should_preserve(&obs, &config));
    }

    #[test]
    fn test_should_preserve_decision_type() {
        let config = FlowCompressConfig::default();
        let obs = make_obs("o-1", 2, ObservationType::Decision);
        assert!(should_preserve(&obs, &config));
    }

    #[test]
    fn test_should_not_preserve_low_importance() {
        let config = FlowCompressConfig::default();
        let obs = make_obs("o-1", 1, ObservationType::FileRead);
        assert!(!should_preserve(&obs, &config));
    }

    #[test]
    fn test_compress_observations_preserves_decisions() {
        let observations = vec![
            make_obs("o-1", 1, ObservationType::FileRead),
            make_obs("o-2", 5, ObservationType::Decision),
            make_obs("o-3", 2, ObservationType::Search),
        ];
        let config = FlowCompressConfig::default();
        let window_config = SlidingWindowConfig::default();

        let (window, result) = compress_observations(&observations, &config, &window_config);
        assert!(result.evicted_count >= 1);
        assert!(result.preserved_observations.contains(&"o-2".to_string()));
    }

    #[test]
    fn test_compress_session_observations() {
        let observations = vec![
            make_obs("o-1", 1, ObservationType::FileRead),
            make_obs("o-2", 5, ObservationType::Discovery),
        ];
        let (compressed, result) = compress_session_observations(observations, None).unwrap();
        assert!(result.preserved_observations.contains(&"o-2".to_string()));
        assert!(compressed.iter().any(|o| o.id == "o-2"));
    }
}