# REMAINING.md — agentmemory vs mempalace_rust Feature Parity Report

**Generated:** 2026-06-01
**Commit:** `352de60` (fix: resolve all remaining build errors for clean compilation)
**Source agentmemory:** rohitg00/agentmemory (cloned to `/tmp/agentmemory`, v0.9.24)
**mempalace_rust LOC:** 69,623 total across 146 source files (57,257 in `crates/core/src/`, 9,574 in subdirs, 2,338 benchmarks)

---

## Summary Scorecard

| Dimension | agentmemory | mempalace_rust | Parity |
|---|---|---|---|
| MCP tool handlers (unique names) | 53 | 87 (73 `mempalace_*` + 76 `memory_*` aliases + duplicates) | ~92% ✅ |
| REST API routes | 125 endpoints | 67 routes | **~54%** ⚠️ |
| Hook scripts | 12 hooks | 2 hooks (`save`, `precompact`) | **~17%** ❌ |
| Internal functions (source files) | 64 functions in `src/functions/` | 95 modules in `crates/core/src/` | ~85% ✅ |
| Embedding providers | 10 (OpenAI, Anthropic, OpenRouter, Cohere, Gemini, Voyage, Minimax, CLIP, FastEmbed, Local) | 3 (FastEmbed, model2vec, tract) | **~30%** ❌ |
| LLM providers | 7 (agent-sdk, anthropic, openai, openrouter, minimax, noop, fallback-chain) | 5 (openai-compat, anthropic, noop, fallback-chain, circuit-breaker) | ~70% ⚠️ |
| State/KV namespaces | 38 (TypeScript `KV` schema) | 19 SQLite tables | **~50%** ⚠️ |
| Prompt templates | 7 (`src/prompts/`) | 6 (`crates/core/src/prompts/`) | ~86% ✅ |
| MCP Resources | 6 resources | 0 | **0%** ❌ |
| MCP Prompts | 3 prompts | 0 | **0%** ❌ |
| Benchmarks | 4 suites, 2,584 LOC | 3 Criterion benches + 1 longmemeval binary, 2,338 LOC | ~80% ✅ |
| Viewer (web UI) | Full HTML viewer at `:3113` | None | **0%** ❌ |
| Tests | 1,293+ vitest cases across 121 files | 1,063 `#[test]` across 95 modules | ~82% ✅ |

**Overall functional parity: ~65%**

---

## 1. MCP Tools

### 1.1 Tool Registry Comparison

agentmemory (53 tools in `tools-registry.ts`):
```
memory_action_create         memory_checkpoint         memory_graph_query        memory_session_recall      memory_slot_replace
memory_action_update         memory_checkpoint_resolve  memory_heal               memory_sessions            memory_smart_search
memory_audit                 memory_claude_bridge_sync  memory_insight_list       memory_signal_read         memory_snapshot_create
memory_checkpoint_list       memory_commit_lookup       memory_lease              memory_signal_send         memory_team_feed
memory_team_share           memory_timeline            memory_verify             memory_vision_search
memory_compress_file         memory_consolidate         memory_crystallize        memory_diagnose
memory_export               memory_facet_query         memory_facet_tag          memory_file_history
memory_frontier             memory_governance_delete   memory_lesson_recall      memory_lesson_save
memory_mesh_sync            memory_next                memory_obsidian_export    memory_patterns
memory_profile              memory_recall              memory_reflect            memory_relations
memory_routine_run          memory_save                memory_sentinel_create    memory_sentinel_trigger
memory_slot_append          memory_slot_create         memory_slot_delete        memory_slot_get
memory_slot_list
```

mempalace_rust (73 `mempalace_*` tools, 76 `memory_*` aliases, 85 handler functions):

**Unique `mempalace_*` tools (20):**
```
mempalace_access_stats      mempalace_add_drawer       mempalace_check_duplicate  mempalace_context_build
mempalace_delete_drawer     mempalace_detect_worktree  mempalace_get_aaak_spec    mempalace_get_taxonomy
mempalace_graph_expand      mempalace_kg_add           mempalace_kg_invalidate    mempalace_kg_query
mempalace_kg_stats          mempalace_kg_timeline      mempalace_list_rooms        mempalace_list_wings
mempalace_replay_import     mempalace_retention_score  mempalace_traverse          mempalace_working_memory
```

**Shared `memory_*` tools (53 — 1:1 alias to `mempalace_*` in `mcp_server.rs` switch):**
All agentmemory tools are mirrored with `memory_*` prefix for cross-compatibility.

### 1.2 Missing MCP Tools

Only 1 agentmemory tool **not implemented** in mempalace_rust:

| Missing Tool | Severity | Notes |
|---|---|---|
| `memory_claude_bridge_sync` | Low | Claude bridge module exists (`claude_bridge.rs`, 232 LOC) but no MCP tool. REST endpoint only. |

All other 52 agentmemory tools have `memory_*` aliases in mempalace's switch dispatch.

### 1.3 MCP Resources & Prompts

agentmemory exposes 6 MCP resources + 3 MCP prompts. mempalace_rust exposes **none**.

**agentmemory MCP resources:**
```
agentmemory://status                          — session count, memory count, health
agentmemory://project/{name}/profile          — top concepts, files, conventions
agentmemory://project/{name}/recent           — last 5 session summaries
agentmemory://memories/latest                 — top 10 latest memories
agentmemory://graph/stats                     — KG node/edge counts by type
agentmemory://team/feed                       — team shared memories
```

**agentmemory MCP prompts:**
```
recall_context(task_description)             — search memories for task context
session_handoff(session_id)                  — generate handoff summary
detect_patterns(project?)                    — detect recurring patterns
```

These are 100% missing from mempalace_rust. No `mcp::resources::*` or `mcp::prompts::*` endpoints.

---

## 2. REST API Endpoints

### 2.1 Coverage Matrix

| Category | agentmemory (125) | mempalace_rust (67) | Coverage | Missing |
|---|---|---|---|---|
| Core memory | 15 | 9 | 60% | auto-crystallize, auto-forget, observations, summarize, forget, remember |
| Search | 6 | 4 | 67% | semantic-list, procedural-list, file-index |
| Session | 7 | 1 | 14% | session::start, session::end, session::commit, session::by-commit, replay/load, replay/sessions |
| Coordination | 18 | 8 | 44% | liveness, branch-detect, branch-sessions, branch-worktrees, config-flags, auto-forget, auto-crystallize |
| Graph | 7 | 7 | 100% | ✅ All covered |
| Team/Mesh | 10 | 4 | 40% | team-profile, mesh-register, mesh-list, mesh-export, mesh-receive |
| Actions/Leases | 8 | 3 | 38% | action-get, action-list, action-edge, lease-acquire, lease-release, lease-renew |
| Signals/Routines | 5 | 2 | 40% | routine-create, routine-status, routine-list |
| Checkpoints | 3 | 3 | 100% | ✅ All covered |
| Sentinels | 4 | 3 | 75% | sentinel-cancel, sentinel-check |
| Knowledge | 5 | 5 | 100% | ✅ All covered |
| Sketches/Crystals | 6 | 4 | 67% | sketch-discard, sketch-gc, sketch-add, crystal-list |
| Facets/Lessons | 6 | 4 | 67% | facet-get, facet-stats, facet-untag, lesson-list, lesson-search, lesson-strengthen |
| Insights | 3 | 1 | 33% | insight-search |
| Slots | 7 | 6 | 86% | slot-reflect |
| Snapshots | 3 | 1 | 33% | snapshot-restore, snapshots |
| Governance/Audit | 4 | 2 | 50% | governance-bulk |
| Replay | 4 | 1 | 25% | replay/load, replay/sessions |
| Misc | 9 | 3 | 33% | cascade-update, flow-compress, generate-rules, evolve, disk-size-manager |
| Health/Config | 3 | 2 | 67% | liveness, config-flags |
| Viewer | 1 | 0 | 0% | viewer |
| Vision | 2 | 1 | 50% | vision-embed |
| Branch-aware | 3 | 0 | 0% | branch-detect, branch-sessions, branch-worktrees |
| Sliding window | 1 | 0 | 0% | flow-compress |
| **Total** | **125** | **67** | **~54%** | **~58 missing** |

### 2.2 Missing REST Endpoints (Priority Order)

#### P0 — Core functionality gaps:

| Endpoint | Why Needed | File to Add |
|---|---|---|
| `POST /observe` | Already exists in mempalace but not in REST (only MCP) | `rest_api.rs` |
| `POST /session/start` | Session lifecycle management for hook integration | `rest_api.rs` |
| `POST /session/end` | Session lifecycle — trigger summarization, auto-forget | `rest_api.rs` |
| `GET/POST /memories` | Core CRUD for memories (beyond drawer-based storage) | `rest_api.rs` |
| `POST /forget` | Explicit memory deletion with audit trail | `evict.rs` exists, expose REST |
| `POST /remember` | Core memory save (alias to diary_write) | `rest_api.rs` |
| `POST /summarize` | Session summarization endpoint (called by `stop` hook) | `summarize.rs` exists, add REST |
| `POST /evict` | Memory eviction with reason tracking | `evict.rs` exists, expose REST |

#### P1 — Operational endpoints:

| Endpoint | Why Needed | File to Add |
|---|---|---|
| `GET /liveness` | Kubernetes health check | `rest_api.rs` |
| `GET /config-flags` | Dynamic feature flag inspection | `rest_api.rs` |
| `GET/POST /semantic-list` | Semantic memory listing | `rest_api.rs` |
| `GET/POST /procedural-list` | Procedural memory listing | `rest_api.rs` |
| `GET/POST /file-index` | File index management | `rest_api.rs` |
| `POST /auto-forget` | Trigger auto-forget manually | `auto_forget.rs` exists, add REST |
| `POST /auto-crystallize` | Trigger auto-crystallize | `rest_api.rs` |
| `POST /flow-compress` | Flow compression endpoint | `rest_api.rs` |
| `GET /replay/sessions` | List replay sessions | `replay.rs` exists, add REST |
| `GET /replay/load` | Load replay session | `replay.rs` exists, add REST |
| `GET /snapshots` | List snapshots | `export/snapshot.rs` exists, add REST |
| `POST /snapshot/restore` | Restore snapshot | `export/snapshot.rs` exists, add REST |
| `POST /governance/bulk` | Bulk governance operations | `rest_api.rs` |
| `POST /sentinel/cancel` | Cancel sentinel | `rest_api.rs` |
| `POST /sentinel/check` | Check sentinel status | `rest_api.rs` |
| `GET /insight/search` | Search insights | `rest_api.rs` |
| `GET /lesson/search` | Search lessons | `rest_api.rs` |
| `GET /lesson/list` | List lessons | `rest_api.rs` |
| `POST /lesson/strengthen` | Strengthen lesson | `rest_api.rs` |
| `GET /team/profile` | Get team profile | `rest_api.rs` |
| `POST /mesh/register` | Register mesh peer | `rest_api.rs` |
| `GET /mesh/list` | List mesh peers | `rest_api.rs` |
| `GET /mesh/export` | Export mesh state | `rest_api.rs` |
| `POST /mesh/receive` | Receive mesh sync | `rest_api.rs` |
| `GET /action/list` | List all actions | `rest_api.rs` |
| `GET /action/{id}` | Get action | `rest_api.rs` |
| `POST /action/edge` | Create action edge | `rest_api.rs` |
| `POST /lease/acquire` | Acquire lease | `rest_api.rs` |
| `POST /lease/release` | Release lease | `rest_api.rs` |
| `POST /lease/renew` | Renew lease | `rest_api.rs` |
| `POST /routine/create` | Create routine | `rest_api.rs` |
| `GET /routine/list` | List routines | `rest_api.rs` |
| `GET /routine/status` | Get routine status | `rest_api.rs` |
| `POST /cascade-update` | Cascade update | `rest_api.rs` |
| `POST /generate-rules` | Generate rules | `rest_api.rs` |
| `POST /evolve` | Evolve palace | `rest_api.rs` |
| `GET /disk-size-manager` | Disk usage info | `export/disk_size_manager.rs` exists, add REST |
| `POST /sketch/add` | Add to sketch | `rest_api.rs` |
| `GET /sketch/discard` | Discard sketch | `rest_api.rs` |
| `GET /sketch/gc` | Garbage collect sketches | `rest_api.rs` |
| `GET /crystal/list` | List crystals | `rest_api.rs` |
| `GET /facet/get` | Get facet | `rest_api.rs` |
| `GET /facet/stats` | Facet stats | `rest_api.rs` |
| `POST /facet/untag` | Untag facet | `rest_api.rs` |
| `POST /branch/detect` | Detect git branch | `rest_api.rs` |
| `GET /branch/sessions` | Sessions by branch | `rest_api.rs` |
| `GET /branch/worktrees` | List worktrees | `rest_api.rs` |
| `GET /viewer` | HTML viewer | **new file** |
| `POST /vision/embed` | Embed image | `vision/embedding_provider.rs` exists, add REST |

---

## 3. Hooks

### 3.1 Hook Coverage

agentmemory has 12 hook scripts. mempalace_rust has **2**.

| Hook | agentmemory | mempalace_rust | Status |
|---|---|---|---|
| `session-start` | `src/hooks/session-start.ts` — inject context on Claude Code wake-up | ❌ Missing | **P0** |
| `stop` | `src/hooks/stop.ts` — calls summarize + session::end on SIGINT | ✅ `hooks_cli.rs` (save) | Ported |
| `session-end` | `src/hooks/session-end.ts` — persist session state | ❌ Missing | **P0** |
| `pre-tool-use` | `src/hooks/pre-tool-use.ts` — optional context enrichment (disabled by default since 0.8.10) | ✅ `hooks_cli.rs` (precompact) | Ported |
| `pre-compact` | `src/hooks/pre-compact.ts` — inject context before Claude Code compact | ✅ `hooks_cli.rs` (precompact) | Ported |
| `post-tool-use` | `src/hooks/post-tool-use.ts` — log tool usage, fire `mem::observe` | ❌ Missing | **P1** |
| `post-tool-failure` | `src/hooks/post-tool-failure.ts` — log failure, trigger heal | ❌ Missing | **P1** |
| `prompt-submit` | `src/hooks/prompt-submit.ts` — capture user prompt as observation | ❌ Missing | **P1** |
| `notification` | `src/hooks/notification.ts` — system notification handler | ❌ Missing | **P2** |
| `subagent-start` | `src/hooks/subagent-start.ts` — log subagent spawn | ❌ Missing | **P2** |
| `subagent-stop` | `src/hooks/subagent-stop.ts` — log subagent completion | ❌ Missing | **P2** |
| `task-completed` | `src/hooks/task-completed.ts` — mark task in action graph | ❌ Missing | **P2** |

### 3.2 Hook Implementation Notes

**agentmemory hook patterns:**
- Context-injecting hooks (`session-start`, `pre-tool-use`, `pre-compact`): use `try/catch` + `AbortSignal.timeout(N)` + write context to stdout for Claude Code to read
- Telemetry-only hooks (`stop`, `session-end`, `post-tool-*`, etc.): use fire-and-forget `fetch(...).catch(() => {})` + `setTimeout(() => process.exit(0), 500).unref()`

**mempalace_rust current hooks (`hooks_cli.rs`):**
- `mpr hook save` — saves session state on stop
- `mpr hook precompact` — context injection before compact
- Both use `serde_json` + `std::fs` + `reqwest` for HTTP calls

### 3.3 Missing Hooks to Implement

| Hook | Priority | Trigger | Action |
|---|---|---|---|
| `session-start` | P0 | Claude Code startup | Call `/observe` + `/context/build`, inject to stdout |
| `session-end` | P0 | Session termination | Call `/session/end` + `/summarize` |
| `post-tool-use` | P1 | After every tool | Call `/observe` with tool name/input/output |
| `post-tool-failure` | P1 | After tool error | Call `/observe` with error + trigger heal check |
| `prompt-submit` | P1 | User submits prompt | Call `/observe` with user_prompt |
| `notification` | P2 | System notification | Call `/observe` with notification data |
| `subagent-start` | P2 | Subagent spawns | Call `/observe` with subagent metadata |
| `subagent-stop` | P2 | Subagent stops | Call `/observe` with subagent result |
| `task-completed` | P2 | Task done | Update action graph, call `/observe` |

---

## 4. Embedding Providers

### 4.1 Coverage

| Provider | agentmemory | mempalace_rust | Notes |
|---|---|---|---|
| FastEmbed (ONNX) | ✅ | ✅ | `embed-fastembed` feature |
| OpenAI | ✅ | ✅ | Via openai-compat provider |
| Anthropic | ✅ | ❌ | Only in `llm/anthropic_provider.rs` (LLM, not embedding) |
| OpenRouter | ✅ | ❌ | — |
| Cohere | ✅ | ❌ | — |
| Gemini | ✅ | ❌ | — |
| Voyage | ✅ | ❌ | — |
| Minimax | ✅ | ❌ | — |
| Local (in-browser) | ✅ | ❌ | — |
| CLIP (image) | ✅ | ❌ | `vision/embedding_provider.rs` has trait but no CLIP impl |
| model2vec | ❌ | ✅ | Rust-native, no C++ |
| tract (pure-Rust ONNX) | ❌ | ✅ | Pure Rust, no C++ |

**Missing embedding providers (7):** Anthropic, OpenRouter, Cohere, Gemini, Voyage, Minimax, CLIP.

### 4.2 LLM Provider Gap

| Provider | agentmemory | mempalace_rust | Notes |
|---|---|---|---|
| agent-sdk | ✅ | ❌ | Agent SDK integration for nested sessions |
| openai | ✅ | ✅ | Via openai-compat |
| anthropic | ✅ | ✅ | Full implementation |
| openrouter | ✅ | ❌ | — |
| minimax | ✅ | ❌ | — |
| noop | ✅ | ✅ | ✅ |
| fallback-chain | ✅ | ✅ | With circuit breaker |

**Missing LLM providers (2):** agent-sdk, openrouter, minimax.

---

## 5. State / Storage

### 5.1 KV Namespace vs SQLite Tables

agentmemory has 38 KV namespaces. mempalace_rust has 19 SQLite tables.

| agentmemory KV | mempalace_rust SQLite Table | Status |
|---|---|---|
| `mem:sessions` | `sessions` | ✅ |
| `mem:obs:{sessionId}` | `observations` | ✅ |
| `mem:memories` | Drawer-based (palace model) | ⚠️ Different — mempalace uses drawer/room/wing hierarchy |
| `mem:summaries` | Part of `sessions` | ⚠️ Embedded |
| `mem:config` | `palace_db` JSON | ⚠️ Different |
| `mem:metrics` | Memory access tracker | ⚠️ Different |
| `mem:health` | REST `/health` endpoint | ⚠️ In-memory |
| `mem:index:bm25` | `bm25.rs` in-memory | ⚠️ In-memory |
| `mem:relations` | `relations.rs` module | ✅ |
| `mem:profiles` | `profiles` table via `project_scanner` | ⚠️ Different |
| `mem:claude-bridge` | `claude_bridge.rs` (file-based) | ⚠️ Different |
| `mem:graph:nodes` | `entities` table | ✅ |
| `mem:graph:edges` | `triples` table | ✅ |
| `mem:semantic` | Insight/crystal system | ⚠️ Different |
| `mem:procedural` | Lesson/routine system | ⚠️ Different |
| `mem:team:{teamId}:shared` | `team_shares` table | ✅ |
| `mem:audit` | `audit` table | ✅ |
| `mem:actions` | `actions` table | ✅ |
| `mem:action-edges` | `action_dependencies` table | ✅ |
| `mem:leases` | `leases` table | ✅ |
| `mem:routines` | `routines` table | ✅ |
| `mem:routine-runs` | In-memory tracking | ⚠️ Missing |
| `mem:signals` | `signals` table | ✅ |
| `mem:checkpoints` | `checkpoints` table | ✅ |
| `mem:mesh` | `mesh.rs` in-memory | ⚠️ Different |
| `mem:sketches` | `sketches` table | ✅ |
| `mem:facets` | `facets` table | ✅ |
| `mem:sentinels` | `sentinels` table | ✅ |
| `mem:crystals` | `crystals` table | ✅ |
| `mem:lessons` | `lessons` table | ✅ |
| `mem:insights` | `insights` table | ✅ |
| `mem:enriched:{sessionId}` | `enrich.rs` module | ⚠️ In-memory |
| `mem:latent:{obsId}` | `image_embeddings` table | ✅ |
| `mem:retention` | Retention scoring via `retention.rs` | ⚠️ Different |
| `mem:access` | `access_tracker.rs` | ✅ |
| `mem:image-refs` | `image_refs` table | ✅ |
| `mem:slots` | `slots` table | ✅ |
| `mem:slots:global` | `slots` table with scope | ✅ |
| `mem:commits` | `commits` tracking via `replay.rs` | ⚠️ Partial |

### 5.2 Missing State Systems

| System | agentmemory | mempalace_rust | Priority |
|---|---|---|---|
| BM25 index persistence | ✅ `index-persistence.ts` | ❌ In-memory only | P1 |
| Vector index persistence | ✅ | ⚠️ Basic (via `embed.rs` manifest) | P2 |
| Session replay timeline | ✅ `replay/timeline.ts` + `replay/jsonl-parser.ts` | ⚠️ Basic (`replay.rs` 240 LOC) | P2 |
| Metrics store | ✅ `eval/metrics-store.ts` | ❌ Not implemented | P3 |
| State snapshot | ✅ `mem:state` | ⚠️ Palace snapshot only | P3 |

---

## 6. Missing Modules (Internal Functions)

### 6.1 Complete Module Mapping

| agentmemory function | mempalace_rust module | Status | Lines |
|---|---|---|---|
| `access-tracker.ts` | `access_tracker.rs` | ✅ | 154 |
| `actions.ts` | `coordination/actions.rs` | ✅ | 307 |
| `audit.ts` | `audit.rs` | ✅ | 220 |
| `auto-forget.ts` | `auto_forget.rs` | ✅ | 328 |
| `branch-aware.ts` | ❌ Missing | ❌ | — |
| `cascade.ts` | ❌ Missing | ❌ | — |
| `checkpoints.ts` | `coordination/checkpoints.rs` | ✅ | 271 |
| `claude-bridge.ts` | `claude_bridge.rs` | ✅ | 232 |
| `compress-file.ts` | `compress_file.rs` | ✅ | 176 |
| `compress-synthetic.ts` | `compress_synthetic.rs` | ✅ | 862 |
| `compress.ts` | `compress.rs` | ✅ | ~200 |
| `consolidate.ts` | `consolidation.rs` | ✅ | ~150 |
| `consolidation-pipeline.ts` | `consolidation_pipeline.rs` | ✅ | 342 |
| `context.ts` | `context.rs` | ✅ | 317 |
| `crystallize.ts` | `crystallize.rs` | ✅ | 234 |
| `dedup.ts` | `dedup.rs` + `dedup_window.rs` | ✅ | ~450 |
| `diagnostics.ts` | `doctor.rs` | ✅ | 758 |
| `disk-size-manager.ts` | `export/disk_size_manager.rs` | ⚠️ Exists but not wired to REST | 104 |
| `enrich.ts` | `enrich.rs` | ✅ | 232 |
| `evict.ts` | `evict.rs` | ✅ | 404 |
| `export-import.ts` | `export/export_import.rs` | ✅ | 275 |
| `facets.ts` | `facets.rs` | ✅ | ~150 |
| `file-index.ts` | `file_index.rs` | ✅ | 197 |
| `flow-compress.ts` | ❌ Missing | ❌ | — |
| `frontier.ts` | `coordination/frontier.rs` | ✅ | 143 |
| `governance.ts` | `governance.rs` | ✅ | 237 |
| `graph-retrieval.ts` | `graph_retrieval.rs` | ✅ | 224 |
| `graph.ts` | `knowledge_graph.rs` + `graph_extraction.rs` | ✅ | 1947+1190 |
| `image-quota-cleanup.ts` | `vision/image_quota.rs` | ✅ | 164 |
| `image-refs.ts` | `vision/image_refs.rs` | ✅ | 133 |
| `leases.ts` | `coordination/leases.rs` | ✅ | 257 |
| `lessons.ts` | `lessons.rs` | ✅ | ~150 |
| `mesh.ts` | `coordination/mesh.rs` | ✅ | 299 |
| `migrate-vector-index.ts` | `migrate.rs` | ✅ | 231 |
| `migrate.ts` | `migrate.rs` | ✅ | 231 |
| `observe.ts` | `observe.rs` | ✅ | 333 |
| `obsidian-export.ts` | `obsidian_export.rs` | ✅ | ~150 |
| `patterns.ts` | `patterns.rs` | ✅ | 240 |
| `privacy.ts` | `privacy.rs` | ✅ | 648 |
| `profile.ts` | `profile.rs` | ✅ | 248 |
| `query-expansion.ts` | `query_sanitizer.rs` | ⚠️ Different — sanitization vs expansion | 169 |
| `reflect.ts` | `reflect.rs` | ✅ | 369 |
| `relations.ts` | `relations.rs` | ✅ | 213 |
| `remember.ts` | `miner.rs` + `diary_ingest.rs` | ⚠️ Different model | 1816 |
| `replay.ts` | `replay.rs` | ⚠️ Basic (240 LOC vs TS complexity) | 240 |
| `retention.ts` | `retention.rs` | ✅ | ~150 |
| `routines.ts` | `coordination/routines.rs` | ✅ | 284 |
| `search.ts` | `searcher.rs` | ✅ | 586 |
| `sentinels.ts` | `sentinels.rs` | ✅ | 265 |
| `signals.ts` | `coordination/signals.rs` | ✅ | 256 |
| `sketches.ts` | `sketches.rs` | ✅ | ~150 |
| `skill-extract.ts` | `skill_extract.rs` | ✅ | 219 |
| `sliding-window.ts` | `sliding_window.rs` | ✅ | 189 |
| `slots.ts` | `slots.rs` | ✅ | 192 |
| `smart-search.ts` | `searcher.rs` (partial) | ⚠️ No progressive disclosure | 586 |
| `snapshot.ts` | `export/snapshot.rs` | ✅ | 190 |
| `summarize.ts` | `summarize.rs` | ✅ | 198 |
| `team.ts` | `coordination/team.rs` | ✅ | 329 |
| `temporal-graph.ts` | `temporal_graph.rs` | ✅ | 261 |
| `timeline.ts` | `timeline.rs` | ✅ | 195 |
| `verify.ts` | `verify.rs` | ✅ | 236 |
| `vision-search.ts` | `vision/vision_search.rs` | ✅ | 285 |
| `working-memory.ts` | `working_memory.rs` | ✅ | 196 |

### 6.2 Missing Modules (3)

| Missing Module | agentmemory file | Why Needed | Implementation Notes |
|---|---|---|---|
| **Branch-aware** | `src/functions/branch-aware.ts` | Detect git branches, worktrees, branch-specific sessions | Needs `git` command integration, branch detection, branch-scoped session queries |
| **Cascade update** | `src/functions/cascade.ts` | Cascading updates across related memories/actions | Triggered when entity changes propagate to dependents |
| **Flow compression** | `src/functions/flow-compress.ts` | Context window management via sliding window | Compress old context while preserving key decisions |

### 6.3 Partial Implementations

| Module | What's Missing | Priority |
|---|---|---|
| `replay.rs` | `timeline.ts` + `jsonl-parser.ts` not fully ported. Missing replay viewer. | P2 |
| `smart_search` | No progressive disclosure (`expand_ids` expansion). No hybrid semantic+keyword MCP tool. | P1 |
| `context.rs` | No prompt template system (agentmemory has `context.ts` + prompt injection). Missing MCP prompt endpoints. | P2 |
| `query_sanitizer` | agentmemory has `query-expansion.ts` with synonym expansion. mempalace has sanitization only. | P2 |
| `disk_size_manager` | Module exists (104 LOC) but not wired to REST or background. | P3 |

---

## 7. Benchmarks

### 7.1 Coverage

| Benchmark | agentmemory | mempalace_rust | Status |
|---|---|---|---|
| LongMemEval-S R@5/10/MRR | ✅ `eval/runner/longmemeval.ts` | ✅ `crates/bench/src/bin/longmemeval-bench.rs` | ✅ Both |
| Load 100k | ✅ `benchmark/load-100k.ts` | ❌ Missing | P2 |
| Quality eval | ✅ `benchmark/quality-eval.ts` | ❌ Missing | P3 |
| Scale eval | ✅ `benchmark/scale-eval.ts` | ❌ Missing | P3 |
| Real embeddings eval | ✅ `benchmark/real-embeddings-eval.ts` | ❌ Missing | P3 |
| Criterion benches | ❌ | ✅ `bench_search.rs`, `bench_palace_db.rs`, `bench_miner.rs` | 🆕 Rust-native |

### 7.2 Missing Benchmark Suites

| Suite | Description | Implementation Notes |
|---|---|---|
| `load-100k.ts` | Synthetic load testing with 100k memories. Tests throughput, latency, memory usage. | Uses `memories` stress, should port to Rust |
| `quality-eval.ts` | Quality scoring against ground truth. Tests precision/recall. | Needs evaluation harness + ground truth dataset |
| `scale-eval.ts` | Scale testing with increasing dataset sizes. Tests $O(\log N)$ retrieval. | Tests vector index scaling |
| `real-embeddings-eval.ts` | Tests with real embeddings (not synthetic). Evaluates embedding provider quality. | Tests actual FastEmbed/model2vec/tract quality |

---

## 8. Viewer / Web UI

agentmemory has a full HTML viewer at `http://localhost:3113` (port = restPort + 2). mempalace_rust has **no viewer**.

**agentmemory viewer features:**
- Session graph visualization (viewer-server.ts)
- Memory timeline with search
- Graph stats dashboard
- Real-time SSE updates (viewer-group stream)
- CSP security headers with nonce

**mempalace_rust status:** None. The `sse_transport.rs` (86 LOC) provides SSE infrastructure but no viewer endpoint.

---

## 9. MCP Server Architecture

### 9.1 Comparison

| Aspect | agentmemory | mempalace_rust |
|---|---|---|
| Transport | iii-sdk WebSocket + stdio | rmcp stdio + axum HTTP |
| MCP handler | `mcp/server.ts` switch on 53+ cases | `mcp_server.rs` switch on 85+ handler fns |
| Tool definition | `tools-registry.ts` declarative | Inline in `mcp_server.rs` (7000+ LOC switch) |
| Standalone mode | ✅ `mcp/standalone.ts` (7 tools, InMemoryKV) | ✅ `mcp/standalone.rs` (in-memory store) |
| REST shim | ✅ Via `server.ts` `mcp::tools::call` | ✅ `rest_api.rs` separate (1343 LOC) |
| MCP resources | 6 resources | 0 |
| MCP prompts | 3 prompts | 0 |
| WAL logging | ✅ Via `append_wal_entry()` in server.ts | ✅ Via `append_wal_entry()` in mcp_server.rs |
| Protocol | MCP 2024-11-05 | MCP via rmcp crate |

### 9.2 MCP Tool Naming Duality

mempalace_rust implements **both** naming conventions:

1. **`mempalace_*`** — 73 tools in `tools-registry` array + switch cases
2. **`memory_*`** — 76 aliases all mapped to `mempalace_*` handlers via alias pairs like:
   ```rust
   "memory_search" | "memory_recall" => tool_search(&state, args),
   "memory_save" | "memory_diary_write" => tool_save(&state, args),
   ```

This allows both `mempalace_search` and `memory_recall` to work as tool names, providing compatibility with both naming schemes.

---

## 10. Prompt Templates

| agentmemory (`src/prompts/`) | mempalace_rust (`crates/core/src/prompts/`) | Lines |
|---|---|---|
| `compression.ts` (67 LOC) | `compression.rs` (161 LOC) | ✅ More complete |
| `consolidation.ts` (48 LOC) | `consolidation.rs` (148 LOC) | ✅ More complete |
| `graph-extraction.ts` (35 LOC) | `graph_extraction.rs` (120 LOC) | ✅ More complete |
| `reflect.ts` (55 LOC) | ❌ Missing — `reflect.rs` uses inline `REFLECT_SYSTEM_PROMPT` | ⚠️ Not modular |
| `summary.ts` (87 LOC) | `summarize.rs` has `SUMMARIZE_SYSTEM_PROMPT` inline | ⚠️ Not modular |
| `vision.ts` (8 LOC) | `vision.rs` (29 LOC) | ✅ More complete |
| `xml.ts` (26 LOC) | `xml.rs` (160 LOC) | ✅ More complete |

**Missing prompt module:** `prompts/reflect.rs` and `prompts/summary.rs` — these are implemented inline in `reflect.rs` and `summarize.rs` instead of as separate modules.

---

## 11. Background / Auto-Management

agentmemory has automatic background tasks started in `src/index.ts`:

| Task | agentmemory | mempalace_rust |
|---|---|---|
| Auto-forget (every 60min) | ✅ `auto-forget.ts` timer | ✅ `background.rs` (60 min interval) |
| Auto-consolidation (every 2h) | ✅ `consolidation-pipeline.ts` timer | ⚠️ Placeholder in `background.rs` (120 min interval) |
| Lesson decay (daily) | ✅ `lessons.ts` | ⚠️ Logs but doesn't execute in `background.rs` |
| Insight decay (daily) | ✅ `insights.ts` | ⚠️ Logs but doesn't execute in `background.rs` |
| Health monitor | ✅ `health/monitor.ts` | ❌ Not implemented |
| Index persistence | ✅ `state/index-persistence.ts` | ✅ Basic via `embed.rs` manifest |
| Dedup map persistence | ✅ `state/dedup.ts` | ✅ `dedup_window.rs` |

---

## 12. Consolidated Gap List

### By Priority

#### P0 — Must Have (core parity blocker)

| Gap | Description | Fix |
|---|---|---|
| **10 missing hooks** | Only `save` + `precompact` exist; session-start/end, post-tool-use, etc. missing | Implement 10 hook scripts in `hooks_cli.rs` pattern |
| **58 missing REST endpoints** | Half the REST API is unimplemented | Add to `rest_api.rs` — especially `observe`, `summarize`, `session/start`, `session/end` |
| **MCP Resources (6)** | 0 resources vs 6 in agentmemory | Add `mcp::resources::list` + `mcp::resources::read` handlers |
| **MCP Prompts (3)** | 0 prompts vs 3 in agentmemory | Add `mcp::prompts::list` + `mcp::prompts::get` handlers |

#### P1 — Should Have (feature parity)

| Gap | Description | Fix |
|---|---|---|
| **7 missing embedding providers** | No Anthropic/OpenRouter/Cohere/Gemini/Voyage/Minimax/CLIP | Add provider implementations |
| **Progressive disclosure smart_search** | No `expand_ids` expansion, no separate `smart_search` MCP tool | Add to `searcher.rs` + add `mempalace_smart_search` MCP |
| **BM25 index persistence** | In-memory only | Implement persistence in `bm25.rs` |
| **2 missing LLM providers** | No agent-sdk, openrouter, minimax | Add to `llm/` |
| **3 missing function modules** | `branch-aware`, `cascade`, `flow-compress` | Implement new modules |
| **Replayer timeline** | Basic only — no `timeline.ts` + `jsonl-parser.ts` | Port full replay system |
| **`memory_claude_bridge_sync` MCP tool** | Only REST endpoint | Add MCP tool wrapper |

#### P2 — Nice to Have (feature quality)

| Gap | Description | Fix |
|---|---|---|
| **Viewer web UI** | None — agentmemory has full HTML viewer | Implement viewer server + HTML template |
| **load-100k benchmark** | Missing load testing | Port to Rust |
| **Query expansion** | Has sanitization, not expansion with synonyms | Add `query-expansion.ts` equivalent |
| **Metrics store** | Not implemented | Add `eval/metrics-store.ts` equivalent |
| **Session replay timeline** | Basic JSONL only | Add viewer + full timeline |
| **Disk size manager REST** | Module exists, not wired | Wire to REST + health endpoint |

#### P3 — Future (nice to have)

| Gap | Description |
|---|---|
| **Quality/scale/real-embeddings eval** | Missing benchmark suites |
| **State snapshot persistence** | Only palace snapshot, not full `mem:state` |
| **`mcp::prompts::get` argument handling** | Arguments `task_description`, `session_id`, `project` not implemented |
| **Routine runs tracking** | `mem:routine-runs` KV not implemented in SQLite |

---

## 13. Lines of Code Comparison

| Layer | agentmemory | mempalace_rust |
|---|---|---|
| Source code | 36,503 LOC | 66,831 LOC (core) + 2,792 (cli+bench) = **69,623 total** |
| Tests | 27,727 LOC (121 test files) | ~10,000 LOC estimated (1,063 tests in source) |
| Benchmarks | 2,584 LOC | 2,338 LOC |
| **Total** | **~67,000 LOC** | **~72,000 LOC + tests** |

mempalace_rust is already larger than agentmemory in source code (~69k vs ~36k), reflecting the Rust/TypeScript density difference and the palace metaphor's additional organizational complexity.

---

## 14. Approved Deviations (Architecture Differences)

These are intentional differences that do NOT block parity:

| Deviation | agentmemory | mempalace_rust | Reason |
|---|---|---|---|
| Embedder backend | ChromaDB + HNSW | Embedvec + Usearch | Rust-native, no C++ dependency |
| WAL design | iii-engine file-based | SQLite transactions | Different crash-recovery model |
| Memory model | Flat sessions/observations/memories | Palace hierarchy (wings/rooms/drawers) | Different organizational metaphor |
| State engine | iii-sdk (Node, external process) | Direct rusqlite | No external dependency |
| Vector index | In-memory + persistence | Embedvec (SQLite-adjacent) | Different storage architecture |
| Hook runtime | Node.js scripts + HTTP | Rust CLI (`mpr hook`) | Different language/runtime |
| MCP transport | iii-sdk stdio + HTTP | rmcp + axum | Different Rust crates |
| Provider architecture | Provider class hierarchy | Trait-based (`LlmProvider`, `Embedder`, `MemoryProvider`) | Idiomatic Rust |

---

## 15. Test Coverage Comparison

| Area | agentmemory | mempalace_rust |
|---|---|---|
| Test files | 121 `.test.ts` files | 95 modules with `#[test]` |
| Test count | ~1,293 test cases | 1,063 `#[test]` cases |
| Test LOC | 27,727 LOC | ~10,000 LOC (estimated) |
| Parity tests | ✅ `test/parity_tests.ts` | ✅ `crates/core/tests/parity_tests.rs` |
| Integration tests | ✅ `test/integration.test.ts` | Part of main suite |
| Benchmark harnesses | 4 (`benchmark/`) | 4 (`crates/bench/` + `crates/core/benches/`) |
| Golden fixture tests | ✅ LongMemEval | ✅ LongMemEval |

---

## Appendix A: Full Tool List

### agentmemory 53 tools:
```
memory_action_create        memory_checkpoint_list     memory_governance_delete   memory_lesson_recall       memory_patterns
memory_action_update        memory_checkpoint_resolve  memory_graph_query         memory_lesson_save         memory_profile
memory_audit                memory_commit_lookup       memory_heal                memory_list                memory_recall
memory_checkpoint           memory_commits             memory_insight_list        memory_mesh_sync           memory_reflect
```

### mempalace_rust 73 `mempalace_*` + 53 `memory_*` aliases = 87 unique tool names:

All 53 agentmemory tools mirrored as `memory_*` aliases + 20 unique palace tools (`mempalace_add_drawer`, `mempalace_delete_drawer`, `mempalace_kg_*`, `mempalace_list_*`, `mempalace_*_build`, `mempalace_*_stats`, `mempalace_traverse`, `mempalace_working_memory`, `mempalace_access_stats`, `mempalace_check_duplicate`, `mempalace_detect_worktree`, `mempalace_get_aaak_spec`, `mempalace_get_taxonomy`, `mempalace_replay_import`, `mempalace_retention_score`).

---

## Appendix B: Quick Reference — What to Implement Next

```
Priority  Filename                   Lines  What to do
────────────────────────────────────────────────────────────────────────
P0        hooks_cli.rs               +400   Add session-start, session-end, 
                                          post-tool-use, post-tool-failure, 
                                          prompt-submit, notification, 
                                          subagent-start/stop, task-completed

P0        rest_api.rs               +500   Add /observe, /session/start, 
                                          /session/end, /summarize, /forget,
                                          /remember, /liveness, /memories

P0        mcp_server.rs             +150   Add mcp::resources::list/read,
                                          mcp::prompts::list/get handlers

P1        vision/embedding_provider.rs +200  Add CLIP embed_image implementation

P1        searcher.rs               +150   Add progressive disclosure smart_search,
                                          expand_ids, hybrid BM25+vector MCP tool

P1        llm/anthropic_provider.rs +100   Add embed_text (Anthropic embeddings)

P1        rest_api.rs               +300   Add branch-aware, cascade, flow-compress

P2        viewer/                   NEW    Implement HTML viewer + SSE stream

P2        bm25.rs                   +80    Add index persistence to disk

P2        context.rs                +100   Modular prompt templates + inject

P2        replay.rs                 +200   Port timeline.ts + jsonl-parser.ts

P3        benchmark/load-100k.rs     NEW    Port load-100k.ts benchmark
```

