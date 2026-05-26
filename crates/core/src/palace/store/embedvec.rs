// =====================================================================
// EmbedvecStore — the default PalaceStore implementation (mp-021 / ADR-2)
// =====================================================================
//
// Wraps the existing `EmbeddingDb` (which wraps embedvec HNSW + vector storage)
// behind the `PalaceStore` trait so `PalaceBuilder` can accept it as
// `Arc<dyn PalaceStore>`.
//
// This is the Tier 0 implementation — used for palaces ≤5 k drawers.
// Tier 1 (hnsw_rs + sqlite), Tier 2 (usearch + sqlite), and Tier 3
// (lancedb) are future work (Phase 5).
//
// Thread safety:
//   `EmbeddingDb` uses embedvec internally (HnswIndex + VectorStorage)
//   which are not `Send + Sync`. Since we're wrapping it behind
//   `Arc<dyn PalaceStore>` and the trait methods are async, we use
//   `tokio::sync::Mutex` to make the inner mutable access safe across
//   task boundaries. An async Mutex is appropriate here because
//   PalaceStore methods are already async — callers `.await` the lock
//   release rather than blocking a thread.
//
// Embedding contract:
//   The store expects pre-embedded vectors passed to `search`.
//   `Palace::search` embeds the query text and forwards the vector here.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::palace::{Drawer, DrawerId, PalaceStore, SearchHit, SearchScope, StoreTier};

/// Wraps the existing `EmbeddingDb` (embedvec HNSW + VectorStorage) behind
/// the `PalaceStore` trait.
///
/// Construction: `EmbedvecStore::new()` loads the legacy `OnnxModel`
/// embedder (384-dim, MiniLM-L6-v2). For the new trait-based API where
/// the embedder is injected via `PalaceBuilder::embedder`, use
/// `EmbedvecStore::with_embedder(Arc<dyn Embedder>)` instead.
///
/// ## Note on embedding
///
/// `search` takes a pre-computed query vector — the embedding step
/// happens in `Palace::search` (which calls `embedder.embed()` first).
/// This keeps the store layer embedder-agnostic: jcode could inject its
/// own embedder while sharing the same `EmbedvecStore` instance.
pub struct EmbedvecStore {
    inner: Arc<Mutex<crate::palace_db::EmbeddingDb>>,
}

impl EmbedvecStore {
    /// New store with a fresh `OnnxModel` (384-dim MiniLM-L6-v2).
    /// Uses the legacy embedder path — for new code prefer
    /// `with_embedder(Arc<dyn Embedder>)` with `fastembed-rs`.
    pub fn new() -> anyhow::Result<Self> {
        let inner = crate::palace_db::EmbeddingDb::new(384)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Access the raw `EmbeddingDb` for cases that need embedvec internals
    /// (e.g. calling `add_batch` directly during bulk mining).
    pub fn raw(&self) -> Arc<Mutex<crate::palace_db::EmbeddingDb>> {
        self.inner.clone()
    }
}

impl Default for EmbedvecStore {
    fn default() -> Self {
        Self::new().expect("EmbedvecStore::default: failed to load OnnxModel")
    }
}

#[async_trait]
impl PalaceStore for EmbedvecStore {
    async fn upsert(&self, drawers: Vec<Drawer>) -> anyhow::Result<()> {
        if drawers.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        // Convert drawers to (id, text) pairs for add_batch.
        // IDs are generated as sequential indices; the actual drawer
        // content + metadata is stored in the JSON layer (PalaceDb).
        let items: Vec<(String, String)> = drawers
            .into_iter()
            .enumerate()
            .map(|(i, d)| {
                let id = d.id.map(|di| di.0).unwrap_or_else(|| format!("drawer-{}", i));
                (id, d.content)
            })
            .collect();
        inner.add_batch(&items)?;
        Ok(())
    }

    async fn delete(&self, _ids: &[DrawerId]) -> anyhow::Result<usize> {
        // Embedvec doesn't support delete by ID in the current implementation.
        // This is a limitation tracked for Phase 5.
        // For now, return 0 (no deletions performed).
        Ok(0)
    }

    async fn search(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let inner = self.inner.lock().await;
        // Normalize the query vector (same logic as EmbeddingDb::query).
        let normalized = normalize_embedding(query_vec);
        let raw_results = inner.query_by_vector(&normalized, limit)?;

        let mut hits = Vec::new();
        for (dist, idx) in raw_results {
            // We need the drawer text and metadata. The EmbeddingDb
            // stores (id, text) pairs in order. We retrieve by index.
            // Note: if the store was opened from a file with existing
            // drawers, those aren't re-embedded. This path is for new
            // writes via add_batch only.
            let text = inner
                .nth_text(idx)
                .unwrap_or_default()
                .to_string();
            let similarity = 1.0 - dist;
            hits.push(SearchHit {
                text,
                wing: scope.wing.clone(),
                room: scope.room.clone(),
                source_file: String::new(),
                similarity: similarity as f64,
                bm25_score: None,
                combined_score: None,
            });
        }
        Ok(hits)
    }

    async fn count(&self, _scope: &SearchScope) -> anyhow::Result<usize> {
        let inner = self.inner.lock().await;
        Ok(inner.len())
    }

    async fn flush(&self) -> anyhow::Result<()> {
        // embedvec is in-memory; no flush needed. The PalaceDb JSON
        // layer handles persistence of drawer content + metadata.
        Ok(())
    }

    fn tier(&self) -> StoreTier {
        StoreTier::Embedvec
    }
}

/// L2-normalize a vector (required for cosine similarity with embedvec).
fn normalize_embedding(embedding: &[f32]) -> Vec<f32> {
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return embedding.to_vec();
    }
    embedding.iter().map(|x| x / norm).collect()
}