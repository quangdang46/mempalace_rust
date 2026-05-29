//! Governance — port of upstream `governance.ts`.
//!
//! Memory governance policies: delete/bulk delete, audit queries.

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Governance filter for querying memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceFilter {
    pub max_age_days: Option<u64>,
    pub min_strength: Option<f64>,
    pub memory_type: Option<String>,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub not_accessed_since_days: Option<u64>,
}

/// Result of a governance delete operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceDeleteResult {
    pub deleted_ids: Vec<String>,
    pub count: usize,
    pub reason: String,
}

/// Apply governance filter to memories.
pub fn filter_memories<'a>(
    memories: &'a [crate::types::Memory],
    filter: &GovernanceFilter,
) -> Vec<&'a crate::types::Memory> {
    let now = Utc::now();

    memories.iter().filter(|m| {
        if let Some(max_age) = filter.max_age_days {
            let age_days = (now - m.created_at).num_days() as u64;
            if age_days < max_age {
                return false;
            }
        }

        if let Some(min_strength) = filter.min_strength {
            if m.strength > min_strength {
                return false;
            }
        }

        if let Some(ref mem_type) = filter.memory_type {
            if format!("{:?}", m.memory_type).to_lowercase() != mem_type.to_lowercase() {
                return false;
            }
        }

        if let Some(ref project) = filter.project {
            if &m.project != project {
                return false;
            }
        }

        if !filter.tags.is_empty() {
            let has_tag = filter.tags.iter().any(|t| m.concepts.contains(t));
            if !has_tag {
                return false;
            }
        }

        true
    }).collect()
}

/// Bulk delete memories matching a filter.
pub fn governance_delete(
    memories: &mut Vec<crate::types::Memory>,
    filter: &GovernanceFilter,
    reason: &str,
) -> GovernanceDeleteResult {
    let matching_ids: Vec<_> = filter_memories(memories, filter)
        .iter()
        .map(|m| m.id.clone())
        .collect();

    let count = matching_ids.len();
    memories.retain(|m| !matching_ids.contains(&m.id));

    GovernanceDeleteResult {
        deleted_ids: matching_ids,
        count,
        reason: reason.to_string(),
    }
}

/// Delete specific memories by ID with audit trail.
pub fn governance_delete_by_ids(
    memories: &mut Vec<crate::types::Memory>,
    ids: &[String],
    reason: &str,
) -> GovernanceDeleteResult {
    let count = ids.len();
    memories.retain(|m| !ids.contains(&m.id));

    GovernanceDeleteResult {
        deleted_ids: ids.to_vec(),
        count,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Memory, MemoryType};

    fn test_memory(id: &str, created_days_ago: i64, strength: f64, mem_type: MemoryType, concepts: Vec<&str>) -> Memory {
        Memory {
            id: id.into(),
            created_at: Utc::now() - chrono::Duration::days(created_days_ago),
            updated_at: Utc::now(),
            memory_type: mem_type,
            title: format!("Memory {}", id),
            content: format!("Content for {}", id),
            concepts: concepts.into_iter().map(String::from).collect(),
            files: vec![], session_ids: vec![],
            strength, version: 1, parent_id: None, supersedes: vec![],
            related_ids: vec![], source_observation_ids: vec![],
            is_latest: true, forget_after: None, image_ref: None, agent_id: None,
            project: "test".into(),
        }
    }

    #[test]
    fn test_filter_by_max_age() {
        let memories = vec![
            test_memory("m-1", 30, 0.5, MemoryType::Semantic, vec![]),
            test_memory("m-2", 5, 0.5, MemoryType::Semantic, vec![]),
        ];
        let filter = GovernanceFilter {
            max_age_days: Some(10),
            min_strength: None,
            memory_type: None,
            project: None,
            tags: vec![],
            not_accessed_since_days: None,
        };
        let result = filter_memories(&memories, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "m-1");
    }

    #[test]
    fn test_filter_by_min_strength() {
        let memories = vec![
            test_memory("m-1", 10, 0.9, MemoryType::Semantic, vec![]),
            test_memory("m-2", 10, 0.3, MemoryType::Semantic, vec![]),
        ];
        let filter = GovernanceFilter {
            max_age_days: None,
            min_strength: Some(0.5),
            memory_type: None,
            project: None,
            tags: vec![],
            not_accessed_since_days: None,
        };
        let result = filter_memories(&memories, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "m-1");
    }

    #[test]
    fn test_filter_by_type() {
        let memories = vec![
            test_memory("m-1", 10, 0.5, MemoryType::Semantic, vec![]),
            test_memory("m-2", 10, 0.5, MemoryType::Procedural, vec![]),
        ];
        let filter = GovernanceFilter {
            max_age_days: None,
            min_strength: None,
            memory_type: Some("semantic".to_string()),
            project: None,
            tags: vec![],
            not_accessed_since_days: None,
        };
        let result = filter_memories(&memories, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "m-1");
    }

    #[test]
    fn test_governance_delete() {
        let mut memories = vec![
            test_memory("m-1", 30, 0.1, MemoryType::Semantic, vec![]),
            test_memory("m-2", 5, 0.9, MemoryType::Semantic, vec![]),
        ];
        let filter = GovernanceFilter {
            max_age_days: Some(10),
            min_strength: Some(0.2),
            memory_type: None,
            project: None,
            tags: vec![],
            not_accessed_since_days: None,
        };
        let result = governance_delete(&mut memories, &filter, "old and weak");
        assert_eq!(result.count, 1);
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, "m-2");
    }

    #[test]
    fn test_governance_delete_by_ids() {
        let mut memories = vec![
            test_memory("m-1", 10, 0.5, MemoryType::Semantic, vec![]),
            test_memory("m-2", 10, 0.5, MemoryType::Semantic, vec![]),
        ];
        let result = governance_delete_by_ids(&mut memories, &["m-1".to_string()], "manual delete");
        assert_eq!(result.count, 1);
        assert_eq!(memories.len(), 1);
    }

    #[test]
    fn test_filter_by_tags() {
        let memories = vec![
            test_memory("m-1", 10, 0.5, MemoryType::Semantic, vec!["auth", "jwt"]),
            test_memory("m-2", 10, 0.5, MemoryType::Semantic, vec!["database"]),
        ];
        let filter = GovernanceFilter {
            max_age_days: None,
            min_strength: None,
            memory_type: None,
            project: None,
            tags: vec!["auth".to_string()],
            not_accessed_since_days: None,
        };
        let result = filter_memories(&memories, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "m-1");
    }
}
