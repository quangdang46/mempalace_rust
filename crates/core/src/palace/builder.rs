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

use super::{Palace, PalaceStore};
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
    store: Option<Arc<dyn PalaceStore>>,
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
    pub fn store(mut self, store: Arc<dyn PalaceStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Open the palace. Validates all required fields and initializes
    /// storage. Returns an error if config or embedder is missing, or
    /// if the embedder fails to load.
    pub async fn open(self) -> anyhow::Result<Palace> {
        let config = self.config.ok_or_else(|| {
            anyhow::anyhow!("PalaceBuilder: config is mandatory. Call .config(PalaceConfig) before .open()")
        })?;

        let embedder = self.embedder.ok_or_else(|| {
            anyhow::anyhow!("PalaceBuilder: embedder is mandatory. Call .embedder(arc_embedder) before .open()")
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
                let manifest = EmbeddingManifest::from_embedder(
                    embedder.as_ref(),
                    &config.embed_model,
                );
                EmbeddingManifest::write(&config.palace_path, &manifest)?;
                manifest
            }
        };

        let store = if let Some(s) = self.store {
            s
        } else {
            // Default: EmbedvecStore matching the embedder dimension.
            // Dimension is validated above; if we reach here, dim matches.
            Arc::new(crate::EmbedvecStore::new()?)
        };

        Ok(Palace { embedder, store })
    }
}

impl Default for PalaceBuilder {
    fn default() -> Self {
        Self::new()
    }
}