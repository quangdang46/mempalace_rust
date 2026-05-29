//! Heal — port of upstream `heal.ts`.
//!
//! Auto-fixes fixable diagnostic issues found by the diagnose tool.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Result of a heal operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealResult {
    pub fixed: Vec<String>,
    pub failed: Vec<String>,
    pub dry_run: bool,
}

/// Diagnostic check that can be auto-fixed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixableIssue {
    pub name: String,
    pub category: String,
    pub message: String,
    pub fix_action: String,
}

/// Auto-fix blocked actions whose dependencies are all done.
pub fn heal_blocked_actions(
    actions: &mut Vec<crate::types::Action>,
    edges: &[crate::types::ActionEdge],
    dry_run: bool,
) -> Result<HealResult> {
    let mut fixed = Vec::new();
    let failed = Vec::new();

    let action_map: std::collections::HashMap<_, _> = actions.iter()
        .map(|a| (a.id.clone(), a.clone()))
        .collect();

    for action in actions.iter_mut() {
        if action.status != crate::types::ActionStatus::Blocked {
            continue;
        }

        let deps: Vec<_> = edges.iter()
            .filter(|e| e.from_id == action.id)
            .collect();

        if deps.is_empty() {
            continue;
        }

        let all_done = deps.iter().all(|e| {
            action_map.get(&e.to_id)
                .map(|a| matches!(a.status, crate::types::ActionStatus::Completed))
                .unwrap_or(false)
        });

        if all_done {
            if dry_run {
                fixed.push(format!("Would unblock: {}", action.id));
            } else {
                action.status = crate::types::ActionStatus::Pending;
                action.updated_at = chrono::Utc::now();
                fixed.push(format!("Unblocked: {}", action.id));
            }
        }
    }

    Ok(HealResult { fixed, failed, dry_run })
}

/// Auto-fix expired leases by releasing them.
pub fn heal_expired_leases(
    leases: &mut Vec<crate::types::Lease>,
    dry_run: bool,
) -> Result<HealResult> {
    let mut fixed = Vec::new();
    let now = chrono::Utc::now();

    leases.retain(|lease| {
        if lease.expires_at < now {
            fixed.push(format!("Released expired lease: {}", lease.id));
            !dry_run // Remove in non-dry-run mode
        } else {
            true
        }
    });

    Ok(HealResult { fixed, failed: Vec::new(), dry_run })
}

/// Heal all fixable issues across categories.
pub fn heal_all(
    actions: &mut Vec<crate::types::Action>,
    action_edges: &[crate::types::ActionEdge],
    leases: &mut Vec<crate::types::Lease>,
    dry_run: bool,
) -> Result<HealResult> {
    let mut all_fixed = Vec::new();
    let mut all_failed = Vec::new();

    let blocked_result = heal_blocked_actions(actions, action_edges, dry_run)?;
    all_fixed.extend(blocked_result.fixed);
    all_failed.extend(blocked_result.failed);

    let lease_result = heal_expired_leases(leases, dry_run)?;
    all_fixed.extend(lease_result.fixed);
    all_failed.extend(lease_result.failed);

    Ok(HealResult {
        fixed: all_fixed,
        failed: all_failed,
        dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Action, ActionEdge, ActionEdgeType, ActionStatus, Lease};

    fn test_action(id: &str, status: ActionStatus) -> Action {
        Action {
            id: id.into(), title: format!("Action {}", id), description: "".into(),
            status, priority: 1, created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            created_by: None, assigned_to: None, project: "test".into(), tags: vec![],
            source_observation_ids: vec![], source_memory_ids: vec![],
            result: None, parent_id: None, metadata: std::collections::HashMap::new(),
            sketch_id: None, crystallized_into: None,
        }
    }

    fn test_lease(id: &str, action_id: &str, hours_until_expiry: i64) -> Lease {
        Lease {
            id: id.into(), action_id: action_id.into(), holder: "agent".into(),
            acquired_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(hours_until_expiry),
            project: "test".into(),
        }
    }

    #[test]
    fn test_heal_blocked_actions() {
        let mut actions = vec![
            test_action("a-1", ActionStatus::Completed),
            test_action("a-2", ActionStatus::Blocked),
        ];
        let edges = vec![ActionEdge {
            from_id: "a-2".into(), to_id: "a-1".into(),
            edge_type: ActionEdgeType::DependsOn,
        }];

        let result = heal_blocked_actions(&mut actions, &edges, false).unwrap();
        assert_eq!(result.fixed.len(), 1);
        assert_eq!(actions[1].status, ActionStatus::Pending);
    }

    #[test]
    fn test_heal_blocked_not_all_deps_done() {
        let mut actions = vec![
            test_action("a-1", ActionStatus::Pending),
            test_action("a-2", ActionStatus::Blocked),
        ];
        let edges = vec![ActionEdge {
            from_id: "a-2".into(), to_id: "a-1".into(),
            edge_type: ActionEdgeType::DependsOn,
        }];

        let result = heal_blocked_actions(&mut actions, &edges, false).unwrap();
        assert!(result.fixed.is_empty());
        assert_eq!(actions[1].status, ActionStatus::Blocked);
    }

    #[test]
    fn test_heal_expired_leases() {
        let mut leases = vec![
            test_lease("l-1", "a-1", -1), // expired
            test_lease("l-2", "a-2", 24),  // not expired
        ];

        let result = heal_expired_leases(&mut leases, false).unwrap();
        assert_eq!(result.fixed.len(), 1);
        assert_eq!(leases.len(), 1);
    }

    #[test]
    fn test_heal_dry_run() {
        let mut actions = vec![
            test_action("a-1", ActionStatus::Completed),
            test_action("a-2", ActionStatus::Blocked),
        ];
        let edges = vec![ActionEdge {
            from_id: "a-2".into(), to_id: "a-1".into(),
            edge_type: ActionEdgeType::DependsOn,
        }];

        let result = heal_blocked_actions(&mut actions, &edges, true).unwrap();
        assert_eq!(result.fixed.len(), 1);
        assert_eq!(actions[1].status, ActionStatus::Blocked); // Not changed
    }

    #[test]
    fn test_heal_result_serialization() {
        let result = HealResult {
            fixed: vec!["Fixed: a-1".to_string()],
            failed: vec![],
            dry_run: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: HealResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fixed.len(), 1);
    }

    #[test]
    fn test_heal_all() {
        let mut actions = vec![
            test_action("a-1", ActionStatus::Completed),
            test_action("a-2", ActionStatus::Blocked),
        ];
        let edges = vec![ActionEdge {
            from_id: "a-2".into(), to_id: "a-1".into(),
            edge_type: ActionEdgeType::DependsOn,
        }];
        let mut leases = vec![test_lease("l-1", "a-1", -1)];

        let result = heal_all(&mut actions, &edges, &mut leases, false).unwrap();
        assert!(!result.fixed.is_empty());
    }
}
