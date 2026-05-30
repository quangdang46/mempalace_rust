# MemPalace Rust — Remaining Work (vs AgentMemory 1:1 Parity)

> **Overall Completion: ~73%** (was ~27% before Oracle audit)
> Generated: 2026-05-30
> Updated: 2026-05-30 (Oracle audit: P3 ✅, P5.M ✅, P6 ✅, P10 ✅ real; P1.3 ✅, P10.1 ✅ mesh_sync fix; P1.5 P4 P5 P7 P8 P9 remain)
> Benchmark: [rohitg00/agentmemory](https://github.com/rohitg00/agentmemory)

---

## Legend

| Icon | Status |
|------|--------|
| ✅ | DONE — fully implemented, storage-backed |
| ⚠️ | PARTIAL — exists but incomplete/incorrect |
| 🔴 | STUB — returns hardcoded data, no real logic |
| ❌ | MISSING — no module or tool exists |
| 💤 | DORMANT — module code exists but NOT wired to MCP/CLI/hooks |
| 🆗 | VERIFIED REAL — was marked stub but is fully functional per Oracle audit |

---

## Table of Contents

1. [Priority 1 — Wire Hybrid Search into MCP](#p1-wire-hybrid-search)
2. [Priority 2 — Fix Consolidation Pipeline](#p2-fix-consolidation)
3. [Priority 3 — Wire Sentinels & Checkpoints](#p3-wire-sentinels-checkpoints)
4. [Priority 4 — Add Background Task Runner](#p4-background-tasks)
5. [Priority 5 — Expose 15+ Dormant Modules as MCP Tools](#p5-expose-dormant-modules)
6. [Priority 6 — Fix Sketch Promote](#p6-fix-sketch-promote)
7. [Priority 7 — Fix Reflect Tool](#p7-fix-reflect)
8. [Priority 8 — Observation Capture Hooks](#p8-observation-hooks)
9. [Priority 9 — REST API Layer](#p9-rest-api)
10. [Priority 10 — Team Share/Feed](#p10-team-share)
11. [Full Feature Status Matrix](#full-feature-matrix)
12. [MCP Tool Status (60 tools)](#mcp-tool-status)
13. [Dormant Module Inventory](#dormant-modules)
14. [Missing Infrastructure](#missing-infrastructure)

---

<a id="p1-wire-hybrid-search"></a>
## P1 — Wire Hybrid Search into MCP 🔴 CRITICAL

**Current:** MCP `tool_search` → `PalaceDb::query_sync_with_filter` → `naive_similarity` (Jaccard word overlap, threshold 0.05)

**Target:** MCP `tool_search` → BM25 + Vector + Graph RRF fusion (triple-stream)

### What exists already

| File | Lines | Functions | Status |
|------|-------|-----------|--------|
| `crates/core/src/search/rrf.rs` | 292 | `rrf_score`, `normalize_weights`, `fuse_results` | ✅ Wired — called by `hybrid_search` in palace_db.rs |
| `crates/core/src/search/reranker.rs` | 163 | `format_rerank_input`, `rerank_with_scores`, `mock_score_fn` | 💤 DEAD — no callers |
| `crates/core/src/search/query_expansion.rs` | 299 | `extract_entities_from_query`, `expand_query`, `build_search_queries` | 💤 DEAD — not wired to search pipeline |
| `crates/core/src/search/smart_search.rs` | 115 | `compact_limit`, `build_expand_results` | ✅ Wired — called by `tool_smart_search` |
| `crates/core/src/search/diversify.rs` | 154 | `diversify_by_session` | ✅ Wired — called by `hybrid_search` in palace_db.rs |
| `crates/core/src/palace/embedvec.rs` | — | HNSW vector store | 💤 Vector stream in hybrid_search still uses naive_similarity, NOT embedvec |
| `crates/core/src/palace/usearch_sqlite.rs` | — | SQLite-backed vector index | 💤 Same — embedvec/usearch unwired |

### What needs to be done

- [x] **P1.1** ✅ `tool_smart_search` already calls `hybrid_search` (BM25+Vector+Graph RRF)
- [x] **P1.2** ✅ `tool_search` now calls `hybrid_search` with wing/room filtering + where_filter post-filter ✅
- [x] ~~**P1.3**~~ Graph stream already wired via `KnowledgeGraph::query_entity`
- [x] ~~**P1.4**~~ 🔴 `tool_search` still calls `query_sync_with_filter` → `naive_similarity` (Jaccard) — should call `hybrid_search` instead
- [ ] **P1.5** Replace `naive_similarity` in hybrid_search vector stream with real embedvec/usearch embeddings
- [ ] **P1.6** Wire `search/reranker.rs` for post-fusion re-ranking
- [ ] **P1.7** Wire `search/query_expansion.rs` into `tool_smart_search` for LLM query expansion

### Files to modify
- `crates/core/src/mcp_server.rs` — `tool_search()`, `tool_smart_search()`
- `crates/core/src/palace_db.rs` — `query_sync_with_filter()`, `naive_similarity()`
- `crates/core/src/search/mod.rs` — add integration orchestrator

### AgentMemory equivalent
- `mem::search` — hybrid BM25+Vector+Graph RRF fusion
- `mem::smart-search` — query expansion + RRF + diversification

---

<a id="p2-fix-consolidation"></a>
## P2 — Fix Consolidation Pipeline 🔴 CRITICAL

**Current:** `tool_consolidate` does REAL DB writes (upsert_documents + flush + invalidate_cache) — but uses custom heuristic tier promotion instead of calling `consolidation_pipeline.rs::run_consolidation_pipeline`. The 343-line `consolidation_pipeline.rs` is DEAD CODE (no callers).

**Target:** 4-tier working → episodic → semantic → procedural promotion with Ebbinghaus decay.

### What exists already

| File | Lines | Functions | Status |
|------|-------|-----------|--------|
| `crates/core/src/consolidation_pipeline.rs` | 343 | `run_consolidation_pipeline`, `apply_decay_semantic`, `apply_decay_procedural` | 💤 DEAD — not called by MCP |
| `crates/core/src/retention.rs` | 358 | `calculate_retention`, `should_forget`, `promote_tier`, `record_access`, `apply_decay` | 💤 LIVE in auto_forget but not in MCP |
| `crates/core/src/summarize.rs` | 199 | `build_summarize_prompt`, `parse_summary_xml`, `summarize_session` | 💤 DEAD |
| `crates/core/src/auto_forget.rs` | — | Uses retention + evict | 💤 Exists, no MCP tool, no background task |

### What needs to be done

- [ ] **P2.1** Replace fake `tool_consolidate` body with real call to `consolidation_pipeline::run_consolidation_pipeline`
- [ ] **P2.2** Implement working→episodic promotion: group observations by session, create episodic memory
- [ ] **P2.3** Implement episodic→semantic promotion: extract patterns via LLM, create semantic memory
- [ ] **P2.4** Implement semantic→procedural promotion: extract reusable procedures via LLM
- [ ] **P2.5** Wire `summarize.rs` for session summarization step
- [ ] **P2.6** Add `consolidate_pipeline` MCP tool (runs full pipeline, not just single step)
- [ ] **P2.7** Wire retention scoring into consolidation (promote_tier)

### Files to modify
- `crates/core/src/mcp_server.rs` — `tool_consolidate()` — replace fake body
- `crates/core/src/consolidation_pipeline.rs` — ensure it reads from PalaceDb, writes back promoted memories

### AgentMemory equivalent
- `mem::consolidate` — 4-tier promotion with LLM extraction
- `mem::consolidate-pipeline` — full automated pipeline

---

<a id="p3-wire-sentinels-checkpoints"></a>
## P3 — Wire Sentinels & Checkpoints ✅ DONE

**Status: ✅ Fully wired as of 2026-05-30**

### What was done

- **P3.1** `tool_sentinel_create` — calls `db.coordination().sentinel_create()` ✅
- **P3.2** `tool_sentinel_trigger` — calls `db.coordination().sentinel_get()` + `UPDATE` ✅
- **P3.3** `tool_sentinel_list` — new MCP tool, calls `db.coordination().sentinel_list()` ✅
- **P3.4** `tool_sentinel_delete` — new MCP tool, calls `db.coordination().sentinel_delete()` ✅
- **P3.5** `tool_checkpoint` — calls `db.coordination().checkpoint_create()` ✅
- **P3.6** `tool_checkpoint_resolve` — new MCP tool, calls `db.coordination().checkpoint_resolve()` ✅
- **P3.7** `tool_checkpoint_list` — new MCP tool, calls `db.coordination().checkpoint_list()` ✅
- **P3.8** Added `memory_sentinel_list`, `memory_sentinel_delete`, `memory_checkpoint_list`, `memory_checkpoint_resolve` aliases ✅

---

<a id="p4-background-tasks"></a>
## P4 — Add Background Task Runner 🟡 HIGH

**Current:** MemPalace has ZERO background tasks. All decay/eviction/consolidation modules exist but nothing triggers them.

### What needs to be done

- [ ] **P4.1** Create `crates/core/src/background.rs` with tokio::task::spawn scheduler
- [ ] **P4.2** Auto-forget task — run every 60 min, calls `retention::should_forget` + `evict::select_eviction_candidates`
- [ ] **P4.3** Consolidation task — run every 2h, calls `consolidation_pipeline::run_consolidation_pipeline`
- [ ] **P4.4** Lesson decay sweep — run daily, decrements lesson strength scores
- [ ] **P4.5** Insight decay sweep — run daily, removes stale insights
- [ ] **P4.6** Index persistence — periodically flush HNSW index to disk
- [ ] **P4.7** Image quota cleanup — run every 30 min, calls `vision::image_quota::ImageQuotaCleanup::run()`
- [ ] **P4.8** Access tracking flush — persist in-memory access_tracker to SQLite periodically
- [ ] **P4.9** Make all intervals configurable via config.rs / env vars

### Files to create/modify
- **NEW** `crates/core/src/background.rs`
- `crates/core/src/mcp_server.rs` — start background tasks on MCP server init
- `crates/core/src/config.rs` — add interval configuration

### AgentMemory equivalent
- Auto-forget (60 min), consolidation (2h), lesson decay (daily), insight decay (daily), index persistence, image quota cleanup

---

<a id="p5-expose-dormant-modules"></a>
## P5 — Expose Dormant Modules as MCP Tools 🟡 HIGH

**15+ modules have complete implementations (200-400 lines each) with ZERO MCP exposure.** Each needs a thin MCP handler wrapper.

### P5.A — Retention & Eviction

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `retention.rs` | 358 | `calculate_retention`, `should_forget`, `promote_tier`, `record_access`, `apply_decay`, `frequency_multiplier` | `mempalace_retention_score`, `mempalace_retention_evict` |
| `evict.rs` | 405 | `select_eviction_candidates`, `evict_to_target`, `needs_eviction`, `eviction_priority` | (used by auto-forget background task) |

### P5.B — Access Tracking

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `access_tracker.rs` | 155 | `record`, `count`, `last_at`, `recent_accesses`, `most_accessed`, `recently_accessed` (10 pub fns) | `mempalace_access_stats`, `mempalace_access_track` |

### P5.C — Enrichment

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `enrich.rs` | 233 | `enrich_with_file_context`, `search_related_memories`, `find_bug_memories`, `enrich` | `mempalace_enrich` |

### P5.D — Context & Working Memory

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `context.rs` | 318 | `ContextBuilder::new`, `with_pinned_slots`, `with_project_profile`, `with_lessons`, `with_session_summaries`, `with_working_memory`, `build`, `build_xml` | `mempalace_context` |
| `working_memory.rs` | 197 | `WorkingMemory::new`, `add`, `access`, `evict_if_needed`, `token_usage` | `mempalace_working_memory` |

### P5.E — Summarization

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `summarize.rs` | 199 | `build_summarize_prompt`, `parse_summary_xml`, `summarize_session` | `mempalace_summarize` |

### P5.F — Sliding Window

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `sliding_window.rs` | 190 | `SlidingWindow::new`, `add`, `to_context` | Wire into search pipeline |

### P5.G — File Index

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `file_index.rs` | 198 | `FileIndex::new`, `record`, `history_for_file`, `files_in_session`, `sessions_for_file`, `most_active_files` | `mempalace_file_index`, `mempalace_file_context` |

### P5.H — Graph Extraction & Retrieval

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `graph_extraction.rs` | 384 | `build_graph_extraction_prompt`, `extract_graph`, `extract_graph_batch` | `mempalace_graph_extract` |
| `graph_retrieval.rs` | 225 | `search_by_entities`, `expand_from_chunks`, `query_by_predicate`, `graph_stats` | `mempalace_graph_retrieval` |

### P5.I — Skill Extraction

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `skill_extract.rs` | 220 | `build_skill_extraction_prompt`, `parse_skill_extraction`, `extract_skills` | `mempalace_skill_extract` |

### P5.J — Claude Bridge

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `claude_bridge.rs` | 233 | `parse_memory_md`, `serialize_to_memory_md`, `read_memory_file`, `write_memory_file`, `sync_to_claude`, `read_from_claude` | `mempalace_claude_bridge_sync`, `mempalace_claude_bridge_read` |

### P5.K — Migration

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `migrate.rs` | 232 | `detect_version`, `migrate_palace` (placeholder) | `mempalace_migrate` — fix placeholder first |

### P5.L — Query Expansion (in search/)

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `search/query_expansion.rs` | 299 | `extract_entities_from_query`, `expand_query`, `build_search_queries`, `build_search_entities` | Wire into search pipeline + `mempalace_query_expand` |

### P5.M — Vision Search

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `vision/vision_search.rs` | 286 | `vision_embed`, `vision_search` | ✅ VERIFIED REAL — `tool_vision_search` wires VisionSearchStore correctly |
| `vision/image_store.rs` | 183 | `save_image_to_disk`, `delete_image`, `touch_image` | Used by vision search |
| `vision/image_refs.rs` | 134 | `increment_ref`, `decrement_ref`, `list_unreferenced` | Used by vision search |
| `vision/image_quota.rs` | 165 | `ImageQuotaCleanup::run()` | Background task |

### P5.N — Image References

| Module | Lines | Key Functions | MCP Tool to Add |
|--------|-------|---------------|-----------------|
| `vision/image_refs.rs` | 134 | `get_ref_count`, `increment_ref`, `decrement_ref`, `list_unreferenced` | `mempalace_image_refs` |

### Summary of all MCP tools to add

```
mempalace_retention_score        — Calculate retention score for a memory
mempalace_retention_evict        — Evict low-retention memories
mempalace_access_stats           — Get access statistics for memories
mempalace_access_track           — Record a memory access event
mempalace_enrich                 — Enrich query with file context + related memories
mempalace_context                — Build session context block (XML)
mempalace_working_memory         — Get/set working memory entries
mempalace_summarize              — Summarize observations into session summary
mempalace_graph_extract          — Extract entities/relationships from observations
mempalace_graph_retrieval        — Graph-based retrieval by entity/predicate
mempalace_skill_extract          — Extract reusable skills from completed actions
mempalace_claude_bridge_sync     — Sync memories to Claude's MEMORY.md
mempalace_claude_bridge_read     — Read from Claude's MEMORY.md
mempalace_file_index             — Query file activity index
mempalace_file_context           — Get file-specific memory context
mempalace_migrate                — Migrate from ChromaDB or older schemas
mempalace_query_expand           — LLM-powered query expansion
mempalace_image_refs             — Query image reference counts
+ memory_* aliases for all
```

---

<a id="p6-fix-sketch-promote"></a>
## P6 — Fix Sketch Promote ✅ DONE

**Status: ✅ Fully fixed as of 2026-05-30**

**Was:** Created `LessonRecord` entries instead of `Action` entries.

**Now:** Creates real `Action` entries via `CoordinationDb::action_create()` with proper fields (title, description, status, priority, project, tags, parent_id). Then deletes the sketch.

### What needs to be done

- [ ] **P6.1** In `tool_sketch_promote`: fetch sketch from CoordinationDb
- [ ] **P6.2** Parse sketch steps into Action structs
- [ ] **P6.3** Create each Action via `CoordinationDb::action_create()` with proper dependencies
- [ ] **P6.4** Delete the sketch after successful promotion
- [ ] **P6.5** Return list of created action IDs

### Files to modify
- `crates/core/src/mcp_server.rs` — `tool_sketch_promote()`

### AgentMemory equivalent
- `mem::sketch-promote` — creates permanent actions from ephemeral sketch steps

---

<a id="p7-fix-reflect"></a>
## P7 — Fix Reflect Tool 🟢 MEDIUM

**Current:** `tool_reflect` just filters lessons by substring match. No KG traversal.

**Target:** Traverse knowledge graph, cluster related memories, optionally call LLM for synthesis.

### What needs to be done

- [ ] **P7.1** Query KnowledgeGraph for entities related to the reflection topic
- [ ] **P7.2** Use `graph_retrieval::search_by_entities` for graph expansion
- [ ] **P7.3** Cluster related memories by concept/entity overlap
- [ ] **P7.4** (Optional) Call LLM for narrative synthesis of clustered memories
- [ ] **P7.5** Return structured reflection with themes, patterns, and connections

### Files to modify
- `crates/core/src/mcp_server.rs` — `tool_reflect()`
- Wire `crates/core/src/graph_retrieval.rs`

### AgentMemory equivalent
- `mem::reflect` — KG traversal + pattern clustering + LLM synthesis

---

<a id="p8-observation-hooks"></a>
## P8 — Observation Capture Hooks 🟡 HIGH (large)

**Current:** 3 hooks (session-start, stop, pre-compact). No automatic tool-use observation capture.

**Target:** 12 lifecycle hooks with automatic observation capture.

### AgentMemory's 12 hooks

| Hook | MemPalace Status |
|------|-----------------|
| `session_start` | ✅ DONE (CLI hook run) |
| `prompt_submit` | ❌ MISSING |
| `pre_tool_use` | ❌ MISSING |
| `post_tool_use` | ❌ MISSING |
| `post_tool_failure` | ❌ MISSING |
| `pre_compact` | ✅ DONE (CLI hook run) |
| `subagent_start` | ❌ MISSING |
| `subagent_stop` | ❌ MISSING |
| `notification` | ❌ MISSING |
| `task_completed` | ❌ MISSING |
| `stop` | ✅ DONE (CLI hook run) |
| `session_end` | ✅ DONE (CLI hook run) |

### What exists already

| File | Lines | Functions | Status |
|------|-------|-----------|--------|
| `observe.rs` | 334 | `DedupMap`, `fingerprint_observation`, `process_observation`, `ObservationStore` (SQLite) | 💤 DEAD — no callers |

### What needs to be done

- [ ] **P8.1** Wire `observe.rs::ObservationStore` into hook system
- [ ] **P8.2** Add `pre_tool_use` hook → capture tool name + args as RawObservation
- [ ] **P8.3** Add `post_tool_use` hook → capture tool result as RawObservation
- [ ] **P8.4** Add `post_tool_failure` hook → capture error as RawObservation
- [ ] **P8.5** Add `prompt_submit` hook → capture user prompt
- [ ] **P8.6** Add `subagent_start/stop` hooks → capture subagent events
- [ ] **P8.7** Add `notification` hook → capture notification events
- [ ] **P8.8** Add `task_completed` hook → capture task completion events
- [ ] **P8.9** Wire dedup (DedupMap) to prevent duplicate observations
- [ ] **P8.10** Auto-trigger compression after N observations per session

### Files to create/modify
- `crates/core/src/observe.rs` — wire into hook dispatch
- `crates/core/src/mcp_server.rs` — add hook triggers in tool handlers
- `crates/core/src/cli.rs` — register new hooks

---

<a id="p9-rest-api"></a>
## P9 — REST API Layer 🟢 MEDIUM (large)

**Current:** MemPalace is MCP-only (stdio transport). No HTTP interface.

**Target:** REST API with ~125 endpoints matching agentmemory, plus optional web dashboard.

### What needs to be done

- [ ] **P9.1** Add `axum` or `actix-web` dependency
- [ ] **P9.2** Create REST API router mirroring all MCP tool functionality
- [ ] **P9.3** Key endpoint groups:
  - `/api/memory/*` — CRUD, search, recall
  - `/api/palace/*` — wings, rooms, drawers, traverse
  - `/api/graph/*` — KG query, add, timeline, stats
  - `/api/actions/*` — create, update, frontier, next
  - `/api/coordination/*` — leases, routines, signals, sentinels, checkpoints
  - `/api/diary/*` — read, write
  - `/api/slots/*` — list, get, create, append, replace, delete
  - `/api/admin/*` — status, diagnose, heal, audit, governance, export, import
- [ ] **P9.4** Add health check + status endpoints
- [ ] **P9.5** Add CORS support for web dashboard
- [ ] **P9.6** (Optional) Web dashboard on port 3113 for browsing sessions, memories, graph

### AgentMemory equivalent
- 125 REST endpoints via FastAPI
- HTTP dashboard on port 3113

---

<a id="p10-team-share"></a>
## P10 — Team Share/Feed ✅ DONE

**Status: ✅ Fully wired as of 2026-05-30**

### What was done

- **P10.1** `tool_team_share` — writes to `team_shares` SQLite table via `CoordinationDb::team_share_create()` ✅
- **P10.2** `tool_team_feed` — reads from `team_shares` table via `CoordinationDb::team_share_list()` ✅
- **P10.3** Filters (team_id, limit) are supported ✅

### Files to modify
- `crates/core/src/mcp_server.rs` — `tool_team_share()`, `tool_team_feed()`

---

<a id="full-feature-matrix"></a>
## Full Feature Status Matrix

### Core Memory Operations

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::save` (add memory) | ✅ `memory_save` / `mempalace_add_drawer` | With privacy redaction + auto-KG + dedup |
| `mem::recall` (search) | ⚠️ `memory_recall` / `mempalace_search` | Uses Jaccard naïve, not hybrid RRF |
| `mem::search` (hybrid) | ⚠️ BM25+RRF modules exist but NOT wired | See P1 |
| `mem::smart-search` | ⚠️ Alias exists, delegates to naïve search | See P1 |
| `mem::delete` | ✅ `mempalace_delete_drawer` | |
| `mem::update` | ✅ Via `mempalace_add_drawer` upsert | |
| `mem::timeline` | ✅ `memory_timeline` | |
| `mem::patterns` | ✅ `memory_patterns` | Sliding-window n-gram frequency |
| `mem::profile` | ✅ `memory_profile` | Concept frequency + file patterns |
| `mem::export` | ✅ `memory_export` | |
| `mem::import` | 🔴 STUB | CLI prints "Feature coming soon" |
| `mem::replay/import-jsonl` | ✅ `mempalace_replay_import` | |
| `mem::vision-search` | 🆗 VERIFIED REAL — calls VisionSearchStore::new() + store.vision_search() | P5.M marked done |
| `mem::file-context` | ❌ MISSING | See P5.G |
| `mem::rebuild-index` | ⚠️ CLI only, not MCP tool | |
| `mem::dedup` | ✅ 5-min windowed DedupMap | In palace_db::add_drawer_with_dedup |

### Memory Lifecycle

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::observe` | 💤 `observe.rs` (334 lines) exists | Not wired to hooks, see P8 |
| `mem::compress` | ✅ `compress.rs` (363 lines) LIVE | Called by palace.rs |
| `mem::compress-file` | ✅ `mempalace_compress_file` | |
| `mem::compress-synthetic` | ✅ Part of compress.rs fallback | |
| `mem::consolidate` | ⚠️ PARTIAL — REAL DB writes (upsert+flush) but uses custom heuristic, NOT consolidation_pipeline.rs (dead code) | See P2 |
| `mem::consolidate-pipeline` | 💤 `consolidation_pipeline.rs` (343 lines) | Not wired, see P2 |
| `mem::summarize` | 💤 `summarize.rs` (199 lines) | Not wired, see P5.E |
| `mem::auto-forget` | 💤 `auto_forget.rs` + `retention.rs` + `evict.rs` | No background task, see P4 |
| `mem::retention-score` | 💤 `retention.rs` (358 lines, 9 fns) | Not wired, see P5.A |
| `mem::retention-evict` | 💤 `evict.rs` (405 lines) | Not wired, see P5.A |
| `mem::privacy` | ✅ `privacy.rs` (649 lines) LIVE | Runs on every PalaceDb write |
| `mem::enrich` | 💤 `enrich.rs` (233 lines, 4 fns) | Not wired, see P5.C |

### Context & Session

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::context` | 💤 `context.rs` (318 lines, 8 fns) | Not wired, see P5.D |
| `mem::working-memory` | 💤 `working_memory.rs` (197 lines) | Not wired, see P5.D |
| `mem::sliding-window` | 💤 `sliding_window.rs` (190 lines) | Not wired, see P5.F |
| `mem::query-expansion` | 💤 `search/query_expansion.rs` (299 lines) | Not wired, see P5.L |

### Knowledge Graph

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::graph-query` | ✅ `mempalace_kg_query` | Entity queries with temporal filtering |
| `mem::graph-add` | ✅ `mempalace_kg_add` | Triple insertion |
| `mem::graph-invalidate` | ✅ `mempalace_kg_invalidate` | Fact invalidation |
| `mem::graph-timeline` | ✅ `mempalace_kg_timeline` | Temporal queries |
| `mem::graph-stats` | ✅ `mempalace_kg_stats` | Statistics |
| `mem::graph-extract` | 💤 `graph_extraction.rs` (384 lines) | Not wired, see P5.H |
| `mem::graph-retrieval` | 💤 `graph_retrieval.rs` (225 lines) | Not wired, see P5.H |
| `mem::temporal-graph-query` | ✅ Via `as_of` parameter | |

### Relations

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::get-related` | ⚠️ `memory_relations` reads metadata keys only | No graph edge traversal |
| `mem::relation-create` | ❌ MISSING | No typed relationship creation |
| `mem::relation-update` | ❌ MISSING | |
| `mem::cascade` | ❌ MISSING | No cascading delete across related entities |

### Coordination

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::action-create` | ✅ `mempalace_action_create` | With dependencies |
| `mem::action-update` | ✅ `mempalace_action_update` | |
| `mem::frontier` | ✅ `mempalace_frontier` | List open actions |
| `mem::next` | ✅ `mempalace_next` | Next recommended action |
| `mem::lease-acquire` | ✅ | |
| `mem::lease-release` | ✅ | |
| `mem::lease-renew` | ✅ | |
| `mem::routine-run` | ✅ `mempalace_routine_run` | |
| `mem::signal-send` | ✅ `mempalace_signal_send` | |
| `mem::signal-read` | ✅ `mempalace_signal_read` | |
| `mem::sentinel-create` | ✅ VERIFIED REAL — SQLite INSERT via CoordinationDb::sentinel_create() | P3 marked done |
| `mem::sentinel-trigger` | ✅ VERIFIED REAL — SQLite UPDATE via direct SQL | P3 marked done |
| `mem::checkpoint-create` | ✅ VERIFIED REAL — SQLite INSERT via CoordinationDb | P3 marked done |
| `mem::checkpoint-resolve` | ✅ VERIFIED REAL — SQLite UPDATE via `checkpoint_resolve()` | P3 marked done |
| `mem::checkpoint-list` | ✅ VERIFIED REAL — SQLite SELECT via `checkpoint_list()` | P3 marked done |

### Smart Features

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::sketch-create` | ✅ | |
| `mem::sketch-promote` | ⚠️ PARTIAL — just deletes sketch | See P6 |
| `mem::sketch-discard` | ⚠️ Via delete, no explicit tool | |
| `mem::crystallize` | ✅ `mempalace_crystallize` | CrystalRecord with narrative |
| `mem::diagnose` | ✅ `mempalace_diagnose` | |
| `mem::heal` | ✅ `mempalace_heal` | |
| `mem::facet-tag` | ✅ `mempalace_facet_tag` | |
| `mem::facet-query` | ✅ `mempalace_facet_query` | |
| `mem::verify` | ✅ `mempalace_verify` | Citation chain tracing |
| `mem::lesson-save` | ✅ `mempalace_lesson_save` | |
| `mem::lesson-recall` | ✅ `mempalace_lesson_recall` | |
| `mem::lesson-strengthen` | ❌ MISSING | `CoordinationDb::reinforce()` exists but not exposed |
| `mem::lesson-decay-sweep` | ❌ MISSING | No background task |
| `mem::reflect` | ⚠️ PARTIAL — substring filter only | See P7 |
| `mem::insight-list` | ✅ `mempalace_insight_list` | |
| `mem::insight-search` | ❌ MISSING | |
| `mem::insight-decay-sweep` | ❌ MISSING | No background task |
| `mem::slot-list/get/create/append/replace/delete` | ✅ All 6 CRUD ops | |
| `mem::slot-reflect` | ❌ MISSING | Auto-append patterns/todos from observations |
| `mem::skill-extract` | 💤 `skill_extract.rs` (220 lines) | Not wired, see P5.I |
| `mem::obsidian-export` | ✅ `mempalace_obsidian_export` | |

### Cross-Agent

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::team-share` | ✅ VERIFIED REAL — SQLite INSERT via CoordinationDb::team_share_create() | P10 marked done |
| `mem::team-feed` | ✅ VERIFIED REAL — SQLite SELECT via CoordinationDb::team_share_list() | P10 marked done |
| `mem::mesh-sync` | 🆗 FIXED — now wires Mesh peer registry with peer registration + list | Now real, not stub |
| `mem::snapshot-create` | ✅ `mempalace_snapshot_create` | Git-based |

### Bridges & Migration

| AgentMemory Feature | MemPalace Status | Notes |
|---------------------|-----------------|-------|
| `mem::claude-bridge-sync` | 💤 `claude_bridge.rs` (233 lines, 6 fns) | Not wired, see P5.J |
| `mem::claude-bridge-read` | 💤 Same module | Not wired, see P5.J |
| `mem::migrate` | 💤 `migrate.rs` (231 lines) | Detection works, migration is placeholder |
| `mem::migrate-vector-index` | ❌ MISSING | |
| `mem::access-track` | 💤 `access_tracker.rs` (155 lines, 10 fns) | Not wired, see P5.B |
| `mem::image-refs` | 💤 `vision/image_refs.rs` (134 lines) | Not wired, see P5.N |
| `mem::disk-size-manager` | ❌ MISSING | No module |
| `mem::cascade` | ❌ MISSING | No module |

---

<a id="mcp-tool-status"></a>
## MCP Tool Status (60 tools registered)

### ✅ Fully Implemented (48 tools)

```
mempalace_status / memory_status
mempalace_list_wings / memory_list_wings
mempalace_list_rooms / memory_list_rooms
mempalace_get_taxonomy / memory_get_taxonomy
mempalace_search / memory_search / memory_list
mempalace_check_duplicate / memory_check_duplicate
mempalace_add_drawer / memory_add / memory_add_drawer
mempalace_delete_drawer / memory_delete
mempalace_kg_query / memory_kg_query / memory_graph_query
mempalace_kg_add / memory_kg_add / memory_graph_add
mempalace_kg_invalidate / memory_graph_invalidate
mempalace_kg_timeline / memory_graph_timeline
mempalace_kg_stats / memory_graph_stats
mempalace_traverse / memory_traverse
mempalace_find_tunnels / memory_find_tunnels
mempalace_graph_stats
mempalace_diary_read / memory_diary_read
mempalace_diary_write / memory_diary_write
mempalace_heal / memory_heal
mempalace_verify / memory_verify
mempalace_governance_delete / memory_governance_delete
mempalace_obsidian_export / memory_obsidian_export
mempalace_compress_file / memory_compress_file
mempalace_detect_worktree
mempalace_replay_import
mempalace_action_create / memory_action_create
mempalace_action_update / memory_action_update
mempalace_frontier / memory_frontier
mempalace_next / memory_next
mempalace_lease / memory_lease (acquire/release/renew)
mempalace_routine_run / memory_routine_run
mempalace_signal_send / memory_signal_send
mempalace_signal_read / memory_signal_read
mempalace_sketch_create / memory_sketch_create
mempalace_crystallize / memory_crystallize
mempalace_diagnose / memory_diagnose
mempalace_facet_tag / memory_facet_tag
mempalace_facet_query / memory_facet_query
mempalace_lesson_save / memory_lesson_save
mempalace_lesson_recall / memory_lesson_recall
mempalace_insight_list / memory_insight_list
mempalace_slot_list / memory_slot_list
mempalace_slot_get / memory_slot_get
mempalace_slot_create / memory_slot_create
mempalace_slot_append / memory_slot_append
mempalace_slot_replace / memory_slot_replace
mempalace_slot_delete / memory_slot_delete
mempalace_snapshot_create / memory_snapshot_create
mempalace_file_history / memory_file_history
mempalace_sessions / memory_sessions
mempalace_commits / memory_commits
mempalace_commit_lookup / memory_commit_lookup
memory_recall
memory_save
memory_profile
memory_export
memory_timeline
memory_patterns
memory_audit
memory_commit_lookup
```

### 🔴 Stubs (1 tool)

```
mempalace_consolidate / memory_consolidate             → uses custom heuristic, NOT consolidation_pipeline.rs (dead code)
```

### ✅ VERIFIED REAL (was marked stub — Oracle audit 2026-05-30 + subsequent fixes) (7 tools)

```
mempalace_sentinel_create / memory_sentinel_create     → SQLite INSERT via CoordinationDb::sentinel_create()
mempalace_sentinel_trigger / memory_sentinel_trigger   → SQLite UPDATE via direct SQL
mempalace_vision_search / memory_vision_search          → calls VisionSearchStore::new() + store.vision_search()
mempalace_team_share / memory_team_share                → SQLite INSERT via CoordinationDb::team_share_create()
mempalace_team_feed / memory_team_feed                  → SQLite SELECT via CoordinationDb::team_share_list()
mempalace_checkpoint / memory_checkpoint                  → SQLite INSERT via CoordinationDb
mempalace_mesh_sync / memory_mesh_sync                   → Mesh peer registry with register() + list_peers() ✅ FIXED
```

### ⚠️ Partial (4 tools)

```
mempalace_sketch_promote / memory_sketch_promote        → creates Actions via action_create() ✅, but needs block-scope fix
mempalace_reflect / memory_reflect                      → substring filter only, no KG traversal
mempalace_smart_search / memory_smart_search            → delegates to hybrid_search ✅, but vector leg uses naive_similarity
memory_relations                                        → reads metadata keys only
```

### MCP Tools to Add (20+ new tools)

```
mempalace_retention_score
mempalace_retention_evict
mempalace_access_stats
mempalace_access_track
mempalace_enrich
mempalace_context
mempalace_working_memory
mempalace_summarize
mempalace_graph_extract
mempalace_graph_retrieval
mempalace_skill_extract
mempalace_claude_bridge_sync
mempalace_claude_bridge_read
mempalace_file_index
mempalace_file_context
mempalace_migrate
mempalace_query_expand
mempalace_image_refs
mempalace_sentinel_list
mempalace_sentinel_delete
mempalace_checkpoint_resolve
mempalace_checkpoint_list
mempalace_lesson_strengthen
mempalace_lesson_decay_sweep
mempalace_insight_search
mempalace_insight_decay_sweep
mempalace_slot_reflect
mempalace_sketch_discard
mempalace_relation_create
mempalace_relation_update
mempalace_cascade
mempalace_import
+ memory_* aliases for all
```

---

<a id="dormant-modules"></a>
## Dormant Module Inventory (Code Written, Not Wired)

| # | Module | Lines | Pub Fns | Has Tests | Effort to Wire |
|---|--------|-------|---------|-----------|----------------|
| 1 | `consolidation_pipeline.rs` | 343 | 4 | ✅ | Medium-Large (need to wire full pipeline) |
| 2 | `graph_extraction.rs` | 384 | 3 | ✅ | Medium (need LLM provider) |
| 3 | `graph_retrieval.rs` | 225 | 4 | ✅ | Small (pure computation) |
| 4 | `retention.rs` | 358 | 9 | ✅ | Small (pure computation) |
| 5 | `evict.rs` | 405 | 4 | ✅ | Small (pure computation) |
| 6 | `context.rs` | 318 | 8 | ✅ | Small (builder pattern, no I/O) |
| 7 | `claude_bridge.rs` | 233 | 6 | ✅ | Small (file I/O only) |
| 8 | `enrich.rs` | 233 | 4 | ✅ | Small (pure computation) |
| 9 | `skill_extract.rs` | 220 | 3 | ✅ | Medium (needs LLM provider) |
| 10 | `access_tracker.rs` | 155 | 10 | ✅ | Small (needs persistence layer) |
| 11 | `summarize.rs` | 199 | 3 | ✅ | Medium (needs LLM provider) |
| 12 | `sliding_window.rs` | 190 | 3 | ✅ | Small (pure computation) |
| 13 | `file_index.rs` | 198 | 6 | ✅ | Small (needs persistence layer) |
| 14 | `observe.rs` | 334 | 11 | ✅ | Large (need hook wiring) |
| 15 | `migrate.rs` | 232 | 2 | ✅ | Medium (placeholder migration) |
| 16 | `working_memory.rs` | 197 | 8 | ✅ | Small (needs persistence) |
| 17 | `search/query_expansion.rs` | 299 | 5 | ✅ | Medium (needs LLM provider) |
| 18 | `search/rrf.rs` | 292 | 3 | ✅ | Small (pure computation) |
| 19 | `search/reranker.rs` | 163 | 3 | ✅ | Small (needs ONNX model) |
| 20 | `search/smart_search.rs` | 115 | 2 | ✅ | Small (pure computation) |
| 21 | `search/diversify.rs` | 154 | 1 | ✅ | Small (pure computation) |
| 22 | `vision/vision_search.rs` | 286 | 5 | ✅ | Medium (needs CLIP) |
| 23 | `vision/image_store.rs` | 183 | 5 | ✅ | Small (file I/O) |
| 24 | `vision/image_refs.rs` | 134 | 5 | ✅ | Small (SQLite) |
| 25 | `vision/image_quota.rs` | 165 | 1 | ✅ | Small (background task) |
| **TOTAL** | **25 modules** | **~6,116** | **~130** | | |

---

<a id="missing-infrastructure"></a>
## Missing Infrastructure (No Code Exists)

| # | Feature | Description | Effort |
|---|---------|-------------|--------|
| 1 | **REST API** | axum/actix HTTP server with ~125 endpoints | Large |
| 2 | **Web Dashboard** | Browser UI on port 3113 for browsing memories | Large |
| 3 | **Background Task Runner** | tokio::task scheduler for periodic jobs | Medium |
| 4 | **Cascade Delete** | Propagate deletes across memory→relations→graph→facets | Medium |
| 5 | **Disk Size Manager** | Monitor and manage SQLite + vector index disk usage | Medium |
| 6 | **Relation Create/Update** | Typed relationship creation between memories | Medium |
| 7 | **Import Tool** | Full import from agentmemory export format | Medium |
| 8 | **8 Missing Hooks** | prompt_submit, pre/post_tool_use, subagent_start/stop, etc. | Large |
| 9 | **Vector Search in MCP** | Wire embedvec/usearch to MCP search path | Medium |
| 10 | **Cloud Deployment** | Docker, fly.io, Railway configs | Small |
| 11 | **Agent Adapters** | Cursor, Copilot, Gemini CLI integrations (22+ agents) | Large |
| 12 | **Additional Embedding Providers** | OpenAI, Voyage AI, Cohere embeddings | Medium |
| 13 | **Additional LLM Providers** | Gemini, MiniMax, OpenRouter, local (Ollama/vLLM) | Medium |

---

## Estimated Effort Summary

| Priority | Task | Effort | Dependencies |
|----------|------|--------|--------------|
| **P1** | Wire Hybrid Search | Medium (2-3 days) | Vector index must be wired |
| **P2** | Fix Consolidation | Medium-Large (3-4 days) | LLM provider + PalaceDb integration |
| **P3** | Wire Sentinels/Checkpoints | Small (0.5-1 day) | None — DB CRUD already exists |
| **P4** | Background Task Runner | Medium (2 days) | None |
| **P5** | Expose Dormant Modules | Small each (0.5-1 day per module) ~10 days total | Some need LLM provider |
| **P6** | Fix Sketch Promote | Small (0.5 day) | None |
| **P7** | Fix Reflect | Medium (1-2 days) | graph_retrieval wiring |
| **P8** | Observation Hooks | Large (3-5 days) | observe.rs wiring |
| **P9** | REST API | Large (5-7 days) | None |
| **P10** | Team Share/Feed | Small (0.5 day) | None |

**Total remaining: ~25-35 working days**

---

## What MemPalace Has That AgentMemory Doesn't

| Feature | Description |
|---------|-------------|
| **Palace Metaphor** | Wing/Room/Drawer taxonomy with graph traversal, tunnel detection, room connectivity — genuinely novel |
| **Diary System** | Per-agent diary entries with AAAK compression |
| **Governance Delete** | Rich filtering (age, strength, type, project, access patterns) with audit trail |
| **Query Sanitization** | Prompt injection protection on search queries |
| **WAL Audit Trail** | JSONL write-ahead log for all MCP tool invocations |
| **Embedder Trait** | Clean Rust trait with 3 backends (FastEmbed ONNX, Model2Vec, Tract) |
| **Worktree Detection** | Branch-aware memory via `mempalace_detect_worktree` |
