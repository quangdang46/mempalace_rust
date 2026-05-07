use crate::palace_db::{PalaceDb, QueryResult};
use crate::bm25::{Bm25Scorer, Bm25Params};
use anyhow::Context;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("No palace found at {0}")]
    NoPalace(String),
    #[error("Search error: {0}")]
    Query(String),
}

#[derive(Debug, Clone, serde::Serialize)]
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
pub struct SearchResponse {
    pub query: String,
    pub filters: SearchFilters,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchFilters {
    pub wing: Option<String>,
    pub room: Option<String>,
}

pub async fn search_memories(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    _embedding_model: Option<&str>,
) -> anyhow::Result<SearchResponse> {
    search_memories_with_rerank(query, palace_path, wing, room, n_results, _embedding_model, false).await
}

/// Search with optional BM25 reranking.
pub async fn search_memories_with_rerank(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    _embedding_model: Option<&str>,
    use_bm25: bool,
) -> anyhow::Result<SearchResponse> {
    let sanitized = crate::query_sanitizer::sanitize_query(query);

    if !palace_path.exists() {
        return Err(SearchError::NoPalace(palace_path.display().to_string()).into());
    }

    let palace_db = PalaceDb::open(palace_path)
        .map_err(|_| SearchError::NoPalace(palace_path.display().to_string()))?;

    // Fetch more results for reranking (3x requested)
    let fetch_count = if use_bm25 { n_results * 3 } else { n_results };
    let results = palace_db
        .query(&sanitized.clean_query, wing, room, fetch_count)
        .await
        .map_err(|e| SearchError::Query(e.to_string()))?;

    let mut search_results: Vec<SearchResult> = results.into_iter().map(SearchResult::from).collect();

    if use_bm25 && !search_results.is_empty() {
        // Extract documents for BM25 scoring
        let documents: Vec<String> = search_results.iter().map(|r| r.text.clone()).collect();
        
        // Create BM25 scorer
        let scorer = Bm25Scorer::new(&documents, Bm25Params::default());
        
        // Calculate BM25 scores for each result
        for result in &mut search_results {
            let bm25_score = scorer.score(&result.text, &sanitized.clean_query);
            result.bm25_score = Some(bm25_score);
            
            // Combine scores: 70% similarity, 30% BM25 (weighted combination)
            result.combined_score = Some(0.7 * result.similarity + 0.3 * (bm25_score / (bm25_score + 1.0)));
        }
        
        // Sort by combined score
        search_results.sort_by(|a, b| {
            b.combined_score
                .unwrap_or(0.0)
                .partial_cmp(&a.combined_score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        
        // Truncate to requested count
        search_results.truncate(n_results);
    }

    Ok(SearchResponse {
        query: sanitized.clean_query,
        filters: SearchFilters {
            wing: wing.map(String::from),
            room: room.map(String::from),
        },
        results: search_results,
    })
}

pub async fn search(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    embedding_model: Option<&str>,
) -> anyhow::Result<i32> {
    let response =
        match search_memories(query, palace_path, wing, room, n_results, embedding_model).await {
            Ok(response) => response,
            Err(error) => {
                if let Some(search_error) = error.downcast_ref::<SearchError>() {
                    match search_error {
                        SearchError::NoPalace(path) => {
                            println!("\n  No palace found at {}", path);
                            println!("  Run: mempalace init <dir> then mempalace mine <dir>");
                        }
                        SearchError::Query(message) => {
                            println!("\n  Search error: {}", message);
                        }
                    }
                }
                return Err(error);
            }
        };

    if response.results.is_empty() {
        println!("\n  No results found for: \"{}\"", query);
        return Ok(1);
    }

    println!("\n{}", "=".repeat(60));
    println!("  Results for: \"{}\"", query);
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
    Ok(0)
}

pub async fn check_duplicate(
    content: &str,
    palace_path: &Path,
    threshold: f64,
) -> anyhow::Result<Option<String>> {
    let sanitized = crate::query_sanitizer::sanitize_query(content);
    let palace_db = PalaceDb::open(palace_path).context("Failed to open palace database")?;

    let results = palace_db
        .query(&sanitized.clean_query, None, None, 1)
        .await
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

    #[tokio::test]
    async fn test_search_memories_result_shape() {
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
        .await
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

    #[tokio::test]
    async fn test_search_memories_respects_n_results_limit() {
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
            .await
            .unwrap();
        assert_eq!(response.results.len(), 2);
    }

    #[tokio::test]
    async fn test_search_memories_no_palace_errors() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let error = search_memories("anything", &missing, None, None, DEFAULT_N_RESULTS, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("No palace found"));
    }

    #[tokio::test]
    async fn test_check_duplicate_returns_top_match_above_threshold() {
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
            .await
            .unwrap();
        assert_eq!(duplicate.as_deref(), Some("dup1"));
    }

    #[tokio::test]
    async fn test_check_duplicate_respects_threshold() {
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
            .await
            .unwrap();
        assert!(duplicate.is_none());
    }
}
