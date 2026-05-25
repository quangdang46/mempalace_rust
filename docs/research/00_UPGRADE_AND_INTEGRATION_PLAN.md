# MemPalace-Rust → jcode Integration: Master Upgrade Plan

> Target: make `mempalace_rust` ready to be consumed by `/data/projects/jcode` as the persistent
> memory backend, while remaining a self-contained CLI/MCP product for standalone users.
>
> Source material: synthesised from
> - [`01_external_rust_memory_libs.md`](./01_external_rust_memory_libs.md) (40 KB)
> - [`02_memory_architectures_papers.md`](./02_memory_architectures_papers.md) (77 KB)
> - [`03_jcode_memory_internals.md`](./03_jcode_memory_internals.md) (67 KB)
> - [`04_mempalace_internals_and_gaps.md`](./04_mempalace_internals_and_gaps.md) (47 KB)
> - [`05_embedding_and_storage_native.md`](./05_embedding_and_storage_native.md) (33 KB)
> - [`06_agentmemory_repo_analysis.md`](./06_agentmemory_repo_analysis.md) (15 KB) — added 2026-05-25 in response to user request
>
> Snapshot date: **2026-05-25**.
> All section references like "(02 §15)" point back to the source reports.

---

## 1. Vision & North Star

After this upgrade, `mempalace_rust` is:

- **Single-binary, zero-Python** — the `mpr` binary statically embeds a native Rust embedder
  (default `fastembed-rs`, fallback `model2vec-rs`, optional `candle`/`tract` features). The
  Python ONNX subprocess and `pip install chromadb` are gone (04 §4, 05 PR 3).
- **Two consumption modes**, picked by the host:
  1. **Library mode** — depend on `mempalace-core` from `Cargo.toml`, get a `Palace` handle,
     call async methods. Used by jcode as the default integration shape (03 §G, 04 P0 #2).
  2. **Sidecar mode** — run `mpr mcp` and talk MCP-stdio. Same 19 tools as today. Used by
     editors and any host that doesn't want to link Rust crates.
- **Trait-based public API** — `MemoryProvider`, `Embedder`, `PalaceStore` are the only types
  consumers must understand. Concrete structs (`PalaceDb`, `OnnxModel`) become implementation
  details (04 P0 #2, 05 §A.5, 03 §A).
- **Tiered, swappable storage** — Tier 1 `hnsw_rs + sqlite` (default, ≤5 k drawers),
  Tier 2 `usearch + sqlite` (5 k–100 k), Tier 3 `lancedb` (100 k+). Auto-recommendations from
  `mpr doctor`, opt-in promotion (05 §B.3, §C.2).
- **Per-palace, not per-user** — `Config` becomes a value passed alongside a `Palace` handle,
  not a process-global singleton. jcode opens one palace per working directory; standalone
  users open the canonical XDG palace. No global mutable state cross-contaminates handles
  (04 P0 #5, 04 P1 #6).
- **Real semantic search in the production code path** — today `searcher::search_memories`
  silently falls back to Jaccard token overlap; the headline 96.6 % R@5 only appears in the
  bench harness. After upgrade, *every* search runs through the embedder + ANN store, with
  BM25 (`bm25.rs` for Tier 1, `tantivy` for Tier 2/3) as a hybrid leg (04 P0 #1, 05 §C.2 PR 6).
- **Honest documentation** — README and AAAK marketing language match the code: AAAK is
  *lossy summarisation*, the +34 % structure boost gets a bench-reproducible methodology, and
  benchmark numbers cite the Rust port not the Python original (02 §14, §15).
- **A clear "moat" feature roadmap**, not just plumbing — bi-temporal KG, Personalised
  PageRank retrieval, sleep-time consolidation, and A-MEM evolution loops are the four
  measurable LongMemEval/LoCoMo levers identified across 2024-2026 research (02 TL;DR,
  §1146-1235).

The end state is a memory backend that jcode can adopt without dragging Python into the
single-binary build, that other Rust agents can also adopt cleanly, and that standalone
users keep using exactly as today.

---

## 2. Architecture Decision Records

Each ADR uses Context / Options / Decision / Consequences. Decisions cite source reports.

### ADR-1 — Native embedder

**Context.** Today `mempalace_rust` shells out to `python3 onnx_embed_python.py --persistent`,
which `import chromadb` and serves embeddings over stdin/stdout. That kills the single-binary
promise, complicates packaging, and is a hard blocker for jcode integration (04 §4, P0 #4).

**Options.**
- A. `fastembed-rs` (uses `ort` C++ ONNX Runtime; 30+ models incl. BGE-M3, multilingual-E5,
  Qwen3-Embedding; auto-download from HF Hub).
- B. `candle-transformers` (pure Rust, slower 2–3×, broader model coverage including LLM
  encoders).
- C. `tract-onnx` + `tokenizers` (pure Rust; what `jcode-embedding` already uses; slower
  than `ort` on transformer ops but no native lib).
- D. `model2vec-rs` (static embeddings, sub-millisecond, lower quality; great for L0/L1
  wake-up paths or tiny machines).

**Decision.** **`fastembed-rs` as default**, behind feature `embed-fastembed`.
Provide `embed-tract` for pure-Rust deployments and `embed-model2vec` for low-power machines.
Provide `jcode-host` feature that re-exports `jcode_embedding::Embedder` so the host process
loads only one model (05 §A.4, §A.5, §A.6).

**Consequences.** Ship `libonnxruntime.so` next to `mpr` (or static-link in CI). Users on
exotic CPUs (riscv, alpha) fall back to `embed-tract`. Existing `MEMPALACE_EMBED_MODEL` env
var keeps semantic — value maps to `fastembed::EmbeddingModel` enum.

### ADR-2 — Vector store

**Context.** Today `embedvec 0.5` is in-tree but the vector path is *unused in production*.
`PalaceDb::query` runs Jaccard token overlap on a `HashMap<String, Drawer>` deserialised
from a single JSON file; every `flush()` rewrites the entire file (04 P0 #1, P0 #3).

**Options.**
- A. Keep `embedvec`, properly wire it into `searcher.rs`.
- B. `hnsw_rs` (pure Rust HNSW) + `rusqlite` for payload — Tier 1.
- C. `usearch` (mmap'd HNSW from Unum, fast SIMD, optional PQ/SQ8 quantisation) — Tier 2.
- D. `arroy` / `hannoy` (LMDB-backed, multi-process safe, Annoy-style) — Tier 2 alternate.
- E. `lancedb` (Apache Arrow columnar, async, SQL-ish payload) — Tier 3.
- F. `qdrant` embedded — heavy, gRPC even in-process.
- G. `surrealdb` embedded — 50+ transitive deps, BSL ambiguity.

**Decision.** **Trait-based with three concrete tiers (B, C, E)**:
- Tier 1 default: **`hnsw_rs` + `rusqlite`** — covers ≤5 k drawers, `<20 MB` RAM, sub-ms
  search.
- Tier 2: **`usearch` + `rusqlite`** — 5 k to 100 k drawers, mmap'd index, true incremental
  insert.
- Tier 3: **`lancedb`** — 100 k+ drawers, async-native, SQL-ish filtering, time-travel
  storage.

Per-`(wing, room)` sub-indexes plus a global cross-wing index eliminate the need for
payload filtering at the ANN layer (01 §1.3). `embedvec` stays available behind
`--store legacy-embedvec` for migration only (05 §B.3, §C).

**Consequences.** Tiers are invisible behind `trait PalaceStore`. `mpr doctor` recommends
promotion. Auto-migration is per-drawer, resumable. `surrealdb`/`qdrant`/`tract` rejected
on dependency-graph weight (01 §1.2 "skip surrealdb / lancedb caveat" — but lancedb at
Tier 3 is a deliberate, opt-in cost).

### ADR-3 — Public library API

**Context.** Today `mempalace-core` re-exports every internal module as `pub`. There is
no curated entry point, no traits, and no SemVer story (04 §2, P0 #2, P2 #14, #17).

**Options.**
- A. Keep modules `pub`; document a "stable subset".
- B. Introduce `pub trait MemoryProvider`, `pub struct Palace`, hide internals.
- C. Write a separate `mempalace` umbrella crate that re-exports a curated API.

**Decision.** **B**, with a public `Palace` struct and three public traits (`MemoryProvider`,
`Embedder`, `PalaceStore`). Internal modules become `pub(crate)` over two minor releases
(deprecate first, then hide). Add `#[non_exhaustive]` on every public enum and config
struct.

**Consequences.** Existing CLI/MCP code stays as-is — they consume the same public surface.
External consumers depend on `mempalace::{Palace, MemoryProvider, Embedder, PalaceStore,
SearchHit, DrawerId, MemoryScope, …}` and nothing else. SemVer becomes meaningful.

### ADR-4 — Integration mode with jcode

**Context.** jcode is a single binary; mempalace today is a separate binary with a Python
subprocess. The two existing extension surfaces in jcode are: the MCP client (orthogonal,
exposes tools to the LLM) and the Tool registry (tool-level swap, doesn't satisfy
auto-recall) (03 §E).

**Options.**
- A. **Library link only** — jcode `Cargo.toml` depends on `mempalace-core`.
- B. **MCP sidecar only** — jcode spawns `mpr mcp` and JSON-RPCs into it.
- C. **Library link + MCP fallback** — default to library; fall back to spawning `mpr mcp`
  if the host opts out.

**Decision.** **C, library link as default.** MCP sidecar is a documented fallback for
deployments that cannot ship the embedder (e.g., very minimal Linux distros) or prefer
process isolation.

**Consequences.** `mempalace-core` must be `Cargo`-publishable, semver-stable, and have a
small dep footprint with `default-features = []` for embedded use. CLI/onboarding/MCP code
moves to feature `cli`/`mcp` (off by default for library consumers) (04 P2 #17, 05 §A.6).
jcode's existing memory-agent runtime (`memory_agent.rs`) stays in jcode and orchestrates
the provider — provider is a *data plane*, not an *agent* (03 §E, F).

### ADR-5 — Knowledge-graph backend

**Context.** Today `knowledge_graph.rs` uses `rusqlite` with WAL, single-timestamp temporal
columns (`valid_from`, `ended`). `Connection` is `!Sync`, requiring caller-side locking
(04 §5, P1 #7).

**Options.**
- A. Keep SQLite + add bi-temporal columns (`t_created`, `t_expired`, `t_valid_from`,
  `t_valid_to`).
- B. Migrate to `redb` (pure Rust, ACID).
- C. Migrate to `fjall` (RocksDB-class LSM in Rust).
- D. Migrate to `HelixDB` (native graph + vector).

**Decision.** **A — keep SQLite, add bi-temporal columns** in a one-shot
migration. Wrap `Connection` in `Arc<Mutex<…>>` at the `KnowledgeGraph` layer so callers
don't need to know about `!Sync`. Defer redb/fjall to a future "v2" if SQLite ever shows up
on a flame graph (02 §1146 "Pain point: contradictions and freshness", 04 P1 #7).

**Consequences.** No dep churn, no migration drama, +4 columns. Bi-temporal validity is the
single highest-correctness-impact change available (02 §1187). HelixDB watched but not
adopted; it is too young for a single-binary CLI.

### ADR-6 — Async story

**Context.** Today `searcher::search_memories` is `async` but the body has no `.await`.
`KnowledgeGraph` is fully sync. `tokio::Runtime::new()` is built only inside MCP (04 §3, P1 #10).

**Options.**
- A. Make every public method true-async (uses `tokio::fs`, async SQLite).
- B. Keep methods sync; consumers wrap with `spawn_blocking` if needed.
- C. Hybrid: methods that touch I/O are async; methods that are pure compute stay sync.

**Decision.** **C, hybrid.** `Palace::search`, `Palace::add_drawer`, `Palace::flush`,
`Palace::extract_from_transcript` are `async fn`. `Palace::compute_aaak` and similar
pure-compute helpers stay sync. We do NOT take a hard `tokio` dependency — implement on
top of `async-trait` + plain futures so non-tokio runtimes (e.g., `smol`) work too. Document
that I/O-heavy operations should run on a `spawn_blocking`-style executor when called from a
non-tokio runtime.

**Consequences.** Matches jcode's tokio runtime. Avoids gratuitously async sync paths.
`async-trait` is unavoidable until Rust 1.85+ stable AFIT for `dyn Trait` lands; we already
require 1.85 in Cargo.toml so we may revisit.

### ADR-7 — Per-project palace lifecycle

**Context.** Today `Config::load()` reads from `$XDG_CONFIG_HOME/mempalace/config.json` —
a process-global. Several modules (entity_registry, identity, embedding_model selection)
bind to global config rather than to a palace handle. jcode is per working directory
(04 §9, P0 #5).

**Options.**
- A. Keep global config; add `--palace` everywhere.
- B. Make config palace-scoped (`<palace>/config.json`); accept env-var/global as defaults.
- C. Remove all process-global config reads from library code; have `Palace::open` take an
  explicit `PalaceConfig`.

**Decision.** **C.** `mempalace-core` accepts `PalaceConfig` by value at `Palace::open`.
The CLI binary loads global XDG config and passes it. The library never reads from disk on
its own. `_GRAPH_CACHE` and similar statics become fields on the `Palace` handle (04 P1 #6).

**Consequences.** jcode opens one `Palace` per working directory, each with its own
config, cache, and graph. They never collide. Shutdown signals become per-palace via
explicit `palace.shutdown()` rather than the global `SHUTDOWN_REQUESTED` atomic.
The CLI gets slightly more boilerplate but the gain in re-entrancy and testability is
worth it.

### ADR-8 — Embedding model identity & multi-tenant isolation

**Context.** When jcode brings its own embedder via the `jcode-host` feature, both jcode and
mempalace must produce *commensurable* vectors (same model, same dim, same tokenisation).
Today `OnnxModel::dimension()` returns hardcoded 384 with no validation (05 §A.5, 03 §D).

**Options.**
- A. Trust the user; if dims mismatch, search returns garbage.
- B. Validate `dim()` at `Palace::open` against a manifest stored in the palace.
- C. Store a per-drawer "embedding model fingerprint" and refuse to mix.

**Decision.** **B + lightweight C.** Each palace persists `embedding.json` containing
`{model_name: String, dim: usize, fingerprint: String}` on first write. Subsequent opens
validate; mismatches fail loud with an actionable message ("re-embed with `mpr migrate
--re-embed`"). Per-drawer fingerprints are stored only when a future migration is in
flight (so we can mix old + new during a re-embed batch).

**Consequences.** No silent corruption when users swap embedders. jcode can safely co-exist
with the standalone CLI on the same palace as long as both use the same model (the default
configuration). Re-embedding has a defined workflow.

### ADR-9 — AAAK compression

**Context.** README claims "30x lossless compression". Code in
`crates/core/src/dialect.rs` says `"AAAK is lossy. ... the original text cannot be
reconstructed from AAAK output"` (02 §15, §1031–1146). The +34 % retrieval boost claim is
also unsubstantiated in the Rust port (no bench fixture).

**Options.**
- A. Drop AAAK entirely.
- B. Keep AAAK, fix the marketing.
- C. Replace AAAK with `LLMLingua-2` or similar published compressor.

**Decision.** **B + bench it.** Update README to "lossy summarisation, ~5–10× token
reduction, optimised for LLM readability." Add a `crates/bench` test that measures the
+34 % structure boost on a public fixture (LongMemEval-S). Keep AAAK behind feature flag
`aaak`, default on. Plan `LLMLingua-2` as a future alternative compressor backend
(02 §15 verdict, §1242 Tier 1 #1).

**Consequences.** Honest claims. No code rip-out. Future flexibility to swap compressor
without breaking consumers.

### ADR-10 — Type sharing with `jcode-memory-types`

**Context.** jcode has its own `MemoryEntry`, `MemoryGraph`, `MemoryScope`, `TrustLevel`,
`Reinforcement`. mempalace has `SearchResult`, `Triple`, `DrawerMeta`, `Wing/Room` slugs.
The shapes are similar but not identical (03 §1, §D, 04 §11).

**Options.**
- A. **Duplicate** — mempalace defines its own; jcode adapts via `From` impls.
- B. **Depend on** — mempalace declares `jcode-memory-types` as a dep.
- C. **Neutral types** — extract a third crate `agent-memory-types` shared by both.

**Decision.** **A duplicate, with adapters in `mempalace-jcode-adapter`.** Keeps
`mempalace-core` independent of jcode's release cycle. Adapter crate (separate repo or
in jcode workspace) implements `From<mempalace::SearchHit> for jcode::MemoryEntry` and
the `MemoryProvider` trait against `Palace`. Roughly 200–400 LOC.

**Consequences.** Slight duplication of type definitions; each project owns its own
contract. No coupling on release timing. If a third Rust agent project adopts mempalace,
they write their own adapter (≤200 LOC). Future option-C "neutral types" stays open if
three or more such adapters appear.

### ADR-11 — Auto-capture lifecycle hooks

**Context.** `rohitg00/agentmemory`'s defining UX is hook-driven auto-capture across the
full agent lifecycle: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `PreCompact`, `SubagentStart/Stop`, `Stop`, `SessionEnd` — 12 hooks
on Claude Code, 6 on Codex CLI, 22 on OpenCode (06 §3.1). mempalace today only ships
`Stop` and `PreCompact` hook scripts. jcode has its own per-turn pipeline in
`memory_agent.rs` but no `PostToolUse`/`UserPromptSubmit` hooks pointing at mempalace.

**Options.**
- A. Mempalace ships standalone hook scripts only (current state).
- B. Mempalace exposes an `EventCapture` trait; the host calls it on every relevant event;
  the standalone CLI ships hook scripts that invoke `mpr observe ...`.
- C. **B** plus shipped hook scripts for Claude Code / Codex / OpenCode in the standalone
  install (matching agentmemory's surface).

**Decision.** **C.** New `pub trait EventCapture` in `mempalace-core`. jcode's adapter
implements it via `memory_agent.rs`. Standalone install ships full hook coverage.

**Consequences.** Adopting mempalace must NOT regress jcode's existing auto-capture; this
is now a P0 deliverable in Phase 4 (06 §6). Three new beads issues added.

### ADR-12 — Privacy filter at ingest

**Context.** mempalace stores raw verbatim ("never lose anything"). jcode's tool-result
blocks routinely contain `OPENAI_API_KEY=sk-...`, OAuth tokens, JWTs after the model
inspects shell output. Storing those is a **P0 security blocker for jcode**. agentmemory
strips secrets at ingest before storage (06 §3.4).

**Options.**
- A. Don't filter; document the risk.
- B. Strip well-known secret patterns at `add_drawer` time; configurable allow-list.
- C. **B** plus reversible per-palace encryption (heavy, defer).

**Decision.** **B.** New `crates/core/src/privacy.rs` with built-in patterns
(`sk-*`, `gh[opsr]_*`, `xox[bpars]-*`, AWS key formats, JWT structure, BEGIN PRIVATE KEY,
high-entropy base64 in `=`-boundary contexts). Replace stripped tokens with
`<REDACTED:type>` placeholders so AAAK compression is still meaningful. Per-palace allow-list
override via `<palace>/config.json`.

**Consequences.** Hard prerequisite for any jcode integration — must land in Phase 1.
Requires careful test suite (false positives over-redact; false negatives leak secrets).
Two new beads issues.

### ADR-13 — Memory tier (Working / Episodic / Semantic / Procedural)

**Context.** agentmemory's 4-tier consolidation model (06 §3.2) is more granular than
mempalace's halls and aligns with the prior research surveyed in report 02 (MemoryBank §5,
Generative Agents §6). Halls and tiers are complementary axes:
- Halls = the *taxonomy* (facts vs events vs advice).
- Tiers = the *lifecycle* (raw observation vs consolidated semantic fact).

**Options.**
- A. Don't add tiers; halls cover it.
- B. Add `tier: MemoryTier` field to `Drawer`; promotion/decay handled in sleep-time
  consolidation (Phase 5).
- C. Replace halls with tiers (breaking change).

**Decision.** **B.** Tiers and halls coexist. Default tier on ingest is `Working` (raw
observations) or `Episodic` (mined session summaries); `Semantic` and `Procedural` are
populated by sleep-time consolidation.

**Consequences.** Two new beads issues (one schema, one promotion logic).

### ADR-14 — Session diversification in retrieval

**Context.** agentmemory caps results at 3 per session post-RRF so a single productive
session doesn't drown out other context (06 §3.3). mempalace today has wing/room as
filters but no per-session cap.

**Options.**
- A. Skip; deliver as today.
- B. Add `max_per_session: usize` to `SearchScope` (default 3), enforced after fusion.

**Decision.** **B.** One-line addition to `SearchScope`, post-RRF filter step.

**Consequences.** One small beads issue in Phase 5.

---

## 3. Concrete API Sketch

The proposed public surface, with all the non-exhaustive markers and async qualifiers
that drop out of ADRs 3, 6, 7. Internal types omitted; only what consumers (jcode, third
parties) need to reason about.

```rust
// crates/core/src/lib.rs (illustrative)

pub use config::PalaceConfig;
pub use embed::{Embedder, NullEmbedder};
pub use store::{PalaceStore, StoreTier};
pub use palace::{Palace, PalaceBuilder};
pub use provider::MemoryProvider;
pub use types::{
    DrawerId, Drawer, DrawerKind, SearchHit, SearchScope, MemoryScope,
    Wing, Room, Hall, Tunnel, GraphStats,
};
pub use kg::{KnowledgeGraph, Triple, BiTemporalRange};
pub use error::{Error, Result};

// ---- core trait surface -------------------------------------------------

/// Pluggable embedding backend. Hosts can inject their own.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync + 'static {
    fn dim(&self) -> usize;
    fn fingerprint(&self) -> &str;            // model-id + tokenizer-hash, see ADR-8
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Pluggable vector store. Three concrete tiers ship in-tree.
#[async_trait::async_trait]
pub trait PalaceStore: Send + Sync + 'static {
    async fn upsert(&self, drawers: Vec<Drawer>) -> Result<()>;
    async fn delete(&self, ids: &[DrawerId]) -> Result<usize>;
    async fn search(
        &self,
        query: &[f32],
        scope: &SearchScope,
        limit: usize,
    ) -> Result<Vec<SearchHit>>;
    async fn count(&self, scope: &SearchScope) -> Result<usize>;
    async fn flush(&self) -> Result<()>;
    fn tier(&self) -> StoreTier;              // for `mpr doctor`
}

/// High-level memory facade. What hosts (jcode, third-party agents) consume.
#[async_trait::async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    async fn add_drawer(&self, drawer: Drawer) -> Result<DrawerId>;
    async fn remember(&self, content: String, scope: MemoryScope) -> Result<DrawerId>;
    async fn forget(&self, id: &DrawerId) -> Result<bool>;
    async fn search(&self, query: &str, scope: &SearchScope) -> Result<Vec<SearchHit>>;
    async fn search_with_embedding(
        &self,
        query_vec: &[f32],
        scope: &SearchScope,
    ) -> Result<Vec<SearchHit>>;
    async fn related(&self, id: &DrawerId, depth: usize) -> Result<Vec<SearchHit>>;
    async fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> Result<Vec<DrawerId>>;
    async fn graph_stats(&self) -> Result<GraphStats>;

    // Provider-level introspection
    fn fingerprint(&self) -> &str;
    fn embedder(&self) -> &dyn Embedder;
    fn store(&self) -> &dyn PalaceStore;
}

// ---- the canonical implementation --------------------------------------

/// The default `MemoryProvider` impl bundled with mempalace-core.
pub struct Palace { /* private */ }

impl Palace {
    pub fn builder() -> PalaceBuilder { PalaceBuilder::new() }
}

#[async_trait::async_trait]
impl MemoryProvider for Palace { /* … */ }

pub struct PalaceBuilder { /* private */ }

impl PalaceBuilder {
    pub fn config(self, cfg: PalaceConfig) -> Self;
    pub fn embedder(self, e: Arc<dyn Embedder>) -> Self;       // ADR-8 BYO embedder
    pub fn store(self, s: Arc<dyn PalaceStore>) -> Self;
    pub fn knowledge_graph(self, kg: KnowledgeGraph) -> Self;
    pub async fn open(self) -> Result<Palace>;                 // ADR-7 explicit open
}

// ---- search scope (replaces wing/room kwargs everywhere) ---------------

#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    pub wing: Option<String>,
    pub room: Option<String>,
    pub hall: Option<String>,
    pub limit: usize,                  // 0 = use Palace default
    pub include_global: bool,          // ADR-7: per-project + fallthrough to global
    pub time_window: Option<BiTemporalRange>,   // ADR-5
    pub fusion: FusionMode,            // semantic | bm25 | hybrid (default)
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
pub enum FusionMode {
    #[default] Hybrid,                 // 70% vec + 30% BM25 + entity bonus
    Semantic,
    Bm25,
}

// ---- knowledge graph (bi-temporal) -------------------------------------

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: Option<f32>,
    pub provenance: Option<DrawerId>,
    pub bi_temporal: BiTemporalRange,  // ADR-5
}

#[derive(Debug, Clone, Copy)]
pub struct BiTemporalRange {
    pub t_created:    DateTime<Utc>,   // when we learned it
    pub t_expired:    Option<DateTime<Utc>>,
    pub t_valid_from: Option<DateTime<Utc>>, // when it was true in the world
    pub t_valid_to:   Option<DateTime<Utc>>,
}
```

### How jcode wires it in

```rust
// jcode side, illustrative

use mempalace::{Palace, MemoryProvider, Embedder};

// jcode brings its own embedder so only one ONNX session is loaded per process.
struct JcodeEmbedderAdapter(Arc<jcode_embedding::Embedder>);

#[async_trait::async_trait]
impl Embedder for JcodeEmbedderAdapter {
    fn dim(&self) -> usize { jcode_embedding::embedding_dim() }
    fn fingerprint(&self) -> &str { "jcode-MiniLM-L6-v2-tract" }
    async fn embed(&self, text: &str) -> mempalace::Result<Vec<f32>> {
        Ok(self.0.embed(text)?)
    }
    async fn embed_batch(&self, texts: &[&str]) -> mempalace::Result<Vec<Vec<f32>>> {
        Ok(self.0.embed_batch(texts)?)
    }
}

// Per-project palace
let palace = Palace::builder()
    .config(PalaceConfig {
        palace_path: project_dir.join(".jcode/palace"),
        ..Default::default()
    })
    .embedder(Arc::new(JcodeEmbedderAdapter(jcode_embedder.clone())))
    .open()
    .await?;

// Adapter to jcode's MemoryProvider trait (lives in jcode-mempalace-adapter crate, ADR-10):
let provider: Arc<dyn jcode_memory_types::MemoryProvider> =
    Arc::new(MempalaceJcodeAdapter::new(palace));
```

That's the full integration shape. ~30 LOC of glue.

---

## 4. Phased Roadmap

Six phases, each independently shippable, each green on CI for ubuntu/macos/windows.
Sized **S** (≤1 week solo agent), **M** (≤3 weeks), **L** (>3 weeks).

### Phase 0 — Stabilise & Baseline (size: M)

**Goal.** Lock the current state: pin SemVer, add benches, fix the AAAK marketing claim.

**Scope.**
- Tag `0.1.0` as the "pre-upgrade" release on the existing branch.
- Add `criterion` benches for `searcher::search_memories`, `palace_db::add`, mining
  throughput on a 5 k-drawer fixture (04 P2 #18).
- Add a LongMemEval-S reproducer to `crates/bench` (02 §1306) and capture the *current*
  Rust port's score for the README.
- Update README/AAAK marketing per ADR-9.
- Add `#[non_exhaustive]` everywhere; mark internal modules `#[doc(hidden)]` (04 P2 #14, #19).

**Exit criteria.** Tagged release. Benches in CI. README claims match code.

### Phase 1 — Native Embedder (size: M)

**Goal.** Delete Python from the runtime requirements (ADR-1, 05 §C.2 PR 1–4).

**Scope.**
- Introduce `pub trait Embedder` (ADR-1, ADR-8).
- Add `FastEmbedEmbedder` behind `embed-fastembed` feature, default on.
- Add `Model2VecEmbedder` behind `embed-model2vec` feature.
- Add `TractEmbedder` behind `embed-tract` feature.
- Wire `MEMPALACE_EMBED_MODEL` to the `fastembed::EmbeddingModel` enum.
- Delete `onnx_embed.rs`, `onnx_embed_python.py`, `__pycache__`, the `pip install` step in
  `install.sh`/`install.ps1`.
- Add `embedding.json` palace manifest (model name, dim, fingerprint) for ADR-8 validation.

**Exit criteria.** `pip install` removed from install scripts. `mpr` runs zero subprocesses.
`mpr doctor` reports active embedder + dim. Bench numbers stable (or improved) vs Phase 0.

### Phase 2 — Trait-Based API + `Palace` Facade (size: M)

**Goal.** Introduce the public library API (ADR-3, ADR-6, ADR-7).

**Scope.**
- Define `MemoryProvider`, `PalaceStore`, `Palace`, `PalaceBuilder` per §3 above.
- `PalaceDb` becomes the default `PalaceStore` impl, but renamed and refactored:
  - `JaccardJsonStore` (legacy fallback, behind `--store legacy-jaccard`)
  - `EmbedvecStore` (the existing embedvec wrapper, used as Tier 0 default until Phase 5).
- All call sites (`searcher.rs`, `miner.rs`, `mcp_server.rs`, `layers.rs`, `sweeper.rs`,
  `convo_miner.rs`) move from concrete types to traits via `Palace`.
- `_GRAPH_CACHE` and `_GRAPH_BUILD_VERSION` migrate from statics to fields on `Palace`
  (ADR-7, 04 P1 #6).
- Make `KnowledgeGraph` `Send + Sync` by wrapping `Connection` in `Arc<Mutex<…>>` at the
  facade layer (ADR-5, 04 P1 #7).
- Mark old top-level modules as `#[deprecated]`; keep them re-exported.
- New `PalaceConfig` struct passed to `Palace::open` (ADR-7).

**Exit criteria.** External Rust crate `mempalace-host-sample` (in `crates/sample`) compiles
and uses `Palace::builder().embedder(custom).open()`. CLI/MCP still pass all 435 tests.
Public API surface documented.

### Phase 3 — First jcode Integration (Read-Only) (size: M)

**Goal.** Land mempalace in jcode behind a feature flag, read-only path (03 §F Phase 0–2).

**Scope, jcode side:**
- Add `pub trait MemoryProvider` to `crates/jcode-memory-types` (03 §A).
- Add `jcode-mempalace-adapter` crate (or module) — the ADR-10 adapter.
- Add `MemoryBackend::{Local, Mempalace}` enum to `AgentsConfig` (03 §D).
- Switch all 12 `MemoryManager::new()` call sites in jcode to `provider()` accessor (03 §C).
- With `Mempalace` selected, the adapter mirrors *reads* against a per-project palace at
  `<workdir>/.jcode/palace`. Inserts are no-op (Local writes still authoritative).
- TUI `MemoryActivity` rendering unchanged (03 §G).

**Scope, mempalace side:**
- Document the per-project palace layout (config in `<palace>/config.json`, embedding manifest
  in `<palace>/embedding.json`).
- `Palace::open(PalaceConfig)` honours `palace_path` from explicit config; never reads global
  XDG when called as a library.

**Exit criteria.** With `agents.memory_backend = "mempalace"`, jcode opens a per-project
palace, ingestion still goes to local JSON, but `find_similar_*` queries return results from
mempalace. Behind feature flag `mempalace-backend` so it can ship dark.

### Phase 4 — Bidirectional Integration (Read+Write) (size: M)

**Goal.** Make mempalace the authoritative memory store for jcode (03 §F Phase 3).

**Scope.**
- Adapter writes to mempalace on `remember_*` / `upsert_*`.
- jcode's `extract_from_transcript` calls `mempalace.add_drawer` per extracted memory
  + `KnowledgeGraph::add_triple` per fact.
- Episode feedback signal (jcode's "verified vs rejected" maintenance) maps to mempalace's
  episodic helpfulness scores.
- Mining lock and PID guard scoped per-palace, not per-process (04 P1 #12, ADR-7).
- Write-WAL path under `<palace>/wal/...` (04 P1 #13).
- One-shot migration from jcode's existing JSON memory store to mempalace
  (`jcode memory migrate --backend mempalace`).

**Exit criteria.** Memory feature parity with jcode's local backend on all 30+ memory tests.
Round-trip latency (add + search) within the documented MEMORY_BUDGET (jcode docs).

### Phase 5 — Advanced Retrieval (size: L)

**Goal.** Land the four moats identified across the 2024–2026 research (02 TL;DR).

**Scope.**
- **Bi-temporal validity** in `KnowledgeGraph` (ADR-5, 02 §1187, 01 §6.2).
- **Personalised PageRank** retrieval over wing/room/tunnel/entity graph (02 §1146 "Pain
  point: Recall", §7 HippoRAG2). Optional fusion mode `FusionMode::Ppr`.
- **Sleep-time consolidation** worker — `mpr daemon` (or library-side
  `Palace::run_consolidation`) periodically refines closets, generates new tunnels,
  consolidates per-hall reflections (02 §1242 Tier 2 #8, 01 §6.2).
- **A-MEM evolution loop** — when a new drawer enters a room with siblings, re-evaluate
  sibling closets (01 TL;DR #4, 02 §2).
- **Tier 2/3 stores** — `UsearchSqliteStore`, `LancedbStore` behind `--store usearch` /
  `--store lancedb`. Auto-promotion advisor in `mpr doctor` (05 §C.2 PR 7, ADR-2).
- **Tantivy BM25** for Tier 2/3 hybrid search (01 §6.1, 05 §B.4).
- **Per-(wing, room) sub-indexes** so payload filtering is unnecessary at the ANN layer
  (01 §1.3, 05 §B.3).

**Exit criteria.** Reproduced LongMemEval-S baseline + measurable lift from each moat
(documented in `docs/research/06_phase5_benchmark_results.md`). PPR is the single biggest
expected jump (target ≥ +5 R@5 vs Phase 4) (02 §7).

### Phase 6 — Polish & Productisation (size: M)

**Goal.** Single-binary release, MCP parity, docs.

**Scope.**
- CI release pipeline emits statically-linked `mpr` binaries for 5 targets (existing matrix).
- `cargo install --no-default-features --features lib-min` produces a binary-free library
  for embedding (ADR-3 dep-trim).
- MCP tools renamed to align with the canonical `@modelcontextprotocol/server-memory`
  surface (`create_entities`, etc.) for free interop (01 §6.1).
- `mpr export --format basic-memory` (Markdown / Obsidian) (01 §6.1, §4.4).
- Extensive docs: `docs/integration_jcode.md`, `docs/integration_third_party.md`,
  `docs/migration_v0_to_v1.md`.
- Cut **`mempalace-core 1.0`** with stable SemVer.

**Exit criteria.** `cargo doc` clean. Integration docs reviewed. 1.0 published to crates.io.
README, jcode docs, and the standalone install path all describe the same architecture.

---

## 5. Risks & Mitigations

| # | Risk | Likelihood | Impact | Mitigation |
|---|------|-----------|--------|------------|
| R1 | `ort`/fastembed shared lib breaks single-binary promise on niche platforms | M | M | Ship `embed-tract` and `embed-model2vec` features (05 §A.4); CI builds all three |
| R2 | Auto-migration from JSON to vector store is slow on large legacy palaces | M | M | Per-drawer, resumable, `--migrate-resume` flag; runs in background (05 §C.3) |
| R3 | jcode and mempalace disagree on embedding model → silent garbage retrieval | M | H | `embedding.json` palace manifest + dim/fingerprint validation at `Palace::open` (ADR-8) |
| R4 | `MemoryProvider` trait is wrong shape, requires breaking change post-1.0 | M | H | Phase 2 ships behind a flag; adapter crate iterates against jcode for 1+ release before stabilising; `#[non_exhaustive]` on every public type |
| R5 | Bi-temporal column migration corrupts existing KGs | L | H | Migration is additive (4 new columns, default to existing `valid_from`/`ended`); old rows remain readable; integration test on a real-world palace before shipping |
| R6 | Personalised PageRank too slow on large graphs | M | M | Cap depth + node budget; pre-compute PPR vectors for hot wings during sleep-time consolidation; HippoRAG2 reports sub-second at 1M nodes (02 §7) |
| R7 | Concurrent jcode + standalone CLI on same palace causes WAL conflicts | M | M | LMDB-style locking via `arroy` or single-writer rule documented; flock on palace dir; `mpr doctor` reports lock status |
| R8 | Phase 5 scope creep (PPR + sleep-time + A-MEM + tier 2/3 + tantivy) blows the schedule | H | M | Treat each as an independent feature flag; ship Phase 4 with Tier 0 (current `embedvec`) and let users opt in to Phase 5 features one at a time |

---

## 6. Open questions for the user/maintainer

1. **Ownership/repo layout.** Does `mempalace_rust` stay in its own repo and become a
   crates.io publish, or get vendored into `jcode/crates/`? **Recommendation: own repo,
   crates.io publish, jcode depends on it via Git tag until 1.0.**
2. **Default embedder for the standalone CLI.** Stick with MiniLM-L6 (384-dim, matches
   jcode), or upgrade to BGE-small (better quality, also 384-dim, drop-in)?
   **Recommendation: BGE-small as default; MiniLM-L6 as the `compat-jcode` profile.**
3. **AAAK as default.** Keep AAAK on by default for new palaces, or only enable when
   the user explicitly opts in? **Recommendation: keep on, but documented as lossy.**
4. **MCP tool renaming for interop.** Breaking change for existing standalone users.
   Acceptable now (pre-1.0)? **Recommendation: yes, alias old names for one minor release.**
5. **Hermes integration (`hermes_integration.rs`).** Currently a stub. Delete entirely or
   refactor against `MemoryProvider`? **Recommendation: delete in Phase 2; refactor was a
   distraction, the trait makes any host equivalent.**
6. **Multi-tenant (multiple agents writing to the same palace concurrently).** Out of
   scope for v1.0? **Recommendation: yes, single-writer + multi-reader is the target.**
7. **Should `mpr` ship a daemon mode** (`mpr daemon` for background sleep-time
   consolidation) or always be invoked on demand? **Recommendation: opt-in daemon, default
   is on-demand `mpr consolidate`.**
8. **Integration with jcode's sidecar (Codex Spark).** mempalace's `closet_llm.rs` and
   `general_extractor.rs` already call out to LLMs. Should the jcode adapter route those
   through jcode's existing sidecar (saving an HTTP round-trip / API key)? **Recommendation:
   yes — add an `LlmClient` trait alongside `Embedder`, same pattern.**
9. **Telemetry.** mempalace currently has no telemetry. jcode does. Should the adapter
   emit jcode telemetry events for memory operations? **Recommendation: optional, behind a
   feature flag in the adapter, off by default.**
10. **Per-project palace size limits.** What's the policy for "this project palace is too
    big, auto-promote to Tier 2 / archive old drawers"? **Recommendation: defer to Phase 5;
    `mpr doctor` advisory only in Phase 3–4.**

---

## 7. Beads Issue Backlog

41 issues, sized **S/M/L**, organised by phase and priority. Titles and one-liners only.
A separate task ("File beads issues for implementation work") will turn these into actual
`br` issues with proper dependency edges.

### Phase 0 — Stabilise & Baseline

- **mp-001 [P0/S]** Tag `0.1.0` and freeze the pre-upgrade branch.
- **mp-002 [P0/S]** Add `criterion` benches for `searcher::search_memories`, `palace_db::add`,
  `miner::mine_dir` on a 5 k-drawer fixture.
- **mp-003 [P0/M]** Add LongMemEval-S reproducer to `crates/bench` and record current Rust
  baseline.
- **mp-004 [P0/S]** Update README + dialect.rs docstrings to align with code: AAAK is lossy
  summarisation, not lossless compression.
- **mp-005 [P1/S]** Add `#[non_exhaustive]` to every public enum + struct.
- **mp-006 [P1/S]** Mark internal modules `#[doc(hidden)]` (palace_graph internals,
  embedding internals, etc.).
- **mp-007 [P2/S]** Document the current public API surface in `docs/PUBLIC_API.md` so
  Phase 2 can show the diff.

### Phase 1 — Native Embedder

- **mp-010 [P0/M]** Define `pub trait Embedder` in `crates/core/src/embed/mod.rs`.
- **mp-011 [P0/S]** Add `NullEmbedder` implementation for tests.
- **mp-012 [P0/M]** Implement `FastEmbedEmbedder` behind feature `embed-fastembed`.
- **mp-013 [P1/M]** Implement `Model2VecEmbedder` behind feature `embed-model2vec`.
- **mp-014 [P1/M]** Implement `TractEmbedder` behind feature `embed-tract` for pure-Rust
  builds.
- **mp-015 [P0/S]** Add palace `embedding.json` manifest (model, dim, fingerprint).
- **mp-016 [P0/M]** Validate manifest at `Palace::open`; fail with actionable error on
  mismatch.
- **mp-017 [P0/M]** Delete `onnx_embed.rs`, `onnx_embed_python.py`, `__pycache__`; remove
  `pip install` from `install.sh` / `install.ps1`.
- **mp-018 [P0/S]** Wire `MEMPALACE_EMBED_MODEL` env var to the new embedder enum.
- **mp-019 [P1/S]** `mpr doctor` reports active embedder, dim, fingerprint.
- **mp-031 [P0/M]** Privacy filter at ingest — strip API keys / JWT / RSA / AWS keys / OAuth
  tokens before `add_drawer` (ADR-12, 06 §3.4).
- **mp-032 [P1/S]** SHA-256 5-min rolling-window dedup in `Palace::add_drawer` (06 §3.5).

### Phase 2 — Trait-Based API + `Palace` Facade

- **mp-020 [P0/L]** Define `MemoryProvider`, `PalaceStore` traits and `Palace` /
  `PalaceBuilder` per §3.
- **mp-021 [P0/M]** Refactor `PalaceDb` into `JaccardJsonStore` + `EmbedvecStore` both
  implementing `PalaceStore`.
- **mp-022 [P0/M]** Migrate `_GRAPH_CACHE`, `_GRAPH_BUILD_VERSION`, `SHUTDOWN_REQUESTED`
  from statics to per-`Palace` fields.
- **mp-023 [P0/M]** Wrap `KnowledgeGraph` connection in `Arc<Mutex<…>>`; expose
  `Send + Sync` API.
- **mp-024 [P0/M]** Move all internal call sites (`searcher.rs`, `miner.rs`, `layers.rs`,
  `sweeper.rs`, `convo_miner.rs`, `mcp_server.rs`) onto the `Palace` facade.
- **mp-025 [P1/M]** Mark old `pub mod` re-exports as `#[deprecated]`; preserve for one
  minor release.
- **mp-026 [P1/S]** Add `crates/sample/` demo crate that consumes `Palace::builder()`
  externally — proves the public API.
- **mp-027 [P1/S]** Cargo features `cli` / `mcp` make CLI/MCP code optional so library
  consumers shed `clap` / `rmcp` deps (04 P2 #17).
- **mp-028 [P1/S]** Add `PalaceConfig` (replaces global config reads from library code).
- **mp-051 [P1/S]** Add `tier: MemoryTier` field (`Working|Episodic|Semantic|Procedural`)
  to `Drawer` (ADR-13, 06 §3.2).
- **mp-052 [P1/S]** Add `derived_from: Vec<DrawerId>` to `Drawer` for citation provenance
  (06 §3.7); populated during AAAK compression and general extraction.

### Phase 3 — First jcode Integration (Read-Only)

- **mp-040 [P0/M]** (jcode) Define `pub trait MemoryProvider` in `crates/jcode-memory-types`
  matching 03 §A signature.
- **mp-041 [P0/M]** (jcode) Convert `MemoryManager` to impl `MemoryProvider`; introduce
  `JcodeLocalProvider` rename.
- **mp-042 [P0/M]** (jcode) Add `MemoryBackend::{Local, Mempalace}` to `AgentsConfig`;
  add `provider()` factory.
- **mp-043 [P0/M]** (jcode) Switch all 12 `MemoryManager::new()` call sites to
  `provider()` accessor.
- **mp-044 [P0/M]** New crate `jcode-mempalace-adapter`: `MempalaceProvider` impl that
  reads from a `Palace` and maps `SearchHit` ↔ `MemoryEntry`.
- **mp-045 [P1/M]** Adapter writes to `Local` backend, reads through `Mempalace`
  (read-only mode).
- **mp-046 [P1/S]** `cargo feature mempalace-backend` in jcode; off by default.

### Phase 4 — Bidirectional Integration

- **mp-060 [P0/M]** Adapter `add_drawer` / `remember_*` / `upsert_*` write through to
  mempalace.
- **mp-061 [P0/M]** Adapter `extract_from_transcript` routes to `Palace::add_drawer` +
  `KnowledgeGraph::add_triple`.
- **mp-062 [P1/M]** Map jcode episode-feedback signal to mempalace's
  `helpfulness_score`.
- **mp-063 [P0/M]** Mining lock + PID guard scoped per-palace (04 P1 #12).
- **mp-064 [P0/M]** WAL writes routed under `<palace>/wal/...` (04 P1 #13).
- **mp-065 [P0/L]** One-shot migration tool `jcode memory migrate --backend mempalace`.
- **mp-066 [P0/M]** Run all 30+ jcode memory tests against `Mempalace` backend in CI.
- **mp-067 [P1/M]** MEMORY_BUDGET regression suite includes mempalace path.
- **mp-068 [P0/M]** Define `pub trait EventCapture` in `mempalace-core` (ADR-11, 06 §3.1)
  and implement it for `Palace`.
- **mp-069 [P0/M]** jcode adapter wires `EventCapture` to the existing `memory_agent.rs`
  runtime: `PostToolUse`, `UserPromptSubmit`, `PreCompact` events flow to mempalace.

### Phase 5 — Advanced Retrieval

- **mp-080 [P0/M]** Bi-temporal columns (`t_created`, `t_expired`, `t_valid_from`,
  `t_valid_to`) added to KG with backward-compat migration.
- **mp-081 [P1/L]** Personalised PageRank retrieval mode in `palace_graph.rs` + new
  `FusionMode::Ppr`.
- **mp-082 [P1/M]** Synonymy edges (cosine > 0.85) created at ingestion (HippoRAG2).
- **mp-083 [P1/L]** Sleep-time consolidation worker; `mpr daemon` and library-side
  `Palace::run_consolidation`.
- **mp-084 [P2/L]** A-MEM evolution loop (re-evaluate sibling closets when new drawers
  arrive).
- **mp-085 [P1/M]** `UsearchSqliteStore` Tier-2 implementation.
- **mp-086 [P1/M]** `LancedbStore` Tier-3 implementation.
- **mp-087 [P1/M]** `mpr doctor` advises Tier promotion based on drawer count.
- **mp-088 [P1/M]** Tantivy-backed BM25 for Tier 2/3 hybrid search.
- **mp-089 [P1/M]** Per-(wing, room) sub-indexes for filterable ANN.
- **mp-090 [P1/M]** Reproduce LongMemEval-S in CI; capture lift per moat in
  `docs/research/06_phase5_benchmarks.md`.
- **mp-091 [P1/M]** Tier-promotion logic in sleep-time consolidation worker
  (Working → Episodic → Semantic → Procedural) using Ebbinghaus decay + reinforcement
  (ADR-13, 06 §3.2).
- **mp-092 [P1/S]** `SearchScope.max_per_session` post-RRF filter, default 3 (ADR-14,
  06 §3.3).
- **mp-093 [P1/M]** Reproduce **agentmemory's 95.2 % R@5 LongMemEval-S** on identical
  fixture in `crates/bench`; gate Phase 5 release on matching it (06 §1).

### Phase 6 — Polish & Productisation

- **mp-100 [P1/M]** CI release pipeline produces statically-linked binaries for 5 targets
  (matches today + ensures `embed-fastembed` static link works).
- **mp-101 [P1/S]** MCP tool renaming alignment with `@modelcontextprotocol/server-memory`;
  alias old names for one minor release.
- **mp-102 [P1/M]** `mpr export --format basic-memory` (Markdown/Obsidian).
- **mp-103 [P1/M]** Write `docs/integration_jcode.md`, `docs/integration_third_party.md`,
  `docs/migration_v0_to_v1.md`.
- **mp-104 [P0/M]** Cut `mempalace-core 1.0` to crates.io; jcode pins to it.
- **mp-105 [P1/M]** Standalone CLI ships full hook-script set
  (`SessionStart`/`UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`PostToolUseFailure`/
  `PreCompact`/`Stop`/`SessionEnd`) for Claude Code, Codex, OpenCode — matches
  agentmemory's auto-capture surface (ADR-11, 06 §3.1).
- **mp-106 [P2/S]** Privacy-filter UX — configurable allow-list and per-pattern severity
  in `mpr config`; `mpr doctor` reports redaction hits per palace (ADR-12, 06 §3.4).

---

## Appendix A — Cross-reference of recommendations

| Recommendation | Source | ADR | Phase |
|---|---|---|---|
| Replace Python ONNX with `fastembed-rs` | 01 §6.1, 04 P0 #4, 05 §A.4 | ADR-1 | 1 |
| Introduce `MemoryProvider` trait | 03 §A, 04 P0 #2 | ADR-3 | 2 |
| Add bi-temporal validity to KG | 02 §1187 | ADR-5 | 5 |
| Add Personalised PageRank retrieval | 02 §7, §1146 | — | 5 |
| Add sleep-time consolidation worker | 01 §6.2, 02 §1242 | — | 5 |
| Tiered vector store (hnsw_rs / usearch / lancedb) | 05 §B.3 | ADR-2 | 2 + 5 |
| Per-`(wing, room)` sub-indexes | 01 §1.3 | — | 5 |
| Honest AAAK marketing | 02 §15 | ADR-9 | 0 |
| `MEMPALACE_EMBED_MODEL` validation manifest | 05 §A.5 | ADR-8 | 1 |
| Per-project palace lifecycle | 04 P0 #5, 03 §F | ADR-7 | 2 + 3 |
| jcode adapter crate | 03 §C–G | ADR-10 | 3 |
| MCP tool renaming alignment | 01 §6.1 | — | 6 |

---

## Appendix B — What this plan deliberately does NOT do

- Does **not** rewrite `knowledge_graph.rs` to use redb/fjall. SQLite stays (ADR-5).
- Does **not** replace `embedvec` immediately. It survives as Tier 0 through Phase 4
  (ADR-2, 05 §C.2 PR 6–8).
- Does **not** introduce a new graph-DB dependency (HelixDB is watched, not adopted).
- Does **not** unify type definitions with `jcode-memory-types` into a third crate
  (ADR-10). Adapter crates handle it.
- Does **not** ship multi-tenant write semantics in v1.0 (open question 6).
- Does **not** replace AAAK with LLMLingua-2 yet. AAAK is gated behind a feature; future
  swap is a non-breaking change (ADR-9).

---

*End of master plan.*
