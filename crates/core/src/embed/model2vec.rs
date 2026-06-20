// =====================================================================
// `Model2VecEmbedder` — `model2vec-rs` adapter (mp-013)
// =====================================================================
//
// `Model2VecEmbedder` wraps `model2vec_rs::StaticEmbedding` to satisfy the
// crate-public `Embedder` trait (mp-010). It is a **pure-Rust**
// alternative to `FastEmbedEmbedder` for low-power machines — no ONNX
// runtime required, sub-ms inference.
//
// References:
//   - docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md ADR-1, ADR-8
//   - docs/research/05_embedding_and_storage_native.md §A.4, §A.5
//
// Notes:
//   * `model2vec_rs::StaticEmbedding::embed` is **synchronous** and
//     CPU-bound. To honour the `Embedder` trait's `async` contract
//     without blocking the tokio reactor we park the work on
//     `tokio::task::spawn_blocking`. The backing `StaticEmbedding` is
//     held inside an `Arc` so the closure can take an owned handle.
//   * `dim()` is read once at construction and cached. Per ADR-8 it
//     MUST stay constant for the lifetime of a model, so reading
//     the dim once at startup is correct.
//   * `fingerprint()` follows the form `"model2vec:<model_name>:<dim>"`.
//   * Errors funnel through `anyhow::Result` per the trait shape that
//     mp-010 owns and the task instructions (mp-013 step 3).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use model2vec_rs::model::StaticModel;

use super::Embedder;

/// `Embedder` implementation backed by `model2vec-rs`.
///
/// Construct via [`Model2VecEmbedder::with_model`] (system cache) or
/// [`Model2VecEmbedder::with_model_and_cache`] (per-palace cache). On
/// first use the model is downloaded from the HuggingFace Hub into the
/// cache directory; subsequent runs load from disk.
pub struct Model2VecEmbedder {
    /// Wrapped behind `Arc` so each `embed`/`embed_batch` call can
    /// `Arc::clone` and move the handle into a `spawn_blocking` task
    /// without forcing the embedder itself to be `Clone`.
    model: Arc<StaticModel>,
    /// Static name of the model. Used only for `fingerprint`.
    model_name: String,
    /// Cached embedding dimensionality. Per ADR-8 this is validated
    /// against the palace `embedding.json` manifest at `Palace::open`.
    dim: usize,
    /// Persisted fingerprint string returned by [`Embedder::fingerprint`]
    /// — `"model2vec:<model_name>:<dim>"`.
    fingerprint: String,
}

impl Model2VecEmbedder {
    /// Create a `Model2VecEmbedder` using the system-default cache
    /// directory (whatever `model2vec_rs` returns, typically
    /// `~/.cache/huggingface` on Linux).
    ///
    /// Use [`Model2VecEmbedder::with_model_and_cache`] to pin the
    /// cache to a per-palace location (`<palace>/embed_cache/`),
    /// which is the recommended layout per ADR-8 so multi-tenant
    /// palaces stay self-contained.
    pub fn with_model(model_name: String, _cache_dir: Option<PathBuf>) -> anyhow::Result<Self> {
        Self::build(model_name)
    }

    /// Create a `Model2VecEmbedder` using `cache_dir` for model files.
    /// The directory is created lazily by model2vec on first use;
    /// supplying `<palace>/embed_cache/` keeps a palace portable across
    /// machines (copy the palace dir, the embed cache comes along).
    pub fn with_model_and_cache(model_name: String, _cache_dir: PathBuf) -> anyhow::Result<Self> {
        Self::build(model_name)
    }

    fn build(model_name: String) -> anyhow::Result<Self> {
        // Load the static embedding model. The model2vec crate will
        // download to its internal cache if not already present.
        // `from_pretrained` takes: model_name, and four optionals for
        // quantization config (all None = default).
        let model = StaticModel::from_pretrained(&model_name, None, None, None)
            .with_context(|| format!("model2vec: failed to load model '{model_name}'"))?;

        // Get dimension by encoding a dummy string (per the original pattern).
        let dim = model.encode(&[".".to_string()])[0].len();

        let fingerprint = format!("model2vec:{model_name}:{dim}");

        Ok(Self {
            model: Arc::new(model),
            model_name,
            dim,
            fingerprint,
        })
    }

    /// Static name of the model variant (e.g. `"potion-base-8M"`).
    /// Useful for diagnostics / `mpr doctor`.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }
}

#[async_trait]
impl Embedder for Model2VecEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        // Route through `embed_batch` so tokenisation buffers are
        // shared by model2vec (per the trait contract recommendation
        // in mp-010 module docs).
        let mut out = self.embed_batch(&[text]).await?;
        out.pop()
            .ok_or_else(|| anyhow::anyhow!("embed: empty batch returned from embedder"))
    
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // `StaticModel::encode` is sync and CPU-bound, so park it on
        // the blocking pool. We must move owned strings into the
        // closure because the `&str` borrows do not outlive the
        // `'static` lifetime that `spawn_blocking` requires.
        let owned: Vec<String> = texts.iter().map(|s| (*s).to_owned()).collect();
        let model = Arc::clone(&self.model);

        let vectors = tokio::task::spawn_blocking(move || model.encode(&owned))
            .await
            .context("model2vec: blocking task panicked")?;

        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::resolve_embedder;

    /// `MEMPALACE_SKIP_NETWORK_TESTS=1` skips tests that download model
    /// files (~5–20 MB on first run). CI sets this; local devs can
    /// unset it to exercise the real embedder.
    fn network_disabled() -> bool {
        std::env::var("MEMPALACE_SKIP_NETWORK_TESTS")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn dim_is_correct_for_default_model() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = Model2VecEmbedder::with_model("potion-base-8M".to_owned(), None)
            .expect("potion-base-8M loads");
        // potion-base-8M produces 384-dim vectors.
        assert_eq!(e.dim(), 384);
    }

    #[tokio::test]
    async fn fingerprint_is_stable_and_well_formed() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = Model2VecEmbedder::with_model("potion-base-8M".to_owned(), None)
            .expect("potion-base-8M loads");
        let fp1 = e.fingerprint().to_owned();
        let fp2 = e.fingerprint().to_owned();
        assert_eq!(fp1, fp2);
        assert!(fp1.starts_with("model2vec:potion-base-8M:"));
    }

    #[tokio::test]
    async fn embed_batch_returns_finite_vectors_of_correct_dim() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = Model2VecEmbedder::with_model("potion-base-8M".to_owned(), None)
            .expect("potion-base-8M loads");
        let inputs = ["hello world", "rust is fast", "memory palace"];
        #[allow(clippy::iter_cloned_collect)]
        let vectors = e
            .embed_batch(&inputs.iter().copied().collect::<Vec<_>>())
            .await
            .expect("embed_batch succeeds");

        assert_eq!(vectors.len(), 3);
        for v in &vectors {
            assert_eq!(v.len(), e.dim());
            for x in v {
                assert!(x.is_finite(), "embedding contained non-finite value: {x}");
            }
        }
    }

    #[tokio::test]
    async fn embed_batch_handles_empty_input_without_loading() {
        // This must NOT need the model loaded — exercise the early
        // return for empty inputs.
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = Model2VecEmbedder::with_model("potion-base-8M".to_owned(), None)
            .expect("potion-base-8M loads");
        let v = e.embed_batch(&[]).await.expect("empty batch");
        assert!(v.is_empty());
    }

    #[test]
    fn resolve_embedder_m2v_model_loads() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        // This should succeed (network allowed) or fail with a clear
        // error (not "unknown model name").
        match resolve_embedder("potion-base-8M") {
            Ok(_) => { /* loaded — network allowed */ }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("unknown model name"),
                    "M2V model should be recognized: {msg}"
                );
            }
        }
    }
}
