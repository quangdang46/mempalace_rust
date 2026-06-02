// =====================================================================
// Public API surface
// =====================================================================

#![allow(deprecated)]
//
// These modules form the curated public API rendered on docs.rs.
// Adding a module here is a SemVer commitment — see docs/research/04
// "Stability assessment" before promoting an internal module.

pub mod auto_forget;
pub mod cli;
pub mod compress;
pub mod compress_synthetic;
pub mod config;
pub mod consolidation;
pub mod consolidation_pipeline;
pub mod constants;
pub mod dialect;
pub mod doctor;
pub mod evict;
pub mod knowledge_graph;
pub mod layers;
pub mod llm;
pub mod mcp;
pub mod mcp_server;
pub mod memory_lifecycle;
pub mod miner;
pub mod onboarding;
pub mod prompts;
pub mod retention;
pub mod search;
pub mod searcher;
pub mod session;
pub mod types;

// =====================================================================
// Internal modules — hidden from docs.rs (mp-006)
// =====================================================================
//
// These remain `pub` so the workspace's `cli`, `bench`, integration
// tests, and the Hermes adapter can reach them, but they are NOT part
// of the curated public API. Hidden via `#[doc(hidden)]` so docs.rs
// only renders the surface above. See research/04 P2 #19.

pub mod audit;
pub mod auth;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod bm25;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod closet_llm;
pub mod context;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod convo_miner;
pub mod coordination;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod corpus_origin;
pub mod crystallize;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod dedup;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod dedup_window;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod diary_ingest;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod entity_detector;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod entity_registry;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod event_capture;
pub mod export;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod exporter;
pub mod facets;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod general_extractor;
pub mod graph_extraction;
pub mod graph_retrieval;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod hermes_integration;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod i18n;
pub mod insight_store;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod instructions;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod languages;
pub mod lessons;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod llm_client;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod llm_refine;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod migrate;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod mine_lock;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod mine_palace_lock;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod mine_pid_guard;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod normalize;
#[doc(hidden)]
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod palace_db;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod palace_graph;
pub mod patterns;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod privacy;
pub mod profile;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod project_scanner;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod query_sanitizer;
pub mod reflect;
pub mod relations;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod repair;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod room_detector_local;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod script_aware;
pub mod sentinels;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod signal_handler;
pub mod sketches;
pub mod slots;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod spellcheck;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod split_mega_files;
pub mod summarize;
#[doc(hidden)]
#[deprecated(since = "0.2.0", note = "use palace:: or embed:: API instead")]
pub mod sweeper;
pub mod temporal_graph;
pub mod timeline;
pub mod vision;
pub mod working_memory;

// =====================================================================
// Health monitoring (feature-gated)
// =====================================================================
//
// Wires the HealthMonitor into the REST API /healthz and /livez endpoints.
// Default-off to avoid pulling in sysinfo for pure-CLI builds.

#[cfg(feature = "health")]
pub mod health;

#[cfg(feature = "health")]
pub use health::{
    get_health_monitor, init_health_monitor, CheckResult, HealthCheck, HealthMonitor, HealthReport,
    HealthStatus,
};

// =====================================================================
// Phase 8 — AgentMemory MCP expansion (internal, evolving)
// =====================================================================

#[doc(hidden)]
pub mod access_tracker;
#[doc(hidden)]
pub mod branch_aware;
#[doc(hidden)]
pub mod cascade;
#[doc(hidden)]
pub mod claude_bridge;
#[doc(hidden)]
pub mod compress_file;
#[doc(hidden)]
pub mod enrich;
#[doc(hidden)]
pub mod file_index;
#[doc(hidden)]
pub mod flow_compress;
#[doc(hidden)]
pub mod governance;
#[doc(hidden)]
pub mod heal;
#[doc(hidden)]
pub mod observe;
#[doc(hidden)]
pub mod obsidian_export;
#[doc(hidden)]
pub mod replay;
#[doc(hidden)]
pub mod skill_extract;
#[doc(hidden)]
pub mod sliding_window;
#[doc(hidden)]
pub mod verify;
// Agent adapter system — `mpr connect <agent-name>` for wiring MCP config.
pub mod connect;

// =====================================================================
// Background task runner (internal)
// =====================================================================

#[doc(hidden)]
pub mod background;

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

#[cfg(feature = "embed-model2vec")]
pub use embed::Model2VecEmbedder;

// =====================================================================
// Live-graph viewer SPA (internal — REMAINING.md G5 follow-up)
// =====================================================================
//
// Serves a self-contained HTML+JS+CSS SPA from /viewer/ on the REST API.
// The assets are embedded at compile time via `include_str!` so the
// binary ships as a single file. The frontend is a minimal stub that
// fetches /api/graph/stats + /api/graph/data when present, and best-
// effort connects to /api/graph/stream (SSE) — when those endpoints
// land (separate follow-up), the SPA gains live updates without any
// binary change.

#[doc(hidden)]
pub mod viewer;
pub use viewer::{viewer_app_js, viewer_html, viewer_styles_css};

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
