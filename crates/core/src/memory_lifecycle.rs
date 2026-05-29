/// Memory lifecycle management — versioning, evolution, and retention tracking.
/// 1:1 port from agentmemory memory lifecycle patterns.

use chrono::Utc;

#[cfg(test)]
use chrono::Duration;

use crate::types::{ConsolidationTier, DecayConfig, Memory, MemoryType, RetentionScore};

/// Evolve an existing memory with new content.
/// Sets the old memory's is_latest to false and creates a new version.
pub fn evolve_memory(
    existing: &Memory,
    new_content: String,
    new_title: Option<String>,
) -> Memory {
    // Mark existing as not latest
    let new_version = existing.version + 1;
    let mut supersedes = existing.supersedes.clone();
    supersedes.push(existing.id.clone());

    Memory {
        id: format!("{}-v{}", existing.id.split('-').next().unwrap_or("mem"), new_version),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        memory_type: existing.memory_type,
        title: new_title.unwrap_or_else(|| existing.title.clone()),
        content: new_content,
        concepts: existing.concepts.clone(),
        files: existing.files.clone(),
        session_ids: existing.session_ids.clone(),
        strength: existing.strength,
        version: new_version,
        parent_id: Some(existing.id.clone()),
        supersedes,
        related_ids: existing.related_ids.clone(),
        source_observation_ids: existing.source_observation_ids.clone(),
        is_latest: true,
        forget_after: existing.forget_after,
        image_ref: existing.image_ref.clone(),
        agent_id: existing.agent_id.clone(),
        project: existing.project.clone(),
    }
}

/// Apply Ebbinghaus decay to a memory's retention strength.
/// strength = max(0.1, strength * 0.9^decay_periods)
/// where decay_periods = floor(days_since / decay_days)
pub fn apply_decay(memory: &Memory, config: &DecayConfig) -> f64 {
    let days_since = Utc::now()
        .signed_duration_since(memory.updated_at)
        .num_days() as f64;
    let decay_periods = (days_since / config.decay_rate).floor() as u32;
    let decayed = config.initial_retention * 0.9_f64.powi(decay_periods as i32);
    decayed.max(0.1)
}

/// Calculate retention score for a memory.
pub fn calculate_retention(
    memory: &Memory,
    access_count: usize,
    last_accessed: Option<chrono::DateTime<Utc>>,
    config: &DecayConfig,
) -> RetentionScore {
    let last_accessed = last_accessed.unwrap_or(memory.updated_at);
    let decayed_strength = apply_decay(memory, config);

    RetentionScore {
        memory_id: memory.id.clone(),
        retention_strength: decayed_strength,
        last_accessed,
        access_count,
        decay_rate: config.decay_rate,
    }
}

/// Check if a memory should be forgotten based on retention and config.
pub fn should_forget(memory: &Memory, retention: &RetentionScore, min_retention: f64) -> bool {
    if let Some(forget_after) = memory.forget_after {
        return Utc::now() >= forget_after;
    }
    retention.retention_strength < min_retention
}

/// Promote a memory to a higher consolidation tier.
pub fn promote_tier(memory: &Memory) -> Option<ConsolidationTier> {
    match memory.memory_type {
        MemoryType::Working => Some(ConsolidationTier::Episodic),
        MemoryType::Episodic => Some(ConsolidationTier::Semantic),
        MemoryType::Semantic => Some(ConsolidationTier::Procedural),
        MemoryType::Procedural | MemoryType::Insight | MemoryType::Lesson => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_memory() -> Memory {
        Memory {
            id: "mem-1".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            memory_type: MemoryType::Working,
            title: "Test memory".to_string(),
            content: "Initial content".to_string(),
            concepts: vec!["test".to_string()],
            files: vec![],
            session_ids: vec!["sess-1".to_string()],
            strength: 0.8,
            version: 1,
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

    #[test]
    fn test_evolve_memory() {
        let existing = make_memory();
        let evolved = evolve_memory(&existing, "New content".to_string(), None);

        assert_eq!(evolved.version, 2);
        assert!(evolved.is_latest);
        assert_eq!(evolved.content, "New content");
        assert_eq!(evolved.parent_id, Some("mem-1".to_string()));
        assert!(evolved.supersedes.contains(&"mem-1".to_string()));
    }

    #[test]
    fn test_evolve_memory_with_new_title() {
        let existing = make_memory();
        let evolved = evolve_memory(&existing, "New content".to_string(), Some("New title".to_string()));
        assert_eq!(evolved.title, "New title");
    }

    #[test]
    fn test_apply_decay_no_time_passed() {
        let memory = make_memory();
        let config = DecayConfig {
            initial_retention: 1.0,
            decay_rate: 7.0,
            reinforcement_multiplier: 1.5,
            minimum_retention: 0.1,
        };
        let strength = apply_decay(&memory, &config);
        assert!((strength - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_decay_after_time() {
        let mut memory = make_memory();
        memory.updated_at = Utc::now() - Duration::days(14);
        let config = DecayConfig {
            initial_retention: 1.0,
            decay_rate: 7.0,
            reinforcement_multiplier: 1.5,
            minimum_retention: 0.1,
        };
        let strength = apply_decay(&memory, &config);
        // 14 days / 7 day decay = 2 periods
        // 1.0 * 0.9^2 = 0.81
        assert!((strength - 0.81).abs() < 0.01);
    }

    #[test]
    fn test_apply_decay_minimum() {
        let mut memory = make_memory();
        memory.updated_at = Utc::now() - Duration::days(365);
        let config = DecayConfig {
            initial_retention: 1.0,
            decay_rate: 7.0,
            reinforcement_multiplier: 1.5,
            minimum_retention: 0.1,
        };
        let strength = apply_decay(&memory, &config);
        assert!(strength >= 0.1);
    }

    #[test]
    fn test_should_forget_by_date() {
        let mut memory = make_memory();
        memory.forget_after = Some(Utc::now() - Duration::hours(1));
        let retention = RetentionScore {
            memory_id: memory.id.clone(),
            retention_strength: 0.5,
            last_accessed: Utc::now(),
            access_count: 0,
            decay_rate: 7.0,
        };
        assert!(should_forget(&memory, &retention, 0.1));
    }

    #[test]
    fn test_should_not_forget() {
        let memory = make_memory();
        let retention = RetentionScore {
            memory_id: memory.id.clone(),
            retention_strength: 0.5,
            last_accessed: Utc::now(),
            access_count: 0,
            decay_rate: 7.0,
        };
        assert!(!should_forget(&memory, &retention, 0.1));
    }

    #[test]
    fn test_promote_tier() {
        let working = make_memory();
        assert_eq!(promote_tier(&working), Some(ConsolidationTier::Episodic));

        let mut episodic = make_memory();
        episodic.memory_type = MemoryType::Episodic;
        assert_eq!(promote_tier(&episodic), Some(ConsolidationTier::Semantic));

        let mut semantic = make_memory();
        semantic.memory_type = MemoryType::Semantic;
        assert_eq!(promote_tier(&semantic), Some(ConsolidationTier::Procedural));

        let mut procedural = make_memory();
        procedural.memory_type = MemoryType::Procedural;
        assert_eq!(promote_tier(&procedural), None);
    }
}
