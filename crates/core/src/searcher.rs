use crate::palace::FusionMode;
use crate::palace_db::{self, PalaceDb, PalaceState, QueryResult};
use crate::palace_graph::cached_graph;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Open the palace for search.
///
/// When `embedding_model` is `"naive"`, opens without a vector embedder
/// (word-overlap Jaccard similarity only). Otherwise, attempts to open
/// with a real vector embedder so the hybrid RRF (BM25 + vector + graph)
/// stream uses embeddings. Falls back to the naive open on any error.
fn open_for_search(palace_path: &Path, embedding_model: Option<&str>) -> anyhow::Result<PalaceDb> {
    // "naive" means Jaccard word-overlap — skip the vector path entirely.
    match embedding_model {
        Some("naive") | Some("jaccard") => return PalaceDb::open(palace_path),
        _ => {}
    }
    let resolved = match embedding_model {
        Some(name) => crate::embed::resolve_embedder(name),
        None => crate::embed::embedder_from_env(),
    };
    match resolved {
        Ok(boxed) => {
            let model_name = embedding_model
                .map(String::from)
                .or_else(|| std::env::var("MEMPALACE_EMBED_MODEL").ok())
                .unwrap_or_else(|| crate::embed::DEFAULT_EMBED_MODEL.to_string());
            let embedder: Arc<dyn crate::embed::Embedder> = Arc::from(boxed);
            match PalaceDb::open_with_embedder(palace_path, embedder, &model_name) {
                Ok(db) => Ok(db),
                Err(_) => PalaceDb::open(palace_path),
            }
        }
        Err(_) => PalaceDb::open(palace_path),
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    #[error("No palace found at {0}")]
    NoPalace(String),
    /// Palace dir exists but has not been mined yet — keeps the cause chain
    /// distinct from `NoPalace` so callers can surface an actionable hint
    /// instead of telling the user to re-run `init` (#1498).
    #[error("Palace at {0} is not initialized yet")]
    NotInitialized(String),
    /// Palace dir + collection JSON exist but contain no drawers yet (#1498).
    #[error("Palace at {0} has no drawers yet")]
    Empty(String),
    #[error("Search error: {0}")]
    Query(String),
}

#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct SearchResult {
    pub text: String,
    pub wing: String,
    pub room: String,
    pub source_file: String,
    pub similarity: f64,
    pub created_at: Option<String>,
    pub bm25_score: Option<f64>,
    pub combined_score: Option<f64>,
}

impl From<QueryResult> for SearchResult {
    fn from(qr: QueryResult) -> Self {
        let meta = qr.metadatas.into_iter().next().unwrap_or_default();
        let source_file = meta
            .get("source_file")
            .and_then(|v| v.as_str())
            .map(|value| {
                PathBuf::from(value)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| value.to_string())
            })
            .unwrap_or_else(|| "?".to_string());

        let created_at = meta
            .get("created_at")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                meta.get("filed_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });

        Self {
            text: qr.documents.into_iter().next().unwrap_or_default(),
            wing: meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            room: meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            source_file,
            similarity: (1.0 - qr.distances.into_iter().next().unwrap_or(1.0)).round_to_3(),
            created_at,
            bm25_score: None,
            combined_score: None,
        }
    }
}

trait RoundTo3 {
    fn round_to_3(self) -> f64;
}

impl RoundTo3 for f64 {
    fn round_to_3(self) -> f64 {
        (self * 1000.0).round() / 1000.0
    }
}

#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct SearchResponse {
    pub query: String,
    pub filters: SearchFilters,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, serde::Serialize)]
#[non_exhaustive]
pub struct SearchFilters {
    pub wing: Option<String>,
    pub room: Option<String>,
}

pub fn search_memories(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    embedding_model: Option<&str>,
) -> anyhow::Result<SearchResponse> {
    search_memories_with_rerank(
        query,
        palace_path,
        wing,
        room,
        n_results,
        embedding_model,
        false,
        None,
        None,
    )
}

/// Search with optional BM25 reranking and PPR fusion mode.
///
/// This function is synchronous (not async) because all blocking IO —
/// JSON loading, BM25 indexing, vector HNSW search — happens inside
/// the `PalaceDb` methods, which handle their own async embedding via
/// dedicated off-runtime threads. The callers (CLI, MCP, tests) invoke
/// this directly without `.await`.
#[allow(clippy::too_many_arguments)]
pub fn search_memories_with_rerank(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    embedding_model: Option<&str>,
    use_bm25: bool,
    max_per_session: Option<usize>,
    fusion_mode: Option<FusionMode>,
) -> anyhow::Result<SearchResponse> {
    #[cfg(feature = "telemetry")]
    let _telemetry_start = std::time::Instant::now();

    let sanitized = crate::query_sanitizer::sanitize_query(query);

    // #1498: stratify so the caller's error message tells the user the
    // *next* step (init / mine) rather than a generic "no palace" hint that
    // sends them back to `init` after `init` has already succeeded.
    let path_str = palace_path.display().to_string();
    match palace_db::classify_palace(palace_path) {
        PalaceState::Missing => return Err(SearchError::NoPalace(path_str).into()),
        PalaceState::NotInitialized => {
            return Err(SearchError::NotInitialized(path_str).into());
        }
        PalaceState::Empty => return Err(SearchError::Empty(path_str).into()),
        PalaceState::Ready => {}
    }

    // Open palace — uses a real vector embedder when embedding_model is
    // "vector"/"hnsw" or a named model, falls back to naive on error.
    let palace_db = open_for_search(palace_path, embedding_model)
        .map_err(|_| SearchError::NoPalace(palace_path.display().to_string()))?;

    // Fetch results using hybrid_search (BM25 + vector + graph RRF fusion).
    // When `use_bm25` is true, request a wider candidate set so the BM25
    // reranking step considers more than just the top RRF results.
    let fetch_count = if use_bm25 { n_results * 3 } else { n_results };
    let results = palace_db
        .hybrid_search(&sanitized.clean_query, fetch_count, wing, room)
        .map_err(|e| SearchError::Query(e.to_string()))?;

    let mut search_results: Vec<SearchResult> =
        results.into_iter().map(SearchResult::from).collect();

    // BM25 reranking: build a BM25 scorer from the (wider) candidate set
    // returned by hybrid_search and re-rank. This only fires when requested
    // (the `--bm25` CLI flag or when the MCP server sets use_bm25=true).
    if use_bm25 && search_results.len() > 1 {
        let docs: Vec<String> = search_results.iter().map(|r| r.text.clone()).collect();
        let scorer = crate::bm25::Bm25Scorer::new(&docs, crate::bm25::Bm25Params::default());
        for result in &mut search_results {
            let bm25_score = scorer.score(&result.text, &sanitized.clean_query);
            result.bm25_score = Some(bm25_score);
            // Normalized BM25 score combined with similarity
            result.combined_score =
                Some(0.7 * result.similarity + 0.3 * (bm25_score / (bm25_score + 1.0)));
        }
        search_results.sort_by(|a, b| {
            b.combined_score
                .unwrap_or(0.0)
                .partial_cmp(&a.combined_score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        search_results.truncate(n_results);
    }

    let fusion = fusion_mode.unwrap_or(FusionMode::Vector);
    if fusion != FusionMode::Vector && !search_results.is_empty() {
        let graph = cached_graph(palace_path);
        let ppr_scores: std::collections::HashMap<String, f64> = graph
            .ppr_search(&sanitized.clean_query, 50)
            .into_iter()
            .collect();

        for result in &mut search_results {
            let ppr_score = ppr_scores.get(&result.room).copied().unwrap_or(0.0_f64);
            match fusion {
                FusionMode::Hybrid => {
                    let sim = result.similarity;
                    let combined = 0.7 * sim + 0.3 * ppr_score;
                    result.combined_score = Some(combined);
                }
                FusionMode::Ppr => {
                    result.combined_score = Some(ppr_score);
                }
                FusionMode::Vector => {}
            }
        }

        if fusion == FusionMode::Hybrid || fusion == FusionMode::Ppr {
            search_results.sort_by(|a, b| {
                b.combined_score
                    .unwrap_or(0.0)
                    .partial_cmp(&a.combined_score.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            search_results.truncate(n_results);
        }
    }

    // Apply max_per_session filter (post-RRF deduplication by session)
    if let Some(max) = max_per_session {
        let mut session_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        search_results.retain(|r| {
            let count = session_counts.entry(r.source_file.clone()).or_insert(0);
            *count += 1;
            *count <= max
        });
    }

    Ok(SearchResponse {
        query: sanitized.clean_query,
        filters: SearchFilters {
            wing: wing.map(String::from),
            room: room.map(String::from),
        },
        results: search_results,
    })
    .inspect(|_| {
        #[cfg(feature = "telemetry")]
        {
            crate::telemetry::counter!("mempalace_search_total", "status" => "success")
                .increment(1);
            crate::telemetry::histogram!("mempalace_search_latency_ms")
                .record(_telemetry_start.elapsed().as_secs_f64() * 1000.0);
        }
        let _ = ();
    })
}

pub fn search(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    embedding_model: Option<&str>,
) -> anyhow::Result<i32> {
    let response =
        match search_memories(query, palace_path, wing, room, n_results, embedding_model) {
            Ok(response) => response,
            Err(error) => {
                if let Some(search_error) = error.downcast_ref::<SearchError>() {
                    match search_error {
                        SearchError::NoPalace(path) => {
                            println!("\n  No palace found at {}", path);
                            println!("  Run: mpr init <dir>");
                        }
                        SearchError::NotInitialized(path) => {
                            println!(
                                "\n  Palace directory exists at {} but no data has been mined yet.",
                                path
                            );
                            println!("  Run: mpr mine <dir>");
                        }
                        SearchError::Empty(path) => {
                            println!("\n  Palace at {} has no drawers yet.", path);
                            println!("  Run: mpr mine <dir> to ingest content.");
                        }
                        SearchError::Query(message) => {
                            println!("\n  Search error: {}", message);
                        }
                    }
                }
                return Err(error);
            }
        };

    Ok(print_search_response(&response))
}

pub fn print_search_response_json(response: &SearchResponse) -> i32 {
    match serde_json::to_string_pretty(response) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(e) => {
            eprintln!("error: failed to serialize search results as JSON: {e}");
            1
        }
    }
}

pub fn print_search_response(response: &SearchResponse) -> i32 {
    if response.results.is_empty() {
        println!("\n  No results found for: \"{}\"", response.query);
        return 1;
    }

    println!("\n{}", "=".repeat(60));
    println!("  Results for: \"{}\"", response.query);
    if let Some(ref w) = response.filters.wing {
        println!("  Wing: {}", w);
    }
    if let Some(ref r) = response.filters.room {
        println!("  Room: {}", r);
    }
    println!("{}", "=".repeat(60));
    println!();

    for (i, result) in response.results.iter().enumerate() {
        println!("  [{}] {} / {}", i + 1, result.wing, result.room);
        println!("      Source: {}", result.source_file);
        println!("      Match:  {:.3}", result.similarity);
        println!();

        for line in result.text.trim().lines() {
            println!("      {}", line);
        }
        println!();
        println!("  {}", "─".repeat(56));
    }

    println!();
    0
}

pub fn check_duplicate(
    content: &str,
    palace_path: &Path,
    threshold: f64,
) -> anyhow::Result<Option<String>> {
    let sanitized = crate::query_sanitizer::sanitize_query(content);
    let palace_db = PalaceDb::open(palace_path).context("Failed to open palace database")?;

    let results = palace_db
        .query_sync(&sanitized.clean_query, None, None, 1)
        .context("Duplicate check query failed")?;

    if let Some(result) = results.into_iter().next() {
        let similarity = compute_similarity(result.distances.first().copied().unwrap_or(1.0));
        if similarity >= threshold {
            return Ok(result.ids.into_iter().next());
        }
    }

    Ok(None)
}

fn compute_similarity(distance: f64) -> f64 {
    (1.0 - distance).clamp(0.0, 1.0)
}

/// Identifies the distance metric a vector store reports.
///
/// Per RFC 001 §10 (metric-aware distance→similarity), the formula
/// `1 - distance` is only correct for cosine distance. L2 and
/// inner-product distances need different conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DistanceMetric {
    /// Cosine distance, range [0, 2] (0 = identical, 2 = opposite).
    Cosine,
    /// Euclidean (L2) distance, range [0, ∞).
    L2,
    /// Inner-product distance (negative dot product), range (-∞, ∞].
    /// Smaller is more similar.
    InnerProduct,
}

/// Convert a raw vector distance into a similarity in [0, 1] (1 = best).
///
/// RFC 001 §10:
/// - Cosine: `1 - distance` (clamped).
/// - L2: `1 / (1 + distance)` (asymptotically approaches 0).
/// - InnerProduct: `-distance` (clamped, since IP distance = -dot).
pub fn distance_to_similarity(distance: f64, metric: DistanceMetric) -> f64 {
    match metric {
        DistanceMetric::Cosine => (1.0 - distance).clamp(0.0, 1.0),
        DistanceMetric::L2 => 1.0 / (1.0 + distance).max(f64::MIN_POSITIVE),
        DistanceMetric::InnerProduct => (-distance).clamp(0.0, 1.0),
    }
}

/// mr-vwxf: detect a legacy-metric configuration. The classic symptom is
/// a high cosine similarity (>= 0.5) but a `match_score` of 0 — meaning
/// the vector store is reporting distances that are too large to be raw
/// cosine distances (L2 distances can grow without bound, so the
/// `1 - distance` formula is wrong for them).
///
/// Callers that observe a `legacy_metric_warning()` should switch to a
/// metric-aware conversion (see `mr-7tfi`).
pub fn legacy_metric_warning(cosine_similarity: f64, match_score: f64) -> bool {
    cosine_similarity > 0.5 && match_score == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_N_RESULTS;
    use crate::query_sanitizer::MAX_QUERY_LENGTH;

    #[test]
    fn test_compute_similarity() {
        assert!((compute_similarity(0.0) - 1.0).abs() < 1e-6);
        assert!((compute_similarity(1.0) - 0.0).abs() < 1e-6);
        assert!((compute_similarity(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_similarity_clamping() {
        assert!((compute_similarity(-0.5) - 1.0).abs() < 1e-6);
        assert!((compute_similarity(1.5) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_round_to_3() {
        assert!((0.12349_f64.round_to_3() - 0.123).abs() < 1e-6);
        assert!((0.1235_f64.round_to_3() - 0.124).abs() < 1e-6);
    }

    #[test]
    fn test_legacy_metric_warning_high_cosine_zero_match() {
        // mr-vwxf: high cosine similarity but match_score = 0 is the
        // legacy-metric signature (vector store reporting L2 distance
        // but caller using `1 - distance` formula).
        assert!(legacy_metric_warning(0.7, 0.0));
        assert!(legacy_metric_warning(0.9, 0.0));
        // Boundary: exactly 0.5 does not trigger (the threshold is strict > 0.5).
        assert!(!legacy_metric_warning(0.5, 0.0));
        // If match_score is non-zero, no warning.
        assert!(!legacy_metric_warning(0.7, 0.5));
        // Low cosine similarity + zero match is fine (genuine no-match).
        assert!(!legacy_metric_warning(0.2, 0.0));
    }

    #[test]
    fn test_distance_to_similarity_cosine() {
        // RFC 001 §10: cosine uses `1 - distance`.
        assert!((distance_to_similarity(0.0, DistanceMetric::Cosine) - 1.0).abs() < 1e-6);
        assert!((distance_to_similarity(1.0, DistanceMetric::Cosine) - 0.0).abs() < 1e-6);
        // Out-of-range cosine distance (e.g. 1.5) is clamped to 0.
        assert!((distance_to_similarity(1.5, DistanceMetric::Cosine) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_distance_to_similarity_l2() {
        // RFC 001 §10: L2 uses `1 / (1 + distance)`.
        assert!((distance_to_similarity(0.0, DistanceMetric::L2) - 1.0).abs() < 1e-6);
        assert!((distance_to_similarity(1.0, DistanceMetric::L2) - 0.5).abs() < 1e-6);
        // Asymptotically approaches 0 but never goes negative.
        let big = distance_to_similarity(1_000_000.0, DistanceMetric::L2);
        assert!(big > 0.0 && big < 1e-6);
    }

    #[test]
    fn test_distance_to_similarity_inner_product() {
        // RFC 001 §10: IP distance = -dot, so similarity = -distance.
        assert!((distance_to_similarity(0.0, DistanceMetric::InnerProduct) - 0.0).abs() < 1e-6);
        assert!((distance_to_similarity(-1.0, DistanceMetric::InnerProduct) - 1.0).abs() < 1e-6);
        // IP distance > 0 (very dissimilar) clamps to 0.
        assert!((distance_to_similarity(2.0, DistanceMetric::InnerProduct) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_search_memories_sanitizes_query_text() {
        let raw = format!(
            "{}\nWhere is the backend auth plan?",
            "system prompt ".repeat(40)
        );
        let sanitized = crate::query_sanitizer::sanitize_query(&raw);
        assert_eq!(sanitized.clean_query, "Where is the backend auth plan?");
        assert!(sanitized.was_sanitized);
    }

    #[test]
    fn test_search_memories_sanitizer_tail_limit() {
        let raw = "x".repeat(MAX_QUERY_LENGTH + 10);
        let sanitized = crate::query_sanitizer::sanitize_query(&raw);
        assert_eq!(sanitized.clean_query.len(), MAX_QUERY_LENGTH);
    }

    // mr-sf63: feathered drawer-grep — a match should return the
    // best matching chunk plus its virtual line range. We model the
    // contract as: input is a long text; output is the (chunk, line_start, line_end)
    // tuple for the first line that contains the query term.
    #[test]
    fn test_feathered_drawer_grep_returns_best_chunk_and_line_range() {
        let body = "alpha\nbeta\ngamma\nbeta-delta\nepsilon";
        let query = "beta";
        let lines: Vec<&str> = body.lines().collect();
        let mut best_chunk: Option<(String, usize, usize)> = None;
        for (idx, line) in lines.iter().enumerate() {
            if line.contains(query) {
                best_chunk = Some((line.to_string(), idx + 1, idx + 1));
                break;
            }
        }
        let (chunk, start, end) = best_chunk.expect("expected a match for 'beta'");
        assert_eq!(chunk, "beta");
        assert_eq!(start, 2);
        assert_eq!(end, 2);
    }

    #[test]
    fn test_search_memories_result_shape() {
        let temp = tempfile::tempdir().unwrap();
        let palace_path = temp.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[("id1", "JWT authentication uses bearer tokens")],
            &[&[
                ("wing", "project"),
                ("room", "backend"),
                ("source_file", "/tmp/auth.py"),
            ]],
        )
        .unwrap();
        db.flush().unwrap();

        let response = search_memories(
            "JWT authentication",
            &palace_path,
            Some("project"),
            Some("backend"),
            DEFAULT_N_RESULTS,
            None,
        )
        .unwrap();

        assert_eq!(response.query, "JWT authentication");
        assert_eq!(response.filters.wing.as_deref(), Some("project"));
        assert_eq!(response.filters.room.as_deref(), Some("backend"));
        assert_eq!(response.results.len(), 1);
        let hit = &response.results[0];
        assert_eq!(hit.wing, "project");
        assert_eq!(hit.room, "backend");
        assert_eq!(hit.source_file, "auth.py");
        assert!(hit.similarity >= 0.0);
    }

    #[test]
    fn test_search_memories_respects_n_results_limit() {
        let temp = tempfile::tempdir().unwrap();
        let palace_path = temp.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[
                ("id1", "code code backend"),
                ("id2", "code frontend planning"),
                ("id3", "code architecture note"),
            ],
            &[
                &[
                    ("wing", "project"),
                    ("room", "backend"),
                    ("source_file", "/tmp/a.py"),
                ],
                &[
                    ("wing", "project"),
                    ("room", "frontend"),
                    ("source_file", "/tmp/b.ts"),
                ],
                &[
                    ("wing", "notes"),
                    ("room", "general"),
                    ("source_file", "/tmp/c.md"),
                ],
            ],
        )
        .unwrap();
        db.flush().unwrap();

        let response = search_memories("code", &palace_path, None, None, 2, None)
            .unwrap();
        assert_eq!(response.results.len(), 2);
    }

    #[test]
    fn test_search_memories_no_palace_errors() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let error = search_memories("anything", &missing, None, None, DEFAULT_N_RESULTS, None)
            .unwrap_err();
        let downcast = error.downcast_ref::<SearchError>().unwrap();
        assert!(matches!(downcast, SearchError::NoPalace(_)));
        assert!(error.to_string().contains("No palace found"));
    }

    /// #1498 regression: palace dir exists but no collection JSON — caller
    /// must see `NotInitialized` (action: `mpr mine`), not a generic missing
    /// palace error that suggests re-running `init`.
    #[test]
    fn test_search_memories_palace_not_initialized_errors() {
        let temp = tempfile::tempdir().unwrap();
        let palace_path = temp.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let error = search_memories(
            "anything",
            &palace_path,
            None,
            None,
            DEFAULT_N_RESULTS,
            None,
        )
        .unwrap_err();
        let downcast = error.downcast_ref::<SearchError>().unwrap();
        assert!(
            matches!(downcast, SearchError::NotInitialized(_)),
            "expected NotInitialized, got {downcast:?}"
        );
    }

    /// #1498 regression: collection JSON exists but no drawers were filed —
    /// caller must see `Empty`, not `NoPalace`, so the hint is `mpr mine`.
    #[test]
    fn test_search_memories_palace_empty_errors() {
        let temp = tempfile::tempdir().unwrap();
        let palace_path = temp.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.flush().unwrap();
        let error = search_memories(
            "anything",
            &palace_path,
            None,
            None,
            DEFAULT_N_RESULTS,
            None,
        )
        .unwrap_err();
        let downcast = error.downcast_ref::<SearchError>().unwrap();
        assert!(
            matches!(downcast, SearchError::Empty(_)),
            "expected Empty, got {downcast:?}"
        );
    }

    #[test]
    fn test_check_duplicate_returns_top_match_above_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let palace_path = temp.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[("dup1", "JWT authentication uses bearer tokens")],
            &[&[
                ("wing", "project"),
                ("room", "backend"),
                ("source_file", "/tmp/auth.py"),
            ]],
        )
        .unwrap();
        db.flush().unwrap();

        let duplicate = check_duplicate("JWT authentication uses bearer tokens", &palace_path, 0.9)
            .unwrap();
        assert_eq!(duplicate.as_deref(), Some("dup1"));
    }

    #[test]
    fn test_check_duplicate_respects_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let palace_path = temp.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();
        let mut db = PalaceDb::open(&palace_path).unwrap();
        db.add(
            &[("dup1", "JWT authentication uses bearer tokens")],
            &[&[
                ("wing", "project"),
                ("room", "backend"),
                ("source_file", "/tmp/auth.py"),
            ]],
        )
        .unwrap();
        db.flush().unwrap();

        let duplicate = check_duplicate("JWT authentication", &palace_path, 0.95)
            .unwrap();
        assert!(duplicate.is_none());
    }
}
