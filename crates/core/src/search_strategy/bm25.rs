//! BM25 strategy — BM25 rerank on top of naive token overlap.
//!
//! Better ranking than pure Jaccard for term-frequency imbalance.
//! 0MB extra, no model.

use super::traits::{SearchHit, SearchStrategy};
use crate::palace_db::PalaceDb;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub struct Bm25Strategy {
    k1: f64, // BM25 parameter, typically 1.2-2.0
    b: f64,  // BM25 length normalization, typically 0.75
}

impl Default for Bm25Strategy {
    fn default() -> Self {
        Self::new()
    }
}

impl Bm25Strategy {
    pub fn new() -> Self {
        Self { k1: 1.5, b: 0.75 }
    }
}

impl SearchStrategy for Bm25Strategy {
    fn name(&self) -> &str {
        "bm25"
    }

    fn search(&self, query: &str, db: &PalaceDb, n: usize) -> Result<Vec<SearchHit>> {
        let q_tokens: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if q_tokens.is_empty() {
            return Ok(vec![]);
        }
        let q_unique: HashSet<String> = q_tokens.iter().cloned().collect();

        // Pull all docs
        let qrs = db.get_all(None, None, usize::MAX);
        let mut all_ids: Vec<String> = Vec::new();
        let mut all_metas: Vec<HashMap<String, serde_json::Value>> = Vec::new();
        let mut all_contents: Vec<String> = Vec::new();
        for qr in qrs {
            for (i, id) in qr.ids.iter().enumerate() {
                all_ids.push(id.clone());
                all_metas.push(qr.metadatas.get(i).cloned().unwrap_or_default());
                let content = qr
                    .documents
                    .get(i)
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                all_contents.push(content);
            }
        }
        if all_ids.is_empty() {
            return Ok(vec![]);
        }

        // Compute avg doc length (in tokens)
        let mut doc_lens: Vec<usize> = Vec::with_capacity(all_contents.len());
        for content in &all_contents {
            doc_lens.push(content.split_whitespace().count());
        }
        let contents = all_contents;
        let avg_dl: f64 = if doc_lens.is_empty() {
            1.0
        } else {
            doc_lens.iter().sum::<usize>() as f64 / doc_lens.len() as f64
        };
        let n_docs = doc_lens.len() as f64;

        // Compute BM25 score for each doc
        let mut scored: Vec<(usize, f64)> = Vec::new();
        for (i, content) in contents.iter().enumerate() {
            let c_tokens: Vec<&str> = content.split_whitespace().collect();
            let dl = doc_lens[i] as f64;
            let mut score = 0.0;
            for term in &q_unique {
                // tf in this doc
                let tf = c_tokens.iter().filter(|x| **x == term.as_str()).count() as f64;
                if tf == 0.0 {
                    continue;
                }
                // df = number of docs containing term
                let df = contents
                    .iter()
                    .filter(|c| c.split_whitespace().any(|x| x == term.as_str()))
                    .count() as f64;
                if df == 0.0 {
                    continue;
                }
                let idf = ((n_docs - df + 0.5) / (df + 0.5) + 1.0).ln();
                let tf_norm =
                    (tf * (self.k1 + 1.0)) / (tf + self.k1 * (1.0 - self.b + self.b * dl / avg_dl));
                score += idf * tf_norm;
            }
            if score > 0.0 {
                scored.push((i, score));
            }
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(n)
            .map(|(i, s)| SearchHit {
                id: all_ids[i].clone(),
                score: s,
                metadata: all_metas
                    .get(i)
                    .cloned()
                    .map(serde_json::to_value)
                    .and_then(Result::ok),
            })
            .collect())
    }
}
