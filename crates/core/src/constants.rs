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

/// Schema version for drawer normalization.
/// Bump when the normalization pipeline changes in a way that existing
/// drawers should be rebuilt (e.g., new noise-stripping rules).
/// v2 (2026-04): introduced strip_noise() for Claude Code JSONL;
/// previous drawers stored system tags / hook chrome verbatim.
pub const NORMALIZE_VERSION: i32 = 2;

/// Directories to skip during mining operations.
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    "coverage",
    ".mempalace",
    ".ruff_cache",
    ".mypy_cache",
    ".pytest_cache",
    ".cache",
    ".tox",
    ".nox",
    ".idea",
    ".vscode",
    ".ipynb_checkpoints",
    ".eggs",
    "htmlcov",
    "target",
];

/// Common capitalized words that look like proper nouns but are usually
/// sentence-starters or filler. Filtered out of entity extraction.
pub const ENTITY_STOPLIST: &[&str] = &[
    "The",
    "This",
    "That",
    "These",
    "Those",
    "When",
    "Where",
    "What",
    "Why",
    "Who",
    "Which",
    "How",
    "After",
    "Before",
    "Then",
    "Now",
    "Here",
    "There",
    "And",
    "But",
    "Or",
    "Yet",
    "So",
    "If",
    "Else",
    "Yes",
    "No",
    "Maybe",
    "Okay",
    "User",
    "Assistant",
    "System",
    "Tool",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Closet character limit — fill closet until ~1500 chars, then start a new one.
pub const CLOSET_CHAR_LIMIT: usize = 1500;

/// How many chars of source content to scan for entities/topics when building closets.
pub const CLOSET_EXTRACT_WINDOW: usize = 5000;
