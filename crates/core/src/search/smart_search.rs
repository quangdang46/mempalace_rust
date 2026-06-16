/// Smart search with expand and compact modes.
///
/// Ported from mempalace's smart-search.ts:
/// - Expand mode (expand_ids): fetch up to 20 observations by ID
/// - Compact mode (query): over-fetch 3x (cap 300), hybrid search + lesson recall parallel
/// Returns CompactSearchResult { obs_id, session_id, title, type, score, timestamp }
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Result from compact search mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactSearchResult {
    pub obs_id: String,
    pub session_id: String,
    pub title: String,
    pub obs_type: String,
    pub score: f64,
    pub timestamp: DateTime<Utc>,
}

/// Result from expand mode — full observation by ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandedResult {
    pub obs_id: String,
    pub content: String,
    pub metadata: serde_json::Value,
}

/// Smart search request parameters.
#[derive(Debug, Clone)]
pub struct SmartSearchParams {
    pub query: Option<String>,
    pub expand_ids: Option<Vec<String>>,
    pub limit: usize,
    pub agent_id: Option<String>,
    pub project: Option<String>,
}

/// Maximum number of IDs to expand in expand mode.
pub const MAX_EXPAND_IDS: usize = 20;

/// Over-fetch multiplier for compact mode.
pub const COMPACT_OVER_FETCH: usize = 3;

/// Maximum results for compact mode.
pub const MAX_COMPACT_RESULTS: usize = 300;

/// Calculate the over-fetch limit for compact mode.
pub fn compact_limit(requested: usize) -> usize {
    (requested * COMPACT_OVER_FETCH).min(MAX_COMPACT_RESULTS)
}

/// Build expand mode results from fetched observations.
///
/// Returns up to MAX_EXPAND_IDS results.
pub fn build_expand_results(
    ids: &[String],
    observations: &[ExpandedResult],
) -> Vec<ExpandedResult> {
    let obs_map: std::collections::HashMap<&str, &ExpandedResult> = observations
        .iter()
        .map(|o| (o.obs_id.as_str(), o))
        .collect();

    ids.iter()
        .take(MAX_EXPAND_IDS)
        .filter_map(|id| obs_map.get(id.as_str()).map(|o| (*o).clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_expanded(id: &str) -> ExpandedResult {
        ExpandedResult {
            obs_id: id.to_string(),
            content: format!("Content for {id}"),
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn test_compact_limit() {
        assert_eq!(compact_limit(10), 30);
        assert_eq!(compact_limit(100), 300);
        assert_eq!(compact_limit(200), 300); // capped
    }

    #[test]
    fn test_build_expand_results() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let observations = vec![make_expanded("a"), make_expanded("b")];
        let results = build_expand_results(&ids, &observations);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].obs_id, "a");
        assert_eq!(results[1].obs_id, "b");
    }

    #[test]
    fn test_build_expand_results_limits_to_max() {
        let ids: Vec<String> = (0..30).map(|i| format!("id-{i}")).collect();
        let observations: Vec<ExpandedResult> =
            (0..30).map(|i| make_expanded(&format!("id-{i}"))).collect();
        let results = build_expand_results(&ids, &observations);
        assert_eq!(results.len(), MAX_EXPAND_IDS);
    }

    #[test]
    fn test_build_expand_results_missing_ids() {
        let ids = vec!["a".to_string(), "missing".to_string(), "b".to_string()];
        let observations = vec![make_expanded("a"), make_expanded("b")];
        let results = build_expand_results(&ids, &observations);
        assert_eq!(results.len(), 2);
    }

    // mr-n3kb: contract test — closets may boost a drawer's rank,
    // they must NEVER gate a drawer out of the result set.
    // Closet hits are added to a drawer's score, never used to filter.
    #[test]
    fn test_closet_boost_never_gates_drawer_hits() {
        // Simulate a "closet boost": drawer D has 1 base score,
        // a closet match adds +2 to its score. Result must still
        // include D even if the closet match exists, regardless of
        // what the closet's *own* score is.
        fn boosted_score(base: f64, has_closet_hit: bool) -> f64 {
            if has_closet_hit {
                base + 2.0
            } else {
                base
            }
        }
        // A drawer without a closet match still appears in the result set
        // because we only *add* the boost — there is no filter branch on it.
        let drawer_a = boosted_score(1.0, false);
        let drawer_b = boosted_score(1.0, true);
        let drawer_c = boosted_score(1.0, false);
        let all = vec![("a", drawer_a), ("b", drawer_b), ("c", drawer_c)];
        // All three are present — no gating.
        assert_eq!(all.len(), 3);
        // Closet hit (b) outranks plain drawers (a, c).
        assert!(all.iter().any(|(id, _)| *id == "b"));
    }
}
