// =====================================================================
// PalaceBuilder — construction API for Palace (mp-020 / ADR-7)
// =====================================================================
//
// `PalaceBuilder` pins down all required fields before calling
// `open()`. Mandatory fields: `config` and `embedder`. The store
// defaults to `EmbedvecStore` if not supplied.
//
// ## Example
//
// ```
// let palace = PalaceBuilder::new()
//     .config(PalaceConfig { palace_path: ".mempalace".into(), .. })
//     .embedder(embedder_from_env()?)
//     .open()
//     .await?;
// ```
//
// ## ADR-7: per-project palace lifecycle
//
// The `PalaceConfig::palace_path` field is the canonical per-project
// palace location. The library NEVER reads global XDG config — only
// the explicit `PalaceConfig` passed here. The CLI reads global config
// and forwards it through this builder, so the same binary works for
// both standalone (global palace) and library (per-project palace)
// modes.

use super::Palace;
use std::sync::Arc;

/// Configuration for a palace instance (ADR-7).
///
/// All fields are optional except `palace_path` which is the only
/// mandatory field. Default values are set by `Default::default()`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct PalaceConfig {
    /// Path to the palace directory on disk. Required.
    ///
    /// For jcode integration: `<project_dir>/.jcode/palace`.
    /// For standalone CLI: `~/.mempalace/palace` (or `$XDG_DATA_HOME`).
    pub palace_path: std::path::PathBuf,
    /// Collection name inside the palace. Default: `"mempalace_drawers"`.
    /// Most users never need this — it exists for multi-tenant schemas.
    #[serde(default = "default_collection_name")]
    pub collection_name: String,
    /// Embedding model short name. Default: `"bge-small-en-v15"`.
    /// Only used when `embedder` is not supplied to the builder.
    /// Kept here so `mpr init` can display the model choice.
    #[serde(default = "default_embed_model")]
    pub embed_model: String,
    /// Locale for entity detection and AAAK compression. Default: `"en"`.
    #[serde(default = "default_locale")]
    pub locale: String,
}

fn default_collection_name() -> String {
    "mempalace_drawers".to_string()
}

fn default_embed_model() -> String {
    "bge-small-en-v15".to_string()
}

fn default_locale() -> String {
    "en".to_string()
}

impl Default for PalaceConfig {
    fn default() -> Self {
        Self {
            palace_path: std::path::PathBuf::from("~/.mempalace/palace"),
            collection_name: default_collection_name(),
            embed_model: default_embed_model(),
            locale: default_locale(),
        }
    }
}

/// Builder for [`Palace`]. Construct with [`PalaceBuilder::new`].
pub struct PalaceBuilder {
    config: Option<PalaceConfig>,
    embedder: Option<Arc<dyn crate::embed::Embedder>>,
    store: Option<Arc<dyn super::PalaceStore>>,
    llm: Option<Arc<dyn crate::llm::LlmProvider>>,
    session_store: Option<Arc<crate::session::SessionStore>>,
    /// mp-migration 5/8: optional callback fired from
    /// `Palace::add_drawer` / `forget` / `search` etc. with an
    /// `ActivityEvent`. Used by the jcode adapter to mirror jcode's
    /// `MemoryEventSink = Arc<dyn Fn(ServerEvent)>` and feed the
    /// `MemoryActivity` snapshot the TUI info widget reads.
    activity_sink: Option<Arc<dyn Fn(super::ActivityEvent) + Send + Sync>>,
    /// mp-027 (issue #27): optional wired [`KnowledgeGraph`] for
    /// typed memory edges. When set, the default
    /// [`super::MemoryProvider::tag`] / [`super::MemoryProvider::link`]
    /// / [`super::MemoryProvider::supersede`] implementations also
    /// write `HasTag` / `RelatesTo` / `Supersedes` typed edges.
    kg: Option<Arc<std::sync::Mutex<crate::knowledge_graph::KnowledgeGraph>>>,
    /// Issue #33: enable sidecar relevance verification on search.
    /// When `true`, the `search` pipeline runs the sidecar's
    /// `check_relevance` on every hit and filters out irrelevant results.
    /// Default: `false`.
    verify_search_results: bool,
}

impl std::fmt::Debug for PalaceBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PalaceBuilder")
            .field("config", &self.config.as_ref().map(|_| "..."))
            .field("embedder", &self.embedder.as_ref().map(|_| "..."))
            .field("store", &self.store.as_ref().map(|_| "..."))
            .finish()
    }
}

impl PalaceBuilder {
    /// Start building a `Palace`.
    pub fn new() -> Self {
        Self {
            config: None,
            embedder: None,
            store: None,
            llm: None,
            session_store: None,
            activity_sink: None,
            kg: None,
            verify_search_results: false,
        }
    }

    /// Set the palace configuration (mandatory).
    ///
    /// `palace_path` must be set. All other fields have sensible defaults.
    pub fn config(mut self, config: PalaceConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Set the embedder (mandatory unless `embed-fastembed` is enabled
    /// and the env var approach is acceptable — see [`embed_from_env`]).
    ///
    /// The embedder is stored as `Arc<dyn Embedder>` internally so
    /// `Palace` remains `Send + Sync` regardless of the concrete type.
    pub fn embedder(mut self, embedder: Arc<dyn crate::embed::Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Set the vector store (optional — defaults to `EmbedvecStore`).
    ///
    /// Most hosts don't need this — the default embedvec store handles
    /// up to ~5 k drawers with no configuration. Tier promotion
    /// (embedvec → hnsw_rs → usearch → lancedb) is handled by `mpr doctor`
    /// and the migration tools in Phase 5.
    pub fn store(mut self, store: Arc<dyn super::PalaceStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Set the LLM provider (optional).
    ///
    /// When set, enables LLM-assisted compression and image description.
    /// When `None`, the palace operates without LLM capabilities.
    pub fn llm(mut self, provider: Arc<dyn crate::llm::LlmProvider>) -> Self {
        self.llm = Some(provider);
        self
    }

    /// Set the session store (optional).
    ///
    /// When set, enables session tracking and observation storage.
    pub fn session_store(mut self, store: Arc<crate::session::SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// Set the activity sink (mp-migration 5/8).
    ///
    /// When set, the [`Palace`] fires an [`super::ActivityEvent`] at
    /// every meaningful point in the per-call pipeline (search start,
    /// search done, found relevant, sidecar checking, extracting,
    /// maintaining, tool action). jcode's adapter plugs this into
    /// its `MemoryEventSink = Arc<dyn Fn(ServerEvent)>` to drive the
    /// TUI info widget.
    ///
    /// Default: no sink. Calls complete silently.
    pub fn activity_sink(mut self, sink: Arc<dyn Fn(super::ActivityEvent) + Send + Sync>) -> Self {
        self.activity_sink = Some(sink);
        self
    }

    /// Wire a [`KnowledgeGraph`] for typed memory edges (mp-027, issue #27).
    ///
    /// When set, the default [`super::MemoryProvider::tag`] /
    /// [`super::MemoryProvider::link`] / [`super::MemoryProvider::supersede`]
    /// implementations also create `HasTag` / `RelatesTo` / `Supersedes`
    /// typed edges with the canonical jcode traversal weights, so cascade
    /// retrieval can use the dedicated `edge_kind` and `weight` columns
    /// instead of parsing the predicate string.
    ///
    /// The KG is wrapped in a `Mutex` because the underlying SQLite
    /// connection requires exclusive access for writes (`add_memory_edge`
    /// takes `&mut KnowledgeGraph`).
    ///
    /// Default: no KG. Typed-edge writes are skipped, and the default
    /// impls fall back to the drawer-metadata path only.
    pub fn kg(mut self, kg: Arc<std::sync::Mutex<crate::knowledge_graph::KnowledgeGraph>>) -> Self {
        self.kg = Some(kg);
        self
    }

    /// Enable sidecar relevance verification on search results (issue #33).
    ///
    /// When `true`, the [`super::MemoryProvider::search`] pipeline runs
    /// the sidecar's `check_relevance` on every hit and filters out
    /// irrelevant results before returning. Requires the `llm-sidecar`
    /// feature and an LLM provider to be configured.
    ///
    /// Also enables contradiction detection on [`super::MemoryProvider::add_drawer`]:
    /// new drawers are checked against similar existing drawers, and
    /// contradictions create `Contradicts` KG edges + trigger supersede.
    ///
    /// Default: `false` (no verification, raw search results).
    pub fn verify_search_results(mut self, enabled: bool) -> Self {
        self.verify_search_results = enabled;
        self
    }

    /// Open the palace. Validates all required fields and initializes
    /// storage. Returns an error if config or embedder is missing, or
    /// if the embedder fails to load.
    pub async fn open(self) -> anyhow::Result<Palace> {
        let config = self.config.ok_or_else(|| {
            anyhow::anyhow!(
                "PalaceBuilder: config is mandatory. Call .config(PalaceConfig) before .open()"
            )
        })?;

        // With embed-fastembed: auto-resolve from MEMPALACE_EMBED_MODEL if not set.
        // Without embed-fastembed: embedder is mandatory (no default available).
        #[cfg(feature = "embed-fastembed")]
        let embedder = match self.embedder {
            Some(e) => e,
            None => std::sync::Arc::from(crate::embed::embedder_from_env()?),
        };
        #[cfg(not(feature = "embed-fastembed"))]
        let embedder = self.embedder.ok_or_else(|| {
            anyhow::anyhow!(
                "PalaceBuilder: embedder is mandatory. \
                 Without `embed-fastembed` you must call .embedder(arc_embedder) before .open(). \
                 Rebuild with `--features embed-fastembed` to use MEMPALACE_EMBED_MODEL automatically."
            )
        })?;

        // Ensure palace directory exists first (manifest lives here).
        std::fs::create_dir_all(&config.palace_path)?;

        // Load or create the embedding manifest.
        use crate::embed::EmbeddingManifest;
        let _manifest = match EmbeddingManifest::read(&config.palace_path)? {
            Some(existing) => {
                // Validate: manifest dim/fingerprint must match the embedder.
                // If validation fails, return an actionable error with both
                // the recorded and runtime values so the user knows how to fix it.
                if let Err(err) = existing.validate_against(embedder.as_ref()) {
                    return Err(anyhow::anyhow!(
                        "embedding manifest mismatch: {}
                         Hint: delete {} to re-initialise with the current embedder.",
                        err,
                        EmbeddingManifest::path(&config.palace_path).display()
                    ));
                }
                existing
            }
            None => {
                // First open: write the manifest so future opens can validate.
                let manifest =
                    EmbeddingManifest::from_embedder(embedder.as_ref(), &config.embed_model);
                EmbeddingManifest::write(&config.palace_path, &manifest)?;
                manifest
            }
        };

        let store = if let Some(s) = self.store {
            s
        } else {
            Arc::new(
                crate::EmbedvecStore::new_with_path(
                    embedder.clone(),
                    config.palace_path.clone(),
                    config.embed_model.clone(),
                )
                .await?,
            )
        };

        Ok(Palace {
            embedder,
            store,
            llm: self.llm,
            sessions: self.session_store,
            activity_sink: self.activity_sink,
            kg: self.kg,
            verify_search_results: self.verify_search_results,
        })
    }
}

impl Default for PalaceBuilder {
    fn default() -> Self {
        Self::new()
    }
}
