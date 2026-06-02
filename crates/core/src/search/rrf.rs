/// Reciprocal Rank Fusion (RRF) for combining multiple search result streams.
///
/// Ported from agentmemory's hybrid-search.ts RRF implementation:
/// - RRF_K = 60
/// - combined = W_bm25 * (1/(RRF_K + bm25_rank)) + W_vector * (1/(RRF_K + vector_rank)) + W_graph * (1/(RRF_K + graph_rank))
/// - Default weights: bm25=0.4, vector=0.6, graph=0.3
/// - Weight normalization: zero out missing streams, normalize to sum=1.0
use serde::{Deserialize, Serialize};

use super::synonyms::SYNONYM_BM25_WEIGHT;

/// RRF constant — controls how much rank position affects score.
/// Higher K = flatter curve (less rank sensitivity).
pub const RRF_K: f64 = 60.0;

/// Default weights for each search stream.
pub const DEFAULT_BM25_WEIGHT: f64 = 0.4;
pub const DEFAULT_VECTOR_WEIGHT: f64 = 0.6;
pub const DEFAULT_GRAPH_WEIGHT: f64 = 0.3;

/// A single search result from one stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamResult {
    pub id: String,
    pub rank: usize,
    pub stream: SearchStream,
}

/// Identifies which search stream produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SearchStream {
    Bm25,
    Vector,
    Graph,
}

/// Fused result after RRF combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusedResult {
    pub id: String,
    pub combined_score: f64,
    pub bm25_score: f64,
    pub vector_score: f64,
    pub graph_score: f64,
    pub bm25_rank: Option<usize>,
    pub vector_rank: Option<usize>,
    pub graph_rank: Option<usize>,
}

/// Configuration for RRF fusion weights.
#[derive(Debug, Clone)]
pub struct RrfConfig {
    pub bm25_weight: f64,
    pub vector_weight: f64,
    pub graph_weight: f64,
}

impl Default for RrfConfig {
    fn default() -> Self {
        Self {
            bm25_weight: DEFAULT_BM25_WEIGHT,
            vector_weight: DEFAULT_VECTOR_WEIGHT,
            graph_weight: DEFAULT_GRAPH_WEIGHT,
        }
    }
}

impl RrfConfig {
    /// Construct an `RrfConfig` that biases BM25 toward synonym
    /// expansion hits. Uses [`SYNONYM_BM25_WEIGHT`] (0.7) for the BM25
    /// stream so that synonym-matched documents rank competitively
    /// with literal text matches, matching agentmemory's
    /// `hybrid-search.ts` BM25+synonym weighting.
    ///
    /// Vector and graph weights are unchanged from [`Self::default`]
    /// so the new bias is localised to the BM25 stream and existing
    /// callers (e.g. `palace_db::search_memories`) keep their current
    /// vector/graph behaviour.
    pub fn with_synonyms() -> Self {
        Self {
            bm25_weight: SYNONYM_BM25_WEIGHT as f64,
            vector_weight: DEFAULT_VECTOR_WEIGHT,
            graph_weight: DEFAULT_GRAPH_WEIGHT,
        }
    }
}

/// Calculate RRF score contribution from a single stream.
///
/// Formula: 1 / (RRF_K + rank)
pub fn rrf_score(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

/// Normalize weights when some streams are empty.
///
/// If a stream has no results, its weight is zeroed and remaining
/// weights are normalized to sum to 1.0.
pub fn normalize_weights(
    config: &RrfConfig,
    has_bm25: bool,
    has_vector: bool,
    has_graph: bool,
) -> (f64, f64, f64) {
    let mut bm25_w = if has_bm25 { config.bm25_weight } else { 0.0 };
    let mut vector_w = if has_vector {
        config.vector_weight
    } else {
        0.0
    };
    let mut graph_w = if has_graph { config.graph_weight } else { 0.0 };

    let total = bm25_w + vector_w + graph_w;
    if total > 0.0 {
        bm25_w /= total;
        vector_w /= total;
        graph_w /= total;
    }

    (bm25_w, vector_w, graph_w)
}

/// Fuse multiple search result streams using Reciprocal Rank Fusion.
///
/// Results from each stream are merged by ID, then scored using RRF.
/// Returns results sorted by combined score (descending).
pub fn fuse_results(
    bm25_results: &[StreamResult],
    vector_results: &[StreamResult],
    graph_results: &[StreamResult],
    config: &RrfConfig,
) -> Vec<FusedResult> {
    let has_bm25 = !bm25_results.is_empty();
    let has_vector = !vector_results.is_empty();
    let has_graph = !graph_results.is_empty();

    let (bm25_w, vector_w, graph_w) = normalize_weights(config, has_bm25, has_vector, has_graph);

    // Build a map of id -> FusedResult
    let mut fused: std::collections::HashMap<String, FusedResult> =
        std::collections::HashMap::new();

    for result in bm25_results {
        let entry = fused
            .entry(result.id.clone())
            .or_insert_with(|| FusedResult {
                id: result.id.clone(),
                combined_score: 0.0,
                bm25_score: 0.0,
                vector_score: 0.0,
                graph_score: 0.0,
                bm25_rank: None,
                vector_rank: None,
                graph_rank: None,
            });
        let score = rrf_score(result.rank);
        entry.bm25_score = score;
        entry.bm25_rank = Some(result.rank);
        entry.combined_score += bm25_w * score;
    }

    for result in vector_results {
        let entry = fused
            .entry(result.id.clone())
            .or_insert_with(|| FusedResult {
                id: result.id.clone(),
                combined_score: 0.0,
                bm25_score: 0.0,
                vector_score: 0.0,
                graph_score: 0.0,
                bm25_rank: None,
                vector_rank: None,
                graph_rank: None,
            });
        let score = rrf_score(result.rank);
        entry.vector_score = score;
        entry.vector_rank = Some(result.rank);
        entry.combined_score += vector_w * score;
    }

    for result in graph_results {
        let entry = fused
            .entry(result.id.clone())
            .or_insert_with(|| FusedResult {
                id: result.id.clone(),
                combined_score: 0.0,
                bm25_score: 0.0,
                vector_score: 0.0,
                graph_score: 0.0,
                bm25_rank: None,
                vector_rank: None,
                graph_rank: None,
            });
        let score = rrf_score(result.rank);
        entry.graph_score = score;
        entry.graph_rank = Some(result.rank);
        entry.combined_score += graph_w * score;
    }

    let mut results: Vec<FusedResult> = fused.into_values().collect();
    results.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_results(stream: SearchStream, count: usize) -> Vec<StreamResult> {
        (0..count)
            .map(|i| StreamResult {
                id: format!("{stream:?}-{i}"),
                rank: i,
                stream,
            })
            .collect()
    }

    fn make_overlapping_results() -> (Vec<StreamResult>, Vec<StreamResult>, Vec<StreamResult>) {
        // All three streams return the same IDs but in different orders
        let bm25 = vec![
            StreamResult {
                id: "a".to_string(),
                rank: 0,
                stream: SearchStream::Bm25,
            },
            StreamResult {
                id: "b".to_string(),
                rank: 1,
                stream: SearchStream::Bm25,
            },
            StreamResult {
                id: "c".to_string(),
                rank: 2,
                stream: SearchStream::Bm25,
            },
        ];
        let vector = vec![
            StreamResult {
                id: "c".to_string(),
                rank: 0,
                stream: SearchStream::Vector,
            },
            StreamResult {
                id: "a".to_string(),
                rank: 1,
                stream: SearchStream::Vector,
            },
            StreamResult {
                id: "b".to_string(),
                rank: 2,
                stream: SearchStream::Vector,
            },
        ];
        let graph = vec![
            StreamResult {
                id: "b".to_string(),
                rank: 0,
                stream: SearchStream::Graph,
            },
            StreamResult {
                id: "c".to_string(),
                rank: 1,
                stream: SearchStream::Graph,
            },
            StreamResult {
                id: "a".to_string(),
                rank: 2,
                stream: SearchStream::Graph,
            },
        ];
        (bm25, vector, graph)
    }

    #[test]
    fn test_rrf_score_rank0_higher_than_rank1() {
        let s0 = rrf_score(0);
        let s1 = rrf_score(1);
        assert!(s0 > s1);
        assert!((s0 - 1.0 / 60.0).abs() < 0.0001);
    }

    #[test]
    fn test_normalize_weights_all_present() {
        let config = RrfConfig::default();
        let (bm25, vector, graph) = normalize_weights(&config, true, true, true);
        assert!((bm25 - 0.4 / 1.3).abs() < 0.0001);
        assert!((vector - 0.6 / 1.3).abs() < 0.0001);
        assert!((graph - 0.3 / 1.3).abs() < 0.0001);
        assert!((bm25 + vector + graph - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_normalize_weights_vector_missing() {
        let config = RrfConfig::default();
        let (bm25, vector, graph) = normalize_weights(&config, true, false, true);
        assert!(vector < 0.0001);
        assert!((bm25 + graph - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_normalize_weights_all_missing() {
        let config = RrfConfig::default();
        let (bm25, vector, graph) = normalize_weights(&config, false, false, false);
        assert!((bm25 + vector + graph).abs() < 0.0001);
    }

    #[test]
    fn test_fuse_results_overlapping() {
        let (bm25, vector, graph) = make_overlapping_results();
        let config = RrfConfig::default();
        let fused = fuse_results(&bm25, &vector, &graph, &config);

        assert_eq!(fused.len(), 3);
        // All IDs present
        let ids: Vec<&str> = fused.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
        assert!(ids.contains(&"c"));
    }

    #[test]
    fn test_fuse_results_sorted_by_score() {
        let (bm25, vector, graph) = make_overlapping_results();
        let config = RrfConfig::default();
        let fused = fuse_results(&bm25, &vector, &graph, &config);

        // Scores should be descending
        for i in 0..fused.len() - 1 {
            assert!(fused[i].combined_score >= fused[i + 1].combined_score);
        }
    }

    #[test]
    fn test_fuse_results_single_stream() {
        let bm25 = make_results(SearchStream::Bm25, 3);
        let fused = fuse_results(&bm25, &[], &[], &RrfConfig::default());

        assert_eq!(fused.len(), 3);
        // With only BM25, weight normalizes to 1.0
        assert!((fused[0].combined_score - rrf_score(0)).abs() < 0.0001);
    }

    #[test]
    fn test_fuse_results_disjoint_streams() {
        let bm25 = vec![StreamResult {
            id: "x".to_string(),
            rank: 0,
            stream: SearchStream::Bm25,
        }];
        let vector = vec![StreamResult {
            id: "y".to_string(),
            rank: 0,
            stream: SearchStream::Vector,
        }];
        let fused = fuse_results(&bm25, &vector, &[], &RrfConfig::default());

        assert_eq!(fused.len(), 2);
    }
}
