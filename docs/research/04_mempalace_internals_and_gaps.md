# MemPalace Rust — Internals and jcode Integration Gaps

**Audit target:** `/data/projects/mempalace_rust` at HEAD (2026-05-25).
**Audit purpose:** Map the *actual* code as it stands, then list what blocks linking it into jcode as a library.
**Approach:** Read every file in the task list and surrounding deps; treat README claims as marketing until verified against source.

> **TL;DR:** The Rust port is structured like a CLI/MCP application, not a library. The "vector DB" referenced in the README is **not active in production search** — `PalaceDb::query` runs Jaccard token-overlap on a JSON file. A real `EmbeddingDb` (using `embedvec` + an ONNX-via-Python subprocess) exists but is only wired into the bench crate. The KG, palace graph, normalizer, AAAK dialect, BM25 reranker, and 19-tool MCP server are all real and reasonably well-tested. For jcode integration, the port is usable as a *building-block library* (most modules are `pub`, types serialize cleanly, nothing requires a daemon), but the embedding/search story has to be replaced and a stable `MemoryStore` trait would need to be carved out around `PalaceDb`.

---

## 1. Crate topology

The workspace declares three members in `Cargo.toml`:

```toml
[workspace]
members = ["crates/core", "crates/cli", "crates/bench"]
```

| Crate | Pkg name | Bin | Purpose |
|-------|----------|-----|---------|
| `crates/core` | `mempalace-core` | — | All logic, library crate |
| `crates/cli` | `mempalace` | `mpr` | 6-line shim → `mempalace_core::cli::run()` |
| `crates/bench` | `mempalace-bench` | `mempalace-bench` | LongMemEval harness |

`crates/cli/src/main.rs` is literally:

```rust
fn main() -> Result<()> { mempalace_core::cli::run() }
```

So **all real code lives in `mempalace-core`**.

### Module layout in `crates/core/src/lib.rs`

`lib.rs` is unusually flat — **every module is `pub`** (45+ modules, no internal/private boundaries):

```
pub mod bm25; cli; closet_llm; config; constants; convo_miner;
pub mod corpus_origin; dedup; dialect; diary_ingest; doctor;
pub mod entity_detector; entity_registry; exporter; fact_checker;
pub mod general_extractor; hermes_integration; hooks_cli; i18n;
pub mod instructions; knowledge_graph; languages; layers;
pub mod llm_client; llm_refine; mcp_server; migrate; mine_lock;
pub mod mine_palace_lock; mine_pid_guard; miner; normalize;
pub mod onboarding; onnx_embed; palace_db; palace_graph;
pub mod project_scanner; query_sanitizer; repair; room_detector_local;
pub mod script_aware; searcher; signal_handler; spellcheck;
pub mod split_mega_files; sweeper;
```

Re-exports from `lib.rs`:
```rust
pub use config::Config;
pub use error::MempalaceError;     // a thiserror enum defined inline in lib.rs
```

### Workspace deps (root `Cargo.toml`)

`tokio`, `rusqlite` (bundled), `serde`/`serde_json`/`serde_yaml`, `regex`, `thiserror`/`anyhow`, `tracing`, `directories`, `walkdir`, `uuid`, `chrono`, `sha2`, `hex`, `clap`, `reqwest` (rustls + blocking), `tempfile`, `mockall`, `tokio-test`.

### Crate-specific deps

`mempalace-core` adds:
- `rmcp = "1.3"` (MCP server transport — this is the official Anthropic SDK)
- `embedvec = "0.5"` (in-process HNSW index)
- `edgebert = "0.4"` (model loader; actually unused inside `core` — only `bench` calls it)
- `unicode-script = "0.5"` (Unicode-aware entity detection)
- `signal-hook = "0.3"` (Ctrl-C)
- `libc` (Unix), `winapi` (Windows console)

### What's leaking implementation details

- **Everything is `pub`** including obviously-internal helpers. Examples:
  - `palace_graph` exposes `_load_tunnels`, `_save_tunnels`, `_GRAPH_CACHE` static, `cache_invalidation_count` — all `pub fn` (some prefixed with `_` as a convention).
  - `cli::run()` is `pub` even though no other crate should ever call it.
- **Internal types are `pub`** without trait abstractions:
  - `PalaceDb` is the *concrete* class consumers must use. There is no `MemoryStore` trait.
  - `DocumentEntry` is `pub(crate)` (only one of very few non-pub items).
- **The error type is undocumented**: `MempalaceError` is declared in `lib.rs` but virtually no code uses it — most modules return `anyhow::Result` instead, which loses the typed error advantage.
- **No `#[non_exhaustive]`** on any public enum/struct → every field/variant addition is a SemVer break.
- **The `cli`, `mcp_server`, `hooks_cli`, `onboarding`, `instructions` modules are interleaved with the library** — they should live in the `cli` crate or behind a feature flag, but instead they pull `clap`, `rmcp`, `directories`, etc. into every dependent.

---

## 2. Public API surface today

### Search entry point

```rust
// crates/core/src/searcher.rs

pub async fn search_memories(
    query: &str,
    palace_path: &Path,
    wing: Option<&str>,
    room: Option<&str>,
    n_results: usize,
    _embedding_model: Option<&str>,   // unused parameter, kept for API stability
) -> anyhow::Result<SearchResponse>;

pub async fn search_memories_with_rerank(
    query: &str, palace_path: &Path,
    wing: Option<&str>, room: Option<&str>,
    n_results: usize, _embedding_model: Option<&str>,
    use_bm25: bool,
) -> anyhow::Result<SearchResponse>;

pub async fn check_duplicate(
    content: &str, palace_path: &Path, threshold: f64,
) -> anyhow::Result<Option<String>>;       // returns drawer ID if duplicate

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult { text, wing, room, source_file, similarity, created_at, bm25_score, combined_score }
#[derive(Debug, Serialize)]
pub struct SearchResponse { query, filters, results }
#[derive(Debug, Serialize)]
pub struct SearchFilters { wing, room }
```

`_embedding_model` is taken as a parameter and **completely ignored** inside the function. There is no model selection at the search layer.

### Storage entry point

```rust
// crates/core/src/palace_db.rs

pub struct PalaceDb { documents, palace_path, collection_name }

impl PalaceDb {
    pub fn open(palace_path: &Path) -> anyhow::Result<Self>;
    pub fn open_collection(palace_path: &Path, collection_name: &str) -> anyhow::Result<Self>;

    pub async fn query(&self, query, wing, room, n) -> Result<Vec<QueryResult>>;
    pub fn query_sync(&self, query, wing, room, n) -> Result<Vec<QueryResult>>;
    pub fn query_sync_with_filter(&self, query, wing, room, n, &HashMap<String,String>) -> ...;

    pub fn add(&mut self, &[(&str,&str)], &[&[(&str,&str)]]) -> Result<()>;
    pub fn upsert_documents(&mut self, &[(String,String,HashMap<...>)]) -> Result<()>;
    pub fn delete_id(&mut self, id) -> Result<bool>;

    pub fn file_already_mined(&self, source_file, check_mtime) -> bool;
    pub fn file_already_mined_with_mode(...) -> bool;

    pub fn flush(&mut self) -> Result<()>;
    pub fn count(&self) -> usize;
    pub fn get_all(&self, wing, room, limit) -> Vec<QueryResult>;
    pub fn get_documents_by_session(&self, session_id) -> Vec<...>;
    pub fn get_document_metadata(&self, id) -> Option<&HashMap<...>>;
    pub fn get_documents(&self, ids: &[String]) -> Vec<String>;
}

// And the (unused-in-search) real vector DB:
pub struct EmbeddingDb { embedder: Arc<OnnxModel>, hnsw, documents, storage }
```

### Stability assessment

- **No public traits.** Consumers bind to concrete types (`PalaceDb`, `SearchResponse`, etc.).
- **No SemVer guarantees** — `version = "0.1.0"`, no API stability docs.
- **The async wrappers are fake async** — `query` calls `query_sync` directly (no tokio spawn, no IO, no `.await`). The `async` keyword is just there for symmetry with the bench harness and KG.
- **`Send + Sync`?** `PalaceDb` is `Send` but `!Sync` because rusqlite `Connection` lives in `KnowledgeGraph::conn` (which is `!Sync`). `PalaceDb` itself only owns a `HashMap` and a `PathBuf`, so it's `Send + Sync`. `OnnxModel` is `Send + Sync` (uses `Arc<Mutex<…>>`).

---

## 3. Storage backend

This is the most important — and most surprising — finding in the audit.

### What the README says

> "ChromaDB (cached, can go stale) → embedvec (in-process, always current)"
> "Centralized embedvec access, thread-safe singleton"

### What the code actually does

`PalaceDb` is **a single JSON file**. There is no `embedvec` index, no SQLite, no HNSW.

```rust
// palace_db.rs

pub struct PalaceDb {
    documents: HashMap<String, DocumentEntry>,   // in-memory, loaded on open()
    palace_path: PathBuf,
    collection_name: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct DocumentEntry {
    content: String,
    metadata: HashMap<String, serde_json::Value>,
}

impl PalaceDb {
    pub fn open(palace_path: &Path) -> anyhow::Result<Self> {
        let docs_path = palace_path.join(format!("{}.json", collection_name));
        let documents = if docs_path.exists() {
            serde_json::from_str(&fs::read_to_string(&docs_path)?).unwrap_or_default()
        } else { HashMap::new() };
        ...
    }

    fn save(&self) -> Result<()> {
        let docs_path = self.palace_path.join(format!("{}.json", self.collection_name));
        fs::write(docs_path, serde_json::to_string_pretty(&self.documents)?)?;
        Ok(())
    }
}
```

**On-disk layout:**
```
<palace_path>/
└── mempalace_drawers.json     # {<id>: {content, metadata}}
```

### How "search" actually works

```rust
// query_sync_with_filter(...)
let similarity = naive_similarity(&query_lower, &entry.content.to_lowercase());
if similarity > 0.05 { ... }

fn naive_similarity(query: &str, content: &str) -> f64 {
    let q: HashSet<_> = query.split_whitespace().collect();
    let c: HashSet<_> = content.split_whitespace().collect();
    intersection / union     // Jaccard on whitespace tokens
}
```

**This is Jaccard token-overlap.** No embeddings, no vectors, no semantic similarity. Linear scan over every document. Lower-cased on every query.

### The unused real index

`palace_db.rs` *does* define a real vector index:

```rust
pub struct EmbeddingDb {
    embedder: Arc<OnnxModel>,
    hnsw: embedvec::HnswIndex,    // 16, 200, Cosine
    documents: Vec<(String, String)>,
    storage: embedvec::VectorStorage, // 384-dim, Quantization::None
}
```

But `EmbeddingDb` is **only constructed in `crates/bench/src/runner.rs`**:

```bash
$ grep -r "EmbeddingDb" crates/
crates/core/src/palace_db.rs:    pub struct EmbeddingDb { ... }
crates/core/src/palace_db.rs:    impl EmbeddingDb { ... }
crates/bench/src/runner.rs:      use mempalace_core::palace_db::EmbeddingDb;
crates/bench/src/runner.rs:      let mut db = EmbeddingDb::with_embedder(embedder.clone(), 384)?;
```

The bench harness *does* run real semantic search (and presumably gets the 96.6% R@5 number). **But the production `mpr search` / `mpr_search` MCP tool does not.** The `embedding_model` config field on `Config` defaults to `"naive"`, which the README candidly admits is "word overlap similarity (current default)".

### Dedup + BM25 ride on top

- `bm25.rs` defines `Bm25Scorer` and `Bm25Params` — fed only after `PalaceDb::query` returns its top-N results. So the BM25 reranker reorders the *Jaccard-already-pre-filtered* top-15 (3× requested), capping any recall improvement.
- `dedup.rs` calls `palace_db.get_all(...)` and checks Jaccard similarity for near-duplicate detection.
- `check_duplicate` runs `query` and checks `1 - distance >= threshold`.

### Persistence semantics

- `add(...)` mutates the in-memory `HashMap` — **does not auto-save**.
- Callers must explicitly call `flush()` (which is just `save()` → write JSON).
- `delete_id(...)` *does* auto-save.
- No WAL, no atomic write, no fsync — just `fs::write`. (Concurrency is "guarded" by `mine_palace_lock` PID file at the *miner* level, not the DB level.)

### Implication

Anyone who reads README → uses MemPalace expecting semantic search → discovers `mpr search "what did we decide about auth"` only matches if those literal words appear in the drawer content. The 96.6% LongMemEval number is from a different code path (`EmbeddingDb` in the bench).

---

## 4. Embedding subprocess

`onnx_embed.rs` is a thin Rust wrapper around `onnx_embed_python.py`:

```rust
pub struct OnnxModel {
    script_path: PathBuf,
    shared: Arc<Mutex<SharedProcess>>,
}

struct SharedProcess {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    started: bool,
}

impl OnnxModel {
    pub fn load() -> anyhow::Result<Self>;     // doesn't spawn yet
    fn ensure_started(&self) -> anyhow::Result<()>;  // lazy spawn on first encode
    pub fn encode(&self, &str) -> Result<Vec<f32>>;
    pub fn encode_batch(&self, &[&str], _normalize: bool) -> Result<Vec<Vec<f32>>>;
    pub fn dimension(&self) -> usize { 384 }   // hardcoded
}
```

```python
# onnx_embed_python.py
from chromadb.utils.embedding_functions.onnx_mini_lm_l6_v2 import ONNXMiniLM_L6_V2
model = ONNXMiniLM_L6_V2()
# stdin: JSON list of strings → stdout: one JSON-encoded float[] per line, then "DONE"
```

### Why it exists (probable reasons)

- Original Python MemPalace used ChromaDB's bundled ONNX runtime (`ONNXMiniLM_L6_V2`) which is non-trivial to invoke directly without ChromaDB.
- Pulling all of ChromaDB into Rust would mean linking ONNX Runtime C++ via FFI (`ort` crate or similar) plus tokenizer code (Hugging Face tokenizers).
- The shortcut: spawn a Python process, talk to it over stdio.

### Costs

| Concern | Impact |
|---------|--------|
| **Startup** | 1.5-3s for first embed call (`import chromadb` + ONNX model load). README's "<10ms startup" claim only counts `mpr` itself, not the embed pipe. |
| **Per-call latency** | ~10-30ms per text via stdio JSON. Fine for 100s of inserts; bad for thousands. |
| **Packaging** | The README's "single binary, zero deps" promise is undermined: `install.sh` `pip install`s `chromadb` separately. The Python is shipped *next to* the binary as `onnx_embed_python.py`, located via `env!("CARGO_MANIFEST_DIR")` (which is wrong post-install — it points to the build directory). |
| **Distribution** | Cross-compilation works for the Rust binary but the consumer still needs a Python 3.8+ install with `pip install chromadb`. |
| **Concurrency** | `Arc<Mutex<SharedProcess>>` serializes embed calls. The Python child gets one batch at a time. No multi-process pool. |
| **Crash recovery** | None. If the child exits mid-batch, `read_line` blocks. No timeout. No keepalive. |
| **`Drop` shutdown** | `Drop::drop` writes `b"QUIT\n"` to stdin without checking if the child is still alive. Best-effort. |

### Why it matters for jcode

If jcode wants real semantic search, embeddings must move to a Rust-native path. Options the project hasn't taken:

- `ort` crate (ONNX Runtime FFI) — known-good route used by `cmrc`, `tract`, etc.
- `candle` (Hugging Face's pure-Rust framework, no Python).
- `fastembed-rs` (already wraps ONNX Runtime + tokenizers and supports `all-MiniLM-L6-v2` directly with a single API call).
- Or: bring-your-own-embeddings via a `trait Embedder { fn embed(&self, &str) -> Vec<f32>; }`.

Crucially, `OnnxModel` is the **only** embedder the codebase knows about; there's no abstraction over it. `EmbeddingDb::new()` calls `OnnxModel::load()` directly.

---

## 5. Knowledge graph

`knowledge_graph.rs` is a real, well-built temporal triple store on SQLite (`rusqlite` bundled).

### Schema

```sql
CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT DEFAULT 'unknown',
    properties TEXT DEFAULT '{}',
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE triples (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    valid_from TEXT,
    valid_to TEXT,
    confidence REAL DEFAULT 1.0,
    source_closet TEXT,
    source_file TEXT,
    source_drawer_id TEXT,        -- RFC 002 §5.5 provenance
    adapter_name TEXT,            -- RFC 002 §5.5 provenance
    extracted_at TEXT DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (subject) REFERENCES entities(id),
    FOREIGN KEY (object) REFERENCES entities(id)
);

CREATE INDEX idx_triples_subject   ON triples(subject);
CREATE INDEX idx_triples_object    ON triples(object);
CREATE INDEX idx_triples_predicate ON triples(predicate);
CREATE INDEX idx_triples_valid     ON triples(valid_from, valid_to);

CREATE TABLE episodes (              -- "did this drawer help when retrieved?"
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    drawer_id TEXT NOT NULL,
    query TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('helpful','unhelpful','neutral')),
    feedback_at TEXT DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_episodes_drawer  ON episodes(drawer_id);
CREATE INDEX idx_episodes_outcome ON episodes(outcome);
```

`PRAGMA journal_mode=WAL` is set on `open()`. There's an in-place `migrate_schema()` for legacy palaces missing the `source_drawer_id`/`adapter_name` columns.

### Temporal model

- Triples carry `valid_from` / `valid_to` (TEXT, ISO-8601). `current` is computed as `valid_to IS NULL`.
- `add_triple` rejects inverted intervals (`valid_to < valid_from`). Open intervals and same-day points are accepted.
- `add_triple` auto-resolves conflicts: if `(subject, predicate, ?)` already exists with `valid_to IS NULL`, the old triple's `valid_to` is set to the new one's `valid_from` (or `now()`).
- `query_entity(name, as_of, direction)` filters by `(valid_from <= as_of) AND (valid_to >= as_of OR valid_to IS NULL)`.
- `invalidate(s, p, o, ended)` sets `valid_to`.

### Performance ceiling

- TEXT date comparison (lexicographic) — works for ISO-8601 but not ms-precision; mixing date/datetime forms is rejected at write time via `config::sanitize_iso_temporal`.
- All indexes are single-column or `(valid_from, valid_to)` — multi-predicate temporal queries do a scan + filter.
- No connection pool, no `Send + Sync` connection sharing — `KnowledgeGraph` owns one `Connection`, so concurrent queries must serialize through `&mut self` for writes (reads are `&self` and SQLite WAL allows concurrent readers).
- 12 tests cover invalidation, point-in-time queries, conflict auto-resolution, schema migration, and inverted-interval rejection.
- For jcode usage at "every turn", expect easily 1000s of triples/day; SQLite + WAL + per-column indexes will hold up well into millions of rows. The bottleneck won't be the KG.

---

## 6. Palace graph

`palace_graph.rs` is the wing/room/hall/tunnel BFS layer.

- **Tunnels** are stored in a separate file at `<config.tunnel_file()>` (default: `<palace>/tunnels.json`) — *not* in the vector DB. Fix #1467 moved this from a hardcoded `~/.mempalace/tunnels.json`.
- Tunnel I/O is atomic with `fsync` + `rename`, secure perms (`0o600` on Unix), and a `tunnels.json.tmp` sidecar.
- In-memory cache is a `static RwLock<GraphCache>` with a 60s TTL plus a `static AtomicU64 _GRAPH_BUILD_VERSION` counter.
- `compute_topic_tunnels` cross-references topics across wings: if two wings both have a topic above `min_count`, a tunnel is created. Wing names are normalized (`config::normalize_wing_name` lowercases and replaces ` `, `-` with `_`).
- BFS traversal is implemented for `traverse(start_room, max_hops)`.

### How it interacts with vector search

It doesn't, directly. The flow is:

1. `mpr_search` → `PalaceDb::query` returns drawers with `wing`/`room` metadata.
2. `mpr_traverse` / `mpr_find_tunnels` work entirely off the tunnels.json file + the wing/room metadata stored on each drawer.
3. The graph is rebuilt from `palace_db.get_all(...)` (scanning every drawer's metadata) when the cache is cold.

This means **the graph is always up-to-date with whatever `PalaceDb` reflects** — but `PalaceDb` is the JSON file from §3, so the graph cap-tops at "however many drawers fit in a single JSON load".

### Statics that block library use

```rust
static _GRAPH_CACHE: RwLock<GraphCache> = RwLock::new(...);
static _GRAPH_BUILD_VERSION: AtomicU64 = AtomicU64::new(0);
```

These are **process-wide mutable state**. If a long-running host (jcode) opens two palaces in the same process, the cache key is "global, last one wins" — a single graph cache for whichever palace was queried most recently. Same problem as the OnceLock-based `SCHEMA_PROPS` in `mcp_server.rs` and `SHUTDOWN_REQUESTED` in `signal_handler.rs`, although those are read-mostly.

---

## 7. Mining pipeline

`miner.rs` (project files) and `convo_miner.rs` (conversation exports) share a normalize/chunk/dedup/upsert structure.

### Normalize formats (`normalize.rs`, 1.4kLOC)

Functions named `try_<format>_jsonl` / `normalize_<format>` for:
- Claude Code JSONL (with `tool_use`/`tool_result` block handling)
- Claude.ai JSON
- ChatGPT JSON (`mapping` shape)
- Slack JSON (with provenance footer)
- Codex CLI JSONL (nested + flat)
- SoulForge JSONL
- OpenCode SQLite (`session` table with `dir` column)
- Aider markdown (`.aider.chat.history.md`)
- Plain text fallback

`strip_noise()` removes Claude Code system tags (`<system-reminder>`, hook chrome, command messages, etc.). `flatten_content()` (in `sweeper.rs`) handles the `[{"type":"text"}, {"type":"tool_use"}, ...]` arrays.

### Chunking

`miner.rs` exposes `CHUNK_SIZE = 800` (chars), `CHUNK_OVERLAP = 100`, `MIN_CHUNK_SIZE = 50`. `MAX_CHUNKS_PER_FILE = 50_000` is the safety rail (was 500, raised in #1455 for full-text books).

### Dedup

`PalaceDb::file_already_mined(source_file, check_mtime)` does an O(n) scan over the in-memory `HashMap` looking for matching `source_file` metadata. There's no index on `source_file`, but the dedup happens in-process so it's a HashMap walk. The README's "O(1) HashSet membership" claim is partially accurate: at the start of mining, all drawers' source paths are walked once into a `HashSet`, then membership is O(1). But that pre-walk is O(n) over the entire palace's documents.

### Batch I/O

`PalaceDb::add` accepts `&[(&str,&str)]` + `&[&[(&str,&str)]]` (parallel slices). The miner accumulates per-file chunks then calls `add` once per file. `flush()` is called at end-of-mine. So per-file we get one HashMap insert pass + one `serde_json::to_string_pretty` + one `fs::write` at the very end.

For large mines this means the entire JSON is rewritten once. There's no incremental persistence; if `mpr mine` crashes mid-run, you lose everything since the last `flush()`. The miner does call `flush()` periodically inside `mine_with_options` (worth verifying — see `mine_pid_guard.rs`).

### Mining mutex

`mine_palace_lock.rs` writes a PID file in an OS-state dir (`mine_palace_<sha256>.lock`) keyed on the *palace path*. Two `mpr mine` processes against the same palace get rejected with `MineAlreadyRunning { pid }`. Stale lock cleanup is `mpr repair --cleanup-pid`.

### General extractor

`general_extractor.rs` (700+ LOC) classifies text into 5 buckets — DECISION / PREFERENCE / MILESTONE / PROBLEM / EMOTION — using regex marker lists + sentiment word lists. Pure heuristic, no LLM. Used by `mpr mine --extract general`.

---

## 8. MCP server

`mcp_server.rs` (~106 KB) is the largest file in the crate. Built on `rmcp = "1.3"` with stdio transport.

### Setup

```rust
pub fn run_server(palace_override: Option<&str>, read_only: bool) -> anyhow::Result<()> {
    let mut config = Config::load()?;
    if let Some(p) = palace_override {
        config.palace_path = resolve_palace_override(p);
    }
    let server = MempalaceServer::new(AppState::new(config, read_only)?);
    let (stdin, stdout) = stdio();
    let rt = Runtime::new()?;
    rt.block_on(async {
        let running = server.serve((stdin, stdout)).await?;
        running.waiting().await?;
    })
}
```

### State

```rust
pub struct AppState {
    pub config: Config,
    pub db: PalaceDb,
    pub read_only: bool,
    pub palace_path: PathBuf,
}
```

`AppState` is shared via `Arc<AppState>` in the dispatch closure. The `db` field is opened *once* at startup, but most tool handlers re-`PalaceDb::open` from the path each call (because `PalaceDb` reads the JSON file fresh each time — there's no persistent connection to share).

### 19 tools

All declared in `make_tools()` returning `Vec<rmcp::model::Tool>`:

```
mempalace_status, mempalace_list_wings, mempalace_list_rooms,
mempalace_get_taxonomy, mempalace_get_aaak_spec, mempalace_search,
mempalace_check_duplicate, mempalace_add_drawer, mempalace_delete_drawer,
mempalace_kg_query, mempalace_kg_add, mempalace_kg_invalidate,
mempalace_kg_timeline, mempalace_kg_stats, mempalace_traverse,
mempalace_find_tunnels, mempalace_graph_stats, mempalace_diary_read,
mempalace_diary_write
```

`MUTATION_TOOLS = [add_drawer, delete_drawer, kg_add, kg_invalidate, diary_write]` are hidden in `tools/list` when `read_only=true` (env var `MEMPALACE_READONLY=1`).

### Dispatch

```rust
fn make_dispatch(state: Arc<AppState>) -> impl Fn(String, JsonObject) -> DynResult {
    move |name, args| Box::pin(async move {
        match name.as_str() {
            "mempalace_status"     => tool_status(&state, args),
            "mempalace_search"     => tool_search(&state, args),
            ...
            other => Err(ErrorData::invalid_params(format!("Unknown tool: {}", other), None)),
        }
    })
}
```

The handlers are **synchronous functions** (`fn tool_xxx(&AppState, JsonObject) -> Result<CallToolResult, ErrorData>`) wrapped in `Box::pin(async move { ... })` to satisfy the trait. Inside, they sometimes start a `Runtime::new()` to call other async functions — see the test helper `dispatch()` which has a `try_current().or_else(|_| Runtime::new())` pattern.

### WAL logging

Every tool call appends to `<XDG_STATE_HOME>/mempalace/wal/write_log.jsonl` with timestamp / tool name / args / result summary / 16-char trace ID. Atomic via temp + rename.

### Can a Rust host call the tools directly?

**Mostly yes, but it's painful.** Each `tool_xxx` function takes `&AppState` and a raw `JsonObject` and returns `Result<CallToolResult, ErrorData>`. To avoid stdio entirely:

```rust
let state = AppState::new(config, false)?;
let result = tool_search(&state, args)?;   // tool_xxx fns are pub(crate), not pub
```

But:
- The `tool_xxx` functions are **private (no `pub`)** — only `make_dispatch` is reachable, and it's pinned to a stdio-shaped trait return type.
- The arg shape is `serde_json::Map<String, serde_json::Value>` per tool — there are no typed wrapper fns.
- `validate_known_params` (which checks for typos) is private and embedded in the dispatch path.
- Calling MCP-as-stdio from inside the same process is silly: spawn the same binary, pipe to it, parse JSON-RPC. That works but at significant overhead.

**Cleaner path for jcode:** call the underlying business-logic modules directly (`searcher::search_memories`, `KnowledgeGraph::add_triple`, etc.) and skip MCP entirely. The 19 tools are each a thin wrapper around 1-3 lines of those.

---

## 9. Configuration & paths

`config.rs` (28 KB) handles config + paths.

### Locations (XDG-conformant)

```rust
fn config_dir() -> Result<PathBuf>;           // $XDG_CONFIG_HOME/mempalace
fn data_dir()   -> Result<PathBuf>;           // $XDG_DATA_HOME/mempalace
fn config_file_path() -> Result<PathBuf>;     // <config_dir>/config.json
fn identity_file_path() -> Result<PathBuf>;   // <config_dir>/identity.txt
fn registry_file_path() -> Result<PathBuf>;   // <config_dir>/entity_registry.json
fn tunnel_file() -> PathBuf;                  // <palace_path>/tunnels.json
```

Falls back to `~/.mempalace/` if XDG can't be resolved or already exists (backward compat).

### Env vars

| Variable | Purpose |
|----------|---------|
| `MEMPALACE_PALACE_PATH` / `MEMPAL_PALACE_PATH` | Override palace location |
| `MEMPALACE_NONINTERACTIVE` | Skip prompts (CI/agents) |
| `MEMPALACE_READONLY` | MCP read-only mode |
| `MEMPALACE_EMBED_MODEL` | Override embedding model name |
| `MEMPALACE_MAX_CHUNKS_PER_FILE` | Per-file chunk cap |
| `MEMPAL_VERBOSE` | Verbose tracing |
| `XDG_STATE_HOME` | MCP WAL location |

### Config struct

```rust
pub struct Config {
    pub palace_path: PathBuf,
    pub collection_name: String,         // default "mempalace_drawers"
    pub people_map: HashMap<String, String>,
    pub topic_wings: Vec<String>,
    pub hall_keywords: HashMap<String, Vec<String>>,
    pub embedding_model: String,         // default "naive"
    pub languages: Vec<String>,
}
```

`Config::load()` reads JSON and merges in env-var overrides. `Config::save()` does atomic write (temp + rename).

### Single-palace assumption

`Config::load()` returns one `Config`. Every module that wants the palace path reads `Config::load()?.palace_path` (or accepts an explicit `--palace`). There's **no notion of "current project's palace" vs "user's global palace"** — it's one canonical location per `~/.config/mempalace/config.json`.

This is an active mismatch with jcode's per-working-directory model.

---

## 10. Test coverage

466 test attributes across 46 files (`#[test]` + `#[tokio::test]`). README claims 435 passing — slight discrepancy is likely a few `#[ignore]`-gated network/LLM tests.

### Test density (count of test attrs per module)

| Module | Tests | Coverage |
|--------|-------|----------|
| mcp_server | 54 | very high — every tool has happy + error path |
| cli | 42 | high — most subcommands |
| miner | 31 | high — gitignore, chunking, chunk-cap |
| layers | 28 | high — all 4 layers |
| entity_detector | 27 | high — Latin, Cyrillic, stoplist |
| config | 19 | high — env override, XDG, locale |
| normalize | 15 | medium — covers most formats |
| onboarding | 15 | medium |
| entity_registry | 15 | medium |
| knowledge_graph | 12 | medium — temporal, conflict, schema migration |
| searcher | 12 | medium — query shape, sanitization, palace-state errors |
| spellcheck | 12 | medium |
| convo_miner | 12 | medium |
| general_extractor | 12 | medium |
| palace_graph | 10 | medium |
| script_aware | 10 | medium |
| split_mega_files | 9 | medium |
| corpus_origin | 9 | medium |
| dialect (AAAK) | 7 | thin |
| sweeper | 7 | thin |
| room_detector_local | 7 | thin |
| dedup | 1 | very thin |
| repair | 1 | very thin |
| hermes_integration | 1 | trivial smoke test |
| doctor | 2 | thin |
| signal_handler | 2 | thin |
| migrate | 3 | thin (the ChromaDB→embedvec migrator) |
| **palace_db** | **5** | **thin — and only tests Jaccard, never EmbeddingDb** |

### Notable gaps

- `palace_db.rs` has **no test for `EmbeddingDb`** — its only consumer is the bench harness, which has 1 test in `runner.rs`.
- `onnx_embed.rs` has **zero tests** — it spawns a Python child unconditionally.
- `dedup.rs` has 1 smoke test.
- `repair.rs` has 1 smoke test.
- `hermes_integration.rs` has one test that only goes through `MemPalaceHermesProvider`'s in-process path.

### Property of test design

- Tests use `tempfile::tempdir()` heavily — there's a `test_env_lock()` helper in `lib.rs` to serialize tests that touch env vars. So parallel `cargo test` works.
- No fuzz tests, no proptest. Heavy reliance on golden strings and `assert_eq!`.

---

## 11. Performance characteristics

### Benches

Only one bench harness exists, `crates/bench/`, and it benchmarks `EmbeddingDb` (the *unused* code path) against LongMemEval. Results aren't checked in.

There are **no `criterion` benchmarks** for the production search path, mining throughput, or KG insert rate. The README's perf claims are unverifiable from this repo:
- "<10ms startup" — `mpr` + `clap` parse, sure. But not including embed-pipe spawn.
- "~10MB memory" — for `mpr status` maybe. Mining a 100k-drawer corpus loads them all into a `HashMap<String, DocumentEntry>` in `PalaceDb::open`.

### Empirical estimates from reading the code

| Operation | Cost |
|-----------|------|
| `PalaceDb::open` | O(n) — `serde_json::from_str` of entire collection JSON. 100k drawers × 800 chars ≈ 80 MB JSON, parsing in seconds. |
| `PalaceDb::query` | O(n) — Jaccard scan over all docs. Pure CPU, unindexed. |
| `PalaceDb::add` | O(1) per doc, but `flush()` is O(n) write-back. |
| `KG::add_triple` | O(log n) on each index — fast (~µs). |
| `KG::query_entity(as_of)` | O(log n) but TEXT comparison + filter chain. |
| `mine_file` | O(file_size / chunk_size) — limited by I/O + dedup pre-walk. |
| MCP tool call | Cold: open palace JSON each call. Each call rebuilds `PalaceDb`. |

### Scalability cliffs

1. **Palace JSON size.** At ~1MB per 1000 drawers (rough), 100k drawers = ~100MB JSON. `serde_json::from_str` on that is 1-3 seconds and uses ~3× the file size in RAM. There's no streaming reader.
2. **Search at scale.** Linear Jaccard scan with no inverted index. 100k drawers × per-query string-set construction → seconds, not milliseconds.
3. **Vector path (when used).** `embedvec::HnswIndex` with M=16, ef=200, dimension=384 — should hit ~ms latency. But `EmbeddingDb` doesn't persist — there's no save/load path. Every `cargo run` rebuilds from scratch.

---

# GAPS for jcode integration

## Library entry vs CLI/MCP

There **is** a library entry. Every business-logic module is `pub mod` and re-exported via the workspace lib crate. So:

```rust
use mempalace_core::searcher::search_memories;
use mempalace_core::palace_db::PalaceDb;
use mempalace_core::knowledge_graph::KnowledgeGraph;
use mempalace_core::dialect;     // AAAK encode/decode
use mempalace_core::layers::MemoryStack;
```

…all work today. The MCP server and CLI are *additional* surfaces, not the only surface.

But the entry is *raw module exports*, not a curated API. Consumers must accept:

- All 45+ modules pulled into their dependency graph (clap, rmcp, reqwest, signal-hook, etc.).
- No traits to swap implementations against.
- Concrete `PalaceDb` is the only `MemoryStore` shape.
- `Config::load()` reads from XDG paths globally — no way to scope per consumer.

## Process boundary

- **No daemon.** Search/KG/graph all run in the calling process. ✅
- **MCP requires stdio.** If jcode wants the 19 tools, it must `Command::new("mpr").arg("mcp").spawn()` and JSON-RPC into the child. ❌ Avoidable by linking directly.
- **Embeddings require Python.** `OnnxModel` spawns `python3` on first `encode()`. Even though the production search path doesn't currently use embeddings (Jaccard only), the moment jcode wants real semantic search through this codebase, Python is in the loop. ❌

## Send/Sync/'static + async

- `PalaceDb` is `Send + Sync`. ✅
- `KnowledgeGraph` owns a `rusqlite::Connection`, which is `!Send + !Sync` by default. The `Connection` is `Send` if you `feature = "rusqlite/with_sqlite_unlock_notify"`, but even then it's `!Sync`. So `KnowledgeGraph` is `Send` but `!Sync` — locking in the consumer required. ⚠️
- `OnnxModel: Send + Sync` (uses `Arc<Mutex<…>>`). ✅
- Async surface is mostly **fake async** — `palace_db::query` is `async` but the body has no `.await`. So calling from tokio is fine but you don't get real concurrency benefits. ⚠️
- Tokio runtime is not assumed (the CLI builds a `Runtime::new()` only inside MCP). ✅

## Global mutable state

| Static | File | Risk |
|--------|------|------|
| `_GRAPH_CACHE: RwLock<GraphCache>` | palace_graph.rs | Cache pinned to last-queried palace. **Conflict** in long-running multi-palace host. |
| `_GRAPH_BUILD_VERSION: AtomicU64` | palace_graph.rs | Same. |
| `SHUTDOWN_REQUESTED: AtomicBool` | signal_handler.rs | Once set, all in-process miners abort. **Conflict** with jcode's own shutdown logic. |
| `SCHEMA_PROPS: OnceLock<HashMap<...>>` | mcp_server.rs | Built once, immutable — fine. |
| `SYSTEM_WORDS: OnceLock<HashSet<String>>` | spellcheck.rs | Same — fine. |
| `ENV_LOCK: OnceLock<Mutex<()>>` | lib.rs | Test-only — fine. |

Two of these (graph cache, shutdown) are real problems for a long-running embedded host.

## Data-model fit (jcode's `MemoryEntry` / `MemoryGraph` / `MemoryScope` / `TrustLevel`)

The MemPalace data model:

- **MemoryEntry** ≈ a "drawer" = `(id: String, content: String, metadata: HashMap<String, Value>)`. ✅ Trivial fit.
- **MemoryGraph** ≈ Knowledge graph triples + palace tunnels. ✅ Knowledge graph is rich (temporal, provenance, episodic feedback). Palace graph adds wing/room/hall/tunnel — heavier than jcode might need.
- **MemoryScope** — MemPalace has one global palace from `Config::palace_path`. There's no first-class "scope" concept. You'd have to encode jcode's scope as a **wing** name (e.g., `wing_<scope_hash>`). ⚠️
- **TrustLevel** — MemPalace has `confidence: Option<f64>` on triples and `helpfulness_score(drawer_id)` on retrieval feedback. No equivalent of "trust this user/agent more than that one." Has to be encoded as drawer metadata. ⚠️

## Per-project palaces

**Not native.** The whole config system assumes one palace per user. To approximate per-project:

- Pass `--palace <project>/.jcode/palace` everywhere. `PalaceDb::open(path)` does accept arbitrary paths. ✅ for the DB.
- But `Config::load()` reads global config (XDG path), so `people_map` / `topic_wings` / `embedding_model` are not per-project. ⚠️
- Tunnels live at `<palace>/tunnels.json` so they *are* per-palace. ✅
- KG lives at `<palace>/knowledge_graph.db` — the MCP code uses `state.palace_path.parent().join("knowledge_graph.db")`, which is `<palace_dir>/knowledge_graph.db`. ✅
- Mining lock and identity file are tied to the *global* config, not the palace path. ⚠️

So the **storage** is per-palace-friendly, but the **user-level config and onboarding** assumes one canonical install.

## Real-time low-latency add/search

If jcode adds memories every conversation turn:

- `add` to `PalaceDb` is HashMap insert — ~µs. ✅
- But every `add` requires a `flush()` to persist, which **rewrites the entire JSON file**. ❌
- `query` is O(n_drawers) Jaccard scan. At 1k drawers fine; at 100k unusable. ❌
- KG adds are SQLite WAL writes — millisecond range. ✅
- Re-opening `PalaceDb` per call (which `mcp_server.rs` does in some handlers) re-parses the entire JSON. ❌

So the storage layer needs **real persistence, real indexing, and connection caching** before any real-time use is viable.

## Bring-your-own-embedder

**Not supported.** `EmbeddingDb::with_embedder(Arc<OnnxModel>, dimension)` accepts a concrete `OnnxModel`, not a trait. To swap:

- You'd fork `palace_db.rs` and replace `embedder: Arc<OnnxModel>` with `embedder: Arc<dyn Embedder>` (where `Embedder` is a new trait).
- Or: don't use `EmbeddingDb` at all and build your own around `embedvec::HnswIndex` directly (small wrapper, ~100 LOC).

The README's "MEMPALACE_EMBED_MODEL env var" is a config string, not a swap mechanism — it's only used at the Python side to pick a different ONNX model file (and even then, only `ONNXMiniLM_L6_V2` is wired up).

## Zero-Python deployment

**Not yet.** As soon as embeddings are needed:
- The Rust binary spawns `python3 onnx_embed_python.py --persistent`.
- `pip install chromadb` is required (or whatever module the script imports).
- `install.sh` does the pip install for users.

For jcode (which is a single binary), this is a **regression**. To fix: replace `OnnxModel` with `ort` / `candle` / `fastembed-rs`.

## Type duplication risk vs `jcode-memory-types`

If jcode already has a `MemoryEntry`/`MemoryGraph`/`MemoryScope`/`TrustLevel` crate, integrating MemPalace will **duplicate types**:

- MemPalace's `searcher::SearchResult` (text, wing, room, source_file, similarity, …) doesn't compose cleanly with a generic `MemoryEntry`.
- MemPalace's `knowledge_graph::Triple` differs from a generic `(Subject, Predicate, Object, Time)`.
- Adapter layer needed: `From<mempalace::SearchResult> for jcode::MemoryEntry` etc. Doable but adds cost on every read.
- The cleanest split: jcode owns the public types; MemPalace becomes an *implementation* of a `jcode::MemoryStore` trait via a thin adapter crate. Currently impossible because `PalaceDb` is concrete and not behind a trait.

---

# Numbered list of integration gaps (severity-ranked)

## P0 — Hard blockers

1. **The "vector DB" is not actually used in production search.**
   `PalaceDb::query` runs Jaccard token-overlap on a JSON HashMap. The README's 96.6% R@5 is from `crates/bench`, which uses the `EmbeddingDb` path that nothing else calls. jcode integrating "as-is" gets a memory system that can only retrieve drawers containing the literal query terms. Either we accept that and route around it, or we wire `EmbeddingDb` into `PalaceDb` first.

2. **No `MemoryStore` trait — the only abstraction is the concrete `PalaceDb` struct.**
   jcode cannot swap implementations, mock the store in tests, or layer caching/policy on top without forking. Integration requires either (a) fork-and-trait-ify, (b) write an adapter crate that re-exports types, or (c) accept the concrete-coupling cost. Without this, every refactor in `mempalace-core` is a SemVer break in jcode.

3. **Persistence is whole-file JSON rewrite.**
   Every `flush()` rewrites the entire `mempalace_drawers.json`. At thousands of drawers, this is hundreds of milliseconds; at 100k+ it's seconds. jcode's "add memory every turn" usage pattern would either hammer disk or risk data loss between flushes. Needs a real KV store (sled, redb, sqlite) before integration.

4. **Embedding subprocess assumes Python + ChromaDB.**
   `OnnxModel` shells out to `python3` running `onnx_embed_python.py`, which `import chromadb`. jcode's "single binary" promise is incompatible with this. To get real embeddings inside jcode, we have to replace the embedding pipeline (ort / candle / fastembed-rs) or drop semantic search entirely.

5. **Single-palace global config conflict.**
   `Config::load()` reads from `$XDG_CONFIG_HOME/mempalace/config.json` — a process-global. jcode is per-working-directory. Modules (entity_registry, identity, embedding_model selection) bind to global config rather than to a palace handle. Refactor needed: thread a `&Config` (or `Arc<PalaceContext>`) through every public API instead of reading a global.

## P1 — Painful but solvable

6. **Process-global mutable statics conflict with long-lived embedded host.**
   `_GRAPH_CACHE` (RwLock) and `_GRAPH_BUILD_VERSION` (AtomicU64) in `palace_graph.rs` are pinned to last-touched palace. `SHUTDOWN_REQUESTED` (AtomicBool) in `signal_handler.rs` is process-wide and triggered by the CLI's Ctrl-C handler — installing it inside jcode would conflict with jcode's own signal handling. Replace globals with palace-scoped state on the `PalaceDb`/handle.

7. **`KnowledgeGraph` is `!Sync` — caller has to lock around it.**
   `rusqlite::Connection` isn't `Sync`, so concurrent reads from multiple tokio tasks need an explicit `Arc<Mutex<KnowledgeGraph>>`. jcode either accepts that (cheap given SQLite's own concurrency) or wraps it in a connection pool. Workable, but documenting it is necessary.

8. **MCP tool handlers are private functions, not a typed Rust API.**
   To call `mempalace_search` *as Rust*, you'd want `state.search(query, wing, room, limit) -> SearchResponse`. Today you must open a stdio MCP child or call the underlying `searcher::search_memories` (which works but is not the same surface — e.g., `search_memories` doesn't expose the `where_filter` metadata feature that `tool_search` adds). Need a thin `MempalaceClient` façade.

9. **No bring-your-own-embedder.**
   `EmbeddingDb::with_embedder(Arc<OnnxModel>)` takes a concrete model. Swapping to a different embedder (jcode's own ONNX wrapper, a Cohere call, a local llama.cpp embed endpoint) requires forking `palace_db.rs` to introduce a `trait Embedder`. ~100 LOC of refactor; not architectural.

10. **Async surface is ceremonial.**
    `search_memories` is `async` but does no `.await`. `KnowledgeGraph` is fully sync. Calling from tokio works but the API gives no async benefits. For real-time use, lifting search and add behind a `tokio::task::spawn_blocking` or true async I/O is necessary if drawer counts grow.

11. **No ingestion of jcode's existing types.**
    `convo_miner.rs` mines disk files (Claude Code JSONL, ChatGPT JSON, …). jcode's runtime memory entries are in-memory `Vec<MemoryEntry>` or similar. There's no `Miner::ingest(memory: &MemoryEntry)` API. We'd need a new entry point that wraps `PalaceDb::add` + `KnowledgeGraph::add_triple` per memory.

12. **Mining lock is per-process PID file, not per-handle.**
    `mine_palace_lock::mine_palace_lock` writes a PID file before starting a mine and refuses if another `mpr` PID holds it. In an embedded host, jcode's PID would write the lock; if jcode crashes, the lock is stale until `mpr repair --cleanup-pid` runs. Unfit for in-process re-entrant use.

13. **WAL log path uses XDG_STATE_HOME globally.**
    Every MCP tool call appends to one global `<XDG_STATE_HOME>/mempalace/wal/write_log.jsonl`. In a per-project setup you'd want `<palace>/wal/...`. Needs threading through `AppState`.

## P2 — Nice to have / future work

14. **No SemVer guarantees** — version is `0.1.0`, no `#[non_exhaustive]`, no published API stability docs. Pin to a commit hash and treat the API as breaking until upstream stabilizes.

15. **Test coverage thin in critical paths.**
    `palace_db.rs` (5 tests) doesn't cover `EmbeddingDb`. `onnx_embed.rs` (0 tests) is untested. `dedup.rs`, `repair.rs`, `migrate.rs` each have one smoke test. If we depend on these, we'd want to add tests in the adapter crate.

16. **`hermes_integration.rs` is mostly stub.**
    The `MemPalaceHermesProvider` ingests a fake "wing_hermes" then does substring filtering on retrieve — not vector search. The README's "Hermes integration" is aspirational. Don't build on this.

17. **CLI/MCP/onboarding modules in the same crate as the library.**
    Pulling `mempalace-core` brings in `clap`, `rmcp`, `directories`, `signal-hook`, `reqwest`, `urlencoding`. Library users pay this dep cost. Could be split with `default-features = ["cli"]` etc. — currently no features defined.

18. **No criterion benchmarks** — search latency and add throughput not measured. Before relying on this for jcode's hot path, add benches.

19. **Public modules expose internal helpers** (`_load_tunnels`, `_save_tunnels`, `_GRAPH_CACHE` access patterns). Hardens the public surface unintentionally; future cleanup may break consumers.

20. **Drawer ID semantics are not stable.**
    Mining computes drawer IDs from `sha256(source_file + chunk_index)` (per `miner.rs`). Re-mining the same file with a different chunk size produces different IDs. For jcode's "delta updates" to work, a stable ID scheme is needed.

---

# Recommendation

For jcode integration the cleanest path is **extract-don't-link**:

1. Take `knowledge_graph.rs`, `dialect.rs` (AAAK), `palace_graph.rs`, `bm25.rs`, `general_extractor.rs`, `normalize.rs`, and `query_sanitizer.rs` — these are well-tested and largely standalone. Vendor or re-crate them under jcode's namespace, behind jcode's own traits.
2. Replace `palace_db.rs` entirely. Use `embedvec::HnswIndex` directly + `redb`/`sled`/`sqlite` for drawer persistence. Skip the JSON-file approach.
3. Replace `onnx_embed.rs` with `fastembed-rs` (single dep, no Python). Same `all-MiniLM-L6-v2` model.
4. Skip `mcp_server.rs`, `cli.rs`, `onboarding.rs`, `instructions.rs`, `hooks_cli.rs`, `install.sh`. None of those belong inside jcode.
5. Keep the **semantics** — wing/room/hall/tunnel, AAAK, KG temporal model — and skip the **structure** (the global config, the JSON-file palace, the Python pipe).

Linking against `mempalace-core` directly is *technically* possible (most things are `pub`), but the resulting jcode would be a P0 blocker (#3 — JSON rewrites) away from production, and that fix is itself a fork. Better to vendor what works and rebuild what doesn't.
