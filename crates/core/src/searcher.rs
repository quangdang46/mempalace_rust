use crate::constants::DEFAULT_N_RESULTS;
use crate::palace_db::{PalaceDb, QueryResult};
use anyhow::Context;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub content: String,
    pub score: f64,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub hall: Option<String>,
    pub source_file: Option<String>,
}

impl From<QueryResult> for SearchResult {
    fn from(qr: QueryResult) -> Self {
        let meta = qr.metadatas.into_iter().next().unwrap_or_default();
        Self {
            id: qr.ids.into_iter().next().unwrap_or_default(),
            content: qr.documents.into_iter().next().unwrap_or_default(),
            score: qr.distances.into_iter().next().unwrap_or(1.0),
            wing: meta.get("wing").and_then(|v| v.as_str().map(String::from)),
            room: meta.get("room").and_then(|v| v.as_str().map(String::from)),
            hall: meta.get("hall").and_then(|v| v.as_str().map(String::from)),
            source_file: meta
                .get("source_file")
                .and_then(|v| v.as_str().map(String::from)),
        }
    }
}

#[derive(Debug)]
pub struct SearchResponse {
    pub query: String,
    pub wing_filter: Option<String>,
    pub room_filter: Option<String>,
    pub results: Vec<SearchResult>,
}

pub async fn search_memories(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    embedding_model: Option<&str>,
) -> anyhow::Result<SearchResponse> {
    let palace_db = PalaceDb::open(palace_path).context("Failed to open palace database")?;

    let results = palace_db
        .query(query, wing, room, n_results)
        .await
        .context("Search query failed")?;

    let search_results: Vec<SearchResult> = results.into_iter().map(SearchResult::from).collect();

    Ok(SearchResponse {
        query: query.to_string(),
        wing_filter: wing.map(String::from),
        room_filter: room.map(String::from),
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
        search_memories(query, palace_path, wing, room, n_results, embedding_model).await?;

    if response.results.is_empty() {
        eprintln!("\n  No results found for: \"{}\"", query);
        return Ok(1);
    }

    println!("\n{}", "=".repeat(60));
    println!("  Results for: \"{}\"", query);
    if let Some(ref w) = response.wing_filter {
        println!("  Wing: {}", w);
    }
    if let Some(ref r) = response.room_filter {
        println!("  Room: {}", r);
    }
    println!("{}", "=".repeat(60));
    println!();

    for (i, result) in response.results.iter().enumerate() {
        let similarity = compute_similarity(result.score);
        let source = result.source_file.as_deref().unwrap_or("?");
        let wing_name = result.wing.as_deref().unwrap_or("?");
        let room_name = result.room.as_deref().unwrap_or("?");

        println!("  [{}] {} / {}", i + 1, wing_name, room_name);
        println!("      Source: {}", source);
        println!("      Match:  {:.3}", similarity);
        println!();

        for line in result.content.trim().lines() {
            println!("      {}", line);
        }
        println!();
        println!("  {}", "-".repeat(56));
    }

    println!();
    Ok(0)
}

pub async fn check_duplicate(
    content: &str,
    palace_path: &Path,
    threshold: f64,
) -> anyhow::Result<Option<String>> {
    let palace_db = PalaceDb::open(palace_path).context("Failed to open palace database")?;

    let results = palace_db
        .query(content, None, None, 1)
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
}
