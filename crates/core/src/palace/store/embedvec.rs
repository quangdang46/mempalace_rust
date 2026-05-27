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
//   task boundaries.
//
// Embedding contract:
//   The store expects pre-embedded vectors passed to `search`.
//   `Palace::search` embeds the query text and forwards the vector here.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::embed::EmbeddingManifest;
use crate::palace::{Drawer, DrawerId, PalaceStore, SearchHit, SearchScope, StoreTier};

/// Wraps the existing `EmbeddingDb` (embedvec HNSW + VectorStorage) behind
/// the `PalaceStore` trait.
///
/// Construction: `EmbedvecStore::new(embedder)` takes an injected
/// `Arc<dyn Embedder>` and uses `embedder.dim()` to size the vector
/// storage.
///
/// ## Note on embedding
///
/// `search` takes a pre-computed query vector — the embedding step
/// happens in `Palace::search` (which calls `embedder.embed()` first).
/// This keeps the store layer embedder-agnostic: jcode could inject its
/// own embedder while sharing the same `EmbedvecStore` instance.
pub struct EmbedvecStore {
    inner: Arc<Mutex<crate::palace_db::EmbeddingDb>>,
    palace_path: Option<PathBuf>,
    model_name: Option<String>,
    embedder: Arc<dyn crate::embed::Embedder>,
}

impl EmbedvecStore {
    /// Construct with an injected embedder.
    pub async fn new(embedder: Arc<dyn crate::embed::Embedder>) -> anyhow::Result<Self> {
        let inner = crate::palace_db::EmbeddingDb::with_embedder(embedder.clone())?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            palace_path: None,
            model_name: None,
            embedder,
        })
    }

    /// Construct with an embedder and a palace path, enabling automatic
    /// `embedding.json` manifest writing on first embed (mp-015).
    pub async fn new_with_path(
        embedder: Arc<dyn crate::embed::Embedder>,
        palace_path: PathBuf,
        model_name: String,
    ) -> anyhow::Result<Self> {
        let inner = crate::palace_db::EmbeddingDb::with_embedder(embedder.clone())?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            palace_path: Some(palace_path),
            model_name: Some(model_name),
            embedder,
        })
    }

    pub fn raw(&self) -> Arc<Mutex<crate::palace_db::EmbeddingDb>> {
        self.inner.clone()
    }
}

#[async_trait]
impl PalaceStore for EmbedvecStore {
    async fn upsert(&self, drawers: Vec<Drawer>) -> anyhow::Result<()> {
        if drawers.is_empty() {
            return Ok(());
        }

        let needs_write = self.model_name.is_some()
            && self.palace_path.is_some()
            && {
                let inner = self.inner.lock().await;
                inner.len() == 0
            };

        if needs_write {
            let manifest = EmbeddingManifest::from_embedder(
                self.embedder.as_ref(),
                self.model_name.as_deref().unwrap_or("unknown"),
            );
            if let Some(ref p) = self.palace_path {
                let _ = EmbeddingManifest::write(p, &manifest);
            }
        }

        let mut inner = self.inner.lock().await;
        let items: Vec<(String, String)> = drawers
            .into_iter()
            .enumerate()
            .map(|(i, d)| {
                let id = d
                    .id
                    .map(|di| di.0)
                    .unwrap_or_else(|| format!("drawer-{}", i));
                (id, d.content)
            })
            .collect();
        inner.add_batch(&items).await?;
        Ok(())
    }

    async fn delete(&self, _ids: &[DrawerId]) -> anyhow::Result<usize> {
        Ok(0)
    }

    async fn search(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let inner = self.inner.lock().await;
        let normalized = normalize_embedding(query_vec);
        let raw_results = inner.query_by_vector(&normalized, limit)?;

        let mut hits = Vec::new();
        for (dist, idx) in raw_results {
            hits.push(SearchHit {
                text: inner.nth_text(idx).unwrap_or_default().to_string(),
                wing: scope.wing.clone(),
                room: scope.room.clone(),
                source_file: String::new(),
                similarity: (1.0 - dist) as f64,
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
        Ok(())
    }

    fn tier(&self) -> StoreTier {
        StoreTier::Embedvec
    }

    async fn get_drawers(
        &self,
        _scope: Option<&SearchScope>,
        _limit: Option<usize>,
    ) -> anyhow::Result<Vec<Drawer>> {
        Ok(vec![])
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
