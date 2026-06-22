# Public API surface


> **⚠️ Outdated:** This snapshot was taken at v0.1.x baseline (May 2026).
> The project is now at **v0.6.5** (June 2026). The API has evolved significantly.
> See `CHANGELOG.md` or `cargo doc --open` for the current public API.

Public API surface — snapshot at commit `8c66350e3ee3cb8229ae60708424d866482e8036` on 2026-05-25. This is the v0.1.x baseline. Phase 2 will replace this with a curated `Palace` facade; this file documents what consumers depend on TODAY.

The crate is `mempalace-core` (workspace crate at `crates/core`). The library entry point is `crates/core/src/lib.rs`.

## Scope

This snapshot lists only the **curated public API** — i.e., modules in `lib.rs` that are NOT marked `#[doc(hidden)]`. `mp-006` (in flight) hid the bulk of the workspace-internal modules behind `#[doc(hidden)]` so they remain `pub` for `crates/cli`, `crates/bench`, integration tests, and the Hermes adapter, but do not render on docs.rs and are not part of the SemVer surface.

The 11 curated public modules are: `cli`, `config`, `constants`, `dialect`, `doctor`, `knowledge_graph`, `layers`, `mcp_server`, `miner`, `onboarding`, `searcher`. Plus the inline `error` module and root re-exports.

`#[doc(hidden)]` modules excluded from this snapshot: `bm25`, `closet_llm`, `convo_miner`, `corpus_origin`, `dedup`, `diary_ingest`, `entity_detector`, `entity_registry`, `exporter`, `fact_checker`, `general_extractor`, `hermes_integration`, `hooks_cli`, `i18n`, `instructions`, `languages`, `llm_client`, `llm_refine`, `migrate`, `mine_lock`, `mine_palace_lock`, `mine_pid_guard`, `normalize`, `onnx_embed`, `palace_db`, `palace_graph`, `project_scanner`, `query_sanitizer`, `repair`, `room_detector_local`, `script_aware`, `signal_handler`, `spellcheck`, `split_mega_files`, `sweeper`.

Note: a few public signatures below leak `#[doc(hidden)]` types (e.g. `miner::Miner::new` takes `Vec<RoomMapping>` from the hidden `room_detector_local` module; `mcp_server::AppState` exposes `palace_db::PalaceDb` as a `pub` field). These leaks are intentional in v0.1.x and will be resolved by the Phase 2 `Palace` facade.

---

## Root re-exports (`mempalace_core`)

```rust
pub use config::Config;
pub use error::MempalaceError;
```

---

## Module: `cli`

### Functions

```rust
pub fn run() -> Result<()>;
```

---

## Module: `config`

### Free functions

```rust
pub fn normalize_wing_name(name: &str) -> String;

pub fn sanitize_iso_temporal(
    value: Option<&str>,
    field_name: &str,
) -> anyhow::Result<Option<String>>;
```

### Struct: `Config`

Public fields:

```rust
pub struct Config {
    pub palace_path: PathBuf,
    pub collection_name: String,
    pub people_map: HashMap<String, String>,
    pub topic_wings: Vec<String>,
    pub hall_keywords: HashMap<String, Vec<String>>,
    pub embedding_model: String,
    pub languages: Vec<String>,
}
```

Methods:

```rust
impl Config {
    pub fn identity_file_path() -> anyhow::Result<PathBuf>;
    pub fn init(&self) -> anyhow::Result<PathBuf>;
    pub fn load() -> anyhow::Result<Self>;
    pub fn load_people_map(&self) -> anyhow::Result<HashMap<String, String>>;
    pub fn registry_file_path() -> anyhow::Result<PathBuf>;
    pub fn save(&self) -> anyhow::Result<()>;
    pub fn save_people_map(
        &self,
        people_map: &HashMap<String, String>,
    ) -> anyhow::Result<PathBuf>;
    pub fn tunnel_file(&self) -> PathBuf;
}
```

---

## Module: `constants`

### Constants

```rust
pub const AAAK_CODE_LENGTH: usize = 3;
pub const AAAK_PROJECT_CODE_LENGTH: usize = 4;
pub const CHUNK_OVERLAP: usize = 100;
pub const CHUNK_SIZE: usize = 800;
pub const CLOSET_CHAR_LIMIT: usize = 1500;
pub const CLOSET_EXTRACT_WINDOW: usize = 5000;
pub const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";
pub const DEFAULT_MAX_HOPS: usize = 2;
pub const DEFAULT_N_RESULTS: usize = 5;
pub const DUPLICATE_THRESHOLD: f64 = 0.9;
pub const ENTITY_STOPLIST: &[&str];                  // initialiser list
pub const MAX_DIARY_ENTRIES: usize = 1000;
pub const MAX_SIMILARITY: f64 = 1.0;
pub const MIN_CHUNK_SIZE: usize = 50;
pub const MIN_PROMPT_LENGTH: usize = 5;
pub const MIN_SEGMENT_LENGTH: usize = 20;
pub const NORMALIZE_VERSION: i32 = 2;
pub const SKIP_DIRS: &[&str];                        // initialiser list
pub const SNIPPET_TRUNCATE_LEN: usize = 100;
pub const TIMELINE_LIMIT: usize = 100;
```

---

## Module: `dialect`

### Free functions

```rust
pub fn compress(text: &str, people_map: &HashMap<String, String>) -> String;

pub fn compress_with_metadata(
    text: &str,
    people_map: &HashMap<String, String>,
    metadata: Option<&HashMap<String, serde_json::Value>>,
) -> String;

pub fn compression_stats(original: &str, compressed: &str) -> CompressionStats;

pub fn count_tokens(text: &str) -> usize;

pub fn decompress(aaak_text: &str, _people_map: &HashMap<String, String>) -> String;

pub fn get_aaak_spec() -> &'static str;
```

### Struct: `CompressionStats`

```rust
pub struct CompressionStats {
    pub original_chars: usize,
    pub original_tokens_est: usize,
    pub note: &'static str,
    pub size_ratio: f64,
    pub summary_chars: usize,
    pub summary_tokens_est: usize,
}
```

---

## Module: `doctor`

### Free functions

```rust
pub fn run_doctor(palace_path: &Path) -> anyhow::Result<DoctorReport>;
```

### Enum: `CheckStatus`

```rust
pub enum CheckStatus {
    Fail,
    Pass,
    Warn,
}
```

### Struct: `CheckResult`

```rust
pub struct CheckResult {
    pub message: String,
    pub name: String,
    pub status: CheckStatus,
}
```

### Struct: `DoctorReport`

```rust
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
    pub healthy: bool,
}
```

---

## Module: `error`

### Enum: `MempalaceError`

```rust
pub enum MempalaceError {
    Config(String),
    Io(#[from] std::io::Error),
    Json(#[from] serde_json::Error),
    KnowledgeGraph(String),
    Mining(String),
    Normalize(String),
    Search(String),
    Sqlite(#[from] rusqlite::Error),
    VectorDb(String),
}
```

Implements `std::error::Error` and `Debug` via `thiserror::Error`.

---

## Module: `knowledge_graph`

### Struct: `Entity`

```rust
pub struct Entity {
    pub entity_type: String,
    pub id: String,
    pub name: String,
    pub properties: serde_json::Value,
}
```

### Struct: `EntityQueryResult`

```rust
pub struct EntityQueryResult {
    pub adapter_name: Option<String>,
    pub confidence: Option<f64>,
    pub current: bool,
    pub direction: String,
    pub object: String,
    pub predicate: String,
    pub source_closet: Option<String>,
    pub source_drawer_id: Option<String>,
    pub source_file: Option<String>,
    pub subject: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
}
```

### Struct: `KgStats`

```rust
pub struct KgStats {
    pub current_facts: usize,
    pub expired_facts: usize,
    pub relationship_types: Vec<String>,
    pub total_entities: usize,
    pub total_triples: usize,
}
```

### Struct: `KnowledgeGraph`

```rust
impl KnowledgeGraph {
    pub fn add_entity(
        &mut self,
        name: &str,
        entity_type: &str,
        properties: Option<&serde_json::Value>,
    ) -> anyhow::Result<String>;

    pub fn add_triple(
        &mut self,
        subject: &str,
        predicate: &str,
        object: &str,
        valid_from: Option<&str>,
        valid_to: Option<&str>,
        confidence: Option<f64>,
        source_closet: Option<&str>,
        source_file: Option<&str>,
        source_drawer_id: Option<&str>,
        adapter_name: Option<&str>,
    ) -> anyhow::Result<String>;

    pub fn get_feedback(
        &self,
        drawer_id: &str,
    ) -> anyhow::Result<Vec<(String, String)>>;

    pub fn helpfulness_score(&self, drawer_id: &str) -> anyhow::Result<f64>;

    pub fn invalidate(
        &mut self,
        subject: &str,
        predicate: &str,
        object: &str,
        ended: Option<&str>,
    ) -> anyhow::Result<()>;

    pub fn open(db_path: &Path) -> anyhow::Result<Self>;

    pub fn query_entity(
        &self,
        name: &str,
        as_of: Option<&str>,
        direction: &str,
    ) -> anyhow::Result<Vec<EntityQueryResult>>;

    pub fn query_relationship(
        &self,
        predicate: &str,
        as_of: Option<&str>,
    ) -> anyhow::Result<Vec<Triple>>;

    pub fn record_feedback(
        &self,
        drawer_id: &str,
        query: &str,
        outcome: &str,
    ) -> anyhow::Result<()>;

    pub fn stats(&self) -> anyhow::Result<KgStats>;

    pub fn timeline(&self, entity_name: Option<&str>) -> anyhow::Result<Vec<Triple>>;
}
```

### Struct: `Triple`

```rust
pub struct Triple {
    pub adapter_name: Option<String>,
    pub confidence: Option<f64>,
    pub current: bool,
    pub object: String,
    pub predicate: String,
    pub source_closet: Option<String>,
    pub source_drawer_id: Option<String>,
    pub source_file: Option<String>,
    pub subject: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
}
```

---

## Module: `layers`

### Struct: `DeepSearchStatus`

```rust
pub struct DeepSearchStatus {
    pub description: String,
}
```

### Struct: `EssentialStatus`

```rust
pub struct EssentialStatus {
    pub description: String,
}
```

### Struct: `IdentityStatus`

```rust
pub struct IdentityStatus {
    pub exists: bool,
    pub path: PathBuf,
    pub tokens: usize,
}
```

### Struct: `Layer0`

```rust
impl Layer0 {
    pub fn new(identity_path: Option<PathBuf>) -> Self;
    pub fn render(&mut self) -> String;
    pub fn token_estimate(&mut self) -> usize;
}
```

### Struct: `Layer1`

```rust
impl Layer1 {
    pub const MAX_CHARS: usize = 3200;
    pub const MAX_DRAWERS: usize = 15;

    pub fn generate(&self, palace_db: &PalaceDb) -> String;  // PalaceDb is doc(hidden)
    pub fn new(wing: Option<String>) -> Self;
}
```

### Struct: `Layer2`

```rust
impl Layer2 {
    pub fn new() -> Self;
    pub fn retrieve(
        &self,
        palace_db: &PalaceDb,                                // doc(hidden)
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String;
}
```

### Struct: `Layer3`

```rust
impl Layer3 {
    pub fn new() -> Self;

    pub async fn search(
        &self,
        palace_db: &PalaceDb,                                // doc(hidden)
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String;

    pub async fn search_raw(/* ... */) -> /* ... */;
}
```

### Struct: `LayerStatus`

```rust
pub struct LayerStatus {
    pub identity_path: PathBuf,
    pub l0_identity: IdentityStatus,
    pub l1_essential: EssentialStatus,
    pub l2_on_demand: OnDemandStatus,
    pub l3_deep_search: DeepSearchStatus,
    pub palace_path: PathBuf,
    pub total_drawers: usize,
}
```

### Struct: `MemoryStack`

```rust
impl MemoryStack {
    pub fn new(palace_path: Option<PathBuf>, identity_path: Option<PathBuf>) -> Self;

    pub fn recall(
        &self,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String;

    pub async fn search(
        &self,
        query: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> String;

    pub fn status(&self) -> LayerStatus;

    pub async fn wake_up(&mut self, wing: Option<&str>) -> String;
}
```

### Struct: `OnDemandStatus`

```rust
pub struct OnDemandStatus {
    pub description: String,
}
```

### Struct: `SearchHit`

```rust
pub struct SearchHit {
    pub metadata: HashMap<String, serde_json::Value>,
    pub room: Option<String>,
    pub similarity: f64,
    pub source_file: String,
    pub text: String,
    pub wing: Option<String>,
}
```

---

## Module: `mcp_server`

### Free functions

```rust
pub fn is_mutation_tool(tool_name: &str) -> bool;

pub fn run_server(
    palace_override: Option<&str>,
    read_only: bool,
) -> anyhow::Result<()>;
```

### Struct: `AppState`

```rust
pub struct AppState {
    pub config: crate::Config,
    pub db: crate::palace_db::PalaceDb,                      // doc(hidden) leak
    pub palace_path: std::path::PathBuf,
    pub read_only: bool,
}

impl AppState {
    pub fn new(config: crate::Config, read_only: bool) -> anyhow::Result<Self>;
}
```

### Struct: `MempalaceServer`

```rust
impl MempalaceServer {
    pub fn new(state: AppState) -> Self;
}
```

---

## Module: `miner`

### Free functions

```rust
pub fn load_config(
    project_dir: &Path,
) -> anyhow::Result<(String, Vec<RoomMapping>)>;             // RoomMapping is doc(hidden)

pub async fn mine(
    project_dir: &Path,
    palace_path: &Path,
    wing_override: Option<&str>,
    exclude_patterns: Option<&[String]>,
) -> anyhow::Result<MiningResult>;

pub async fn mine_with_options(
    project_dir: &Path,
    palace_path: &Path,
    wing_override: Option<&str>,
    exclude_patterns: Option<&[String]>,
    max_chunks_per_file: Option<usize>,
) -> anyhow::Result<MiningResult>;

pub fn resolve_max_chunks_per_file(override_value: Option<usize>) -> usize;

pub fn scan_project(
    project_dir: &Path,
    respect_gitignore: bool,
    include_ignored: Option<&[String]>,
) -> Vec<std::path::PathBuf>;
```

### Enum: `SkipReason`

```rust
pub enum SkipReason {
    ChunkCap,
}
```

### Struct: `Miner`

(Fields are private.)

```rust
impl Miner {
    pub async fn mine_file(/* ... */) -> /* ... */;
    pub fn new(
        palace_path: &Path,
        wing: &str,
        rooms: Vec<RoomMapping>,                             // doc(hidden) leak
    ) -> anyhow::Result<Self>;
    pub async fn scan_and_mine(&mut self, project_dir: &Path) -> MiningResult;
    pub fn with_max_chunks_per_file(self, override_value: Option<usize>) -> Self;
}
```

### Struct: `MiningResult`

```rust
pub struct MiningResult {
    pub chunks_created: usize,
    pub errors: Vec<String>,
    pub files_processed: usize,
    pub files_skipped_chunk_cap: usize,
}
```

---

## Module: `onboarding`

### Constants

```rust
pub const DEFAULT_WINGS_COMBO: &[&str];     // ["family","work","health","creative","projects","reflections"]
pub const DEFAULT_WINGS_PERSONAL: &[&str];  // ["family","health","creative","reflections","relationships"]
pub const DEFAULT_WINGS_WORK: &[&str];      // ["projects","clients","team","decisions","research"]
```

### Free functions

```rust
pub fn auto_detect_from_directory(
    directory: &Path,
    known_people: &[PersonEntity],                           // doc(hidden) leak
) -> Vec<PersonEntity>;

pub fn generate_aaak_bootstrap(
    people: &[PersonEntry],
    projects: &[String],
    wings: &[String],
    mode: Mode,
    config_dir: &Path,
) -> anyhow::Result<(PathBuf, PathBuf)>;

pub fn is_interactive() -> bool;

pub fn is_non_interactive() -> bool;

pub fn prompt_mode() -> Mode;

pub fn prompt_or_default<T: Clone + ToString>(prompt: &str, default: T) -> T;

pub fn prompt_people(mode: Mode) -> (Vec<PersonEntry>, HashMap<String, String>);

pub fn prompt_string(prompt: &str, default: &str) -> String;

pub fn quick_setup(
    config_dir: &Path,
    mode: Mode,
    people: Vec<(String, String, String)>,
    projects: Vec<String>,
    aliases: Option<HashMap<String, String>>,
) -> anyhow::Result<EntityRegistry>;                         // doc(hidden) leak

pub fn run_onboarding(
    directory: &Path,
    config_dir: &Path,
    auto_detect: bool,
) -> anyhow::Result<EntityRegistry>;                         // doc(hidden) leak

pub fn warn_ambiguous(people: &[PersonEntry]) -> Vec<String>;
```

### Enum: `Mode`

```rust
pub enum Mode {
    Combo,
    Personal,
    Work,
}

impl Mode {
    pub fn as_str(&self) -> &'static str;
    pub fn default_wings(&self) -> Vec<String>;
}
```

### Struct: `PersonEntry`

```rust
pub struct PersonEntry {
    pub context: String,
    pub name: String,
    pub relationship: String,
}
```

---

## Module: `searcher`

### Free functions

```rust
pub async fn check_duplicate(
    content: &str,
    palace_path: &Path,
    threshold: f64,
) -> anyhow::Result<Option<String>>;

pub fn print_search_response(response: &SearchResponse) -> i32;

pub async fn search(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    embedding_model: Option<&str>,
) -> anyhow::Result<i32>;

pub async fn search_memories(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    _embedding_model: Option<&str>,
) -> anyhow::Result<SearchResponse>;

pub async fn search_memories_with_rerank(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    _embedding_model: Option<&str>,
    use_bm25: bool,
) -> anyhow::Result<SearchResponse>;
```

### Enum: `SearchError`

```rust
pub enum SearchError {
    Empty(String),
    NoPalace(String),
    NotInitialized(String),
    Query(String),
}
```

### Struct: `SearchFilters`

```rust
pub struct SearchFilters {
    pub room: Option<String>,
    pub wing: Option<String>,
}
```

### Struct: `SearchResponse`

```rust
pub struct SearchResponse {
    pub filters: SearchFilters,
    pub query: String,
    pub results: Vec<SearchResult>,
}
```

### Struct: `SearchResult`

```rust
pub struct SearchResult {
    pub bm25_score: Option<f64>,
    pub combined_score: Option<f64>,
    pub created_at: Option<String>,
    pub room: String,
    pub similarity: f64,
    pub source_file: String,
    pub text: String,
    pub wing: String,
}
```

---

## Counts (curated public surface only)

- **Public modules:** 12 (`cli`, `config`, `constants`, `dialect`, `doctor`, `error`, `knowledge_graph`, `layers`, `mcp_server`, `miner`, `onboarding`, `searcher`)
- **Public functions (incl. inherent methods):** 75
- **Public structs:** 28
- **Public enums:** 5
- **Public consts:** 25
- **Public traits:** 0
- **Public type aliases:** 0
- **Root re-exports (`pub use`):** 2 (`config::Config`, `error::MempalaceError`)

Breakdown by module:

| Module             | fns | structs | enums | consts |
|--------------------|----:|--------:|------:|-------:|
| `cli`              |   1 |       0 |     0 |      0 |
| `config`           |  10 |       1 |     0 |      0 |
| `constants`        |   0 |       0 |     0 |     20 |
| `dialect`          |   6 |       1 |     0 |      0 |
| `doctor`           |   1 |       2 |     1 |      0 |
| `error`            |   0 |       0 |     1 |      0 |
| `knowledge_graph`  |  11 |       5 |     0 |      0 |
| `layers`           |  15 |      11 |     0 |      2 |
| `mcp_server`       |   4 |       2 |     0 |      0 |
| `miner`            |   9 |       2 |     1 |      0 |
| `onboarding`       |  13 |       1 |     1 |      3 |
| `searcher`         |   5 |       3 |     1 |      0 |
| **Total**          |  75 |      28 |     5 |     25 |

(Constructor and method counts include inherent `impl` methods. `Layer1::MAX_CHARS` and `Layer1::MAX_DRAWERS` are counted under `layers` consts.)

---

## Stability

Stability: ALL items in this file are subject to change before `mempalace-core` 1.0. See plan ADR-3 and Phase 2 in `docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md`.

Notable shape risks the Phase 2 `Palace` facade is expected to address:

- No `#[non_exhaustive]` on any public enum or struct — every variant or field addition is a SemVer break today.
- No public traits — consumers bind to concrete types (`PalaceDb`, `SearchResponse`, `KnowledgeGraph`, etc.).
- `searcher::search_memories(..)` accepts `_embedding_model: Option<&str>` but the parameter is unused (kept for API stability).
- `mcp_server::AppState` exposes `palace_db::PalaceDb` (a `#[doc(hidden)]` type) as a `pub` field.
- `miner::Miner::new` and `miner::load_config` reference `RoomMapping` from the `#[doc(hidden)]` `room_detector_local` module.
- `onboarding::quick_setup`, `onboarding::run_onboarding`, and `onboarding::auto_detect_from_directory` return / accept types from `#[doc(hidden)]` modules (`EntityRegistry`, `PersonEntity`).
- The `async` wrappers on `searcher` and `palace_db` are not real async — they delegate synchronously.

Diff this file against the Phase 2 facade snapshot to measure the curated narrowing.
