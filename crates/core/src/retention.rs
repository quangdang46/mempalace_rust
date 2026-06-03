/// Retention scoring and Ebbinghaus decay calculations.
///
/// Ported from agentmemory's retention system:
/// - calculate_retention: Ebbinghaus forgetting curve
/// - should_forget: retention below threshold or past forget_after
/// - promote_tier: upgrade memory to higher consolidation tier
/// - record_access: update access_count and last_accessed in RetentionScore
use crate::types::*;
use chrono::{DateTime, Duration, Utc};

/// Calculate retention strength using Ebbinghaus forgetting curve.
///
/// Formula: retention = initial * e^(-decay_rate * elapsed_days)
///
/// Matches agentmemory's Ebbinghaus-based decay calculation.
pub fn calculate_retention(
    retention_score: &RetentionScore,
    config: &DecayConfig,
    now: Option<DateTime<Utc>>,
) -> f64 {
    let now = now.unwrap_or_else(Utc::now);
    let elapsed = now.signed_duration_since(retention_score.last_accessed);
    let elapsed_days = elapsed.num_seconds() as f64 / 86400.0;

    let retention = config.initial_retention * (-config.decay_rate * elapsed_days).exp();

    retention.max(0.0).min(1.0)
}

/// Determine if a memory should be forgotten.
///
/// A memory is forgettable if:
/// 1. It has a `forget_after` date that has passed, OR
/// 2. Its retention strength is below the minimum retention threshold
pub fn should_forget(
    memory: &Memory,
    retention_score: &RetentionScore,
    config: &DecayConfig,
    now: Option<DateTime<Utc>>,
) -> bool {
    let now = now.unwrap_or_else(Utc::now);

    // Check explicit forget_after date
    if let Some(forget_after) = memory.forget_after {
        if now >= forget_after {
            return true;
        }
    }

    // Check retention threshold
    let retention = calculate_retention(retention_score, config, Some(now));
    retention < config.minimum_retention
}

/// Promote a memory to a higher consolidation tier based on retention strength.
///
/// Tier promotion thresholds:
/// - retention >= 0.9 → Insight (permanent, highest tier)
/// - retention >= 0.7 → Lesson (long-term)
/// - retention >= 0.5 → Semantic (medium-term)
/// - retention >= 0.3 → Episodic (short-term)
/// - retention < 0.3 → Working (eligible for forgetting)
pub fn promote_tier(retention_strength: f64) -> MemoryType {
    if retention_strength >= 0.9 {
        MemoryType::Insight
    } else if retention_strength >= 0.7 {
        MemoryType::Lesson
    } else if retention_strength >= 0.5 {
        MemoryType::Semantic
    } else if retention_strength >= 0.3 {
        MemoryType::Episodic
    } else {
        MemoryType::Working // Lowest tier, eligible for forgetting
    }
}

/// Record a memory access, updating access_count and last_accessed.
///
/// Returns the updated RetentionScore. Each access reinforces the memory
/// by resetting the decay timer.
pub fn record_access(
    retention_score: &RetentionScore,
    now: Option<DateTime<Utc>>,
) -> RetentionScore {
    let now = now.unwrap_or_else(Utc::now);
    RetentionScore {
        memory_id: retention_score.memory_id.clone(),
        retention_strength: retention_score.retention_strength,
        last_accessed: now,
        access_count: retention_score.access_count + 1,
        decay_rate: retention_score.decay_rate,
    }
}

/// Apply decay to a memory's strength based on its retention score.
///
/// Returns the updated Memory with adjusted strength.
pub fn apply_decay(
    memory: &Memory,
    retention_score: &RetentionScore,
    config: &DecayConfig,
    now: Option<DateTime<Utc>>,
) -> Memory {
    let new_strength = calculate_retention(retention_score, config, now);
    Memory {
        strength: new_strength,
        updated_at: now.unwrap_or_else(Utc::now),
        ..memory.clone()
    }
}

/// Calculate a reinforcement multiplier based on access frequency.
///
/// More frequently accessed memories decay slower.
/// Matches agentmemory's frequency-based reinforcement.
pub fn frequency_multiplier(access_count: usize) -> f64 {
    if access_count == 0 {
        1.0
    } else if access_count <= 3 {
        1.2
    } else if access_count <= 10 {
        1.5
    } else if access_count <= 30 {
        1.8
    } else {
        2.0
    }
}

/// Calculate decay rate based on memory type.
///
/// Different memory types have different natural decay rates:
/// - Insight: 0.01 (nearly permanent)
/// - Lesson: 0.05 (long-term)
/// - Semantic: 0.1 (medium-term)
/// - Procedural: 0.15 (pattern-based)
/// - Episodic: 0.2 (event-based)
/// - Working: 0.3 (short-term)
pub fn decay_rate_for_type(memory_type: &MemoryType) -> f64 {
    match memory_type {
        MemoryType::Insight => 0.01,
        MemoryType::Lesson => 0.05,
        MemoryType::Semantic => 0.1,
        MemoryType::Procedural => 0.15,
        MemoryType::Episodic => 0.2,
        MemoryType::Working => 0.3,
    }
}

/// Create a default DecayConfig with reasonable values.
///
/// Defaults:
/// - initial_retention: 1.0
/// - decay_rate: 0.1 (medium)
/// - reinforcement_multiplier: 1.5
/// - minimum_retention: 0.1
pub fn default_decay_config() -> DecayConfig {
    DecayConfig {
        initial_retention: 1.0,
        decay_rate: 0.1,
        reinforcement_multiplier: 1.5,
        minimum_retention: 0.1,
    }
}

/// Create a default RetentionScore for a new memory.
pub fn default_retention_score(memory_id: &str) -> RetentionScore {
    RetentionScore {
        memory_id: memory_id.to_string(),
        retention_strength: 1.0,
        last_accessed: Utc::now(),
        access_count: 0,
        decay_rate: 0.1,
    }
}

/// Issue #30: jcode-compatible memory score.
///
/// Formula (matches jcode's `MemoryEntry::effective_confidence`):
/// ```text
///   score = confidence * 2^(-elapsed_days / half_life)
///                * (1.0 + 0.1 * ln(max(1, access_count + 1)))
/// ```
///
/// where `half_life` is the category-specific half-life (365d for
/// `Correction`, 90d for `Preference`, 60d for `Entity`, 30d for
/// `Fact` / default kinds, 45d for `Custom(_)`).
///
/// Returns 0.0 if `!drawer.active` — superseded memories are dormant
/// until reactivated. Used by `MemoryProvider::recent()` as the
/// ranking function.
///
/// The `now` parameter is exposed so tests can simulate time
/// progression without sleeping.
pub fn memory_score(
    drawer: &crate::palace::Drawer,
    now: DateTime<Utc>,
) -> f64 {
    if !drawer.active {
        return 0.0;
    }
    let half_life = drawer.kind.half_life_days();
    if half_life <= 0.0 {
        return drawer.confidence;
    }
    let elapsed_days = (now - drawer.created_at).num_days().max(0) as f64;
    let decay = (-std::f64::consts::LN_2 * elapsed_days / half_life).exp();
    let access_boost = 1.0 + (drawer.access_count as f64 + 1.0).ln().max(0.0) * 0.1;
    // Note: we don't clamp the final score to 1.0 — the access_boost
    // is intentionally unbounded so a heavily-accessed drawer can score
    // above 1.0 (this is how jcode surfaces "high-traffic" memories).
    // Decay always returns <= 1.0, so the score is bounded above by
    // `access_boost` and below by 0.0.
    (drawer.confidence * decay * access_boost).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_memory() -> Memory {
        Memory {
            id: "mem-1".to_string(),
            created_at: Utc::now(),
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
            access_count: 0,
            decay_rate: 0.1,
        }
    }

    fn test_config() -> DecayConfig {
        DecayConfig {
            initial_retention: 1.0,
            decay_rate: 0.1,
            reinforcement_multiplier: 1.5,
            minimum_retention: 0.1,
        }
    }

    #[test]
    fn test_calculate_retention_fresh() {
        let score = test_retention_score();
        let config = test_config();
        let retention = calculate_retention(&score, &config, Some(score.last_accessed));
        assert!((retention - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_calculate_retention_decays_over_time() {
        let mut score = test_retention_score();
        score.last_accessed = Utc::now() - Duration::days(30);
        let config = test_config();
        let retention = calculate_retention(&score, &config, None);
        assert!(retention < 1.0);
        assert!(retention > 0.0);
    }

    #[test]
    fn test_calculate_retention_approaches_zero() {
        let mut score = test_retention_score();
        score.last_accessed = Utc::now() - Duration::days(365);
        let config = test_config();
        let retention = calculate_retention(&score, &config, None);
        assert!(retention < config.minimum_retention);
        assert!(retention >= 0.0);
    }

    #[test]
    fn test_should_forget_past_forget_after() {
        let mut memory = test_memory();
        memory.forget_after = Some(Utc::now() - Duration::hours(1));
        let score = test_retention_score();
        let config = test_config();
        assert!(should_forget(&memory, &score, &config, None));
    }

    #[test]
    fn test_should_forget_low_retention() {
        let memory = test_memory();
        let mut score = test_retention_score();
        score.last_accessed = Utc::now() - Duration::days(365);
        let config = test_config();
        assert!(should_forget(&memory, &score, &config, None));
    }

    #[test]
    fn test_should_not_forget_recent_high_retention() {
        let memory = test_memory();
        let score = test_retention_score();
        let config = test_config();
        assert!(!should_forget(&memory, &score, &config, None));
    }

    #[test]
    fn test_promote_tier_insight() {
        assert_eq!(promote_tier(0.95), MemoryType::Insight);
        assert_eq!(promote_tier(0.9), MemoryType::Insight);
    }

    #[test]
    fn test_promote_tier_lesson() {
        assert_eq!(promote_tier(0.8), MemoryType::Lesson);
        assert_eq!(promote_tier(0.7), MemoryType::Lesson);
    }

    #[test]
    fn test_promote_tier_semantic() {
        assert_eq!(promote_tier(0.6), MemoryType::Semantic);
        assert_eq!(promote_tier(0.5), MemoryType::Semantic);
    }

    #[test]
    fn test_promote_tier_working() {
        assert_eq!(promote_tier(0.4), MemoryType::Episodic);
        assert_eq!(promote_tier(0.3), MemoryType::Episodic);
        assert_eq!(promote_tier(0.1), MemoryType::Working);
    }

    #[test]
    fn test_record_access() {
        let score = test_retention_score();
        let before_count = score.access_count;
        let updated = record_access(&score, None);
        assert_eq!(updated.access_count, before_count + 1);
        assert!(updated.last_accessed >= score.last_accessed);
    }

    #[test]
    fn test_apply_decay() {
        let memory = test_memory();
        let score = test_retention_score();
        let config = test_config();
        let decayed = apply_decay(&memory, &score, &config, None);
        assert!(decayed.strength <= 1.0);
        assert!(decayed.updated_at >= memory.updated_at);
    }

    #[test]
    fn test_frequency_multiplier() {
        assert!((frequency_multiplier(0) - 1.0).abs() < 0.01);
        assert!((frequency_multiplier(1) - 1.2).abs() < 0.01);
        assert!((frequency_multiplier(5) - 1.5).abs() < 0.01);
        assert!((frequency_multiplier(20) - 1.8).abs() < 0.01);
        assert!((frequency_multiplier(50) - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_decay_rate_for_type() {
        assert!((decay_rate_for_type(&MemoryType::Insight) - 0.01).abs() < 0.001);
        assert!((decay_rate_for_type(&MemoryType::Lesson) - 0.05).abs() < 0.001);
        assert!((decay_rate_for_type(&MemoryType::Semantic) - 0.1).abs() < 0.001);
        assert!((decay_rate_for_type(&MemoryType::Working) - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_default_decay_config() {
        let config = default_decay_config();
        assert!((config.initial_retention - 1.0).abs() < 0.01);
        assert!((config.decay_rate - 0.1).abs() < 0.01);
        assert!((config.reinforcement_multiplier - 1.5).abs() < 0.01);
        assert!((config.minimum_retention - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_default_retention_score() {
        let score = default_retention_score("mem-test");
        assert_eq!(score.memory_id, "mem-test");
        assert!((score.retention_strength - 1.0).abs() < 0.01);
        assert_eq!(score.access_count, 0);
    }

    // Issue #30: memory_score

    fn test_drawer(kind: crate::palace::DrawerKind, days_old: i64, access_count: u64) -> crate::palace::Drawer {
        let mut d = crate::palace::Drawer::new("test content");
        d.kind = kind;
        d.created_at = Utc::now() - Duration::days(days_old);
        d.confidence = 1.0;
        d.access_count = access_count;
        d.active = true;
        d
    }

    #[test]
    fn test_memory_score_fresh_fact_is_high() {
        let d = test_drawer(crate::palace::DrawerKind::Fact, 0, 0);
        let score = memory_score(&d, Utc::now());
        // Fresh fact: confidence * 1.0 (no decay) * 1.0 (no access boost) = 1.0
        assert!((score - 1.0).abs() < 0.01, "expected ~1.0, got {score}");
    }

    #[test]
    fn test_memory_score_correction_decays_slower_than_fact() {
        // After 30 days, a Correction (365d half-life) should score higher than a Fact (30d half-life)
        let corr = test_drawer(crate::palace::DrawerKind::Correction, 30, 0);
        let fact = test_drawer(crate::palace::DrawerKind::Fact, 30, 0);
        let s_corr = memory_score(&corr, Utc::now());
        let s_fact = memory_score(&fact, Utc::now());
        assert!(
            s_corr > s_fact,
            "Correction should outscore Fact after 30d: corr={s_corr}, fact={s_fact}"
        );
    }

    #[test]
    fn test_memory_score_inactive_returns_zero() {
        let mut d = test_drawer(crate::palace::DrawerKind::Fact, 0, 0);
        d.active = false;
        let score = memory_score(&d, Utc::now());
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_memory_score_access_count_boosts() {
        let a = test_drawer(crate::palace::DrawerKind::Fact, 0, 0);
        let b = test_drawer(crate::palace::DrawerKind::Fact, 0, 100);
        let s_a = memory_score(&a, Utc::now());
        let s_b = memory_score(&b, Utc::now());
        assert!(s_b > s_a, "more accesses should boost score: a={s_a}, b={s_b}");
    }
}
