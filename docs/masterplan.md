# Master Plan: mempalace_rust Full Feature Completion

## Context

Project `quangdang46/mempalace_rust` (v0.6.5) is a Rust rewrite of the `mempalace/mempalace` (Python v3.4.1) memory palace tool. The good news: **~85% of the backend code already exists** across `crates/core/src/` (~72,604 lines, 103+ modules, 12 coordination stores, 12 embedding providers, 7 LLM providers, 8 search modules, 4 vision modules). **All 10 CLI stubs have now been wired to real backends (v0.5.0).** This plan is preserved for historical reference; see CHANGELOG.md for the current state.

This plan covers:
1. Wire 10 CLI stubs to existing backend code
2. Complete MCP tool coverage for all features
3. Add missing features from agentmemory that truly don't exist
4. Comprehensive test suite
5. Polish and hardening

## Phase 1: Wire CLI Stubs → Backend Code (Highest Priority)

**Effort: ~3-5 days** | **Impact: 10 features enabled with 0 new backend code**

Each of these needs a `cmd_*` function in `crates/core/src/cli.rs` that calls already-existing backend modules. Follow the exact same pattern as existing `cmd_*` functions (resolve palace path → open store/db → call backend → print results).

### 1.1 actions — `crates/core/src/coordination/actions.rs`
- **Backend**: `ActionStore` with full CRUD (470 lines)
- **CLI params**: `status: Option<String>`, `limit: usize`
- **Task**: Write `cmd_actions()` that opens `ActionStore::open()` and calls `store.list_actions()`. Parse status string into `ActionStatus` enum.
- **Pattern**: Same as `cmd_sessions()` — open store, query, print table.

### 1.2 frontier — `crates/core/src/coordination/frontier.rs`
- **Backend**: `compute_frontier()` function (157 lines)
- **CLI params**: `agent: Option<String>`, `include_completed: bool`
- **Task**: Write `cmd_frontier()` that opens `ActionStore`, gets actions, calls `compute_frontier()`, prints ranked results sorted by score.

### 1.3 signals — `crates/core/src/coordination/signals.rs`
- **Backend**: `SignalStore` with send/read/list (268 lines)
- **CLI params**: `operation: String` (read/send/list), `to`, `payload`
- **Task**: Write `cmd_signals()` with 3 match arms. New: for `list` need `get_threads()`, for `read` need `read_signals()`.

### 1.4 context — `crates/core/src/context.rs`
- **Backend**: `ContextBuilder` (334 lines) — builds context blocks from slots, working memory, lessons, profiles, session summaries
- **CLI params**: `levels: usize`
- **Task**: Write `cmd_context()` that creates `ContextBuilder::new(DEFAULT_TOKEN_BUDGET)`, loads data from PalaceDb, calls `.build_xml()`, prints result.
- **Depends on**: PalaceDb, ProfileStore, LessonStore, SessionStore, WorkingMemory

### 1.5 snapshot — `crates/core/src/export/snapshot.rs`
- **Backend**: `SnapshotStore` with save/load/list (190 lines)
- **CLI params**: `name: Option<String>`, `with_embeddings: bool`
- **Task**: Write `cmd_snapshot()` — if name given, serialize palace state and call `store.save_state()`; if not, call `store.list_snapshots()`.

### 1.6 import — `crates/core/src/export/export_import.rs`
- **Backend**: `ExportImportStore` with import/export (368 lines)
- **CLI params**: `format: String`, `input: PathBuf`
- **Task**: Write `cmd_import()` — read file, parse per format, call `store.import()`.
- ⚠️ CSV/Markdown parsing may need additional code.

### 1.7 profile — `crates/core/src/profile.rs`
- **Backend**: `ProfileStore` with compute/get/cache (312 lines)
- **CLI params**: `wing: Option<String>`, `refresh: bool`
- **Task**: Write `cmd_profile()` — load observations from PalaceDb, call `store.compute_profile()` if refresh/cache miss.

### 1.8 diagnose — `crates/core/src/doctor.rs`
- **Backend**: `run_doctor()` (758 lines) — 7 health checks
- **CLI params**: `deep: bool`
- **Task**: Write `cmd_diagnose()` — call `run_doctor()` (default) or `run_doctor_with_options()` (deep). Print each `CheckResult`.

### 1.9 forget — `crates/core/src/auto_forget.rs` + `crates/core/src/evict.rs` + `crates/core/src/retention.rs`
- **Backend**: `evaluate_batch()`, `apply_forgetting()`, `select_eviction_candidates()` (407 + 450 + 234 lines)
- **CLI params**: `older_than_days`, `memory_type`, `dry_run`
- **Task**: Write `cmd_forget()` — load memories, apply retention evaluation, optionally persist evictions.

### 1.10 evolve — `crates/core/src/memory_lifecycle.rs`
- **Backend**: `evolve_memory()`, `apply_decay()`, `calculate_retention()`, `promote_tier()` (234 lines)
- **CLI params**: `wing: Option<String>`, `count: usize`
- **Task**: Write `cmd_evolve()` — select oldest/weakest memories, call LLM to refine content, call `evolve_memory()` to create new version, persist.
- **Requires**: Async LLM call (`runtime().block_on(...)`) — pattern already exists in `cmd_consolidate`.

### Implementation Pattern for All cmd_* Functions
```rust
fn cmd_example(palace_arg: Option<&str>, ...params...) -> Result<()> {
    let palace_path = resolve_palace_path(palace_arg)?;
    // open store(s)
    // call backend function
    // print results (table or JSON)
    Ok(())
}
```

**Files to modify:**
- `crates/core/src/cli.rs` — replace 10 stubs with `cmd_*()` calls
- `crates/core/src/cli.rs` (new functions) — add 10 `cmd_*` functions
- May need small additions to existing modules if missing a specific function

## Phase 2: Truly Missing Features

**Effort: ~2-3 days** | **Code truly not present**

### 2.1 recent-searches-sweep
- Agentmemory: `recent-searches-sweep.ts` — periodically cleans old search entries
- **Task**: Add `recent_searches_sweep.rs` ~100 lines, register in background tasks

### 2.2 migrate-vector-index
- Agentmemory: `migrate-vector-index.ts` — handles vector index schema migrations
- **Task**: Add `migrate_vector_index.rs` ~200 lines, integrate with `repair` command

### 2.3 Governance (GovernanceManager)
- Source: `crates/core/src/governance.rs` likely doesn't exist (not in agentmemory comparison directly)
- Check agentmemory `governance.ts` (5.6KB)
- **Task**: Add governance module for agent access control ~200 lines
- **Priority**: Low — nice to have

## Phase 3: MCP Tool Completeness

**Effort: ~2-3 days** | **8996-line MCP server needs gaps filled**

The `mcp_server.rs` (8996 lines) already registers ~80+ tools. Audit against existing features:

1. **Cross-reference CLI commands with MCP tools** — every CLI command that has a backend should have an MCP tool equivalent
2. **Tools possibly missing MCP wrappers:**
   - `claude_bridge_sync` — check if exposed
   - `governance_delete` — check if exposed
   - `patterns` / `relations` / `timeline` — partial coverage
   - `elastic_recall` / `smart_recall` — check agentmemory equivalents
3. **Error handling** — ensure all MCP tool handlers return proper `isError` responses
4. **MCP resources** — add any missing resource endpoints (e.g., `mempalace://context/`)

## Phase 4: Test Suite

**Effort: ~4-5 days** | **Critical for quality**

### 4.1 CLI Integration Tests
- `tests/cli_tests.rs` — test each CLI command end-to-end
- Pattern: create temp palace → init → mine fixture → run command → assert output
- Cover all implemented + newly wired commands

### 4.2 MCP Integration Tests
- `tests/mcp_tests.rs` — test MCP tool calls against a running server
- Pattern: create temp palace → start MCP server → send JSON-RPC requests → assert responses
- Test each tool registered in `mcp_server.rs`

### 4.3 Unit Test Gaps
- Current: 155 files with `#[cfg(test)]` but only 2 dedicated test files
- Add focused integration tests in `crates/core/tests/`
- Target: wire up each coordination store test, each export store test
- Aim for continuous test suite that runs in <30s (no LLM calls)

### 4.4 Test Fixtures
- `crates/core/tests/fixtures/` — small sample palace DB, sample config
- Reusable across CLI + MCP + unit tests

## Phase 5: Polish & Hardening

**Effort: ~2-3 days**

### 5.1 Error Messages
- Review all 10 new `cmd_*` functions for actionable error messages
- Pattern: `bail!("mempalace init required first — run `mpr init <dir>` and `mpr mine <dir>`")`

### 5.2 Output Formatting
- Consistent table/JSON output across all commands
- JSON mode (`--json` flag) for machine-readable output
- Colorized tables for human-readable mode

### 5.3 Documentation
- Update `mpr --help` descriptions for newly enabled commands
- Add examples to README.md for each new command
- Update AGENTS.md with current capability matrix

### 5.4 CI/CD
- Add workflow for `cargo test` (all tests except those marked `slow`)
- Add workflow for test coverage reporting
- Verify `cargo clippy` passes on all new code

## Timeline Summary

| Phase | Days | Output |
|-------|------|--------|
| 1. Wire CLI stubs | 3-5 | 10 features enabled, 0 new backend modules |
| 2. Missing features | 2-3 | 2-3 new modules (~500 lines total) |
| 3. MCP completeness | 2-3 | All features exposed via MCP |
| 4. Test suite | 4-5 | 30-50 integration tests, running in CI |
| 5. Polish | 2-3 | Consistent UX, docs, CI pipeline |
| **Total** | **13-19** | **Feature parity with agentmemory ~90%** |

## Key Files to Modify

| File | Change |
|------|--------|
| `crates/core/src/cli.rs` | Replace 10 stubs, add 10 `cmd_*()` functions |
| `crates/core/src/mcp_server.rs` | Add missing MCP tool wrappers |
| `crates/core/tests/cli_tests.rs` | New: CLI integration tests |
| `crates/core/tests/mcp_tests.rs` | New: MCP integration tests |
| `crates/core/src/recent_searches_sweep.rs` | New: missing feature |
| `crates/core/src/migrate_vector_index.rs` | New: missing feature |
| `.github/workflows/ci.yml` | Add test workflows |

## Verification

1. **Phase 1**: `cargo build --release && mpr actions --help` and each new command
2. **Phase 2**: Manual test of new features
3. **Phase 3**: `mpr mcp` shows all tools, `/mcp reconnect` shows all tools in IDE
4. **Phase 4**: `cargo test` — all tests pass, including new integration tests
5. **Phase 5**: `cargo clippy — -D warnings`, `cargo fmt —check`

---

## Appendix: Existing Architecture Overview (for reference)

**103 source files** in `crates/core/src/` (72,604 lines total):

| Module | Files | Lines | Status |
|--------|-------|-------|--------|
| Core palace | palace.rs, palace_db.rs, palace/ | 7,200 | ✅ |
| MCP server | mcp_server.rs | 8,996 | ✅ 
| CLI | cli.rs | 3,585 | ⚠️ 10 stubs |
| Knowledge graph | knowledge_graph.rs, palace_graph.rs, graph_retrieval.rs | 6,804 | ✅ |
| Search | searcher.rs, search/* (9 files) | 4,336 | ✅ |
| Coordination | coordination/* (12 files) | 4,884 | ✅ |
| Embedding | embed/* (12 providers) | ~3,000 | ✅ |
| LLM | llm/* (7 providers) + llm_client.rs, llm_refine.rs, closet_llm.rs | ~3,500 | ✅ |
| Export | export/* (3 files) | 662 | ✅ |
| Vision | vision/* (6 files) | ~1,500 | ✅ |
| Prompts | prompts/* (6 files) | 681 | ✅ |
| Context/Reflect | context.rs, reflect.rs, summarize.rs | 972 | ✅ |
| Lifecycle | memory_lifecycle.rs, auto_forget.rs, evict.rs, retention.rs | 1,325 | ✅ |
| Other | privacy, doctor, profile, health, layers, etc. | ~15,000 | ✅ |
