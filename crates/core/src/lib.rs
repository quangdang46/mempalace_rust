// =====================================================================
// Public API surface
// =====================================================================
//
// These modules form the curated public API rendered on docs.rs.
// Adding a module here is a SemVer commitment — see docs/research/04
// "Stability assessment" before promoting an internal module.

pub mod cli;
pub mod config;
pub mod constants;
pub mod dialect;
pub mod doctor;
pub mod knowledge_graph;
pub mod layers;
pub mod mcp_server;
pub mod miner;
pub mod onboarding;
pub mod searcher;

// =====================================================================
// Internal modules — hidden from docs.rs (mp-006)
// =====================================================================
//
// These remain `pub` so the workspace's `cli`, `bench`, integration
// tests, and the Hermes adapter can reach them, but they are NOT part
// of the curated public API. Hidden via `#[doc(hidden)]` so docs.rs
// only renders the surface above. See research/04 P2 #19.

#[doc(hidden)]
pub mod bm25;
#[doc(hidden)]
pub mod closet_llm;
#[doc(hidden)]
pub mod convo_miner;
#[doc(hidden)]
pub mod corpus_origin;
#[doc(hidden)]
pub mod dedup;
#[doc(hidden)]
pub mod dedup_window;
#[doc(hidden)]
pub mod diary_ingest;
#[doc(hidden)]
pub mod entity_detector;
#[doc(hidden)]
pub mod entity_registry;
#[doc(hidden)]
pub mod event_capture;
#[doc(hidden)]
pub mod exporter;
#[doc(hidden)]
pub mod general_extractor;
#[doc(hidden)]
pub mod hermes_integration;
#[doc(hidden)]
pub mod hooks_cli;
#[doc(hidden)]
pub mod i18n;
#[doc(hidden)]
pub mod instructions;
#[doc(hidden)]
pub mod languages;
#[doc(hidden)]
pub mod llm_client;
#[doc(hidden)]
pub mod llm_refine;
#[doc(hidden)]
pub mod migrate;
#[doc(hidden)]
pub mod mine_lock;
#[doc(hidden)]
pub mod mine_palace_lock;
#[doc(hidden)]
pub mod mine_pid_guard;
#[doc(hidden)]
pub mod normalize;
#[doc(hidden)]

#[doc(hidden)]
pub mod palace_db;
#[doc(hidden)]
pub mod palace_graph;
#[doc(hidden)]
pub mod privacy;
#[doc(hidden)]
pub mod project_scanner;
#[doc(hidden)]
pub mod query_sanitizer;
#[doc(hidden)]
pub mod repair;
#[doc(hidden)]
pub mod room_detector_local;
#[doc(hidden)]
pub mod script_aware;
#[doc(hidden)]
pub mod signal_handler;
#[doc(hidden)]
pub mod spellcheck;
#[doc(hidden)]
pub mod split_mega_files;
#[doc(hidden)]
pub mod sweeper;

// =====================================================================
// New-architecture surface (mp-010 onwards) — Embedder trait
// =====================================================================
//
// Placed AFTER the `#[doc(hidden)]` internal block (mp-006) so this
// module renders on docs.rs as part of the curated public API. See
// docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md ADR-1, ADR-3,
// ADR-8, §3 "Concrete API Sketch".

pub mod embed;

pub use embed::{embedder_from_env, resolve_embedder, DEFAULT_EMBED_MODEL};
pub use embed::{Embedder, EmbeddingManifest, ManifestMismatch, NullEmbedder};

pub use event_capture::{
    EmbedderEvent, EventCapture, EventCaptureBox, MemoryWriteEvent, MultiEventCapture,
    NoOpEventCapture, PostToolEvent, PreToolEvent, SessionStartEvent, StopEvent, UserPromptEvent,
};

// =====================================================================
// New-architecture surface (mp-020 / ADR-3 / ADR-6 / ADR-7)
// =====================================================================

pub mod palace;
pub use palace::{
    Drawer, DrawerId, DrawerKind, FusionMode, MemoryProvider, MemoryScope, Palace, PalaceBuilder,
    SearchHit, SearchScope, StoreTier,
};
// PalaceConfig lives in the builder module; re-export from here for ergonomic public API.
pub use palace::builder::PalaceConfig;
// PalaceStore lives in the palace module (not builder); re-export from palace.
pub use palace::PalaceStore;
// EmbedvecStore is the default concrete store implementation.
pub use palace::store::EmbedvecStore;

#[cfg(feature = "embed-fastembed")]
pub use embed::FastEmbedEmbedder;

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

pub use config::Config;
pub use error::MempalaceError;
pub use privacy::{redact, RedactionConfig, RedactionKind, RedactionResult};

pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    #[non_exhaustive]
    pub enum MempalaceError {
        #[error("IO error: {0}")]
        Io(#[from] std::io::Error),

        #[error("JSON error: {0}")]
        Json(#[from] serde_json::Error),

        #[error("Vector DB error: {0}")]
        VectorDb(String),

        #[error("SQLite error: {0}")]
        Sqlite(#[from] rusqlite::Error),

        #[error("Configuration error: {0}")]
        Config(String),

        #[error("Mining error: {0}")]
        Mining(String),

        #[error("Search error: {0}")]
        Search(String),

        #[error("Knowledge graph error: {0}")]
        KnowledgeGraph(String),

        #[error("Normalize error: {0}")]
        Normalize(String),

        /// Embedding manifest mismatch on `Palace::open` (mp-016 / ADR-8).
        ///
        /// The wrapped [`crate::embed::ManifestMismatch`] carries the
        /// recorded vs runtime values inline so the user-facing message
        /// always includes the recovery command (`mpr migrate --re-embed`).
        ///
        /// `#[error("{0}")]` forwards Display to the inner while keeping
        /// `source()` returning `Some(&inner)`, so callers walking the
        /// anyhow chain can `downcast_ref::<ManifestMismatch>()` on the
        /// inner error. (`#[error(transparent)]` would also forward
        /// Display but additionally forward `source()` past the inner,
        /// hiding it from the chain — see mp-016 test.)
        #[error("{0}")]
        ManifestMismatch(
            #[from]
            #[source]
            crate::embed::ManifestMismatch,
        ),
    }
}
