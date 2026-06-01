/// Automatic forgetting of memories that have decayed below threshold.
///
/// Ported from agentmemory's auto-forget system:
/// - AutoForgetConfig: configurable thresholds and scheduling
/// - evaluate_forgetting: scan memories and return list of forgettable memory IDs
/// - batch_forget: mark multiple memories as not latest and set forget_after
///
/// The auto-forget system works with the retention module to identify
/// memories that should no longer be retrieved in search results.
use crate::retention::*;
use crate::types::*;
use chrono::{DateTime, Duration, Utc};

/// Configuration for automatic forgetting behavior.
#[derive(Debug, Clone)]
pub struct AutoForgetConfig {
    /// Minimum retention strength to keep a memory (default: 0.1)
    pub min_retention: f64,
    /// Maximum age in days before forced evaluation (default: 90)
    pub max_age_days: i64,
    /// Whether to actually delete or just mark as not latest (default: true)
    pub mark_not_latest: bool,
    /// How often to run the auto-forget cycle (default: 24 hours)
    pub cycle_interval: Duration,
}

impl Default for AutoForgetConfig {
    fn default() -> Self {
        Self {
            min_retention: 0.1,
            max_age_days: 90,
            mark_not_latest: true,
            cycle_interval: Duration::hours(24),
        }
    }
}

/// Result of evaluating a single memory for forgetting.
#[derive(Debug, Clone)]
pub struct ForgetEvaluation {
    pub memory_id: String,
    pub should_forget: bool,
    pub reason: ForgetReason,
    pub current_retention: f64,
}

/// Reason why a memory was marked for forgetting.
#[derive(Debug, Clone, PartialEq)]
pub enum ForgetReason {
    /// Retention strength dropped below minimum threshold
    LowRetention,
    /// Memory has passed its forget_after date
    Expired,
    /// Memory is too old and has never been accessed
    NeverAccessed,
    /// Memory is old enough to warrant re-evaluation
    Stale,
}

/// Evaluate a single memory for potential forgetting.
pub fn evaluate_memory(
    memory: &Memory,
    retention_score: &RetentionScore,
    config: &DecayConfig,
    auto_config: &AutoForgetConfig,
    now: Option<DateTime<Utc>>,
) -> ForgetEvaluation {
    let now = now.unwrap_or_else(Utc::now);

    // Check explicit expiration
    if let Some(forget_after) = memory.forget_after {
        if now >= forget_after {
            return ForgetEvaluation {
                memory_id: memory.id.clone(),
                should_forget: true,
                reason: ForgetReason::Expired,
                current_retention: calculate_retention(retention_score, config, Some(now)),
            };
        }
    }

    // Check retention threshold
    let retention = calculate_retention(retention_score, config, Some(now));
    if retention < auto_config.min_retention {
        return ForgetEvaluation {
            memory_id: memory.id.clone(),
            should_forget: true,
            reason: ForgetReason::LowRetention,
            current_retention: retention,
        };
    }

    // Check if never accessed and very old
    if retention_score.access_count == 0 {
        let age_days = now.signed_duration_since(memory.created_at).num_days();
        if age_days > auto_config.max_age_days {
            return ForgetEvaluation {
                memory_id: memory.id.clone(),
                should_forget: true,
                reason: ForgetReason::NeverAccessed,
                current_retention: retention,
            };
        }
    }

    ForgetEvaluation {
        memory_id: memory.id.clone(),
        should_forget: false,
        reason: ForgetReason::Stale, // Not actually stale, just evaluated
        current_retention: retention,
    }
}

/// Evaluate a batch of memories for forgetting.
///
/// Returns a list of ForgetEvaluation results. Memories with
/// `should_forget: true` should be marked as not latest.
pub fn evaluate_batch(
    memories: &[Memory],
    retention_scores: &[RetentionScore],
    config: &DecayConfig,
    auto_config: &AutoForgetConfig,
    now: Option<DateTime<Utc>>,
) -> Vec<ForgetEvaluation> {
    let score_map: std::collections::HashMap<&str, &RetentionScore> =
        retention_scores.iter().map(|s| (s.memory_id.as_str(), s)).collect();

    memories
        .iter()
        .map(|memory| {
            let default_score = default_retention_score(&memory.id);
            let score = score_map.get(memory.id.as_str()).copied().unwrap_or(&default_score);
            evaluate_memory(memory, score, config, auto_config, now)
        })
        .collect()
}

/// Apply forgetting to a list of memories.
///
/// Sets `is_latest = false` and updates `updated_at` for each forgettable memory.
/// Returns the updated memories.
pub fn apply_forgetting(
    evaluations: &[ForgetEvaluation],
    memories: &[Memory],
) -> Vec<Memory> {
    let forget_ids: std::collections::HashSet<&str> = evaluations
        .iter()
        .filter(|e| e.should_forget)
        .map(|e| e.memory_id.as_str())
        .collect();

    memories
        .iter()
        .map(|memory| {
            if forget_ids.contains(memory.id.as_str()) {
                Memory {
                    is_latest: false,
                    updated_at: Utc::now(),
                    ..memory.clone()
                }
            } else {
                memory.clone()
            }
        })
        .collect()
}

/// Determine if it's time to run the auto-forget cycle.
///
/// Returns true if the elapsed time since last run exceeds the cycle interval.
pub fn should_run_cycle(
    last_run: DateTime<Utc>,
    config: &AutoForgetConfig,
    now: Option<DateTime<Utc>>,
) -> bool {
    let now = now.unwrap_or_else(Utc::now);
    now.signed_duration_since(last_run) > config.cycle_interval
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_memory() -> Memory {
        Memory {
            id: "mem-1".to_string(),
            created_at: Utc::now() - Duration::days(10),
            updated_at: Utc::now(),
            memory_type: MemoryType::Working,
            title: "Test".to_string(),
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

    fn test_retention_score() -> RetentionScore {
        RetentionScore {
            memory_id: "mem-1".to_string(),
            retention_strength: 1.0,
            last_accessed: Utc::now(),
            access_count: 1,
            decay_rate: 0.1,
        }
    }

    fn test_config() -> DecayConfig {
        default_decay_config()
    }

    #[test]
    fn test_evaluate_memory_keep() {
        let memory = test_memory();
        let score = test_retention_score();
        let config = test_config();
        let auto_config = AutoForgetConfig::default();
        let result = evaluate_memory(&memory, &score, &config, &auto_config, None);
        assert!(!result.should_forget);
    }

    #[test]
    fn test_evaluate_memory_expired() {
        let mut memory = test_memory();
        memory.forget_after = Some(Utc::now() - Duration::hours(1));
        let score = test_retention_score();
        let config = test_config();
        let auto_config = AutoForgetConfig::default();
        let result = evaluate_memory(&memory, &score, &config, &auto_config, None);
        assert!(result.should_forget);
        assert_eq!(result.reason, ForgetReason::Expired);
    }

    #[test]
    fn test_evaluate_memory_low_retention() {
        let memory = test_memory();
        let mut score = test_retention_score();
        score.last_accessed = Utc::now() - Duration::days(365);
        let config = test_config();
        let mut auto_config = AutoForgetConfig::default();
        auto_config.min_retention = 0.5; // High threshold
        let result = evaluate_memory(&memory, &score, &config, &auto_config, None);
        assert!(result.should_forget);
        assert_eq!(result.reason, ForgetReason::LowRetention);
    }

    #[test]
    fn test_evaluate_memory_never_accessed() {
        let mut memory = test_memory();
        memory.created_at = Utc::now() - Duration::days(100);
        let mut score = test_retention_score();
        score.access_count = 0;
        let config = test_config();
        let auto_config = AutoForgetConfig::default();
        let result = evaluate_memory(&memory, &score, &config, &auto_config, None);
        assert!(result.should_forget);
        assert_eq!(result.reason, ForgetReason::NeverAccessed);
    }

    #[test]
    fn test_apply_forgetting() {
        let evaluations = vec![
            ForgetEvaluation {
                memory_id: "mem-1".to_string(),
                should_forget: true,
                reason: ForgetReason::LowRetention,
                current_retention: 0.05,
            },
            ForgetEvaluation {
                memory_id: "mem-2".to_string(),
                should_forget: false,
                reason: ForgetReason::Stale,
                current_retention: 0.8,
            },
        ];

        let mut mem1 = test_memory();
        mem1.id = "mem-1".to_string();
        let mut mem2 = test_memory();
        mem2.id = "mem-2".to_string();

        let memories = vec![mem1, mem2];
        let result = apply_forgetting(&evaluations, &memories);

        assert!(!result[0].is_latest);
        assert!(result[1].is_latest);
    }

    #[test]
    fn test_should_run_cycle() {
        let config = AutoForgetConfig::default();
        let last_run = Utc::now() - Duration::hours(25);
        assert!(should_run_cycle(last_run, &config, None));

        let last_run = Utc::now() - Duration::hours(1);
        assert!(!should_run_cycle(last_run, &config, None));
    }

    #[test]
    fn test_evaluate_batch() {
        let mut mem1 = test_memory();
        mem1.id = "mem-1".to_string();
        let mut mem2 = test_memory();
        mem2.id = "mem-2".to_string();
        mem2.forget_after = Some(Utc::now() - Duration::hours(1));

        let memories = vec![mem1, mem2];
        let scores = vec![test_retention_score()];
        let config = test_config();
        let auto_config = AutoForgetConfig::default();

        let results = evaluate_batch(&memories, &scores, &config, &auto_config, None);
        assert_eq!(results.len(), 2);
        assert!(!results[0].should_forget);
        assert!(results[1].should_forget);
    }
}
