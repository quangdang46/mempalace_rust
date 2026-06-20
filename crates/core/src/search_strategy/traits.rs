//! Search strategy trait — the core abstraction.

use crate::palace_db::PalaceDb;
use anyhow::Result;

/// A single search hit returned by any strategy.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Drawer ID
    pub id: String,
    /// Relevance score (higher = better)
    pub score: f64,
    /// Optional metadata (wing, room, etc.)
    pub metadata: Option<serde_json::Value>,
}

/// Trait for search strategies. All strategies must implement this.
pub trait SearchStrategy: Send + Sync {
    /// Short identifier (e.g. "fts5", "naive").
    fn name(&self) -> &str;

    /// Run a search against the given PalaceDb.
    fn search(&self, query: &str, db: &PalaceDb, n: usize) -> Result<Vec<SearchHit>>;

    /// Does this strategy require downloading a model? (embedding only)
    fn requires_model(&self) -> bool {
        false
    }

    /// Approximate disk footprint in MB (model + index).
    fn disk_size_mb(&self) -> f64 {
        0.0
    }
}
