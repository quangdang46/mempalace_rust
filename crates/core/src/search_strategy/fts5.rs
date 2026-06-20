//! FTS5 strategy — in-memory substring matching.
//!
//! Default strategy. 0MB extra, instant, lexical matching.
//! Reads directly from PalaceDb's in-memory documents HashMap instead of
//! opening a separate SQLite connection (no `mempalace.db` in v0.6.0+).
//! Falls back to naive Jaccard search if no hits found.

use super::naive::NaiveJaccardStrategy;
use super::traits::{SearchHit, SearchStrategy};
use crate::palace_db::PalaceDb;
use anyhow::Result;

pub struct Fts5Strategy;

impl Fts5Strategy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Fts5Strategy {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchStrategy for Fts5Strategy {
    fn name(&self) -> &str {
        "fts5"
    }

    fn search(&self, query: &str, db: &PalaceDb, n: usize) -> Result<Vec<SearchHit>> {
        match fts5_search(db, query, n) {
            Ok(hits) if !hits.is_empty() => Ok(hits),
            Ok(_) | Err(_) => {
                NaiveJaccardStrategy::new().search(query, db, n)
            }
        }
    }
}

/// Search in-memory documents with case-insensitive substring matching.
///
/// Each query term is matched as a substring against document content.
/// Results are scored by the number of matching terms and truncated to `n`.
fn fts5_search(db: &PalaceDb, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|s| s.to_string())
        .collect();

    if terms.is_empty() {
        return Ok(vec![]);
    }

    let mut hits: Vec<SearchHit> = db
        .documents()
        .iter()
        .filter_map(|(id, entry)| {
            let content_lower = entry.content.to_lowercase();
            let match_count = terms
                .iter()
                .filter(|t| content_lower.contains(t.as_str()))
                .count();
            if match_count == 0 {
                return None;
            }
            let metadata = if entry.metadata.is_empty() {
                None
            } else {
                serde_json::to_value(&entry.metadata).ok()
            };
            Some(SearchHit {
                id: id.clone(),
                score: match_count as f64,
                metadata,
            })
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(n);
    Ok(hits)
}
