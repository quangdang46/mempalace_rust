pub mod cli;
pub mod closet_llm;
pub mod config;
pub mod corpus_origin;
pub mod llm_client;
pub mod llm_refine;
pub mod constants;
pub mod convo_miner;
pub mod dedup;
pub mod dialect;
pub mod diary_ingest;
pub mod doctor;
pub mod entity_detector;
pub mod entity_registry;
pub mod exporter;
pub mod fact_checker;
pub mod general_extractor;
pub mod hermes_integration;
pub mod hooks_cli;
pub mod instructions;
pub mod knowledge_graph;
pub mod languages;
pub mod layers;
pub mod mcp_server;
pub mod migrate;
pub mod mine_lock;
pub mod mine_palace_lock;
pub mod miner;
pub mod normalize;
pub mod onboarding;
pub mod onnx_embed;
pub mod palace_db;
pub mod palace_graph;
pub mod project_scanner;
pub mod query_sanitizer;
pub mod repair;
pub mod room_detector_local;
pub mod searcher;
pub mod spellcheck;
pub mod split_mega_files;
pub mod sweeper;

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

pub use config::Config;
pub use error::MempalaceError;

pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
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
    }
}
