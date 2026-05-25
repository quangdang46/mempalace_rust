# jcode Memory Internals — Code-Level Analysis

> Source: `/data/projects/jcode` @ `2026-05-25` working tree
> Purpose: Map the existing memory architecture so `mempalace_rust` can plug in cleanly as an alternative provider.
> Method: Direct code reading of `src/memory*.rs`, `src/memory/*`, `src/memory_agent.rs`, `src/sidecar.rs`, `src/embedding*.rs`, `src/tool/memory.rs`, the `jcode-memory-types` and `jcode-embedding` crates, plus relevant TUI/protocol surfaces.

---

## TL;DR

jcode already has a **two-tier memory system**:

1. **Persistent store**: A typed graph (`MemoryGraph`) of `MemoryEntry` nodes connected by tags, clusters, and semantic edges. Stored as plain JSON on disk under `~/.jcode/memory/`. Local 384-dim ONNX embeddings (`all-MiniLM-L6-v2` via `tract-onnx`).

2. **Memory agent runtime**: A tokio singleton (`MemoryAgent`) that runs in the background, receives context updates over an mpsc channel, embeds, does graph cascade BFS, optionally verifies via a `Sidecar` (Codex Spark / Claude Haiku), and surfaces results to the main agent through a global `PENDING_MEMORY` map. Result of turn N becomes available at turn N+1.

There is **no provider trait yet** — `MemoryManager` is concretely instantiated everywhere and reads/writes JSON via `crate::storage::{read_json, write_json}`. The pure-types crate `jcode-memory-types` exists but only contains data types and pipeline state structs, not a provider abstraction. To plug `mempalace_rust` in, we need to:

- Introduce a `MemoryProvider` trait at the seam where `MemoryManager` is constructed (~12 call sites in `tui/app/turn_memory.rs`, `agent/turn_execution.rs`, `agent/prompting.rs`, `tool/memory.rs`, `memory_agent.rs`).
- Keep the `PendingMemory` / `MemoryActivity` event surface as-is so the TUI keeps working.
- Wire it through the existing config feature flag (`config().features.memory`) with a new `memory_backend` enum alongside `memory_sidecar_enabled`.

---

## 1. Current Data Model

All persistent types live in the workspace crate **`jcode-memory-types`** (`crates/jcode-memory-types/`). The binary crate (`src/memory_types.rs`) is just `pub use jcode_memory_types::*;`.

### 1.1 `MemoryEntry` — the leaf node

`crates/jcode-memory-types/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub tags: Vec<String>,
    /// Pre-normalized lowercase search text for content + tags.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub search_text: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub access_count: u32,
    pub source: Option<String>,
    /// Trust level for this memory
    #[serde(default)]
    pub trust: TrustLevel,
    /// Consolidation strength (how many times this was reinforced)
    #[serde(default)]
    pub strength: u32,
    /// Whether this memory is active or superseded
    #[serde(default = "default_active")]
    pub active: bool,
    /// ID of memory that superseded this one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Reinforcement provenance (breadcrumbs of when/where this was reinforced)
    #[serde(default)]
    pub reinforcements: Vec<Reinforcement>,
    /// Embedding vector for similarity search (384 dimensions for MiniLM)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// Confidence score (0.0-1.0) - decays over time, boosted by use
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}
```

Key methods on `MemoryEntry`:
- `new(category, content)` — generates `mem_*` id via `jcode_core::id::new_id`, sets `confidence = 1.0`, `strength = 1`, `active = true`.
- `effective_confidence()` — applies category-specific exponential decay (correction = 365d half-life, preference = 90d, fact = 30d, entity = 60d, custom = 45d) plus a log-scale access-count boost.
- `boost_confidence(amount)` / `decay_confidence(amount)` / `touch()` / `reinforce(session_id, msg_idx)` / `supersede(new_id)`.
- `searchable_text()` returns the normalized `search_text` (a Cow), regenerating it from content+tags if empty.

### 1.2 Categorisation enums

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Fact,
    Preference,
    Entity,
    Correction,
    Custom(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum TrustLevel {
    High,           // user explicitly stated
    #[default] Medium, // observed
    Low,            // inferred
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reinforcement {
    pub session_id: String,
    pub message_index: usize,
    pub timestamp: DateTime<Utc>,
}
```

`MemoryCategory::from_extracted(s)` does fuzzy mapping for LLM output (e.g. `"observation"` / `"lesson"` / `"learning"` → `Fact`).

### 1.3 `MemoryScope`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Project,
    Global,
    All,
}
impl MemoryScope {
    pub fn includes_project(self) -> bool { matches!(self, Self::Project | Self::All) }
    pub fn includes_global(self) -> bool  { matches!(self, Self::Global | Self::All) }
}
```

There is **no `Session` scope at the persistent layer** — the `MEMORY_ARCHITECTURE.md` document mentions one but the actual code only persists `Project` and `Global`. Per-session state lives in `PENDING_MEMORY` / `INJECTED_MEMORY_IDS` / `MemoryAgent.sessions: HashMap<String, SessionState>`.

### 1.4 Legacy `MemoryStore` (still on disk for migration)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStore {
    pub entries: Vec<MemoryEntry>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}
```

It is the pre-graph format. `MemoryGraph::from_legacy_store(store)` migrates it to the graph; `MemoryManager::load_*_graph` does this on first read and writes a `.bak` sidecar.

### 1.5 `MemoryGraph` and edges (`crates/jcode-memory-types/src/graph.rs`)

```rust
pub const GRAPH_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeKind {
    HasTag,
    InCluster,
    RelatesTo { #[serde(default = "default_weight")] weight: f32 },
    Supersedes,
    Contradicts,
    DerivedFrom,
}

impl EdgeKind {
    pub fn traversal_weight(&self) -> f32 {
        match self {
            EdgeKind::HasTag           => 0.8,
            EdgeKind::InCluster        => 0.6,
            EdgeKind::RelatesTo { weight } => *weight,
            EdgeKind::Supersedes       => 0.9,
            EdgeKind::Contradicts      => 0.3,
            EdgeKind::DerivedFrom      => 0.7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub target: String,
    #[serde(flatten)]
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagEntry {
    pub id: String,            // "tag:{name}"
    pub name: String,
    pub description: Option<String>,
    pub count: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterEntry {
    pub id: String,            // "cluster:{id}"
    pub name: Option<String>,
    pub centroid: Vec<f32>,
    pub member_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphMetadata {
    pub last_cluster_update: Option<DateTime<Utc>>,
    pub retrieval_count: u64,
    pub link_discovery_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGraph {
    pub graph_version: u32,
    pub memories: HashMap<String, MemoryEntry>,
    pub tags: HashMap<String, TagEntry>,
    #[serde(default)]
    pub clusters: HashMap<String, ClusterEntry>,
    #[serde(default)]
    pub edges: HashMap<String, Vec<Edge>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub reverse_edges: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub metadata: GraphMetadata,
}
```

> **Note**: `MEMORY_ARCHITECTURE.md` describes a `MemoryNode` enum (`Memory | Tag | Cluster`) and a petgraph `DiGraph<MemoryNode, EdgeKind>`. That design was abandoned during implementation — the actual store is "HashMap-based for clean JSON serialization" (literal comment from `graph.rs`). There is **no petgraph dependency** in `jcode-memory-types/Cargo.toml`.

### 1.6 `MemoryGraph` operations

All graph manipulations live in `crates/jcode-memory-types/src/graph.rs`:

- Insertion: `add_memory(MemoryEntry) -> String` (also auto-creates tag nodes + `HasTag` edges and a `Supersedes` edge if `superseded_by` is set).
- Lookup: `get_memory`, `get_memory_mut`, `all_memories`, `active_memories`, `get_memories_by_tag`.
- Tags: `ensure_tag`, `tag_memory`, `untag_memory`.
- Edges: `add_edge`, `remove_edge`, `link_memories(from, to, weight)`, `supersede(newer, older)`, `mark_contradiction(a, b)`.
- Stats: `node_count` (memories + tags + clusters), `edge_count`.
- **Cascade retrieval**: `cascade_retrieve(seed_ids, seed_scores, max_depth, max_results) -> Vec<(String, f32)>`. BFS through forward edges; when a tag node is reached, fans out to all incoming `HasTag` sources. Score decays as `seed_score * edge_weight * 0.7^depth`.
- Migration: `from_legacy_store(MemoryStore) -> MemoryGraph`.

### 1.7 Pipeline-state types (also in `jcode-memory-types`)

`MemoryActivity` is the **runtime state** the TUI subscribes to (NOT persisted):

```rust
pub struct MemoryActivity {
    pub state: MemoryState,
    pub state_since: Instant,
    pub pipeline: Option<PipelineState>,
    pub recent_events: Vec<MemoryEvent>,
}

pub enum MemoryState {
    Idle,
    Embedding,
    SidecarChecking { count: usize },
    FoundRelevant { count: usize },
    Extracting { reason: String },
    Maintaining { phase: String },
    ToolAction { action: String, detail: String },
}

pub struct PipelineState {
    pub search:    StepStatus, pub search_result:    Option<StepResult>,
    pub verify:    StepStatus, pub verify_result:    Option<StepResult>,
                               pub verify_progress:  Option<(usize, usize)>,
    pub inject:    StepStatus, pub inject_result:    Option<StepResult>,
    pub maintain:  StepStatus, pub maintain_result:  Option<StepResult>,
    pub started_at: Instant,
}

pub enum StepStatus { Pending, Running, Done, Error, Skipped }
pub struct StepResult { pub summary: String, pub latency_ms: u64 }

pub enum MemoryEventKind {
    EmbeddingStarted, EmbeddingComplete { latency_ms: u64, hits: usize },
    SidecarStarted, SidecarRelevant { memory_preview: String }, SidecarNotRelevant,
    SidecarComplete { latency_ms: u64 },
    MemorySurfaced { memory_preview: String },
    MemoryInjected { count, prompt_chars, age_ms, preview, items: Vec<InjectedMemoryItem> },
    MaintenanceStarted { verified, rejected }, MaintenanceLinked { links },
    MaintenanceConfidence { boosted, decayed }, MaintenanceCluster { clusters, members },
    MaintenanceTagInferred { tag, applied }, MaintenanceGap { candidates },
    MaintenanceComplete { latency_ms: u64 },
    ExtractionStarted { reason }, ExtractionComplete { count },
    Error { message },
    ToolRemembered { content, scope, category }, ToolRecalled { query, count },
    ToolForgot { id }, ToolTagged { id, tags }, ToolLinked { from, to }, ToolListed { count },
}
```

### 1.8 `MemoryManager` — the orchestrator (`src/memory.rs`)

```rust
#[derive(Debug, Clone)]
pub struct MemoryManager {
    project_dir: Option<PathBuf>,
    test_mode: bool,        // routes to ~/.jcode/memory/test/
    include_skills: bool,   // synthesizes skill registry entries as virtual memories
}
```

Constructors: `new()`, `new_test()`, `with_project_dir(p)`, `with_skills(bool)`. Cloned everywhere — it is *just* a `(PathBuf, bool, bool)` triple, all real state lives on disk + in module-level statics.

Public surface (~50 methods, grouped):

- **Storage**: `load_project`, `load_global`, `save_project`, `save_global`, `load_project_graph`, `load_global_graph`, `save_project_graph`, `save_global_graph`. Loaders auto-migrate legacy `MemoryStore` → `MemoryGraph` and import legacy `~/.jcode/notes/<hash>.json` files.
- **Insert**: `remember_project(MemoryEntry)`, `remember_global(MemoryEntry)`, `upsert_project_memory`, `upsert_global_memory`. Both `remember_*` perform storage-layer dedup at `STORAGE_DEDUP_THRESHOLD = 0.85` cosine sim — duplicates are *reinforced*, not added.
- **Search**: `search`, `search_scoped`, `find_similar`, `find_similar_scoped`, `find_similar_with_embedding(_scoped)`, `find_similar_with_cascade(_scoped)`, `get_relevant_keywords`.
- **Retrieval orchestration**: `get_relevant_for_context(ctx, max)` (sequential sidecar), `get_relevant_parallel(session_id, messages, event_tx)` (4-step pipeline with progress events), `relevant_prompt_for_messages`, `relevant_prompt_for_context`, `spawn_relevance_check` (background tokio task).
- **Graph ops**: `tag_memory`, `link_memories`, `get_related(id, depth)`, `graph_stats() -> (memories, tags, edges, clusters)`.
- **Lifecycle**: `forget(id)`, `backfill_embeddings()`, `extract_from_transcript(transcript, session_id)`.
- **Constants** (file scope): `EMBEDDING_SIMILARITY_THRESHOLD: f32 = 0.5`, `EMBEDDING_MAX_HITS: usize = 10`, `MEMORY_RELEVANCE_MAX_CANDIDATES: usize = 30`, `MEMORY_RELEVANCE_MAX_RESULTS: usize = 10`, `STORAGE_DEDUP_THRESHOLD = 0.85`.

---

## 2. Storage Layout

### 2.1 Where on disk

Resolved by `jcode_storage::jcode_dir()` (`crates/jcode-storage/src/lib.rs`):

| Setting | Path |
|---|---|
| `JCODE_HOME` set | `$JCODE_HOME` |
| `JCODE_USE_XDG=1` (and `XDG_DATA_HOME` set) | `$XDG_DATA_HOME/jcode` |
| `JCODE_USE_XDG=1` (no XDG var) | `$HOME/.local/share/jcode` |
| Default | `$HOME/.jcode` (legacy) |

Memory subtree (from `MemoryManager::project_memory_path` and `global_memory_path`):

```
<jcode_dir>/memory/
├── global.json                           # MemoryGraph (graph_version=2)
├── global.json.bak                       # post-migration backup
├── projects/
│   └── <hex16(hash(project_dir))>.json   # per-CWD MemoryGraph
│   └── <hex16(...)>.json.bak
└── test/                                 # only when test_mode=true
    ├── test_global.json
    └── test_project.json

<jcode_dir>/notes/                        # legacy "remember" notes, migrated lazily
└── <hex16(hash(project_dir))>.json
```

The project-key derivation is `format!("{:016x}", DefaultHasher::hash(project_dir))` — there is no path canonicalisation, so two paths that differ only in symlinks or trailing slashes land in different files.

There is **also** an unrelated runtime-memory log directory at `<jcode_dir>/logs/memory-events-YYYY-MM-DD.jsonl` (written by `src/memory_log.rs`) and a process-RSS log under `<jcode_dir>/logs/{server,client}-runtime-memory-*.jsonl` (written by `src/runtime_memory_log.rs`). These are observability artifacts, not user memories.

### 2.2 Format

- **JSON, no SQLite anywhere.** `serde_json::to_vec` → atomic temp+rename via `jcode_storage::write_json`. Reads via `read_json_with_recovery_handler` which falls back to `*.bak` on parse error.
- Top-level object is a `MemoryGraph` (with `graph_version: u32 = 2`). Three node maps (`memories`, `tags`, `clusters`) plus forward+reverse edge maps and metadata.
- Embeddings are inlined directly: `embedding: Option<Vec<f32>>` → ~3 KB per entry as JSON numbers. There is no separate `embeddings/<id>.vec` directory despite what `MEMORY_ARCHITECTURE.md` shows.

### 2.3 Versioning and migrations

- `GRAPH_VERSION = 2` is checked in `MemoryManager::load_project_graph` / `load_global_graph`. If a file deserialises as `MemoryGraph` and `graph_version == 2`, it's used as-is.
- Else it falls back to `MemoryStore` (the legacy flat list), runs `MemoryGraph::from_legacy_store`, copies the original to `*.json.bak`, writes the new graph, and logs `"Migrated memory store to graph format"`.
- A second migration path: `import_legacy_notes_into_graph` reads `<jcode_dir>/notes/<hash>.json` (a `LegacyNotesFile { entries: Vec<LegacyNoteEntry> }`) and folds each entry in with `MemoryCategory::Custom("note")`.
- An in-process cache (`src/memory/cache.rs`, `GRAPH_CACHE`) holds parsed graphs keyed by path with mtime invalidation, so repeated `load_*_graph` calls within a process are O(1).
- `normalize_graph_search_text` is run on every load to refresh `search_text` for any entries whose normalisation drifted.

### 2.4 Atomicity and durability

`jcode_storage::write_json_inner` writes to `path.tmp.<pid>.<rand>`, fsyncs the file (in durable mode — used here), renames over the target after copying the previous version to `*.bak`, and on Linux fsyncs the parent directory. This means each `save_project_graph` rewrites the **entire** project graph file. There is no append log and no incremental write path.

---

## 3. Embedding Pipeline

### 3.1 Crate layout

- **`crates/jcode-embedding`** — pure embedding backend. Depends only on `tract-onnx`, `tract-hir`, `tokenizers`, `reqwest` (blocking, for first-time download). Exposes `Embedder::load_from_dir(&Path)`, `embed`, `embed_batch`, `cosine_similarity`, `batch_cosine_similarity`, `find_similar`, `is_model_available`, `MODEL_NAME = "all-MiniLM-L6-v2"`, `embedding_dim() -> usize { 384 }`.
- **`src/embedding.rs`** (binary crate) — process-wide caching facade around the workspace crate. Adds: idle-unload, an LRU on the *output* embeddings (capacity 128, hashed by text), `EmbedderStats`, integration with `runtime_memory_log` and `process_memory::purge_allocator`.
- **`src/embedding_stub.rs`** — used when the `embeddings` cargo feature is disabled. Same public API, all `embed`/`load` calls return errors. `cosine_similarity` and `batch_cosine_similarity` still work on naked vectors.

`src/lib.rs` selects between them:
```rust
#[cfg(feature = "embeddings")] pub mod embedding;
#[cfg(not(feature = "embeddings"))] pub mod embedding_stub;
#[cfg(not(feature = "embeddings"))] pub use embedding_stub as embedding;
```

### 3.2 Model

- **Name**: `all-MiniLM-L6-v2` (sentence-transformers).
- **Dim**: 384 (`EMBEDDING_DIM` const in `jcode-embedding`).
- **Tokenizer**: HuggingFace `tokenizers` crate, loaded from `tokenizer.json`.
- **Runtime**: `tract-onnx` ONNX session, max sequence length **256**, three int64 inputs (`input_ids`, `attention_mask`, `token_type_ids`).
- **Pooling**: mean over the `valid_tokens` first positions, L2-normalised.
- **Files**: `<jcode_dir>/models/all-MiniLM-L6-v2/{model.onnx, tokenizer.json}`. Downloaded from HuggingFace on first call (`MODEL_URL`/`TOKENIZER_URL` consts) inside a spawned blocking thread.

### 3.3 Lifecycle

1. **Lazy load**: `embedding::get_embedder()` returns an `Arc<Embedder>`. Builds the `tract` runnable plan once and caches it in `OnceLock<Mutex<EmbedderCache>>`. `last_used_at = Instant::now()` is updated on every access.
2. **Cache hit**: `embedding::embed(text)` first hashes the input, looks up the LRU (`embedding_lru: HashMap<u64, (Vec<f32>, lru_counter)>`), and returns the cached vector if present. Misses run the ONNX session and insert; eviction is min-`lru_counter`.
3. **Idle unload**: A background task in `server.rs` calls `embedding::maybe_unload_if_idle(idle_for)` every `EMBEDDING_IDLE_CHECK_SECS`. If the model has been unused for `embedding_idle_unload_secs`, it drops the `Arc<Embedder>`, clears the LRU, and on Linux calls `malloc_trim(0)` (or jemalloc purge if `--features jemalloc`).
4. **Force unload**: `embedding::unload_now()` for shutdown.
5. **Stats**: `embedding::stats() -> EmbedderStats` includes load/unload counts, embed call count + failures, total/avg latency, idle/loaded seconds, cache hit count, cache size + estimated bytes (`cache_bytes_estimate = entries * dim * 4`), and current model artifact sizes via `std::fs::metadata`.

### 3.4 Where embeddings are generated

- **On insert**: `MemoryManager::remember_*` calls `entry.ensure_embedding()` (a method added by the local `MemoryEntryEmbeddingExt` trait in `src/memory.rs`), which embeds `entry.content` and stores it in `entry.embedding`.
  - Skipped when `test_mode == true`.
  - Skipped under `#[cfg(test)]` unless `JCODE_TEST_ALLOW_MEMORY_EMBEDDINGS` is set.
  - Skipped for `MemoryCategory::Custom("goal")` (initiative goals are not searchable by embedding).
- **On retrieval**: `MemoryAgent::process_context` and `MemoryManager::get_relevant_parallel` embed the formatted *context* string via `tokio::task::spawn_blocking(|| embedding::embed(...))`.
- **Backfill**: `MemoryManager::backfill_embeddings()` iterates project + global graphs and fills missing embeddings.
- **Cosine math**: `embedding::batch_cosine_similarity(query, &refs)` is used to score candidates. There is no SIMD; it is plain Rust `iter().zip().map().sum()`.

### 3.5 The stub fallback

`src/embedding_stub.rs` is a drop-in replacement compiled when `embeddings` feature is off:

- `Embedder::load()` always returns `Err("Embeddings feature not compiled in this build")`.
- `embed(text)` returns the same error.
- `cosine_similarity` and `batch_cosine_similarity` work on raw vectors (so legacy stored embeddings can still be compared).
- `find_similar` works against pre-computed candidates.
- `embedding_dim()` still returns `384` so type signatures match.

The downstream effect when running without the feature: `MemoryEntry::ensure_embedding` always returns `false`, `find_similar*` returns `Ok(Vec::new())`, and the memory pipeline silently degrades to keyword search through `MemoryStore::search` / `memory_matches_search`.

---

## 4. Memory Agent Runtime

`src/memory_agent.rs` (~1700 LOC). The persistent agent that listens for context updates, embeds, runs cascade retrieval + sidecar verification, and surfaces results.

### 4.1 Singleton + channel

```rust
static MEMORY_AGENT: tokio::sync::OnceCell<MemoryAgentHandle> =
    tokio::sync::OnceCell::const_new();

const CONTEXT_CHANNEL_CAPACITY: usize = 16;

pub struct MemoryAgentHandle {
    tx: mpsc::Sender<AgentMessage>,
}

enum AgentMessage {
    Context {
        session_id: String,
        messages: Arc<[crate::message::Message]>,
        working_dir: Option<String>,
        timestamp: Instant,
    },
    Reset,
}
```

Lifecycle:
- `pub async fn init() -> Result<MemoryAgentHandle>` builds the channel, spawns `agent.run()` on tokio, and stores the handle in `OnceCell`. Called from `server.rs:988` *only* when `config().features.memory == true`.
- `pub fn update_context_sync_with_dir(...)` is the hot path — `try_send` to the mpsc channel from anywhere (sync or async). If `MEMORY_AGENT` isn't initialised yet, it spawns a tokio task to `init()` and forward.
- `pub fn reset()` sends `AgentMessage::Reset` (clears `sessions: HashMap<String, SessionState>` and `INJECTED_MEMORY_IDS`).
- `pub fn trigger_final_extraction_with_dir(transcript, session_id, working_dir)` is fire-and-forget — bypasses the channel and spawns a tokio task that calls `Sidecar::extract_memories_with_existing(...)` directly.

### 4.2 Per-session state

```rust
#[derive(Default)]
struct SessionState {
    working_dir: Option<String>,
    last_context_embedding: Option<Vec<f32>>,
    last_context_string: Option<String>,
    last_relevance_context_signature: Option<String>,
    last_relevance_check_at: Option<Instant>,
    surfaced_memories: HashSet<String>,
    turn_count: usize,
    turns_since_extraction: usize,
}
```

Tuning constants:
- `TOPIC_CHANGE_THRESHOLD: f32 = 0.3` — cosine sim below this between consecutive context embeddings = topic change.
- `MAX_MEMORIES_PER_TURN: usize = 5` — cap on memories surfaced per turn.
- `TURN_RESET_INTERVAL: usize = 50` — every 50 turns, clear `surfaced_memories` to allow re-surfacing.
- `MIN_TURNS_FOR_EXTRACTION: usize = 4` — minimum age before topic-change-triggered extraction.
- `PERIODIC_EXTRACTION_INTERVAL: usize = 12` — extract every 12 turns even without topic change.
- `RELEVANCE_CONTEXT_REPEAT_SUPPRESSION_SECS: u64 = 30` — skip repeat checks for unchanged context.
- `CLUSTER_REFINEMENT_INTERVAL: u64 = 50` — cluster refresh cadence in maintenance ticks.

### 4.3 Cascade retrieval and verification flow

`MemoryAgent::process_context(session_id, messages, _ts)`:

1. Format context: `memory::format_context_for_relevance(&messages)` (last 12 messages, max 8 KB total, max 1.2 KB per content block; from `src/memory_prompt.rs`).
2. **Repeat suppression**: hash the line-normalised context and skip if the same context was checked within 30 s.
3. **Embed** the context (off-thread via `spawn_blocking`).
4. **Topic-change check** vs `session_state.last_context_embedding`. If sim < 0.3, optionally trigger **incremental extraction** of the *previous* context (`extract_from_context`), then clear `surfaced_memories` + injected-tracking for this session.
5. **Periodic extraction** check (every 12 turns).
6. `MemoryManager::find_similar_with_embedding(&context_embedding, threshold=0.5, limit=10)` — searches both project and global graphs.
7. Filter out already-surfaced (per-session set + global `is_memory_injected`).
8. **Sidecar verify** in parallel batches of 5 (`MAX_MEMORIES_PER_TURN`) — `Sidecar::check_relevance(memory.content, context)` returns `(bool, reason)`. Skipped entirely when `memory_sidecar_enabled() == false`, in which case top-K semantic hits pass through.
9. Format with `memory::format_relevant_prompt(&relevant, MAX_MEMORIES_PER_TURN)` and put into `PENDING_MEMORY` keyed by session id via `set_pending_memory_with_ids_and_display(...)`.
10. Build a `RetrievalContext { verified_ids, rejected_ids, context_snippet }` and **post-retrieval maintenance** spawns as a background task.

### 4.4 PENDING_MEMORY pattern (`src/memory/pending.rs`)

The seam between the async memory agent and the synchronous main turn loop:

```rust
pub struct PendingMemory {
    pub prompt: String,                  // injected into system prompt
    pub display_prompt: Option<String>,  // richer UI version
    pub computed_at: Instant,
    pub count: usize,
    pub memory_ids: Vec<String>,         // for dedup tracking
}

static PENDING_MEMORY: Mutex<Option<HashMap<String, PendingMemory>>>;
static LAST_INJECTED_PROMPT_SIGNATURE: Mutex<Option<HashMap<String, (String, Instant)>>>;
static LAST_INJECTED_MEMORY_SET: Mutex<Option<HashMap<String, (HashSet<String>, Instant)>>>;
static INJECTED_MEMORY_IDS: Mutex<Option<HashMap<String, HashSet<String>>>>;
static MEMORY_CHECK_IN_PROGRESS: Mutex<Option<HashSet<String>>>;
```

Suppression rules in `take_pending_memory`:
- Stale (>120 s old) → drop.
- Same prompt signature within 90 s → drop.
- Memory-id overlap ≥ 0.8 within 180 s → drop.

The main turn calls `take_pending_memory(session_id)` at the start of each turn (in `tui/app/turn_memory.rs::build_memory_prompt_nonblocking` and `agent/prompting.rs::build_memory_prompt_nonblocking_shared`). The result is passed straight into `build_system_prompt_split(memory_prompt: Some(&p.prompt))`. The same call also fires `update_context_sync_with_dir(...)` for the *next* turn — that is the "results from turn N are available at turn N+1" guarantee.

### 4.5 Sidecar (Haiku / Codex Spark) usage

`src/sidecar.rs`:

```rust
pub const SIDECAR_OPENAI_MODEL: &str = "gpt-5.3-codex-spark";
const SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL: &str = "gpt-5.4";
const SIDECAR_CLAUDE_MODEL: &str = "claude-haiku-4-5-20241022";

pub struct Sidecar {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,        // 1024
    backend: SidecarBackend, // OpenAI | Claude
}
```

Backend auto-selection: prefers Codex/OpenAI creds, falls back to Claude OAuth, falls back to a stub Claude that errors at use. `agents.memory_model` config can override, validated against `provider::provider_for_model`. Two small endpoints used:

- `pub async fn check_relevance(memory_content, current_context) -> Result<(bool, String)>` — system prompt asks for a `RELEVANT: yes/no\nREASON: ...` reply, parsed line-by-line.
- `pub async fn check_contradiction(new, existing) -> Result<bool>` — single-token YES/NO.
- `pub async fn extract_memories(transcript) -> Result<Vec<ExtractedMemory>>` and `extract_memories_with_existing(transcript, existing)` — returns lines of `CATEGORY|CONTENT|TRUST` parsed into `ExtractedMemory { category, content, trust }`.

The sidecar is **strictly optional**: `memory::memory_sidecar_enabled()` reads `config().agents.memory_sidecar_enabled`. When disabled, semantic ranking is the final answer (top `MAX_MEMORIES_PER_TURN`).

### 4.6 BFS / cascade in `MemoryGraph`

Implemented in `MemoryGraph::cascade_retrieve` (in `jcode-memory-types/src/graph.rs`):

```rust
pub fn cascade_retrieve(
    &mut self,
    seed_ids: &[String],
    seed_scores: &[f32],
    max_depth: usize,
    max_results: usize,
) -> Vec<(String, f32)> { ... }
```

- BFS from seeds. Each hop: `new_score = score * edge.kind.traversal_weight() * 0.7^(depth+1)`.
- Tag nodes act as relays — when the BFS reaches a `tag:*` node, all incoming `HasTag` sources become candidates at the next depth.
- `metadata.retrieval_count` is incremented every call.
- Used by `MemoryManager::find_similar_with_cascade_scoped` and `MemoryManager::get_related`.

### 4.7 Clusters and post-retrieval maintenance

`MemoryAgent::post_retrieval_maintenance(memory_manager, retrieval_ctx)` spawns a background task that runs (with state machine `MemoryState::Maintaining { phase: "graph upkeep" }`):

1. **Link discovery** — for verified memories, add `RelatesTo` edges between co-relevant pairs.
2. **Confidence boost** — `+0.05` on verified.
3. **Confidence decay** — `-0.02` on rejected.
4. **Gap detection** — if `verified.is_empty() && !rejected.is_empty()` log a `MaintenanceGap` event with the candidate count and context snippet.
5. **Periodic cluster refinement** — every 50 maintenance ticks, `apply_cluster_assignment` builds/updates a cluster around the verified memories and adds `InCluster` edges. New "co-relevance" clusters get an LLM-generated name via `name_cluster_with_sidecar`.
6. **Tag inference** — if 2+ verified memories and the context snippet contains a repeated non-stopword, apply it as a tag.
7. **GC** — every 250 ticks (`50 * 5`), `prune_low_confidence` removes memories with `confidence < 0.05 && strength <= 1`.

Each step emits a `MemoryEventKind::Maintenance*` event (which feeds both the in-memory ring buffer and the JSONL log).

### 4.8 Stats

`MEMORY_AGENT_STATS: Mutex<MemoryAgentStats>` tracks `turns_processed`, `maintenance_runs`, `last_maintenance_ms`. Exposed via `pub fn stats() -> MemoryAgentStats`.

---

## 5. Activity, Events, and Protocol

### 5.1 Local activity store

`src/memory/activity.rs`:

```rust
static MEMORY_ACTIVITY: Mutex<Option<MemoryActivity>>;
const MAX_RECENT_EVENTS: usize = 10;
const STALENESS_TIMEOUT_SECS: u64 = 10;
```

- `set_state(MemoryState)` — overwrites `state` and `state_since`.
- `add_event(MemoryEventKind)` — pushes to `recent_events` (front-inserted, truncated to 10) **and** calls `crate::memory_log::log_event` to append to `<jcode_dir>/logs/memory-events-YYYY-MM-DD.jsonl`.
- `pipeline_start()` / `pipeline_update(|p| ...)` — manage the four-step pipeline.
- `check_staleness()` — auto-resets to `Idle` if state has been non-idle for ≥ 10 s.
- `record_injected_prompt(prompt, count, age_ms)` — emits `MemoryInjected` (with parsed bullet items via `parse_injected_items`) and `MemorySurfaced` events, plus `telemetry::record_memory_injected`.

### 5.2 Wire protocol

`crates/jcode-protocol/src/wire.rs`:

```rust
pub enum ServerEvent {
    // ... ~60 variants ...
    #[serde(rename = "memory_activity")]
    MemoryActivity { activity: MemoryActivitySnapshot },
    // ... and a separate event for the actual injection (search for "auto-recalled")
}
```

`crates/jcode-protocol/src/protocol_memory.rs` defines the **wire snapshot** types — they mirror the runtime types one-to-one but use `#[serde]` derives:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStateSnapshot { Idle, Embedding, SidecarChecking { count }, ... }

pub enum MemoryStepStatusSnapshot { Pending, Running, Done, Error, Skipped }
pub struct MemoryStepResultSnapshot { pub summary: String, pub latency_ms: u64 }

pub struct MemoryPipelineSnapshot {
    pub search: MemoryStepStatusSnapshot,
    pub search_result: Option<MemoryStepResultSnapshot>,
    pub verify: MemoryStepStatusSnapshot,
    pub verify_result: Option<MemoryStepResultSnapshot>,
    pub verify_progress: Option<(usize, usize)>,
    pub inject: MemoryStepStatusSnapshot,
    pub inject_result: Option<MemoryStepResultSnapshot>,
    pub maintain: MemoryStepStatusSnapshot,
    pub maintain_result: Option<MemoryStepResultSnapshot>,
}

pub struct MemoryActivitySnapshot {
    pub state: MemoryStateSnapshot,
    #[serde(default)] pub state_age_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<MemoryPipelineSnapshot>,
}
```

### 5.3 Server → client emission

`src/memory.rs:78-86`:

```rust
pub type MemoryEventSink = Arc<dyn Fn(crate::protocol::ServerEvent) + Send + Sync>;

fn emit_memory_activity(event_tx: Option<&MemoryEventSink>) {
    let (Some(event_tx), Some(activity)) = (event_tx, activity_snapshot()) else { return; };
    (event_tx)(crate::protocol::ServerEvent::MemoryActivity { activity });
}
```

`MemoryManager::get_relevant_parallel` (and the spawn helper `spawn_relevance_check`) accept an optional `MemoryEventSink` and call `emit_memory_activity` after every `set_state`/`add_event`/`pipeline_update` to push fresh snapshots out the wire.

### 5.4 Client side

`src/tui/app/remote/server_events.rs:1308`:

```rust
ServerEvent::MemoryActivity { activity } => {
    if app.memory_enabled {
        crate::memory::apply_remote_activity_snapshot(&activity);
    }
    false
}
```

`apply_remote_activity_snapshot` rebuilds a local `MemoryActivity` (preserving the existing `recent_events` ring) so the TUI memory widget can render exactly as on a local-only run.

### 5.5 TUI rendering

`src/tui/info_widget_memory_render.rs` is the renderer (~700 lines). It draws:

- A header with a status badge (`SEARCH`/`VERIFY`/`INJECT`/`UPDATE`/`READY`/`DONE`/`IDLE`/`FAILED`) sourced from `memory_status_badge`.
- A count line ("N memories").
- A status line ("Now: …" while running, "Last: … · 12s" when idle).
- A 4-step pipeline tree (`╭ Find matches`, `├ Check relevance`, `├ Inject context`, `╰ Update memory`) with colored markers, progress like `3/10`, and step results from `StepResult.summary`.
- A trace line showing the most recent "interesting" `MemoryEventKind`.

`src/tui/ui_memory.rs` parses the **injected prompt** into per-category `MemoryTile`s for the dedicated memory popup pane.

The renderer reads from `info.activity: Option<MemoryActivity>` which the TUI layer obtains via `crate::memory::get_activity()`.

---

## 6. Tools Exposed to the Model

### 6.1 The `memory` tool — `src/tool/memory.rs`

```rust
pub struct MemoryTool { manager: MemoryManager }

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str { "memory" }
    fn description(&self) -> &str { "Manage memory." }
    fn parameters_schema(&self) -> Value { /* see below */ }
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> { ... }
}
```

It is registered as a built-in tool in `src/tool/mod.rs:233`:

```rust
Self::insert_tool_timed(&mut m, &mut timings, "memory", memory::MemoryTool::new);
```

The `Tool` trait itself is `jcode_tool_core::Tool` — defined in `crates/jcode-tool-core/src/lib.rs`:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput>;
    fn to_definition(&self) -> ToolDefinition { /* default */ }
}
```

### 6.2 Verbs (the `action` field)

Schema declares `action ∈ ["remember", "recall", "search", "list", "forget", "tag", "link", "related"]`:

| Action | Required | Optional | Effect |
|---|---|---|---|
| `remember` | `content` | `category` (default `fact`), `scope` (default `project`), `tags` | `MemoryManager::remember_project` or `remember_global` (with auto-dedup at 0.85 cosine). |
| `recall` | — | `query`, `mode` (`recent` / `semantic` / `cascade`, default `cascade` if `query` else `recent`), `scope`, `limit` (10) | `recent` → `get_prompt_memories_scoped`; `semantic` → `find_similar_scoped(query, 0.5, limit, scope)`; `cascade` → `find_similar_with_cascade_scoped`. |
| `search` | `query` | `scope` | `MemoryManager::search_scoped` (substring match). |
| `list` | — | `scope` | `list_all_scoped` sorted by `updated_at` desc. |
| `forget` | `id` | — | `MemoryManager::forget(id)` removes from project then global graph. |
| `tag` | `id`, `tags` | — | `tag_memory(id, tag)` for each tag. |
| `link` | `from_id`, `to_id` | `weight` (0.5) | `link_memories(from, to, weight)` — same store only. |
| `related` | `id` | `depth` (2) | `get_related(id, depth)` — cascade BFS. |

Every action wraps its work with `memory::set_state(MemoryState::ToolAction { action, detail })` + `memory::add_event(MemoryEventKind::Tool*)` so the TUI shows the tool action in the same widget as automatic activity.

The published JSON schema deliberately omits `weight`, `depth`, and `mode` to keep the surface minimal (verified by a unit test in the same file). They still accept those fields when passed.

---

## 7. Lifecycle Hooks

### 7.1 Memory injection into prompts

`src/memory_prompt.rs` (re-exported as `crate::memory::prompt_support`). Two formatting paths that share `format_message_context_with`:

- `format_context_for_relevance(messages)` — last 12 messages, max 8 KB total, max 1.2 KB per content block, used for embedding/sidecar checks. Suppresses non-error tool results and reasoning blocks.
- `format_context_for_extraction(messages)` — last 40 messages, max 24 KB, includes tool input/output. Used by `extract_from_context` and by `extract_session_memories`.

Once the agent has retrieved relevant memories, it formats them via:

- `format_relevant_prompt(&entries, limit)` → `# Memory\n\n## Corrections\n1. ...\n## Facts\n1. ...` (see `format_entries_for_prompt_with_header`). The category order is fixed: Corrections → Facts → Preferences → Entities → custom (alphabetical).
- `format_relevant_display_prompt` is the same with `<!-- updated_at: ... -->` inline comments for the TUI popup parser.

The injection point is `Agent::build_system_prompt_split(memory_prompt: Option<&str>)` (in `src/agent/prompting.rs` and `src/tui/app/turn_memory.rs`):

```rust
let memory_pending = self.build_memory_prompt_nonblocking(&provider_messages);
let split_prompt =
    self.build_system_prompt_split(memory_pending.as_ref().map(|p| p.prompt.as_str()));
```

This calls `crate::prompt::build_system_prompt_split(skill, skills, is_canary, memory_prompt, working_dir)`. Inside, the memory prompt is appended to the *static* part so it benefits from prompt caching. The `dynamic_part` is reserved for things that change every turn.

### 7.2 When extraction runs

Extraction happens at four different cadences:

1. **Topic change** (`MemoryAgent::process_context`, sim < 0.3) — calls `extract_from_context(session_id, prev_context, "topic change")` if `turns_since_extraction >= MIN_TURNS_FOR_EXTRACTION` (4). Resets `surfaced_memories` for the session.
2. **Periodic** (every `PERIODIC_EXTRACTION_INTERVAL = 12` turns) — calls `extract_from_context(..., "periodic")` if extraction context ≥ 200 chars.
3. **End-of-session** — `Agent::extract_session_memories()` in `agent/turn_execution.rs:707` (CLI) and `App::extract_session_memories()` in `tui/app/turn_memory.rs:126` (TUI). Triggered from:
   - `Agent::run` exit path (`agent/turn_execution.rs:700`).
   - TUI `App::run_shell` exit (`tui/app/run_shell.rs:255`).
   - `/save` slash command via `App::trigger_save_memory_extraction` (`tui/app/conversation_state.rs:437` → `crate::memory_agent::trigger_final_extraction_with_dir(transcript, sid, dir)`).
   - Server-side hooks: `client_actions.rs:696`, `client_disconnect_cleanup.rs:169`, `comm_session.rs:718`, `debug_session_admin.rs:129`.
4. **Manual** — `mpr_status` debug command etc., not part of normal flow.

Inside `extract_from_context` (the incremental path, `memory_agent.rs:651`):

1. Skip if context < 200 chars or sidecar disabled.
2. Pull "existing" content for dedup: `manager.find_similar(context_summary, 0.25, 80)` first, fallback to `list_all().take(40)`.
3. `sidecar.extract_memories_with_existing(transcript, &existing)` returns `Vec<ExtractedMemory>`.
4. For each memory:
   - Check duplicate (`find_similar(content, 0.90, 1)`) — if hit, **reinforce** existing entry instead of inserting (project graph first, fall back to global).
   - Check contradiction (`find_similar(content, 0.5, 5)` filtered by category, then `sidecar.check_contradiction` per candidate) — if hit, store new and add `Contradicts` edge + supersede old.
   - Otherwise `manager.remember_project(entry)` (with `STORAGE_DEDUP_THRESHOLD = 0.85` further dedup).
5. Add `DerivedFrom` edges between every pair of newly-stored memories.

### 7.3 Per-turn pipeline kickoff

The non-blocking pattern is the same for both client modes:

```rust
// at the start of each turn
let pending = if message::ends_with_fresh_user_turn(&messages) {
    crate::memory::take_pending_memory(session_id)  // last turn's result
} else { None };

// fire context update for next turn (try_send, never blocks)
crate::memory_agent::update_context_sync_with_dir(session_id, messages, working_dir);

// inject `pending.prompt` into system prompt for THIS turn
```

Defined in:
- `src/agent/prompting.rs:20` — `Agent::build_memory_prompt_nonblocking_shared` (CLI agent).
- `src/tui/app/turn_memory.rs:98` — `App::build_memory_prompt_nonblocking` (TUI app).
- The TUI variant additionally renders the injected memory through `App::show_injected_memory_context` which records to session for replay and emits a "🧠 auto-recalled N memories" status notice.

### 7.4 Reset hooks

- `crate::memory_agent::reset()` clears all per-session state. Called from `tui/app/conversation_state.rs:422` on `/clear` and similar events.
- `crate::memory::clear_pending_memory(session_id)` and `clear_injected_memories(session_id)` — used on session reload, topic change, and `/clear`.
- `crate::memory::sync_injected_memories(session_id, ids)` — restores the injected-id set when a persisted session resumes (called from `tui/app/tui_lifecycle_runtime.rs:269` and `agent.rs:422`).

---

## 8. Per-Session vs Per-Project vs Global Memory

### 8.1 What's actually persisted

Only **two** scopes hit disk:

- **Project**: `<jcode_dir>/memory/projects/<hash16(project_dir)>.json`. The "project dir" is `MemoryManager.project_dir` (passed in via `with_project_dir`) or `std::env::current_dir()` as fallback.
- **Global**: `<jcode_dir>/memory/global.json`. Shared across all sessions.

`MemoryScope::All` aggregates both at read time (`collect_memories_scoped`).

### 8.2 What's per-session (in-memory only)

Per-session state is **runtime-only** — it lives in module-level `Mutex<HashMap<String, _>>` keyed by `session_id`:

- `PENDING_MEMORY` (most-recent retrieval result waiting to be injected on next turn).
- `LAST_INJECTED_PROMPT_SIGNATURE` (suppression).
- `LAST_INJECTED_MEMORY_SET` (suppression).
- `INJECTED_MEMORY_IDS` (cross-turn dedup of which memories have already been put into the prompt for this session).
- `MEMORY_CHECK_IN_PROGRESS` (single-flight gate for `spawn_relevance_check`).
- `MemoryAgent.sessions: HashMap<String, SessionState>` (per-session embeddings, surfaced memory ids, turn counts).

These are not persisted across server restarts. Session resume relies on `sync_injected_memories(session_id, ids)` reading the persisted session's already-injected ids and re-priming the dedup map.

### 8.3 Cross-store rules

- **Dedup checks both stores** (`find_duplicate_in_graph` runs against project then global). A duplicate found in global will reinforce the global entry even when calling `remember_project`.
- **Cascade retrieval** runs on each store independently and merges by best score (`find_similar_with_cascade_scoped`).
- **Tagging and linking** must happen within a single store. `link_memories` returns an error if the two ids are not in the same graph.
- **Skill-as-memory**: `MemoryManager.include_skills` (default `true` unless an `active_skill` is set) prepends `crate::skill::SkillRegistry::shared_snapshot().list().map(skill.as_memory_entry)` to the candidate set whenever `scope.includes_global()`. These synthetic entries are never persisted. They get a `skill_retrieval_bonus` (+0.08 to +0.20) added to their cosine score.

---

## 9. Test Surface and Memory Budget

### 9.1 Tests directly covering memory behaviour

| File | Scope |
|---|---|
| `src/memory_tests.rs` | `MemoryManager` end-to-end with a `JCODE_HOME` temp dir; `format_context_for_relevance` / `_for_extraction`; pending-memory lifecycle (freshness, suppression, per-session isolation); `MemoryStore::format_for_prompt` ordering. |
| `src/memory_agent_tests.rs` | `infer_candidate_tag` (tag inference from repeated non-stopwords); `apply_cluster_assignment` (cluster creation + InCluster edges + member count). |
| `src/runtime_memory_log_tests.rs` | Process-RSS log file rotation (unrelated to user memories — it logs `embedding::stats()` though). |
| `crates/jcode-memory-types/src/graph_tests.rs` | (Referenced by `mod graph_tests;`) — graph CRUD, edge bookkeeping, cascade retrieval. |
| Coverage in `agent_tests.rs` | `build_memory_prompt_nonblocking` deferral during tool loops, `set_pending_memory_with_ids` interactions. |
| Other call sites with assertions | `protocol_tests/misc_events.rs` (round-trip `ServerEvent::MemoryActivity`), `tui/info_widget_tests.rs` (rendering with mock activity), `tui/app/remote_tests.rs`. |

### 9.2 Key invariants exercised by tests

- `PendingMemory` becomes stale at >120 s and is dropped by `take_pending_memory`.
- Identical prompts within 90 s are suppressed.
- Memory-id sets with ≥80% overlap within 180 s are suppressed.
- Per-session `PendingMemory` slots do not bleed (test `pending_memory_per_session_isolation`).
- Replacement of a fresh, similar payload **keeps the original** rather than overwriting (test `pending_memory_keeps_existing_similar_payload_instead_of_replacing_it`).
- `MemoryStore::format_for_prompt` produces sections in the order Corrections → Facts → Preferences → Entities → custom.
- Cluster assignment creates exactly one cluster for ≥2 verified memories at the same timestamp and links each member with `EdgeKind::InCluster`.

### 9.3 The `MEMORY_BUDGET.md` regression budget

This file is about **process RSS / cache memory**, not user memories. It is the regression budget the team uses to prevent the host process from bloating. Relevant to `mempalace_rust` because `embedding::stats()` is part of the budget surface — the budget *implicitly assumes* that the embedding cache exists and stays bounded.

**Hard caps** (from `MEMORY_BUDGET.md`):

| Source | Cap |
|---|---|
| `HIGHLIGHT_CACHE_LIMIT` (markdown) | 256 entries |
| `RENDER_CACHE_MAX` (mermaid) | 64 entries |
| `IMAGE_STATE_MAX` (mermaid) | 12 entries |
| `SOURCE_CACHE_MAX` (mermaid) | 8 entries |
| `ACTIVE_DIAGRAMS_MAX` (mermaid) | 128 |
| Mermaid disk PNG cache | 50 MiB / 3 days |

**Implicit memory-system contributions** (from `runtime_memory_log.rs`):
- `EMBEDDING_CACHE_CAPACITY = 128` LRU entries × 384 dims × 4 bytes ≈ 200 KB.
- Project + global graph in-memory clones held in `GRAPH_CACHE` — bounded only by the on-disk JSON size (typically a few MB).
- `MEMORY_ACTIVITY.recent_events` capped at `MAX_RECENT_EVENTS = 10`.

**The implicit invariant the budget enforces**: any plug-in memory provider must not significantly grow the per-process RSS or the runtime log payload. `embedding::stats()` is included verbatim in the server runtime memory log (`ServerRuntimeMemoryEmbeddings.stats`), so swapping providers must keep that struct meaningful (or stub it out cleanly).

---

## 10. Crate Boundaries

### 10.1 What's in `jcode-memory-types`

`crates/jcode-memory-types/Cargo.toml`:

```toml
[dependencies]
chrono = { version = "0.4", features = ["serde"] }
jcode-core = { path = "../jcode-core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

It's deliberately **dependency-light** — no tokio, no reqwest, no embedding crate. It contains:

- All persistent types: `MemoryEntry`, `MemoryCategory`, `TrustLevel`, `Reinforcement`, `MemoryStore`.
- Graph types: `MemoryGraph`, `Edge`, `EdgeKind`, `TagEntry`, `ClusterEntry`, `GraphMetadata`, `GRAPH_VERSION`.
- Runtime activity types: `MemoryActivity`, `MemoryState`, `MemoryEvent`, `MemoryEventKind`, `PipelineState`, `StepStatus`, `StepResult`, `InjectedMemoryItem`.
- Pure helpers: `memory_score`, `format_entries_for_prompt`, `format_relevant_prompt`, `format_relevant_display_prompt`, `normalize_search_text`, `normalize_memory_search_text`, `memory_matches_search`, `is_skill_memory`, `collect_skill_query_terms`, `skill_retrieval_bonus`, `ranking::{top_k_by_score, top_k_by_ord}`.
- Migration helper: `MemoryGraph::from_legacy_store`.
- Cascade retrieval: `MemoryGraph::cascade_retrieve` (fully self-contained — uses pre-computed `f32` similarity scores, no embedding dependency).

What's **not** here: the `MemoryManager`, sidecar usage, embedding generation, file IO, the agent runtime, or any tokio/network code.

### 10.2 What's in `jcode-embedding`

Pure embedding backend (`crates/jcode-embedding/Cargo.toml`):

```toml
anyhow = "1"
reqwest = { version = "0.12", features = ["blocking"] }
tokenizers = { version = "0.21", default-features = false, features = ["onig"] }
tract-hir = "0.21"
tract-onnx = "0.21"
```

Public API (`crates/jcode-embedding/src/lib.rs`):
- `pub const MODEL_NAME: &str = "all-MiniLM-L6-v2"`
- `pub type EmbeddingVec = Vec<f32>`
- `pub struct Embedder` with `load_from_dir(&Path)`, `embed`, `embed_batch`.
- `pub const fn embedding_dim() -> usize { 384 }`
- `cosine_similarity`, `batch_cosine_similarity`, `find_similar`, `is_model_available`.

No tokio, no jcode-core dependency. Designed so the binary's `embedding.rs` facade can wrap it with caching/idle-unload.

### 10.3 What's in the binary `src/`

Everything that needs jcode-specific types (`crate::message`, `crate::config`, `crate::sidecar`, `crate::storage`, `crate::logging`, `crate::telemetry`, `crate::skill`, `crate::protocol`):

- `src/memory.rs` — `MemoryManager` (file-backed loader/saver, sidecar callouts, retrieval orchestration).
- `src/memory_agent.rs` — singleton, channel, per-session state, BFS+sidecar pipeline, post-retrieval maintenance, final extraction.
- `src/memory/{activity,cache,pending}.rs` — runtime statics.
- `src/memory_log.rs` — JSONL event log writer.
- `src/memory_prompt.rs` — `format_context_for_*` formatters (depends on `crate::message::Message`).
- `src/memory_graph.rs`, `src/memory_types.rs` — thin re-export shims that expose `jcode-memory-types` symbols at the old paths.
- `src/sidecar.rs` — Codex Spark / Claude Haiku LLM client (depends on `crate::auth`, `crate::provider`).
- `src/embedding.rs` — process-wide embedding facade (depends on `crate::storage::jcode_dir`).
- `src/tool/memory.rs` — the LLM-callable `memory` tool.
- `src/protocol_memory.rs` — re-export of `jcode_protocol::protocol_memory::*`.

### 10.4 What an external memory provider crate would need to provide

To replace this with mempalace_rust without losing the existing UI surface, a provider crate must satisfy:

1. **Read API** that returns `MemoryEntry` (or its mempalace equivalent mappable to `MemoryEntry`):
   - `find_similar(text, threshold, limit) -> Vec<(MemoryEntry, f32)>`
   - `find_similar_scoped(text, threshold, limit, scope)`
   - `search(query) -> Vec<MemoryEntry>` and `search_scoped`
   - `list_all_scoped(scope)`
   - `get_related(id, depth)` — graph traversal
   - `graph_stats() -> (memories, tags, edges, clusters)` (or compatible counts)
2. **Write API**:
   - `remember_project(MemoryEntry) -> String` and `remember_global(MemoryEntry) -> String` with built-in semantic dedup (current threshold 0.85).
   - `upsert_project_memory(MemoryEntry) -> String` and `upsert_global_memory`.
   - `forget(id) -> bool`.
   - `tag_memory(id, tag)` / `link_memories(from, to, weight)`.
   - `boost_confidence(id, amount)` / `decay_confidence(id, amount)` — used by post-retrieval maintenance.
3. **Retrieval orchestration** (could remain in jcode if mempalace_rust handles only persistence):
   - `get_relevant_parallel(session_id, messages, sink) -> (Option<prompt>, Vec<id>, Option<display>)`
   - `relevant_prompt_for_messages(messages) -> Option<String>`
4. **Lifecycle**:
   - `extract_from_transcript(transcript, session_id) -> Vec<String>` (returns inserted ids).
5. **Activity reporting**: write to `crate::memory::set_state` / `add_event` (or accept an event sink so the renderer keeps working). The TUI does not need to change if the same `MemoryActivity` is populated.

The cleanest seam puts a `MemoryProvider` trait in `jcode-memory-types` (or a new `jcode-memory-core` crate) and lets `MemoryManager` become one impl, mempalace another. The `MemoryEntry`/`MemoryGraph` types are already in the pure-types crate and would not need to move.

---

## Integration Seams for `mempalace_rust`

### A. Trait(s) a provider needs to implement

There is **no provider trait today** — `MemoryManager` is a concrete struct cloned through the call graph. To plug in mempalace cleanly, we should introduce one trait at the `MemoryManager`-shaped level (synchronous filesystem access) and let the `memory_agent` orchestrator continue to live in jcode.

Suggested shape (place in `crates/jcode-memory-types` — keeps the dep tree flat):

```rust
use anyhow::Result;
use std::sync::Arc;

/// A provider for persistent and graph-backed memories.
/// Implementations are expected to be cheap to clone (`Arc`-shaped) because
/// they are passed by value all over the codebase today (see MemoryManager).
pub trait MemoryProvider: Send + Sync + 'static {
    // ---- Insert / mutate ----
    fn remember_project(&self, entry: MemoryEntry) -> Result<String>;
    fn remember_global(&self, entry: MemoryEntry) -> Result<String>;
    fn upsert_project_memory(&self, entry: MemoryEntry) -> Result<String>;
    fn upsert_global_memory(&self, entry: MemoryEntry) -> Result<String>;
    fn forget(&self, id: &str) -> Result<bool>;
    fn tag_memory(&self, memory_id: &str, tag: &str) -> Result<()>;
    fn link_memories(&self, from_id: &str, to_id: &str, weight: f32) -> Result<()>;

    // ---- Confidence (used by post-retrieval maintenance) ----
    fn boost_confidence(&self, id: &str, amount: f32) -> Result<()>;
    fn decay_confidence(&self, id: &str, amount: f32) -> Result<()>;

    // ---- Read / search ----
    fn list_all_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>>;
    fn search_scoped(&self, query: &str, scope: MemoryScope) -> Result<Vec<MemoryEntry>>;
    fn find_similar_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>>;
    fn find_similar_with_embedding_scoped(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>>;
    fn find_similar_with_cascade_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>>;
    fn get_related(&self, memory_id: &str, depth: usize) -> Result<Vec<MemoryEntry>>;

    // ---- Stats ----
    fn graph_stats(&self) -> Result<GraphStats>;

    // ---- Lifecycle ----
    fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<String>>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GraphStats {
    pub memories: usize,
    pub tags: usize,
    pub edges: usize,
    pub clusters: usize,
}
```

A second optional trait gives mempalace a way to report activity through the existing UI plumbing, without forcing every provider to know about jcode's protocol crate:

```rust
pub trait MemoryActivitySink: Send + Sync + 'static {
    fn set_state(&self, state: MemoryState);
    fn add_event(&self, kind: MemoryEventKind);
    fn pipeline_start(&self);
    fn pipeline_update(&self, f: &mut dyn FnMut(&mut PipelineState));
    fn record_injected(&self, prompt: &str, count: usize, age_ms: u64);
}
```

`crate::memory::activity` would implement that trait so jcode keeps its singleton, and tests can stub it out.

A third small trait abstracts the embedding backend (already largely shaped this way by `jcode-embedding`):

```rust
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn cosine(&self, a: &[f32], b: &[f32]) -> f32;
}
```

This lets mempalace bring its own embedder if it wants (and lets us run jcode without `tract-onnx` baked in).

### B. Ideal external API (Rust trait sketches)

```rust
// crates/jcode-memory-types/src/provider.rs (new module)

use anyhow::Result;
use std::sync::Arc;
use crate::{MemoryEntry, MemoryScope};

#[derive(Debug, Clone)]
pub struct MemoryProviderConfig {
    pub project_dir: Option<std::path::PathBuf>,
    pub include_skills: bool,
    pub test_mode: bool,
    pub embedding_threshold: f32,         // default 0.5
    pub embedding_max_hits: usize,        // default 10
    pub storage_dedup_threshold: f32,     // default 0.85
}

pub trait MemoryProvider: Send + Sync + 'static {
    fn config(&self) -> &MemoryProviderConfig;
    // ... all methods listed in section A above ...
}

/// Convenience handle used by the rest of jcode.
pub type DynMemoryProvider = Arc<dyn MemoryProvider>;
```

Selection wires up like this:

```rust
// src/memory_provider_factory.rs (new)
pub fn build_memory_provider(cfg: &crate::config::Config) -> DynMemoryProvider {
    match cfg.agents.memory_backend {
        MemoryBackend::Local      => Arc::new(JcodeLocalProvider::new(...)),
        MemoryBackend::Mempalace  => Arc::new(MempalaceProvider::open(...)),
    }
}
```

The current `MemoryManager` becomes `JcodeLocalProvider` with one rename + the trait impl. Most call sites currently look like:

```rust
let manager = MemoryManager::new().with_project_dir(dir);
manager.remember_project(entry)?;
```

After the seam:

```rust
let provider = jcode::memory::provider();   // returns DynMemoryProvider
provider.remember_project(entry)?;
```

### C. Where in the code mempalace_rust would be wired in

These are the **exact** call sites where `MemoryManager::new()` (or the no-`new` `manager_for_working_dir`) is invoked. They would all switch to a provider factory:

| File | Line | Function | Current call |
|---|---|---|---|
| `src/memory_agent.rs` | ~135 | `manager_for_working_dir(working_dir)` | `MemoryManager::new().with_project_dir(dir)` |
| `src/memory_agent.rs` | ~220 (handle send path), ~265 (`run_final_extraction`) | extraction helpers | uses `manager_for_working_dir` |
| `src/memory_agent.rs` | ~380 | `MemoryAgent::manager_for_session(&self, session_id)` | constructs `MemoryManager` from `SessionState.working_dir` |
| `src/memory.rs` | top | `MemoryManager::new()`, `with_project_dir`, `with_skills` | concrete type |
| `src/memory.rs` | ~1130 | `MemoryManager::spawn_relevance_check` | clones `self`; if no `project_dir`, fills it from `current_dir()` |
| `src/agent/turn_execution.rs` | ~770 | `Agent::extract_session_memories` | `MemoryManager::new().with_project_dir(dir)` |
| `src/tui/app/turn_memory.rs` | ~155 | `App::extract_session_memories` | builds `MemoryManager::new().with_project_dir(dir).with_skills(active_skill.is_none())` |
| `src/tool/memory.rs` | ~12, ~17 | `MemoryTool::new`, `MemoryTool::new_test` | concrete `MemoryManager::new()` / `new_test()` |
| `src/tool/mod.rs` | 233 | builtin tool registry | constructs `MemoryTool::new` factory |
| `src/server.rs` | ~988 | `crate::memory_agent::init().await` | only entry point that decides whether to start the agent at all |

Plus the consumers of activity events (no changes needed if we keep emitting through `crate::memory::set_state`/`add_event`):

- `src/tui/app/remote/server_events.rs:1308` — `ServerEvent::MemoryActivity`.
- `src/tui/info_widget_memory_render.rs` — TUI renderer reads from `get_activity()`.
- `src/protocol_tests/misc_events.rs` — wire round-trip test.

### D. Types that cross the boundary vs stay internal

**Must cross the boundary** (mempalace produces or accepts these directly):

- `MemoryEntry`, `MemoryCategory`, `TrustLevel`, `Reinforcement`, `MemoryScope` (already in `jcode-memory-types`).
- `MemoryEntry`'s `embedding: Option<Vec<f32>>` field — provider must round-trip it. If mempalace uses a different dim, we need a coordinator to make sure jcode's `embedding::embed` and mempalace's both produce the same dim — easiest path is to have mempalace accept the existing 384-dim `all-MiniLM-L6-v2` vectors generated by `jcode-embedding`.
- `GraphStats` (new, simple struct) for `mpr_status` / TUI display.
- A minimal `MemoryEvent` / `MemoryState` if we want mempalace to push activity directly (otherwise mempalace just returns results and jcode's wrapper does the bookkeeping).

**Can stay internal to jcode**:

- `MemoryGraph`, `Edge`, `EdgeKind`, `TagEntry`, `ClusterEntry`, `GraphMetadata`, `GRAPH_VERSION` — only meaningful for the JSON-on-disk backend. Mempalace doesn't need to expose its internal palace structure here.
- `MemoryActivity` / `PipelineState` / `MemoryEventKind` — UI surface. Mempalace just needs to feed `MemoryActivitySink` events; the structs themselves stay in jcode-memory-types.
- `PendingMemory`, `INJECTED_MEMORY_IDS`, suppression statics — purely jcode's "next-turn" plumbing. Mempalace returns results synchronously; jcode owns the per-turn dance.
- Sidecar (`crate::sidecar::Sidecar`, `ExtractedMemory`) — orthogonal LLM client. Mempalace can either supply its own extraction flow or let jcode keep using the sidecar to extract and then call `mempalace.remember_*`.

**Configuration**:

- `agents.memory_model` and `agents.memory_sidecar_enabled` already exist in `AgentsConfig` (`crates/jcode-config-types/src/lib.rs:371`). Add a new field:

```rust
pub struct AgentsConfig {
    // existing
    pub memory_model: Option<String>,
    pub memory_sidecar_enabled: bool,
    // new
    pub memory_backend: MemoryBackend,                // default Local
    pub mempalace: Option<MempalaceConfig>,           // path, embed model name, etc.
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryBackend { #[default] Local, Mempalace }
```

The existing `features.memory: bool` toggle remains the master kill-switch.

### E. Existing plugin / extension surfaces we could reuse

jcode already has three extension mechanisms; only the third is a clean fit for memory:

1. **`src/skill.rs`** — file-driven skills (`SKILL.md` frontmatter). They can target a different model and have allowed-tool lists, but they are fundamentally prompts + tool gates, not data providers. **Not a fit.**
2. **`src/mcp/`** — MCP client that connects to external `stdio`/HTTP MCP servers and exposes their tools to the model (`crates/jcode-tool-core::Tool`, registered via `mcp::tool::create_mcp_tools`). This **could** be used to expose mempalace as an MCP server, but it would only give the model an additional tool; it would not satisfy the memory-agent path (auto-injection on every turn, post-retrieval maintenance, activity events). **Useful as a fallback / read-only deployment, not a replacement.**
3. **`extension_policy.rs`** + the `Tool` trait + the registry's `JCODE_NO_BUILTIN_TOOLS=1` knob — a registry-level swap. Practical for replacing the `MemoryTool` (so the LLM-facing tool calls hit mempalace), but doesn't change the auto-recall path. **Useful to retire the old tool once the new provider is wired.**

There is **no existing provider/plugin trait for memory specifically**. That is the cleanest place to introduce a new trait without colliding with current extension mechanisms. The `Tool` trait stays; `MemoryProvider` is new.

### F. Suggested integration phases

1. **Phase 0 — preserve types**: Move nothing. Add `pub trait MemoryProvider` to `crates/jcode-memory-types` and have the existing `MemoryManager` impl it (mostly a `cargo check` exercise). Switch all 12 call sites to take `Arc<dyn MemoryProvider>` via a single `crate::memory::provider()` accessor. Tests should still pass against the local backend.
2. **Phase 1 — feature flag**: Add `agents.memory_backend` config + factory. With `Mempalace` selected, return a stub provider that errors loudly. Confirm `features.memory = false` cleanly disables both.
3. **Phase 2 — read-through mempalace**: Implement `MempalaceProvider` that mirrors `MemoryManager` reads against a mempalace palace (palace path from `mempalace.path` config). Insert path can stay no-op or write-through to both backends.
4. **Phase 3 — full**: Make mempalace authoritative for inserts; wire `extract_from_transcript` to mempalace's `add_drawer`/general-extractor; map mempalace search results to `MemoryEntry` (round-trip embedding via `Vec<f32>` if dimension matches, else recompute via `jcode-embedding`). Keep the agent orchestrator (`memory_agent.rs`) untouched — it is provider-agnostic.

The biggest non-obvious risk is the **embedding-dimension contract**: anything that reads `MemoryEntry.embedding` and runs `embedding::cosine_similarity` against the in-process `embedding::embed(query)` result assumes the same model. mempalace_rust currently defaults to the same `all-MiniLM-L6-v2` (384-dim) via its Python ONNX subprocess (per its README), so there is no mismatch as long as we don't switch jcode to a different embedder at the same time.

### G. What does NOT need to change

- `crates/jcode-memory-types` data shapes (`MemoryEntry` etc.) and their wire snapshots.
- The TUI renderer (`info_widget_memory_render.rs`, `ui_memory.rs`) — it reads `MemoryActivity` only.
- The protocol surface (`MemoryActivitySnapshot` in `jcode-protocol`) — unchanged.
- The `Tool` trait registration in `tool/mod.rs:233` — `MemoryTool::new` just gets a different provider behind the wheel.
- The pending-memory dance, suppression rules, and turn-N+1 injection — stays in jcode.
- The sidecar (`Sidecar`) — orthogonal; mempalace_rust can ignore it or jcode can route extracted memories from the sidecar into mempalace.

---

*End of report.*
