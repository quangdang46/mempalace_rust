//! Naive Jaccard strategy — token overlap linear scan.
//!
//! Slowest of the four strategies but 0MB and always available.
//! Suitable for small palaces (<10K drawers).

use super::traits::{SearchHit, SearchStrategy};
use crate::palace_db::PalaceDb;
use anyhow::Result;
use std::collections::HashSet;

pub struct NaiveJaccardStrategy;

impl NaiveJaccardStrategy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NaiveJaccardStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchStrategy for NaiveJaccardStrategy {
    fn name(&self) -> &str {
        "naive"
    }

    fn search(&self, query: &str, db: &PalaceDb, n: usize) -> Result<Vec<SearchHit>> {
        let q_tokens: HashSet<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if q_tokens.is_empty() {
            return Ok(vec![]);
        }
        let qrs = db.get_all(None, None, usize::MAX);
        let mut hits: Vec<SearchHit> = Vec::new();
        for qr in qrs {
            for (i, id) in qr.ids.iter().enumerate() {
                let content = qr
                    .documents
                    .get(i)
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                let c_tokens: HashSet<&str> = content.split_whitespace().collect();
                let intersection: usize = q_tokens
                    .iter()
                    .filter(|t| c_tokens.contains(t.as_str()))
                    .count();
                if intersection == 0 {
                    continue;
                }
                let union = q_tokens.len() + c_tokens.len() - intersection;
                let score = if union > 0 {
                    intersection as f64 / union as f64
                } else {
                    0.0
                };
                let metadata = qr
                    .metadatas
                    .get(i)
                    .cloned()
                    .map(serde_json::to_value)
                    .and_then(Result::ok);
                hits.push(SearchHit {
                    id: id.clone(),
                    score,
                    metadata,
                });
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(n);
        Ok(hits)
    }
}
