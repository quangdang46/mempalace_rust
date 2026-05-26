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

pub mod manifest;
pub mod null;

#[cfg(feature = "embed-fastembed")]
pub mod fastembed;

pub use manifest::{EmbeddingManifest, ManifestMismatch};
pub use null::NullEmbedder;

#[cfg(feature = "embed-fastembed")]
pub use fastembed::FastEmbedEmbedder;

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
    // BGE family — default first
    ("bge-small-en-v15", "BGESmallENV15"),
    ("bge-small-en", "BGESmallENV15"),
    ("bge-base-en", "BGEBaseENV15"),
    ("bge-base-en-v15", "BGEBaseENV15"),
    ("bge-large-en", "BGELargeENV15"),
    ("bge-large-en-v15", "BGELargeENV15"),
    // Multilingual E5
    ("multilingual-e5-small", "MultilingualE5Small"),
    ("multilingual-e5-base", "MultilingualE5Base"),
    ("multilingual-e5-large", "MultilingualE5Large"),
    // Nomic / MxBai
    ("nomic-embed-text-v15", "NomicEmbedTextV15"),
    ("mxbai-embed-large", "MxbaiEmbedLargeV1"),
    // Legacy compat with the Python ONNX embedder default
    ("all-minilm-l6-v2", "AllMiniLML6V2"),
    ("all-minilm-l12-v2", "AllMiniLML12V2"),
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

/// Construct the concrete embedder for a resolved fastembed enum
/// identifier. Split out so [`resolve_embedder`] can validate the name
/// independently of the (feature-gated) backend wiring.
#[cfg(feature = "embed-fastembed")]
fn construct_embedder(target: &'static str) -> anyhow::Result<Box<dyn Embedder>> {
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

/// Without `embed-fastembed`, no concrete embedder can be wired up.
/// Validation of the alias still works (so config errors surface even
/// in a `--no-default-features` build) but loading fails with a
/// pointer to the feature flag.
#[cfg(not(feature = "embed-fastembed"))]
fn construct_embedder(target: &'static str) -> anyhow::Result<Box<dyn Embedder>> {
    anyhow::bail!(
        "no embedder backend compiled in. Recognised alias '{target}' \
         requires the `embed-fastembed` feature (the default). Rebuild \
         with `--features embed-fastembed`."
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
