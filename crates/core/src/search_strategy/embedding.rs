//! Embedding strategy — uses ONNX MiniLM + HNSW for semantic search.
//!
//! Best quality but 90MB+ model download on first use.
//! Use this when lexical search is not enough.

use super::traits::{SearchHit, SearchStrategy};
use crate::palace_db::PalaceDb;
use anyhow::Result;

pub struct EmbeddingStrategy;

impl EmbeddingStrategy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmbeddingStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchStrategy for EmbeddingStrategy {
    fn name(&self) -> &str {
        "embedding"
    }

    fn search(&self, query: &str, db: &PalaceDb, n: usize) -> Result<Vec<SearchHit>> {
        // Delegate to existing searcher.
        let response = crate::searcher::search_memories_with_rerank(
            query,
            db.path(),
            None,
            None,
            n,
            Some("hnsw"),
            false,
            None,
            Some(crate::palace::FusionMode::Vector),
        )?;
        Ok(response
            .results
            .into_iter()
            .map(|r| SearchHit {
                id: r.text,
                score: r.similarity,
                metadata: None,
            })
            .collect())
    }

    fn requires_model(&self) -> bool {
        true
    }

    fn disk_size_mb(&self) -> f64 {
        90.0
    }
}
