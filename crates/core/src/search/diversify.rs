/// Session-based diversification of search results.
///
/// Ported from mempalace's diversifyBySession:
/// - Groups results by session_id
/// - Limits results per session (default: 3)
/// - Fallback fill: if selected < limit, add remaining items regardless of session
use serde::{Deserialize, Serialize};

/// Input result for diversification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiversifiableResult {
    pub id: String,
    pub session_id: String,
    pub score: f64,
}

/// Diversify results by session, limiting items per session.
///
/// Algorithm:
/// 1. Iterate results in score-sorted order (assumed pre-sorted)
/// 2. Track session counts; skip sessions that hit max_per_session
/// 3. Fill up to limit with selected items
/// 4. Fallback: if selected < limit, add remaining items (session diversity exhausted)
pub fn diversify_by_session(
    results: &[DiversifiableResult],
    limit: usize,
    max_per_session: usize,
) -> Vec<DiversifiableResult> {
    let mut session_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut selected: Vec<DiversifiableResult> = Vec::new();
    let mut skipped: Vec<&DiversifiableResult> = Vec::new();

    for result in results {
        if selected.len() >= limit {
            break;
        }

        let count = session_counts.entry(result.session_id.clone()).or_insert(0);
        if *count >= max_per_session {
            skipped.push(result);
        } else {
            *count += 1;
            selected.push(result.clone());
        }
    }

    // Fallback fill: add skipped items if we haven't reached limit
    for result in skipped {
        if selected.len() >= limit {
            break;
        }
        selected.push(result.clone());
    }

    selected
}

/// Default max results per session.
pub const DEFAULT_MAX_PER_SESSION: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(id: &str, session: &str, score: f64) -> DiversifiableResult {
        DiversifiableResult {
            id: id.to_string(),
            session_id: session.to_string(),
            score,
        }
    }

    #[test]
    fn test_diversify_limits_per_session() {
        let results = vec![
            make_result("a1", "s1", 0.9),
            make_result("a2", "s1", 0.8),
            make_result("a3", "s1", 0.7),
            make_result("a4", "s1", 0.6),
            make_result("b1", "s2", 0.5),
        ];

        let diversified = diversify_by_session(&results, 10, 2);
        // First pass: a1, a2 (s1 capped at 2), b1 → 3 selected
        // Fallback: a3, a4 added → total 5
        assert_eq!(diversified.len(), 5);
        // But first 2 from s1 are the highest-scoring ones
        assert_eq!(diversified[0].id, "a1");
        assert_eq!(diversified[1].id, "a2");
    }

    #[test]
    fn test_diversify_respects_limit() {
        let results = vec![
            make_result("a1", "s1", 0.9),
            make_result("b1", "s2", 0.8),
            make_result("c1", "s3", 0.7),
        ];

        let diversified = diversify_by_session(&results, 2, 3);
        assert_eq!(diversified.len(), 2);
    }

    #[test]
    fn test_diversify_fallback_fill() {
        let results = vec![
            make_result("a1", "s1", 0.9),
            make_result("a2", "s1", 0.8),
            make_result("a3", "s1", 0.7),
        ];

        // max_per_session=1, limit=10 → should get all 3 via fallback
        let diversified = diversify_by_session(&results, 10, 1);
        assert_eq!(diversified.len(), 3);
    }

    #[test]
    fn test_diversify_multiple_sessions() {
        let results = vec![
            make_result("a1", "s1", 0.9),
            make_result("b1", "s2", 0.85),
            make_result("a2", "s1", 0.8),
            make_result("b2", "s2", 0.75),
            make_result("c1", "s3", 0.7),
        ];

        let diversified = diversify_by_session(&results, 10, 2);
        assert_eq!(diversified.len(), 5);
        let s1_count = diversified.iter().filter(|r| r.session_id == "s1").count();
        let s2_count = diversified.iter().filter(|r| r.session_id == "s2").count();
        assert_eq!(s1_count, 2);
        assert_eq!(s2_count, 2);
    }

    #[test]
    fn test_diversify_empty_results() {
        let results: Vec<DiversifiableResult> = vec![];
        let diversified = diversify_by_session(&results, 10, 3);
        assert!(diversified.is_empty());
    }

    #[test]
    fn test_diversify_single_session() {
        let results = vec![make_result("a1", "s1", 0.9), make_result("a2", "s1", 0.8)];

        let diversified = diversify_by_session(&results, 5, 3);
        assert_eq!(diversified.len(), 2);
    }
}
