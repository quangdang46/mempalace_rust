// =====================================================================
// PalaceStore — vector storage abstraction (mp-020 / ADR-2)
// =====================================================================
//
// Pluggable vector store trait. Concrete tiers:
//   Tier 0: embedvec (current default, via `EmbedvecStore`)
//
// Phase 2 adds: Tier 1 `hnsw_rs + sqlite`, Tier 2 `usearch + sqlite`
// Phase 5 adds: Tier 3 `lancedb`
//
// The trait is `async_trait + Send + Sync + 'static` so implementations
// can be wrapped in `Arc<dyn PalaceStore>` and shared across tokio worker
// tasks without a single heap allocation per call.

/// Which tier the store belongs to — used by `mpr doctor` to advise
/// promotion and by the upgrade plan's ADR-2 to scope per-tier work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum StoreTier {
    /// embedvec (≤5 k drawers). The default today.
    Embedvec,
    /// hnsw_rs + sqlite (≤20 k drawers). Phase 2 default.
    HnswRs,
    /// usearch + sqlite (5 k–100 k). Phase 5.
    Usearch,
    /// lancedb (100 k+). Phase 5.
    Lancedb,
}

impl Default for StoreTier {
    fn default() -> Self {
        Self::Embedvec
    }
}

// Re-export the concrete store implementations.
pub mod embedvec;
pub use embedvec::EmbedvecStore;