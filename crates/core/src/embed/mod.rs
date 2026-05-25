// =====================================================================
// Pluggable embedding backend (mp-010 / ADR-1 / ADR-8)
// =====================================================================
//
// `Embedder` is the public trait that lets hosts (jcode, OpenClaw, third
// parties) inject their own embedding implementation so that exactly one
// model is loaded per process. It also lets us swap between
// `fastembed-rs`, `model2vec-rs`, `tract-onnx`, and remote APIs without
// touching the search/store path.
//
// References:
//   - docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md, ADR-1, ADR-8,
//     §3 "Concrete API Sketch"
//   - docs/research/05_embedding_and_storage_native.md §A.5
//     "Reuse vs trait — the jcode question"
//   - docs/research/03_jcode_memory_internals.md §10.4
//
// Contract notes (per ADR-8):
//   * `dim()` MUST return the constant dimensionality of the model and is
//     validated at `Palace::open` against the stored `embedding.json`
//     manifest (mp-015 / mp-016).
//   * `fingerprint()` SHOULD be a stable identifier of the form
//     `"<model_name>:<tokenizer_hash>"` (or any unique string the
//     implementation chooses). It is persisted on first write and
//     compared on subsequent opens; a mismatch fails loud.
//   * The trait is `async_trait`-based so remote-API embedders
//     (OpenAI/Cohere/Voyage/Ollama) and local-but-blocking ONNX
//     embedders both fit. Local CPU-bound implementations should park
//     work on `tokio::task::spawn_blocking` internally.
//   * `Send + Sync + 'static` is non-negotiable — jcode threads the
//     embedder through `Arc<dyn Embedder>` across worker tasks.
//
// Migration debt:
//   The trait currently returns `anyhow::Result<…>` because the curated
//   `crate::Result` alias from §3 of the upgrade plan does not yet
//   exist. When mp-022 (or whichever issue introduces
//   `pub use error::{Error, Result}`) lands, retype these signatures to
//   `crate::Result<…>`. See ADR-3 for the planned error story.

use async_trait::async_trait;

pub mod null;

pub use null::NullEmbedder;

/// Pluggable embedding backend. Hosts can inject their own.
///
/// See the module-level docs for the contract: dimensionality
/// validation, fingerprint format, async semantics, and the
/// `Send + Sync + 'static` requirement that lets implementations be
/// shared across `tokio` worker tasks via `Arc<dyn Embedder>`.
#[async_trait]
pub trait Embedder: Send + Sync + 'static {
    /// Constant dimensionality of the embedding vectors this backend
    /// produces. Used by `Palace::open` to validate against the stored
    /// `embedding.json` manifest (ADR-8). Must never change for a
    /// given `(model_name, tokenizer)` pair.
    fn dim(&self) -> usize;

    /// Stable identifier for this embedder, persisted on first write
    /// and compared on subsequent opens. Conventionally
    /// `"<model_name>:<tokenizer_hash>"`, e.g.
    /// `"BAAI/bge-small-en-v1.5:sha256:…"` or `"null:384"`.
    fn fingerprint(&self) -> &str;

    /// Embed a single string. Implementations SHOULD route through
    /// `embed_batch` internally to share tokenisation buffers.
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;

    /// Embed a batch of strings. Returns one vector per input, in
    /// order. Implementations should reject empty inputs cheaply
    /// rather than load a model.
    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time + runtime check that `dyn Embedder` is object-safe
    /// and can be used behind `Box<dyn Embedder>` / `Arc<dyn Embedder>`,
    /// as required by `PalaceBuilder::embedder` (§3 Concrete API).
    #[test]
    fn embedder_is_dyn_compatible() {
        let _: Box<dyn Embedder> = Box::new(NullEmbedder::new(384));
    }
}
