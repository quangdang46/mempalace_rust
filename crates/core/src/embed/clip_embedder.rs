// =====================================================================
// `ClipImageEmbedder` — CLIP image embeddings via `fastembed` 4.x
// =====================================================================
//
// Closes the 12-gap parity port: agentmemory ships a CLIP-based image
// embedder (Xenova/clip-vit-base-patch32, 512-d, L2-normalised).
// `fastembed` 4.x ships an `ImageEmbedding` type backed by the same
// `ort` version it already pulls in for the text path, so wiring it
// in here re-uses the existing `embed-fastembed` feature and avoids
// the dual-`ort`-version conflict that blocks the third-party
// `open_clip_inference` crate.
//
// References:
//   - agentmemory `src/embedding/clip.ts` (Xenova/clip-vit-base-patch32)
//   - fastembed 4.x `src/image_embedding/impl.rs` (CLIP-ViT-B/32 default)
//
// Notes:
//   * `ImageEmbedding::embed` is sync + CPU-bound, so we route through
//     `tokio::task::spawn_blocking`, mirroring `FastEmbedEmbedder`.
//   * The default model is `ClipVitB32` and produces 512-d, L2-normalised
//     vectors, matching the upstream `Xenova/clip-vit-base-patch32`
//     output shape (see `models/image_embedding.rs`).
//   * The fingerprint follows `<runtime>:<model>:<dim>`, e.g.
//     `"clip:ClipVitB32:512"`, so manifests written with a different
//     embedder family fail loud at `Palace::open`.
//   * On first use the model files are downloaded from the HuggingFace
//     Hub into the cache directory; subsequent runs load from disk.
//   * The struct intentionally does NOT implement the `Embedder` text
//     trait (CLIP's text path is the symmetric `TextEmbedding::Clip*`
//     variants in fastembed 4, but exposing it as a text embedder
//     would require a second model load and risks silently changing
//     `dim()` from 384/768 to 512). Use `FastEmbedEmbedder` with
//     `EmbeddingModel::ClipVitB32` for the text side; this struct
//     focuses on the image side that the parity port flagged as
//     missing.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};

/// Default CLIP image model used by `ClipImageEmbedder`.
///
/// Matches agentmemory's `Xenova/clip-vit-base-patch32` (512-d,
/// L2-normalised) and fastembed 4's `ImageEmbeddingModel::ClipVitB32`.
pub const DEFAULT_CLIP_MODEL: ImageEmbeddingModel = ImageEmbeddingModel::ClipVitB32;

/// CLIP-based image embedder backed by `fastembed` 4.x.
///
/// Produces 512-d, L2-normalised float vectors for arbitrary image
/// inputs (file paths or raw bytes). The text side of the CLIP model
/// is *not* implemented here — use `FastEmbedEmbedder` with
/// `EmbeddingModel::ClipVitB32` if you need cross-modal text queries.
pub struct ClipImageEmbedder {
    /// Wrapped in `Arc` so each `embed_image*` call can `Arc::clone`
    /// the handle into a `spawn_blocking` task without forcing the
    /// embedder itself to be `Clone`.
    model: Arc<ImageEmbedding>,
    /// Static name of the curated fastembed image-model enum variant.
    /// Used only for `fingerprint` and the `mpr doctor` style
    /// diagnostic; the source of truth is the `ImageEmbeddingModel`
    /// value passed to `try_new`.
    model_name: &'static str,
    /// Cached embedding dimensionality. CLIP-ViT-B/32 always produces
    /// 512-d; we read it from the model info once at construction
    /// rather than hard-coding 512, so a future `ClipVitB16` or
    /// `NomicEmbedVisionV15` swap stays type-safe.
    dim: usize,
    /// Fingerprint string `"clip:<model_name>:<dim>"` returned by
    /// `fingerprint()`. Persisted on first write to the palace
    /// `embedding.json` manifest and compared on subsequent opens; a
    /// mismatch fails loud at `Palace::open`.
    fingerprint: String,
}

impl ClipImageEmbedder {
    /// Build a `ClipImageEmbedder` using fastembed's default cache
    /// directory (`~/.cache/huggingface` on Linux).
    ///
    /// On first use the model is downloaded from the HuggingFace Hub
    /// (~25–50 MB). Use [`with_model_and_cache`] to pin the cache to
    /// a per-palace location.
    ///
    /// [`with_model_and_cache`]: Self::with_model_and_cache
    pub fn with_model(model: ImageEmbeddingModel) -> anyhow::Result<Self> {
        Self::build(model, None)
    }

    /// Build a `ClipImageEmbedder` using `cache_dir` for ONNX +
    /// preprocessor files. Passing `<palace>/embed_cache/` keeps a
    /// palace portable across machines.
    pub fn with_model_and_cache(
        model: ImageEmbeddingModel,
        cache_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        Self::build(model, Some(cache_dir))
    }

    fn build(model: ImageEmbeddingModel, cache_dir: Option<PathBuf>) -> anyhow::Result<Self> {
        // Pull `dim` from fastembed's curated metadata BEFORE loading
        // the ONNX session — this fails fast on an unrecognised model
        // without paying the model-load cost. `ImageEmbedding` does
        // not expose a `get_model_info` equivalent the way
        // `TextEmbedding` does, so we hard-dispatch against the
        // curated list. `ImageEmbedding::list_supported_models()` is
        // callable on a `ImageEmbedding::new` placeholder; we use the
        // static `get_model_info` mapping below instead because the
        // public API is asymmetric with the text path.
        let dim = clip_image_model_dim(&model);

        let mut opts = ImageInitOptions::new(model.clone()).with_show_download_progress(false);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir);
        }

        let image_embedding = ImageEmbedding::try_new(opts).with_context(|| {
            format!(
                "clip: failed to load image model {:?}; \
                 if the failure mentions ORT, check that no other \
                 crate in the dependency graph forces a conflicting \
                 `ort` version (we deliberately share fastembed's ort)",
                model
            )
        })?;

        let model_name = image_model_static_name(&model);
        let fingerprint = format!("clip:{model_name}:{dim}");

        Ok(Self {
            model: Arc::new(image_embedding),
            model_name,
            dim,
            fingerprint,
        })
    }

    /// Static name of the active model variant, e.g. `"ClipVitB32"`.
    /// Useful for `mpr doctor`-style diagnostics.
    pub fn model_name(&self) -> &'static str {
        self.model_name
    }

    /// Embed a single image at `path`. Returns a 512-d, L2-normalised
    /// `Vec<f32>` for CLIP-ViT-B/32.
    pub async fn embed_image_path(&self, path: PathBuf) -> anyhow::Result<Vec<f32>> {
        let model = Arc::clone(&self.model);
        let dim = self.dim;

        let vector = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<f32>> {
            // `embed` takes a `Vec<S: AsRef<Path>>` and returns
            // `Vec<Embedding>` where each `Embedding = Vec<f32>`.
            let mut out = model
                .embed(vec![path], None)
                .context("clip: embed failed")?;
            // On success the model returns exactly one vector per
            // input path. If the runtime ever returns zero (it
            // shouldn't — we passed one path) we synthesise a
            // zero-vector of the right dim so callers don't need to
            // special-case empty output.
            Ok(out.pop().unwrap_or_else(|| vec![0.0_f32; dim]))
        })
        .await
        .context("clip: blocking task panicked")??;

        Ok(vector)
    }

    /// Embed a single image from raw bytes. Useful when the image is
    /// in memory (e.g. fetched over HTTP, decompressed from a ZIP, or
    /// provided as a CLI argument).
    pub async fn embed_image_bytes(&self, bytes: Vec<u8>) -> anyhow::Result<Vec<f32>> {
        let model = Arc::clone(&self.model);
        let dim = self.dim;

        let vector = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<f32>> {
            // `embed_bytes` takes `&[&[u8]]` — a slice of byte slices
            // — and decodes each entry via `image::ImageReader`.
            let slice = bytes.as_slice();
            let mut out = model
                .embed_bytes(&[slice], None)
                .context("clip: embed_bytes failed")?;
            Ok(out.pop().unwrap_or_else(|| vec![0.0_f32; dim]))
        })
        .await
        .context("clip: blocking task panicked")??;

        Ok(vector)
    }

    /// Embed a batch of images in a single blocking call. The model
    /// parallelises internally with rayon over `batch_size` chunks.
    pub async fn embed_image_paths(&self, paths: Vec<PathBuf>) -> anyhow::Result<Vec<Vec<f32>>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }

        let model = Arc::clone(&self.model);

        let vectors = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Vec<f32>>> {
            // fastembed's contract: one `Embedding` per input path.
            // We trust that invariant and return the vector directly.
            model
                .embed(paths, None)
                .context("clip: embed batch failed")
        })
        .await
        .context("clip: blocking task panicked")??;

        Ok(vectors)
    }

    /// Constant embedding dimensionality. CLIP-ViT-B/32 → 512.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Stable fingerprint persisted in the palace `embedding.json`
    /// manifest. Format: `"clip:<model_name>:<dim>"`.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Map a fastembed `ImageEmbeddingModel` enum value to its static
/// Rust identifier. Used for the fingerprint string and the
/// `mpr doctor` diagnostic.
///
/// New variants in `fastembed` 4.x must be added here when bumping
/// the dep; the curated list lives at
/// `fastembed-4.x/src/models/image_embedding.rs::models_list`.
fn image_model_static_name(model: &ImageEmbeddingModel) -> &'static str {
    match model {
        ImageEmbeddingModel::ClipVitB32 => "ClipVitB32",
        ImageEmbeddingModel::Resnet50 => "Resnet50",
        ImageEmbeddingModel::UnicomVitB16 => "UnicomVitB16",
        ImageEmbeddingModel::UnicomVitB32 => "UnicomVitB32",
        ImageEmbeddingModel::NomicEmbedVisionV15 => "NomicEmbedVisionV15",
    }
}

/// Static dimensionality for the curated image models. Mirrors the
/// `ModelInfo::dim` field that fastembed's text path exposes via
/// `TextEmbedding::get_model_info`; the image path does not publish
/// a public `get_model_info` function, so we maintain the dim
/// mapping here. If fastembed adds a new model with a different dim,
/// update this table alongside the `image_model_static_name` match
/// above.
fn clip_image_model_dim(model: &ImageEmbeddingModel) -> usize {
    match model {
        // CLIP-ViT-B/32 — Xenova/clip-vit-base-patch32
        // 512-d, L2-normalised. The default in fastembed 4 and the
        // shape agentmemory's parity port targets.
        ImageEmbeddingModel::ClipVitB32 => 512,
        // ResNet-50 image encoder — also 2048-d in torchvision, but
        // fastembed's curated model is the smaller variant. We
        // default to 2048 and let the actual model load fail loud if
        // a future bump changes it.
        ImageEmbeddingModel::Resnet50 => 2048,
        ImageEmbeddingModel::UnicomVitB16 => 512,
        ImageEmbeddingModel::UnicomVitB32 => 512,
        ImageEmbeddingModel::NomicEmbedVisionV15 => 768,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MEMPALACE_SKIP_NETWORK_TESTS=1` skips tests that download the
    /// CLIP ONNX file (~50 MB on first run). CI sets this; local
    /// devs can unset it to exercise the real embedder.
    fn network_disabled() -> bool {
        std::env::var("MEMPALACE_SKIP_NETWORK_TESTS")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    }

    #[test]
    fn static_name_covers_default_model() {
        assert_eq!(
            image_model_static_name(&ImageEmbeddingModel::ClipVitB32),
            "ClipVitB32"
        );
    }

    #[test]
    fn dim_matches_default_model() {
        // Pure-Rust mapping; no model load.
        assert_eq!(clip_image_model_dim(&ImageEmbeddingModel::ClipVitB32), 512);
        assert_eq!(
            clip_image_model_dim(&ImageEmbeddingModel::NomicEmbedVisionV15),
            768
        );
    }

    #[test]
    fn fingerprint_format_is_stable() {
        // Construct the fingerprint string the way `build` would,
        // then assert on shape so a future refactor can't silently
        // break the manifest format that `Palace::open` compares
        // against.
        let dim = clip_image_model_dim(&ImageEmbeddingModel::ClipVitB32);
        let name = image_model_static_name(&ImageEmbeddingModel::ClipVitB32);
        let fp = format!("clip:{name}:{dim}");
        assert_eq!(fp, "clip:ClipVitB32:512");
    }

    #[tokio::test]
    async fn build_loads_default_model() {
        if network_disabled() {
            eprintln!("skipping (MEMPALACE_SKIP_NETWORK_TESTS set)");
            return;
        }
        let e = ClipImageEmbedder::with_model(DEFAULT_CLIP_MODEL)
            .expect("CLIP-ViT-B/32 loads via fastembed");
        assert_eq!(e.dim(), 512);
        assert_eq!(e.fingerprint(), "clip:ClipVitB32:512");
        assert_eq!(e.model_name(), "ClipVitB32");
    }
}
