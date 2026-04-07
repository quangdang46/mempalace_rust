//! constants.rs — MemPalace shared constants
//!
//! Extracted magic numbers and configuration constants.
//! All modules should import from here instead of hardcoding values.

/// Default chunk size for file parsing (tokens/characters).
pub const CHUNK_SIZE: usize = 800;

/// Overlap between consecutive chunks for context continuity.
pub const CHUNK_OVERLAP: usize = 100;

/// Default number of search results to return.
pub const DEFAULT_N_RESULTS: usize = 5;

/// Maximum entries to return in diary list operations.
pub const MAX_DIARY_ENTRIES: usize = 1000;

/// Maximum timeline entries to return.
pub const TIMELINE_LIMIT: usize = 100;

/// Minimum segment length to be considered valid content.
pub const MIN_SEGMENT_LENGTH: usize = 20;

/// Minimum prompt length to be considered meaningful.
pub const MIN_PROMPT_LENGTH: usize = 5;

/// Minimum chunk size to preserve.
pub const MIN_CHUNK_SIZE: usize = 50;

/// Snippet truncation length for display.
pub const SNIPPET_TRUNCATE_LEN: usize = 100;

/// Similarity threshold for duplicate detection.
pub const DUPLICATE_THRESHOLD: f64 = 0.9;

/// Maximum similarity score (used for clamping).
pub const MAX_SIMILARITY: f64 = 1.0;

/// Default graph traversal maximum depth (hops).
pub const DEFAULT_MAX_HOPS: usize = 2;

/// Default collection name for palace drawers.
pub const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";

/// AAAK entity code length (first N chars of name).
pub const AAAK_CODE_LENGTH: usize = 3;

/// AAAK project code length (first N chars of name).
pub const AAAK_PROJECT_CODE_LENGTH: usize = 4;
