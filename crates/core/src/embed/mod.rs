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

#[doc(hidden)]
pub mod manifest;
#[doc(hidden)]
pub mod null;

#[cfg(feature = "embed-fastembed")]
#[doc(hidden)]
pub mod fastembed;

#[cfg(feature = "embed-clip")]
#[doc(hidden)]
pub mod clip_embedder;

#[cfg(feature = "embed-model2vec")]
#[doc(hidden)]
pub mod model2vec;

#[cfg(feature = "embed-tract")]
#[doc(hidden)]
pub mod tract;

#[cfg(feature = "embed-openai")]
#[doc(hidden)]
pub mod openai_remote;

#[cfg(feature = "embed-voyage")]
#[doc(hidden)]
pub mod voyage_remote;

#[cfg(feature = "embed-openrouter")]
#[doc(hidden)]
pub mod openrouter_remote;

#[cfg(feature = "embed-gemini")]
#[doc(hidden)]
pub mod gemini_remote;

#[cfg(feature = "embed-cohere")]
#[doc(hidden)]
pub mod cohere_remote;

// mr-75zk / mr-8mjs: deferred — no qdrant backend in this build.
// When a qdrant client crate is added, port the embedder-identity
// manifest contract from `manifest.rs` to a `qdrant_remote.rs` and
// surface the same three-state `EmbedderIdentity` enum.

pub use manifest::{EmbeddingManifest, ManifestMismatch};
pub use null::NullEmbedder;

#[cfg(feature = "embed-openai")]
pub use openai_remote::OpenAIRemoteEmbedder;

#[cfg(feature = "embed-voyage")]
pub use voyage_remote::VoyageRemoteEmbedder;

#[cfg(feature = "embed-openrouter")]
pub use openrouter_remote::OpenRouterRemoteEmbedder;

#[cfg(feature = "embed-gemini")]
pub use gemini_remote::GeminiRemoteEmbedder;

#[cfg(feature = "embed-cohere")]
pub use cohere_remote::CohereRemoteEmbedder;

#[cfg(feature = "embed-fastembed")]
pub use fastembed::FastEmbedEmbedder;

#[cfg(feature = "embed-clip")]
pub use clip_embedder::{ClipImageEmbedder, DEFAULT_CLIP_MODEL};

#[cfg(feature = "embed-model2vec")]
pub use model2vec::Model2VecEmbedder;

#[cfg(feature = "embed-tract")]
pub use tract::TractEmbedder;

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

// =====================================================================
// `MEMPALACE_EMBED_MODEL` runtime selection (mp-018)
// =====================================================================
//
// Maps the legacy-stable `MEMPALACE_EMBED_MODEL` env var (and any
// short-name string from config) to the right `Embedder`
// implementation. The intent (ADR-1, 05 §A.4 / §C.2 PR 2) is that the
// feature flag picks the *runtime* (`embed-fastembed`,
// `embed-tract`, …) and `MEMPALACE_EMBED_MODEL` picks the *model file*
// inside that runtime.
//
// Today only `fastembed` is wired — `embed-tract` (mp-014) and
// `embed-model2vec` (mp-013) will plug in here as additional matches
// when they land. The function deliberately fails *loud* on an
// unknown short name, listing the accepted ones so users (and CI logs)
// get an actionable message instead of a silent default.

/// Aliases recognised by [`resolve_embedder`]. Order matters only for
/// the human-facing accepted-list error message: the canonical
/// short name appears first for each fastembed model.
const MODEL_ALIASES: &[(&str, &str)] = &[
    ("bge-small-en-v15", "BGESmallENV15"),
    ("bge-small-en", "BGESmallENV15"),
    ("bge-base-en", "BGEBaseENV15"),
    ("bge-base-en-v15", "BGEBaseENV15"),
    ("bge-large-en", "BGELargeENV15"),
    ("bge-large-en-v15", "BGELargeENV15"),
    ("multilingual-e5-small", "MultilingualE5Small"),
    ("multilingual-e5-base", "MultilingualE5Base"),
    ("multilingual-e5-large", "MultilingualE5Large"),
    ("nomic-embed-text-v15", "NomicEmbedTextV15"),
    ("mxbai-embed-large", "MxbaiEmbedLargeV1"),
    ("all-minilm-l6-v2", "AllMiniLML6V2"),
    ("all-minilm-l12-v2", "AllMiniLML12V2"),
    ("potion-base-8M", "M2V:potion-base-8M"),
    ("potion-base-4M", "M2V:potion-base-4M"),
    ("potion-base-2M", "M2V:potion-base-2M"),
    ("potion-multilingual-128M", "M2V:potion-multilingual-128M"),
    ("potion-base-8M-tract", "TRACT:potion-base-8M"),
    ("potion-base-4M-tract", "TRACT:potion-base-4M"),
    // Remote OpenAI-compatible models (feature `embed-openai`).
    ("openai-3-small", "OPENAI:text-embedding-3-small"),
    ("text-embedding-3-small", "OPENAI:text-embedding-3-small"),
    ("openai-3-large", "OPENAI:text-embedding-3-large"),
    ("text-embedding-3-large", "OPENAI:text-embedding-3-large"),
    ("openai-ada-002", "OPENAI:text-embedding-ada-002"),
    ("text-embedding-ada-002", "OPENAI:text-embedding-ada-002"),
    // Remote Voyage AI models (feature `embed-voyage`).
    ("voyage-3", "VOYAGE:voyage-3"),
    ("voyage-3-lite", "VOYAGE:voyage-3-lite"),
    ("voyage-large", "VOYAGE:voyage-large-2"),
    ("voyage-code", "VOYAGE:voyage-code-3"),
    // Remote OpenRouter models (feature `embed-openrouter`). OpenRouter
    // proxies many upstream models; the full model string (with vendor
    // prefix) is preserved in the fingerprint so manifests written via
    // openrouter are distinguishable from manifests written via direct
    // openai.
    (
        "openrouter-3-small",
        "OPENROUTER:openai/text-embedding-3-small",
    ),
    ("or-3-small", "OPENROUTER:openai/text-embedding-3-small"),
    (
        "openrouter-3-large",
        "OPENROUTER:openai/text-embedding-3-large",
    ),
    ("or-3-large", "OPENROUTER:openai/text-embedding-3-large"),
    (
        "openrouter-ada-002",
        "OPENROUTER:openai/text-embedding-ada-002",
    ),
    ("or-ada-002", "OPENROUTER:openai/text-embedding-ada-002"),
    // Remote Google Gemini models (feature `embed-gemini`).
    ("gemini-text-004", "GEMINI:text-embedding-004"),
    ("gemini-001", "GEMINI:embedding-001"),
    ("text-embedding-004", "GEMINI:text-embedding-004"),
    // Remote Cohere models (feature `embed-cohere`).
    ("cohere-english-v3", "COHERE:embed-english-v3.0"),
    ("cohere-multilingual-v3", "COHERE:embed-multilingual-v3.0"),
    ("cohere-english-light", "COHERE:embed-english-light-v3.0"),
    (
        "cohere-multilingual-light",
        "COHERE:embed-multilingual-light-v3.0",
    ),
];

/// Default short name used when `MEMPALACE_EMBED_MODEL` is unset.
/// Pinned to BGE-small so out-of-the-box behaviour matches the
/// upgrade plan (ADR-1) and the README's "no config required" claim.
pub const DEFAULT_EMBED_MODEL: &str = "bge-small-en-v15";

/// Read `MEMPALACE_EMBED_MODEL` (defaulting to [`DEFAULT_EMBED_MODEL`])
/// and return the corresponding [`Embedder`].
///
/// Returns an error when:
///   * the env-var value is not in the accepted-name list, or
///   * the underlying embedder fails to load (e.g. ORT shared lib
///     missing, model download blocked).
///
/// Without `embed-fastembed` enabled the function returns an error
/// pointing the user at the feature flag — no embedder backend is
/// linked in.
pub fn embedder_from_env() -> anyhow::Result<Box<dyn Embedder>> {
    let model =
        std::env::var("MEMPALACE_EMBED_MODEL").unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_owned());
    resolve_embedder(&model)
}

/// Map a short model name (case-insensitive) to a boxed [`Embedder`].
///
/// Names are normalised by lowercasing, so `"BGE-Small-EN"`,
/// `"bge-small-en"`, and `"Bge-Small-En"` all resolve to the same
/// model. Unknown names produce an error whose message lists every
/// accepted short name — keep this contract intact, the test suite
/// asserts on it.
pub fn resolve_embedder(name: &str) -> anyhow::Result<Box<dyn Embedder>> {
    let key = name.trim().to_ascii_lowercase();

    // Resolve the static fastembed enum identifier (e.g.
    // `"BGESmallENV15"`) — same string the
    // `embedding_model_static_name` map produces, so fingerprints are
    // commensurable across CLI / lib / future embedders.
    let target = MODEL_ALIASES
        .iter()
        .find(|(alias, _)| *alias == key)
        .map(|(_, target)| *target);

    let target = match target {
        Some(t) => t,
        None => {
            let accepted: Vec<&str> = MODEL_ALIASES.iter().map(|(a, _)| *a).collect();
            anyhow::bail!(
                "unknown model name '{name}' for MEMPALACE_EMBED_MODEL. \
                 Accepted names (case-insensitive): {}",
                accepted.join(", ")
            );
        }
    };

    construct_embedder(target)
}

/// Try to construct an OpenAI-compatible remote embedder for an `OPENAI:`
/// target. Returns `None` for non-OpenAI targets so callers can fall through.
/// Feature-gated body lives here so both `construct_embedder` variants share it.
fn try_construct_openai(target: &str) -> Option<anyhow::Result<Box<dyn Embedder>>> {
    let model = target.strip_prefix("OPENAI:")?;
    #[cfg(feature = "embed-openai")]
    {
        Some(
            openai_remote::OpenAIRemoteEmbedder::from_env(model)
                .map(|e| Box::new(e) as Box<dyn Embedder>),
        )
    }
    #[cfg(not(feature = "embed-openai"))]
    {
        let _ = model;
        Some(Err(anyhow::anyhow!(
            "openai remote embedder not compiled in. Enable the `embed-openai` feature \
             (e.g. `--features embed-openai`) to use OpenAI-compatible embedding models."
        )))
    }
}

/// Try to construct a Voyage AI remote embedder for a `VOYAGE:`
/// target. Returns `None` for non-Voyage targets so callers can fall through.
/// Feature-gated body lives here so both `construct_embedder` variants share it.
fn try_construct_voyage(target: &str) -> Option<anyhow::Result<Box<dyn Embedder>>> {
    let model = target.strip_prefix("VOYAGE:")?;
    #[cfg(feature = "embed-voyage")]
    {
        Some(
            voyage_remote::VoyageRemoteEmbedder::from_env(model)
                .map(|e| Box::new(e) as Box<dyn Embedder>),
        )
    }
    #[cfg(not(feature = "embed-voyage"))]
    {
        let _ = model;
        Some(Err(anyhow::anyhow!(
            "voyage remote embedder not compiled in. Enable the `embed-voyage` feature \
             (e.g. `--features embed-voyage`) to use Voyage AI embedding models."
        )))
    }
}

/// Try to construct an OpenRouter remote embedder for an `OPENROUTER:`
/// target. Returns `None` for non-OpenRouter targets so callers can fall through.
/// Feature-gated body lives here so both `construct_embedder` variants share it.
fn try_construct_openrouter(target: &str) -> Option<anyhow::Result<Box<dyn Embedder>>> {
    let model = target.strip_prefix("OPENROUTER:")?;
    #[cfg(feature = "embed-openrouter")]
    {
        Some(
            openrouter_remote::OpenRouterRemoteEmbedder::from_env(model)
                .map(|e| Box::new(e) as Box<dyn Embedder>),
        )
    }
    #[cfg(not(feature = "embed-openrouter"))]
    {
        let _ = model;
        Some(Err(anyhow::anyhow!(
            "openrouter remote embedder not compiled in. Enable the `embed-openrouter` feature \
             (e.g. `--features embed-openrouter`) to use OpenRouter embedding models."
        )))
    }
}

/// Try to construct a Cohere remote embedder for a `COHERE:`
/// target. Returns `None` for non-Cohere targets so callers can fall through.
/// Feature-gated body lives here so both `construct_embedder` variants share it.
fn try_construct_cohere(target: &str) -> Option<anyhow::Result<Box<dyn Embedder>>> {
    let model = target.strip_prefix("COHERE:")?;
    #[cfg(feature = "embed-cohere")]
    {
        Some(
            cohere_remote::CohereRemoteEmbedder::from_env(model)
                .map(|e| Box::new(e) as Box<dyn Embedder>),
        )
    }
    #[cfg(not(feature = "embed-cohere"))]
    {
        let _ = model;
        Some(Err(anyhow::anyhow!(
            "cohere remote embedder not compiled in. Enable the `embed-cohere` feature \
             (e.g. `--features embed-cohere`) to use Cohere embedding models."
        )))
    }
}

/// Try to construct a Google Gemini remote embedder for a `GEMINI:`
/// target. Returns `None` for non-Gemini targets so callers can fall through.
/// Feature-gated body lives here so both `construct_embedder` variants share it.
fn try_construct_gemini(target: &str) -> Option<anyhow::Result<Box<dyn Embedder>>> {
    let model = target.strip_prefix("GEMINI:")?;
    #[cfg(feature = "embed-gemini")]
    {
        Some(match gemini_remote::GeminiRemoteEmbedder::from_env(model) {
            Some(e) => Ok(Box::new(e) as Box<dyn Embedder>),
            None => Err(anyhow::anyhow!(
                "GEMINI_API_KEY (or GOOGLE_API_KEY) is required for the gemini remote embedder"
            )),
        })
    }
    #[cfg(not(feature = "embed-gemini"))]
    {
        let _ = model;
        Some(Err(anyhow::anyhow!(
            "gemini remote embedder not compiled in. Enable the `embed-gemini` feature \
             (e.g. `--features embed-gemini`) to use Google Gemini embedding models."
        )))
    }
}

/// Construct the concrete embedder for a resolved model alias.
/// Split out so [`resolve_embedder`] can validate the name
/// independently of the (feature-gated) backend wiring.
#[cfg(feature = "embed-fastembed")]
fn construct_embedder(target: &str) -> anyhow::Result<Box<dyn Embedder>> {
    if let Some(res) = try_construct_openai(target) {
        return res;
    }
    if let Some(res) = try_construct_openrouter(target) {
        return res;
    }
    if let Some(res) = try_construct_voyage(target) {
        return res;
    }
    if let Some(res) = try_construct_gemini(target) {
        return res;
    }
    if let Some(res) = try_construct_cohere(target) {
        return res;
    }
    if target.starts_with("M2V:") {
        #[cfg(feature = "embed-model2vec")]
        {
            return construct_model2vec_embedder(target);
        }
        #[cfg(not(feature = "embed-model2vec"))]
        {
            anyhow::bail!(
                "model2vec backend not compiled in. Enable `embed-model2vec` feature \
                 or use one of the fastembed models: bge-small-en-v15, bge-base-en, etc."
            );
        }
    }
    if target.starts_with("TRACT:") {
        #[cfg(feature = "embed-tract")]
        {
            return construct_tract_embedder(target);
        }
        #[cfg(not(feature = "embed-tract"))]
        {
            anyhow::bail!(
                "tract backend not compiled in. Enable `embed-tract` feature \
                 or use one of the fastembed models: bge-small-en-v15, bge-base-en, etc."
            );
        }
    }
    use ::fastembed::EmbeddingModel;

    let model = match target {
        "BGESmallENV15" => EmbeddingModel::BGESmallENV15,
        "BGEBaseENV15" => EmbeddingModel::BGEBaseENV15,
        "BGELargeENV15" => EmbeddingModel::BGELargeENV15,
        "MultilingualE5Small" => EmbeddingModel::MultilingualE5Small,
        "MultilingualE5Base" => EmbeddingModel::MultilingualE5Base,
        "MultilingualE5Large" => EmbeddingModel::MultilingualE5Large,
        "NomicEmbedTextV15" => EmbeddingModel::NomicEmbedTextV15,
        "MxbaiEmbedLargeV1" => EmbeddingModel::MxbaiEmbedLargeV1,
        "AllMiniLML6V2" => EmbeddingModel::AllMiniLML6V2,
        "AllMiniLML12V2" => EmbeddingModel::AllMiniLML12V2,
        other => anyhow::bail!(
            "internal: alias mapped to unknown fastembed model '{other}' — \
             extend `construct_embedder` when adding new aliases"
        ),
    };
    let embedder = FastEmbedEmbedder::with_model(model)?;
    Ok(Box::new(embedder))
}

#[cfg(feature = "embed-model2vec")]
fn construct_model2vec_embedder(target: &str) -> anyhow::Result<Box<dyn Embedder>> {
    let model_name = target.strip_prefix("M2V:").unwrap_or(target);
    let embedder = Model2VecEmbedder::with_model(model_name.to_owned(), None)?;
    Ok(Box::new(embedder))
}

#[cfg(feature = "embed-tract")]
fn construct_tract_embedder(target: &str) -> anyhow::Result<Box<dyn Embedder>> {
    let model_path = target.strip_prefix("TRACT:").unwrap_or(target);
    let embedder = TractEmbedder::with_model(model_path.to_owned(), None)?;
    Ok(Box::new(embedder))
}

/// Without `embed-fastembed`, no concrete embedder can be wired up.
/// Validation of the alias still works (so config errors surface even
/// in a `--no-default-features` build) but loading fails with a
/// pointer to the feature flag.
#[cfg(not(feature = "embed-fastembed"))]
fn construct_embedder(target: &str) -> anyhow::Result<Box<dyn Embedder>> {
    if let Some(res) = try_construct_openai(target) {
        return res;
    }
    if let Some(res) = try_construct_openrouter(target) {
        return res;
    }
    if let Some(res) = try_construct_voyage(target) {
        return res;
    }
    if let Some(res) = try_construct_gemini(target) {
        return res;
    }
    if let Some(res) = try_construct_cohere(target) {
        return res;
    }
    if target.starts_with("M2V:") {
        #[cfg(feature = "embed-model2vec")]
        {
            return construct_model2vec_embedder(target);
        }
    }
    if target.starts_with("TRACT:") {
        #[cfg(feature = "embed-tract")]
        {
            return construct_tract_embedder(target);
        }
    }
    anyhow::bail!(
        "no embedder backend compiled in. Recognised alias '{target}' \
         requires the `embed-fastembed`, `embed-model2vec` or `embed-tract` feature. \
         Rebuild with `--features embed-fastembed` (default), `--features embed-model2vec`, \
         or `--features embed-tract`."
    )
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn unknown_name_lists_accepted() {
        let _g = crate::test_env_lock().lock();
        let err = match resolve_embedder("does-not-exist") {
            Ok(_) => panic!("unknown short name must be rejected"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("bge-small-en-v15"));
        assert!(msg.contains("multilingual-e5-small"));
        assert!(msg.contains("all-minilm-l6-v2"));
    }

    #[test]
    fn alias_is_case_insensitive_at_parse_time() {
        // We can't safely assert on construction succeeding without a
        // network and `embed-fastembed` (the model would have to
        // download). What we *can* assert is that the failure mode is
        // NOT "unknown model name" — i.e. the parse step accepts a
        // mixed-case alias.
        let _g = crate::test_env_lock().lock();
        match resolve_embedder("BGE-Small-EN") {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("unknown model name"),
                    "case-insensitive parse rejected mixed-case alias: {msg}"
                );
            }
        }
    }

    #[test]
    fn alias_table_targets_resolve_in_construct() {
        // Smoke: every alias points to a target string that
        // `construct_embedder` knows about. If someone adds an alias
        // without extending the match arm, this fails fast.
        for (alias, target) in MODEL_ALIASES {
            let r = construct_embedder(target);
            // We don't care whether the model loaded; we care that
            // we never hit the "internal: alias mapped to unknown
            // fastembed model" branch.
            if let Err(e) = r {
                let msg = e.to_string();
                assert!(
                    !msg.contains("internal: alias mapped to unknown"),
                    "alias '{alias}' -> '{target}' has no construct arm: {msg}"
                );
            }
        }
    }
}
