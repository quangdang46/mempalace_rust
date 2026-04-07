pub mod config;
pub mod convo_miner;
pub mod dialect;
pub mod doctor;
pub mod entity_detector;
pub mod entity_registry;
pub mod general_extractor;
pub mod knowledge_graph;
pub mod layers;
pub mod miner;
pub mod normalize;
pub mod onboarding;
pub mod mcp_server;
pub mod palace_db;
pub mod palace_graph;
pub mod room_detector_local;
pub mod searcher;
pub mod spellcheck;
pub mod split_mega_files;

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
