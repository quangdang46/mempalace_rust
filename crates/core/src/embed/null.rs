// =====================================================================
// `NullEmbedder` — zero-vector backend for tests and stub deployments
// (mp-011)
// =====================================================================
//
// `NullEmbedder` returns all-zero vectors of a configured dimensionality
// and exposes a constant fingerprint of the form `"null:<dim>"`. It is
// **not** semantically meaningful — its job is to let unit tests and
// integration scaffolding exercise the `Embedder` / `PalaceStore` /
// `MemoryProvider` stack without paying the cost of loading a real
// model.
//
// Use cases:
//   * Unit tests that need an `Arc<dyn Embedder>` but don't care about
//     similarity scores.
//   * `mpr doctor` smoke checks that exercise the full open / search
//     path without an ONNX runtime present.
//   * Reproducing storage bugs deterministically (every text → same
//     vector → similarity always 1.0).
//
// Production code MUST NOT default to this; ADR-1 mandates a real
// embedder (`fastembed-rs`, `model2vec-rs`, `tract-onnx`, or remote
// API) for the shipping configuration.

use async_trait::async_trait;

use super::Embedder;

/// Embedder that returns zero-vectors of a configured dimension.
///
/// See the module docs for intended use (tests / stubs only). The
/// fingerprint is `"null:<dim>"` so manifests written by tests are
/// trivially distinguishable from production palaces.
#[derive(Debug, Clone)]
pub struct NullEmbedder {
    dim: usize,
    fingerprint: String,
}

impl NullEmbedder {
    /// Create a new `NullEmbedder` that produces zero-vectors of the
    /// given dimension. Panics if `dim == 0` because a zero-dimensional
    /// vector store is meaningless and would silently mask bugs.
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "NullEmbedder dim must be > 0");
        Self {
            dim,
            fingerprint: format!("null:{dim}"),
        }
    }
}

#[async_trait]
impl Embedder for NullEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; self.dim])
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![vec![0.0; self.dim]; texts.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_encodes_dim() {
        let e = NullEmbedder::new(384);
        assert_eq!(e.dim(), 384);
        assert_eq!(e.fingerprint(), "null:384");
    }

    #[test]
    fn clone_keeps_fingerprint() {
        let a = NullEmbedder::new(64);
        let b = a.clone();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.dim(), b.dim());
    }

    #[tokio::test]
    async fn embed_returns_zero_vector_of_correct_dim() {
        let e = NullEmbedder::new(16);
        let v = e.embed("anything").await.unwrap();
        assert_eq!(v.len(), 16);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[tokio::test]
    async fn embed_batch_handles_empty_input() {
        let e = NullEmbedder::new(8);
        let v = e.embed_batch(&[]).await.unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn embed_batch_returns_one_vector_per_input() {
        let e = NullEmbedder::new(4);
        let v = e.embed_batch(&["a", "b", "c"]).await.unwrap();
        assert_eq!(v.len(), 3);
        for inner in &v {
            assert_eq!(inner.len(), 4);
            assert!(inner.iter().all(|&x| x == 0.0));
        }
    }

    #[test]
    #[should_panic(expected = "NullEmbedder dim must be > 0")]
    fn zero_dim_panics() {
        let _ = NullEmbedder::new(0);
    }

    /// Confirm `NullEmbedder` satisfies the `Send + Sync + 'static`
    /// bound demanded by `Embedder` and the `MemoryProvider` facade.
    #[test]
    fn is_send_sync_static() {
        fn assert_bounds<T: Send + Sync + 'static>() {}
        assert_bounds::<NullEmbedder>();
    }
}
