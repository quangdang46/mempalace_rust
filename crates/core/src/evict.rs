/// Memory eviction policies for managing storage capacity.
///
/// Ported from mempalace's eviction system:
/// - EvictionStrategy: LRU, LFU, or retention-based eviction
/// - select_eviction_candidates: choose memories to evict based on strategy
/// - evict_to_target: evict memories until storage is below target count
///
/// Eviction is distinct from forgetting:
/// - Forgetting: semantic decision based on retention/decay
/// - Eviction: capacity management when storage is full
use crate::retention::*;
use crate::types::*;
use chrono::{DateTime, Utc};

/// Strategy for selecting memories to evict.
#[derive(Debug, Clone, PartialEq)]
pub enum EvictionStrategy {
    /// Evict least recently accessed memories first
    LeastRecentlyUsed,
    /// Evict least frequently accessed memories first
    LeastFrequentlyUsed,
    /// Evict memories with lowest retention strength first
    LowestRetention,
    /// Evict oldest memories first
    OldestFirst,
}

impl Default for EvictionStrategy {
    fn default() -> Self {
        EvictionStrategy::LowestRetention
    }
}

/// Configuration for eviction behavior.
#[derive(Debug, Clone)]
pub struct EvictionConfig {
    /// Maximum number of memories to keep (evict when exceeded)
    pub max_memories: usize,
    /// Strategy for selecting eviction candidates
    pub strategy: EvictionStrategy,
    /// Minimum retention strength to protect from eviction (default: -1.0 = no protection)
    pub protected_threshold: f64,
    /// Whether to evict only non-latest memories (default: false)
    pub evict_only_not_latest: bool,
}

impl Default for EvictionConfig {
    fn default() -> Self {
        Self {
            max_memories: 1000,
            strategy: EvictionStrategy::LowestRetention,
            protected_threshold: -1.0,
            evict_only_not_latest: false,
        }
    }
}

/// Result of eviction candidate selection.
#[derive(Debug, Clone)]
pub struct EvictionResult {
    /// Memories selected for eviction
    pub candidates: Vec<String>,
    /// Total memories evaluated
    pub evaluated: usize,
    /// Memories protected from eviction
    pub protected: usize,
}

/// Select memories for eviction based on the configured strategy.
///
/// Returns a list of memory IDs that should be evicted.
pub fn select_eviction_candidates(
    memories: &[Memory],
    retention_scores: &[RetentionScore],
    config: &EvictionConfig,
    target_count: usize,
) -> EvictionResult {
    let score_map: std::collections::HashMap<&str, &RetentionScore> = retention_scores
        .iter()
        .map(|s| (s.memory_id.as_str(), s))
        .collect();

    // Filter eligible memories
    let eligible: Vec<&Memory> = memories
        .iter()
        .filter(|m| {
            if config.evict_only_not_latest && m.is_latest {
                return false;
            }
            true
        })
        .collect();

    // Sort based on strategy
    let mut sorted = eligible.clone();
    match config.strategy {
        EvictionStrategy::LeastRecentlyUsed => {
            sorted.sort_by(|a, b| {
                let a_score = score_map.get(a.id.as_str());
                let b_score = score_map.get(b.id.as_str());
                let a_time = a_score.map(|s| s.last_accessed).unwrap_or(a.created_at);
                let b_time = b_score.map(|s| s.last_accessed).unwrap_or(b.created_at);
                a_time.cmp(&b_time)
            });
        }
        EvictionStrategy::LeastFrequentlyUsed => {
            sorted.sort_by(|a, b| {
                let a_score = score_map.get(a.id.as_str());
                let b_score = score_map.get(b.id.as_str());
                let a_count = a_score.map(|s| s.access_count).unwrap_or(0);
                let b_count = b_score.map(|s| s.access_count).unwrap_or(0);
                a_count.cmp(&b_count)
            });
        }
        EvictionStrategy::LowestRetention => {
            sorted.sort_by(|a, b| {
                let a_score = score_map.get(a.id.as_str());
                let b_score = score_map.get(b.id.as_str());
                let a_strength = a_score.map(|s| s.retention_strength).unwrap_or(0.0);
                let b_strength = b_score.map(|s| s.retention_strength).unwrap_or(0.0);
                a_strength
                    .partial_cmp(&b_strength)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        EvictionStrategy::OldestFirst => {
            sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        }
    }

    // Apply protection threshold and select candidates
    let mut candidates = Vec::new();
    let mut protected = 0;
    let has_protection = config.protected_threshold >= 0.0;

    for memory in &sorted {
        let score = score_map.get(memory.id.as_str());
        let retention = score.map(|s| s.retention_strength).unwrap_or(0.0);

        if has_protection && retention >= config.protected_threshold {
            protected += 1;
        } else if candidates.len() < target_count {
            candidates.push(memory.id.clone());
        }
    }

    EvictionResult {
        candidates,
        evaluated: eligible.len(),
        protected,
    }
}

/// Evict memories until the total count is at or below the target.
///
/// Returns a list of memory IDs to evict.
pub fn evict_to_target(
    memories: &[Memory],
    retention_scores: &[RetentionScore],
    config: &EvictionConfig,
) -> EvictionResult {
    let current_count = memories.len();
    if current_count <= config.max_memories {
        return EvictionResult {
            candidates: Vec::new(),
            evaluated: current_count,
            protected: 0,
        };
    }

    let target_count = current_count - config.max_memories;
    select_eviction_candidates(memories, retention_scores, config, target_count)
}

/// Check if eviction is needed based on current memory count.
pub fn needs_eviction(memory_count: usize, config: &EvictionConfig) -> bool {
    memory_count > config.max_memories
}

/// Calculate eviction priority score for a single memory.
///
/// Lower scores = more likely to be evicted.
/// Returns a score between 0.0 (evict immediately) and 1.0 (keep forever).
pub fn eviction_priority(
    memory: &Memory,
    retention_score: &RetentionScore,
    _config: &EvictionConfig,
) -> f64 {
    let mut score = 0.0;

    // Retention component (0-0.4)
    score += retention_score.retention_strength * 0.4;

    // Access frequency component (0-0.3)
    let freq_score = (retention_score.access_count as f64 / 100.0).min(1.0);
    score += freq_score * 0.3;

    // Recency component (0-0.3)
    let now = Utc::now();
    let days_since_access = now
        .signed_duration_since(retention_score.last_accessed)
        .num_days() as f64;
    let recency_score = (-0.1 * days_since_access).exp();
    score += recency_score * 0.3;

    // Latest memories get a bonus
    if memory.is_latest {
        score += 0.1;
    }

    score.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_memory(id: &str, created_days_ago: i64) -> Memory {
        Memory {
            id: id.to_string(),
            created_at: Utc::now() - chrono::Duration::days(created_days_ago),
            updated_at: Utc::now(),
            memory_type: MemoryType::Working,
            title: format!("Test {id}"),
            content: "Content".to_string(),
            concepts: vec![],
            files: vec![],
            session_ids: vec![],
            strength: 1.0,
            version: 0,
            parent_id: None,
            supersedes: vec![],
            related_ids: vec![],
            source_observation_ids: vec![],
            is_latest: true,
            forget_after: None,
            image_ref: None,
            agent_id: None,
            project: "test".to_string(),
        }
    }

    fn test_retention_score(
        memory_id: &str,
        retention_strength: f64,
        access_count: usize,
        days_since_access: i64,
    ) -> RetentionScore {
        RetentionScore {
            memory_id: memory_id.to_string(),
            retention_strength,
            last_accessed: Utc::now() - chrono::Duration::days(days_since_access),
            access_count,
            decay_rate: 0.1,
        }
    }

    #[test]
    fn test_needs_eviction() {
        let config = EvictionConfig {
            max_memories: 100,
            ..Default::default()
        };
        assert!(needs_eviction(101, &config));
        assert!(!needs_eviction(100, &config));
        assert!(!needs_eviction(50, &config));
    }

    #[test]
    fn test_evict_to_target_under_limit() {
        let memories = vec![test_memory("mem-1", 1)];
        let scores = vec![test_retention_score("mem-1", 1.0, 1, 0)];
        let config = EvictionConfig {
            max_memories: 100,
            ..Default::default()
        };
        let result = evict_to_target(&memories, &scores, &config);
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn test_evict_lowest_retention() {
        let memories = vec![
            test_memory("mem-1", 10),
            test_memory("mem-2", 5),
            test_memory("mem-3", 1),
        ];
        let scores = vec![
            test_retention_score("mem-1", 0.1, 0, 30),
            test_retention_score("mem-2", 0.5, 5, 7),
            test_retention_score("mem-3", 0.9, 20, 1),
        ];
        let config = EvictionConfig {
            max_memories: 2,
            strategy: EvictionStrategy::LowestRetention,
            ..Default::default()
        };

        let result = evict_to_target(&memories, &scores, &config);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0], "mem-1");
    }

    #[test]
    fn test_evict_lru() {
        let memories = vec![
            test_memory("mem-1", 10),
            test_memory("mem-2", 5),
            test_memory("mem-3", 1),
        ];
        let scores = vec![
            test_retention_score("mem-1", 0.5, 5, 30),
            test_retention_score("mem-2", 0.5, 5, 7),
            test_retention_score("mem-3", 0.5, 5, 1),
        ];
        let config = EvictionConfig {
            max_memories: 2,
            strategy: EvictionStrategy::LeastRecentlyUsed,
            ..Default::default()
        };

        let result = evict_to_target(&memories, &scores, &config);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0], "mem-1"); // Least recently used
    }

    #[test]
    fn test_evict_lfu() {
        let memories = vec![
            test_memory("mem-1", 10),
            test_memory("mem-2", 5),
            test_memory("mem-3", 1),
        ];
        let scores = vec![
            test_retention_score("mem-1", 0.5, 1, 1),
            test_retention_score("mem-2", 0.5, 10, 1),
            test_retention_score("mem-3", 0.5, 50, 1),
        ];
        let config = EvictionConfig {
            max_memories: 2,
            strategy: EvictionStrategy::LeastFrequentlyUsed,
            ..Default::default()
        };

        let result = evict_to_target(&memories, &scores, &config);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0], "mem-1"); // Least frequently used
    }

    #[test]
    fn test_protection_threshold() {
        let memories = vec![test_memory("mem-1", 10), test_memory("mem-2", 5)];
        let scores = vec![
            test_retention_score("mem-1", 0.3, 0, 30),
            test_retention_score("mem-2", 0.8, 10, 1),
        ];
        let config = EvictionConfig {
            max_memories: 1,
            strategy: EvictionStrategy::LowestRetention,
            protected_threshold: 0.5,
            ..Default::default()
        };

        let result = evict_to_target(&memories, &scores, &config);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0], "mem-1");
        assert_eq!(result.protected, 1); // mem-2 is protected
    }

    #[test]
    fn test_eviction_priority() {
        let memory = test_memory("mem-1", 1);
        let score = test_retention_score("mem-1", 0.9, 50, 1);
        let config = EvictionConfig::default();
        let priority = eviction_priority(&memory, &score, &config);
        assert!(priority > 0.5); // High priority

        let memory = test_memory("mem-2", 100);
        let score = test_retention_score("mem-2", 0.1, 0, 90);
        let priority = eviction_priority(&memory, &score, &config);
        assert!(priority < 0.3); // Low priority
    }

    #[test]
    fn test_evict_only_not_latest() {
        let mut mem1 = test_memory("mem-1", 10);
        mem1.is_latest = false;
        let mut mem2 = test_memory("mem-2", 5);
        mem2.is_latest = true;

        let memories = vec![mem1, mem2];
        let scores = vec![
            test_retention_score("mem-1", 0.1, 0, 30),
            test_retention_score("mem-2", 0.1, 0, 30),
        ];
        let config = EvictionConfig {
            max_memories: 1,
            strategy: EvictionStrategy::LowestRetention,
            evict_only_not_latest: true,
            ..Default::default()
        };

        let result = evict_to_target(&memories, &scores, &config);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0], "mem-1"); // Only not_latest eligible
    }
}
