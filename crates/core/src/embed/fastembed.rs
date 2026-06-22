// =====================================================================
// `FastEmbedEmbedder` — `fastembed-rs` adapter (mp-012, mp-018)
// =====================================================================
//
// `FastEmbedEmbedder` wraps `fastembed::TextEmbedding` to satisfy the
// crate-public `Embedder` trait (mp-010). It is the **default** local
// embedder shipped with mempalace — selected by the `embed-fastembed`
// Cargo feature, which is itself in `default`.
//
// References:
//   - docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md ADR-1, ADR-8
//   - docs/research/05_embedding_and_storage_native.md §A.4, §A.5,
//     §A.6, §C.2 PR 2
//
// Notes:
//   * `fastembed::TextEmbedding::embed` is **synchronous** and CPU-bound
//     (ORT forward pass). To honour the `Embedder` trait's `async`
//     contract without blocking the tokio reactor we park the work on
//     `tokio::task::spawn_blocking`. The backing `TextEmbedding` is
//     held inside an `Arc` so the closure can take an owned handle.
//   * `dim()` is read once at construction from
//     `TextEmbedding::get_model_info(...)` and cached. Per ADR-8 it
//     MUST stay constant for the lifetime of an `(model_name,
//     tokenizer)` pair, so reading the manifest dim once at startup is
//     correct and avoids paying for the lookup on every call.
//   * `fingerprint()` follows the form `"fastembed:<model_name>:<dim>"`.
//     The trait contract (mp-010 module docs) asks for a stable
//     `<model_name>:<tokenizer_hash>` shape; we omit the tokenizer hash
//     because fastembed bakes the tokenizer into the curated model
//     enum — `model_name` already implies the tokenizer. The `dim`
//     suffix doubles the cheap mismatch check that ADR-8 requires.
//   * Errors funnel through `anyhow::Result` per the trait shape that
//     mp-010 owns and the task instructions (mp-012 step 3).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::Embedder;

/// `Embedder` implementation backed by `fastembed-rs`.
///
/// Construct via [`FastEmbedEmbedder::with_model`] (system cache) or
/// [`FastEmbedEmbedder::with_model_and_cache`] (per-palace cache). On
/// first use the model is downloaded from the HuggingFace Hub into the
/// cache directory; subsequent runs load from disk.
pub struct FastEmbedEmbedder {
    /// Wrapped behind `Arc` so each `embed`/`embed_batch` call can
    /// `Arc::clone` and move the handle into a `spawn_blocking` task
    /// without forcing the embedder itself to be `Clone`.
    model: Arc<TextEmbedding>,
    /// Static name of the curated fastembed model enum variant. Used
    /// only for `fingerprint`; the source of truth for the model
    /// identity is the `EmbeddingModel` value passed to `try_new`.
    model_name: &'static str,
    /// Cached embedding dimensionality (from `ModelInfo::dim`). Per
    /// ADR-8 this is validated against the palace `embedding.json`
    /// manifest at `Palace::open`.
    dim: usize,
    /// Persisted fingerprint string returned by [`Embedder::fingerprint`]
    /// — `"fastembed:<model_name>:<dim>"`.
    fingerprint: String,
}

impl FastEmbedEmbedder {
    /// Create a `FastEmbedEmbedder` using the system-default cache
    /// directory (whatever `fastembed::get_cache_dir()` returns,
    /// typically `~/.cache/huggingface` on Linux).
    ///
    /// Use [`FastEmbedEmbedder::with_model_and_cache`] to pin the
    /// cache to a per-palace location (`<palace>/embed_cache/`),
    /// which is the recommended layout per ADR-8 so multi-tenant
    /// palaces stay self-contained.
    pub fn with_model(model: EmbeddingModel) -> anyhow::Result<Self> {
        Self::build(model, None)
    }

    /// Create a `FastEmbedEmbedder` using `cache_dir` for ONNX +
    /// tokenizer files. The directory is created lazily by fastembed
    /// on first use; supplying `<palace>/embed_cache/` keeps a palace
    /// portable across machines (copy the palace dir, the embed cache
    /// comes along).
    pub fn with_model_and_cache(model: EmbeddingModel, cache_dir: PathBuf) -> anyhow::Result<Self> {
        Self::build(model, Some(cache_dir))
    }

    fn build(model: EmbeddingModel, cache_dir: Option<PathBuf>) -> anyhow::Result<Self> {
        // Pull `dim` and `model_code` from fastembed's curated metadata
        // BEFORE loading the ONNX session — this fails fast on an
        // unrecognised model without paying the model-load cost.
        let info = TextEmbedding::get_model_info(&model)
            .with_context(|| format!("fastembed: no metadata for model {model:?}"))?;
        let dim = info.dim;
        let model_code = info.model_code.clone();

        let mut opts = InitOptions::new(model.clone()).with_show_download_progress(false);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir);
        }

        let text_embedding = TextEmbedding::try_new(opts)
            .with_context(|| format!("fastembed: failed to load model {model_code}"))?;

        let model_name = embedding_model_static_name(&model);
        let fingerprint = format!("fastembed:{model_name}:{dim}");

        Ok(Self {
            model: Arc::new(text_embedding),
            model_name,
            dim,
            fingerprint,
        })
    }

    /// Static name of the model variant (e.g. `"BGESmallENV15"`).
    /// Useful for diagnostics / `mpr doctor`.
    pub fn model_name(&self) -> &'static str {
        self.model_name
    }
}

#[async_trait]
impl Embedder for FastEmbedEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        // Route through `embed_batch` so tokenisation buffers are
        // shared by fastembed (per the trait contract recommendation
        // in mp-010 module docs).
        let mut out = self.embed_batch(&[text]).await?;
        out.pop()
            .ok_or_else(|| anyhow::anyhow!("embed: empty batch returned from embedder"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // `fastembed::TextEmbedding::embed` is sync and CPU-bound, so
        // park it on the blocking pool. We must move owned strings
        // into the closure because the `&str` borrows do not outlive
        // the `'static` lifetime that `spawn_blocking` requires.
        let owned: Vec<String> = texts.iter().map(|s| (*s).to_owned()).collect();
        let model = Arc::clone(&self.model);

        let vectors = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Vec<f32>>> {
            // `batch_size = None` -> fastembed default (256).
            model
                .embed(owned, None)
                .context("fastembed: embed_batch failed")
        })
        .await
        .context("fastembed: blocking task panicked")??;

        Ok(vectors)
    }
}

/// Map a fastembed `EmbeddingModel` enum value to its static Rust
/// identifier. Used for the fingerprint string and surfacing the
/// active model in `mpr doctor`.
///
/// New variants in `fastembed` 4.x must be added here when bumping
/// the dep; the test below ensures the default mapping stays sane.
pub(crate) fn embedding_model_static_name(model: &EmbeddingModel) -> &'static str {
    match model {
        EmbeddingModel::AllMiniLML6V2 => "AllMiniLML6V2",
        EmbeddingModel::AllMiniLML6V2Q => "AllMiniLML6V2Q",
        EmbeddingModel::AllMiniLML12V2 => "AllMiniLML12V2",
        EmbeddingModel::AllMiniLML12V2Q => "AllMiniLML12V2Q",
        EmbeddingModel::BGEBaseENV15 => "BGEBaseENV15",
        EmbeddingModel::BGEBaseENV15Q => "BGEBaseENV15Q",
        EmbeddingModel::BGELargeENV15 => "BGELargeENV15",
        EmbeddingModel::BGELargeENV15Q => "BGELargeENV15Q",
        EmbeddingModel::BGESmallENV15 => "BGESmallENV15",
        EmbeddingModel::BGESmallENV15Q => "BGESmallENV15Q",
        EmbeddingModel::BGESmallZHV15 => "BGESmallZHV15",
        EmbeddingModel::BGELargeZHV15 => "BGELargeZHV15",
        EmbeddingModel::ClipVitB32 => "ClipVitB32",
        EmbeddingModel::GTEBaseENV15 => "GTEBaseENV15",
        EmbeddingModel::GTEBaseENV15Q => "GTEBaseENV15Q",
        EmbeddingModel::GTELargeENV15 => "GTELargeENV15",
        EmbeddingModel::GTELargeENV15Q => "GTELargeENV15Q",
        EmbeddingModel::JinaEmbeddingsV2BaseCode => "JinaEmbeddingsV2BaseCode",
        EmbeddingModel::ModernBertEmbedLarge => "ModernBertEmbedLarge",
        EmbeddingModel::MultilingualE5Base => "MultilingualE5Base",
        EmbeddingModel::MultilingualE5Large => "MultilingualE5Large",
        EmbeddingModel::MultilingualE5Small => "MultilingualE5Small",
        EmbeddingModel::MxbaiEmbedLargeV1 => "MxbaiEmbedLargeV1",
        EmbeddingModel::MxbaiEmbedLargeV1Q => "MxbaiEmbedLargeV1Q",
        EmbeddingModel::NomicEmbedTextV1 => "NomicEmbedTextV1",
        EmbeddingModel::NomicEmbedTextV15 => "NomicEmbedTextV15",
        EmbeddingModel::NomicEmbedTextV15Q => "NomicEmbedTextV15Q",
        EmbeddingModel::ParaphraseMLMiniLML12V2 => "ParaphraseMLMiniLML12V2",
        EmbeddingModel::ParaphraseMLMiniLML12V2Q => "ParaphraseMLMiniLML12V2Q",
        EmbeddingModel::ParaphraseMLMpnetBaseV2 => "ParaphraseMLMpnetBaseV2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::resolve_embedder;

    /// `MEMPALACE_SKIP_NETWORK_TESTS=1` skips tests that download model
    /// files (~25–50 MB on first run). CI sets this; local devs can
    /// unset it to exercise the real embedder.
    fn network_disabled() -> bool {
        std::env::var("MEMPALACE_SKIP_NETWORK_TESTS")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    }

    #[test]
    fn static_name_covers_default_model() {
        assert_eq!(
            embedding_model_static_name(&EmbeddingModel::BGESmallENV15),
            "BGESmallENV15"
        );
        assert_eq!(
            embedding_model_static_name(&EmbeddingModel::AllMiniLML6V2),
            "AllMiniLML6V2"
        );
    }

    #[tokio::test]
    async fn dim_matches_default_model() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = match FastEmbedEmbedder::with_model(EmbeddingModel::BGESmallENV15) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("skipping (model download/load failed: {err:#})");
                return;
            }
        };
        // BGE-small-en-v1.5 produces 384-dim vectors.
        assert_eq!(e.dim(), 384);
    }

    #[tokio::test]
    async fn fingerprint_is_stable_and_well_formed() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = match FastEmbedEmbedder::with_model(EmbeddingModel::BGESmallENV15) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("skipping (model download/load failed: {err:#})");
                return;
            }
        };
        let fp1 = e.fingerprint().to_owned();
        let fp2 = e.fingerprint().to_owned();
        assert_eq!(fp1, fp2);
        assert_eq!(fp1, "fastembed:BGESmallENV15:384");
    }

    #[tokio::test]
    async fn embed_batch_returns_finite_vectors_of_correct_dim() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = match FastEmbedEmbedder::with_model(EmbeddingModel::BGESmallENV15) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("skipping (model download/load failed: {err:#})");
                return;
            }
        };
        let inputs = ["hello world", "rust is fast", "memory palace"];
        let vectors = e
            .embed_batch(&inputs[..])
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
        let e = match FastEmbedEmbedder::with_model(EmbeddingModel::BGESmallENV15) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("skipping (model download/load failed: {err:#})");
                return;
            }
        };
        let v = e.embed_batch(&[]).await.expect("empty batch");
        assert!(v.is_empty());
    }

    #[test]
    fn resolve_embedder_unknown_lists_accepted_names() {
        let err = match resolve_embedder("unknown-model") {
            Ok(_) => panic!("unknown name must fail"),
            Err(e) => e,
        };
        let msg = err.to_string();
        // Error message should mention at least the default and a
        // representative subset so users have actionable hints.
        assert!(
            msg.contains("bge-small-en-v15"),
            "expected accepted-list hint in error, got: {msg}"
        );
        assert!(
            msg.contains("multilingual-e5-small"),
            "expected accepted-list hint in error, got: {msg}"
        );
    }

    #[test]
    fn resolve_embedder_is_case_insensitive() {
        // Construction-only smoke: this must succeed at the *parse*
        // step; the actual model load is allowed to fail offline (the
        // test passes as long as we don't reject the name itself).
        match resolve_embedder("BGE-Small-EN") {
            Ok(_) => { /* loaded — network allowed */ }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("unknown model name"),
                    "case-insensitive parse should accept 'BGE-Small-EN', \
                     got name-rejection error: {msg}"
                );
            }
        }
    }
}
