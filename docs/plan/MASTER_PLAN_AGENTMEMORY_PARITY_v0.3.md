# Master Plan: agentmemory Post-Migration Parity (mempalace_rust v0.3.0)

**Created:** 2026-06-10
**Owner:** tranquangdang21
**Tracked by:** `br` epic `mr-mp-105-agentmemory-post-merge`
**Status:** Draft → In Progress

---

## 1. Context & Baseline

| Item | Value |
|---|---|
| mempalace_rust current | v0.2.0 (released 2026-06-07) |
| agentmemory baseline (migration) | v0.9.27 (released 2026-06-07) |
| agentmemory today | v0.9.27 + 8 post-merge commits (HEAD `a842ade`) |
| agentmemory source | https://github.com/rohitg00/agentmemory |
| Local clone for diffing | `/tmp/agentmemory` |
| Parity model | mempalace_rust ≈ 90-95% per `REMAINING.md`; this plan closes the post-merge delta |
| Existing closed epic | `mr-agentmemory-parity-m937` (v0.2.0 migration) — DO NOT confuse |

The closed epic `mr-agentmemory-parity-m937` covered the v0.2.0 migration (53+ MCP tools, coordination, smart features). This plan covers everything that landed in agentmemory **after** the v0.9.27 migration baseline — features/fixes mempalace_rust has not yet seen.

---

## 2. Scope (What We Are Porting)

### 2.1 Source Commits (post-merge, 2026-06-07 → 2026-06-10)

| Commit | Date | Title | Type | In scope? |
|---|---|---|---|---|
| `45de643` | 2026-06-07 18:36 | Detailed, self-updating skills covering the whole system (#854) | Feature | ✅ |
| `749c280` | 2026-06-07 | Refresh competitor star counts and add benchmark caveat (#855) | Docs | ❌ skip |
| `c504b79` | 2026-06-07 | Add oracleagentmemory to the comparison (#861) | Docs | ❌ skip |
| `a76224f` | 2026-06-08 | Update README.md | Docs | ❌ skip |
| `a688e50` | 2026-06-10 10:29 | Join session summaries into GET /agentmemory/sessions (#882) | Bug fix | ✅ |
| `1a7b5ca` | 2026-06-08 | Serve docs at agent-memory.dev/docs via Mintlify rewrite (#885) | Docs/infrastructure | ❌ skip (N/A in Rust) |
| `c2d836d` | 2026-06-08 | Add opencode connect adapter, single-source the onboarding picker (#883) | Feature | ⚠️ partial (OpenCode already supported, verify adapter parity) |
| `a842ade` | 2026-06-10 | Add Docs link to website navbar (#886) | Docs | ❌ skip |

### 2.2 Source Features in v0.9.25 / v0.9.27 (already shipped at baseline, but verify ported correctly)

From `CHANGELOG.md` v0.9.25 (2026-06-03) — within migration baseline:

| Feature | PR | Rust port status | Action |
|---|---|---|---|
| Sharded BM25/vector index persistence with manifest commit/rollback | #764 (957 LOC) | ❌ Not in `crates/core/src/` (no `Shard*` symbol) | **Phase 6 (heavy)** |
| Smart-search followup-rate diagnostic (OTEL + REST `/diagnostics/followup` + `agentmemory status`) | #786 | ❌ Not in `crates/core/src/` (no `Followup` symbol) | **Phase 4** |
| Cross-provider fallback model resolution (env-driven defaults per provider) | #791 | ⚠️ Verify `crates/core/src/llm/fallback_chain.rs` | **Phase 1.1** |
| Markdown XML fence stripping in `parseSummaryXml` | #791 | ⚠️ Verify `crates/core/src/summarize.rs` | **Phase 1.2** |
| AsyncLocalStorage recursion guard | #782 | N/A (TypeScript) | ❌ skip |
| Obsidian-export 4-layer hardening (null guards, sanitizers, fail-safe sort) | #780 | ⚠️ Verify `crates/core/src/obsidian_export.rs` if exists | **Phase 1.3** |
| `iii-sdk trigger` API migration (`triggerVoid` → `trigger + TriggerAction.Void()`) | #773 | N/A (TypeScript) | ❌ skip |
| pi integration `tool_input`/`tool_output` field name | #772 | N/A (TS) — but verify Rust pi hook in `integrations/pi/` | **Phase 1.4** |
| Global install fallback to `~/.agentmemory/bin/` | #774 | N/A (TS) — but check Rust install.sh | **Phase 1.5** |
| Graph query pagination (`limit`/`offset`, 500-cap ranked by degree, `totalNodes`/`truncated`) | #789 | ⚠️ Verify `crates/core/src/knowledge_graph.rs` REST handler | **Phase 3.1** |

From `CHANGELOG.md` v0.9.27 (2026-06-07) — same day as migration, so partially overlapping baseline:

| Feature | PR | Rust port status | Action |
|---|---|---|---|
| `AGENTMEMORY_AGENT_SCOPE=isolated` enforced on `mem::search` / `memory_recall` / `recall_context` (PR #817 security fix) | #817 | ⚠️ Audit agent isolation in `crates/core/src/mcp_server.rs` | **Phase 1.0 — SECURITY** |
| `POST /agentmemory/graph/snapshot-rebuild` (refuses pre-flight when no snapshot, accepts `force: true`) | v0.9.27 | ❌ Not in Rust REST | **Phase 3.2** |
| `POST /agentmemory/graph/reset` (writes empty snapshot with `resetAt` epoch marker) | v0.9.27 | ❌ Not in Rust REST | **Phase 3.3** |
| `--instance N` CLI flag (multi-daemon port block 100-port offset, max N=50) | #815 | ❌ Not in `crates/cli/` | **Phase 2.1** |
| `GraphSnapshot.topDegrees` (synchronous degree lookup keyed by nodeId) | v0.9.27 | ❌ Not in `crates/core/src/` | **Phase 3.4** |
| `GraphSnapshot.resetAt` (ISO timestamp driving post-reset orphan detection) | v0.9.27 | ❌ Not in `crates/core/src/` | **Phase 3.5** |
| `GraphQueryResult.fromSnapshot` + `warning` (response envelope for cache vs rebuild) | v0.9.27 | ❌ Not in `crates/core/src/` | **Phase 3.6** |
| Multi-instance port collisions fix (REST+1 streams, REST+46023 engine) | #815 | ❌ Not in `crates/cli/src/port.rs` | **Phase 2.2** |
| iii console install version pin (Windows/POSIX path) | v0.9.27 | N/A (TS-only iii ecosystem) | ❌ skip |
| 11 README translations | #675 | N/A (docs-only) | ❌ skip (defer) |
| `npx skills add` hint in `connect` output | #709 | N/A (TS) | ❌ skip |

### 2.3 Adapter Gap (agentmemory: 19, mempalace_rust: 9)

mempalace_rust currently wires 9 adapters in `install.sh`: Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, Droid. AgentMemory wires 19.

| Adapter | Source file | Approach | Effort |
|---|---|---|---|
| `antigravity` | `src/cli/connect/antigravity.ts` (1.1KB) | `mcp_config.json` → platform User dir | S |
| `cline` | `src/cli/connect/cline.ts` (916B) | VSCode-style `mcpServers` block | S |
| `continue` | `src/cli/connect/continue.ts` (5.5KB) | `~/.continue/config.yaml` `mcpServers:` | S |
| `copilot-cli` | `src/cli/connect/copilot-cli.ts` (3.0KB) | LSP-style `Content-Length` framed JSON-RPC + Windows `cmd.exe` wrapper | M |
| `hermes` | `src/cli/connect/hermes.ts` (1.3KB) | Hermes-specific Python plugin manifest | M |
| `kiro` | `src/cli/connect/kiro.ts` (692B) | `~/.kiro/settings/mcp.json` | S |
| `openclaw` | `src/cli/connect/openclaw.ts` (505B) | OpenClaw-specific | M |
| `openhuman` | `src/cli/connect/openhuman.ts` (1.3KB) | OpenHuman-specific | M |
| `qwen` | `src/cli/connect/qwen.ts` (766B) | `~/.qwen/settings.json` | S |
| `warp` | `src/cli/connect/warp.ts` (879B) | Warp-specific | S |
| `zed` | `src/cli/connect/zed.ts` (952B) | `context_servers` (not `mcpServers`) | S |

**Total:** 10 missing adapters (Phase 5).

### 2.4 Out of Scope (deferred to v0.4+)

- Sharded BM25/vector index persistence (957 LOC, 25 tests) — Phase 6, but **eligible for deferral** if v0.3 release date is tight
- AAAK dialect v2
- LongMemEval R@5 reproduction (currently 0.083 vs 0.143 naive, vs 96.6% Python) — ongoing separate work
- OpenCode single-source onboarding picker (already supported in Rust via different path)

---

## 3. Phasing

### Phase 0 — Pre-flight Verification (1 day, ~6 h)

**Goal:** Confirm exact gap matrix. The plan above lists *expected* state — we need file:line proof.

| Task | Files to inspect | Output |
|---|---|---|
| Audit AGENT_SCOPE isolation | `crates/core/src/mcp_server.rs`, `crates/core/src/rest_api.rs` | `phase0_audit_agent_scope.md` |
| Verify sessions REST handler joins summary | `crates/core/src/rest_api.rs` GET `/sessions` route | inline comment or N/A note |
| Verify graph query supports `limit`/`offset`/`totalNodes`/`truncated` | `crates/core/src/knowledge_graph.rs` (or `kg.rs`), `crates/core/src/rest_api.rs` | inline |
| Verify embedder fallback uses per-provider env defaults | `crates/core/src/llm/fallback_chain.rs`, `crates/core/src/llm/mod.rs` | inline |
| Verify obsidian export has null guards | `crates/core/src/obsidian_export.rs` or `crates/cli/src/obsidian_export.rs` | inline |
| Verify pi integration uses `tool_input`/`tool_output` (not `input`/`output`) | `integrations/pi/` and any `crates/core/src/adapters/pi.rs` | inline |
| Verify install.sh handles global `iii` mismatch | `install.sh` (already POSIX; check) | inline |
| Audit existing 8 action skills for parity with agentmemory v0.9.27 SKILL.md format | `plugin/skills/{recall,recap,…}/SKILL.md` | per-skill diff |
| Audit OpenCode adapter against v0.9.27 single-source picker | `install.sh` OpenCode branch, `plugin/opencode/` | inline |

**Exit criteria:** A short `phase0_audit.md` with confirmed gap matrix. Update this plan's "Rust port status" column from "verify" to "✅ present" or "❌ missing at file:line".

---

### Phase 1 — Critical Security & Fixes (3 days, ~24 h) — TARGET v0.3.0-rc1

#### 1.0 SECURITY: AGENT_SCOPE isolation (issue #817 analogue) — **highest priority**
- **Files:** `crates/core/src/mcp_server.rs`, `crates/core/src/rest_api.rs`
- **Spec:** `AGENTMEMORY_AGENT_SCOPE=isolated` (or `MEMPALACE_AGENT_SCOPE=isolated` in Rust) must filter every recall path:
  - `mempalace_smart_search` (search path) — currently unknown
  - `mempalace_recall` / `memory_recall` alias
  - `mempalace_recall_context` prompt
  - Wildcard `agentId: "*"` bypasses; explicit `agentId` pins; isolated mode falls back to env `AGENT_ID`
  - **Fail-closed:** if isolated mode is on and no agent id resolves from any source, throw — do not drop the filter
- **Audit steps:**
  1. `git grep -nE "AGENT_SCOPE|agent_scope|AGENT_ID" crates/`
  2. For each recall tool, check whether it reads `AGENT_ID` env and filters rows
  3. Add tests `crates/core/tests/agent_scope_isolation.rs` covering: 4 tools × 3 modes (shared / isolated-with-env / isolated-no-env-fails-closed) × 2 agents (A/B)
- **Acceptance:** All 24 test cases pass; existing tests still green

#### 1.1 Cross-provider fallback model resolution
- **File:** `crates/core/src/llm/fallback_chain.rs` (or wherever `createFallbackProvider` lives in Rust)
- **Bug:** Each fallback provider must resolve its own env-driven default model, not copy primary's `model`
- **Mapping (Rust analogues):** `OPENAI_MODEL` / `GEMINI_MODEL` / `ANTHROPIC_MODEL` / `MINIMAX_MODEL` / `OPENROUTER_MODEL` (only those in `crates/core/src/llm/`)
- **Acceptance:** Unit test asserts primary=OpenAI fallback=Gemini uses Gemini's env, not OpenAI's

#### 1.2 Markdown XML fence stripping in summary parser
- **File:** `crates/core/src/summarize.rs`
- **Bug:** Some LLM providers wrap structured XML in ` ```xml ... ``` ` fences with pre/postamble
- **Acceptance:** New helper `strip_xml_wrappers(s: &str) -> &str` peels fences + pre/postamble. Unit tests cover 4 fence variants (with/without lang tag, with/without trailing text)

#### 1.3 Obsidian-export null-record hardening
- **File:** search `crates/core/src/obsidian_export.rs` or wherever
- **Bug:** Records missing `id` throw `[object Object]` and escape the whole handler
- **Fix:** Four-layer hardening:
  1. `id` filter (drop records without id)
  2. Safe-array / safe-string / safe-timestamp normalizers
  3. Outer try/catch per record
  4. Fail-safe session sort
- **Acceptance:** Unit test with 1000-record mixed batch where 30% have null `id` → all 1000 logged, none crash

#### 1.4 pi integration `tool_input`/`tool_output` field name
- **File:** `integrations/pi/`, possibly `crates/core/src/adapters/pi.rs`
- **Bug:** pi sends `data.input`/`data.output` but Rust observe.rs reads `data.tool_input`/`data.tool_output`
- **Acceptance:** Add a fixture in `crates/core/tests/fixtures/pi_observation.json` and assert the observation extracts content

#### 1.5 Install fallback for global `iii` mismatch
- **File:** `install.sh`, `install.ps1`
- **Note:** Rust is iii-independent. This is N/A unless mempalace_rust embeds iii. **Skip if N/A.**

#### Phase 1 Exit criteria
- `cargo test --workspace` green
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- All 5 items above have test coverage

---

### Phase 2 — Multi-instance + Sessions Summary (2 days, ~16 h)

#### 2.1 `--instance N` CLI flag
- **Files:** `crates/cli/src/commands/start.rs` (or similar), `crates/cli/src/port.rs`
- **Spec:** `--instance N` picks a 100-port block off base 3111. `N=1` → 3211/3212/3213/49234. `N=2` → 3311/3312/3313/49334. Max N=50.
- **Constraint:** `--port N` (single) and `--instance N` (block) are mutually exclusive
- **Per-port env overrides** still win: `MEMPALACE_STREAM_PORT`, `MEMPALACE_ENGINE_PORT`, `MEMPALACE_ENGINE_URL`
- **Acceptance:** `mpr start --instance 1` starts daemon on 3211 REST / 3212 streams; `mpr status` shows correct ports; `lsof -i :3211` confirms single process

#### 2.2 Multi-instance port collisions fix
- **Files:** `crates/cli/src/port.rs` (or wherever the port-resolution constants live)
- **Fix:** Streams port = REST+1, engine port = REST+46023 (default 3111 → 3112/49134 unchanged; --port 3211 → 3212/49234)
- **Acceptance:** All `--port` values produce consistent port triples; per-port env overrides documented

#### 2.3 `GET /sessions` joins summaries
- **File:** `crates/core/src/rest_api.rs` (find the `GET /sessions` route)
- **Bug:** Sessions returned from KV.sessions never read KV.summaries → summary field missing in response
- **Fix:** Mirror the pattern in `crates/core/src/recall.rs` (or wherever `mempalace_context` does its summary join). Map session.id → `kv.get_summary(id)`, attach if present
- **Acceptance:** End-to-end test: ingest 3 sessions, summarize 2, GET /sessions returns 3 with 2 carrying `summary: {...}`

#### Phase 2 Exit criteria
- `mpr start --instance 1` and `--port 3211` both work, documented in `--help`
- `GET /sessions` regression test green

---

### Phase 3 — Graph Snapshot/Reset (5 days, ~40 h)

#### 3.1 Graph query pagination
- **Files:** `crates/core/src/knowledge_graph.rs`, `crates/core/src/rest_api.rs` (POST `/graph/query` route)
- **Spec:** Accept `limit` (default 500) and `offset`. Rank by node degree. Restrict page edges to in-page endpoints. Response: `{ nodes, edges, totalNodes, totalEdges, truncated }`
- **Acceptance:** Fixture graph 11k+ nodes; `limit=100` returns 100 ranked by degree; `offset=100` returns next 100; `totalNodes` matches truth

#### 3.2 `POST /graph/snapshot-rebuild`
- **File:** `crates/core/src/rest_api.rs` (new route)
- **Spec:**
  - Pre-flight: refuses with `{ success: false, tooLarge: true, totalNodes, ceiling }` when totalNodes > 25_000 and no prior snapshot
  - Strict `force: true` boolean check bypasses the pre-flight refusal
  - Persists `top-degree subgraph` + aggregate counts
- **Acceptance:** Fixture with 100 nodes: rebuild returns `{ success: true, snapshotId, totalNodes, totalEdges }`; fixture with 30_000 nodes without prior snapshot: refuses; with `force: true`: succeeds; subsequent queries return `fromSnapshot: true`

#### 3.3 `POST /graph/reset`
- **File:** `crates/core/src/rest_api.rs` (new route)
- **Spec:** Enumeration-free. Writes empty snapshot with `resetAt: <ISO timestamp>`. Future extracts compare each name-index hit's `createdAt` against `resetAt`; older rows treated as not-found
- **Acceptance:** After reset, query returns 0 nodes; subsequent `mempalace_extract` (or equivalent) creates fresh nodes

#### 3.4 `GraphSnapshot.topDegrees` (synchronous degree lookup)
- **File:** `crates/core/src/knowledge_graph.rs`
- **Spec:** `topDegrees: HashMap<NodeId, usize>` keyed by nodeId. Updated inline on every extract
- **Acceptance:** Re-ranking after edge writes runs sync over `topDegrees` not async kv.get

#### 3.5 `GraphSnapshot.resetAt`
- **File:** `crates/core/src/knowledge_graph.rs`
- **Spec:** ISO timestamp set by `graph/reset`. Drives post-reset orphan detection
- **Acceptance:** After reset, an extract sees a pre-reset node with `createdAt < resetAt` and treats it as orphan

#### 3.6 `GraphQueryResult.fromSnapshot` + `warning`
- **File:** `crates/core/src/knowledge_graph.rs`, `crates/core/src/rest_api.rs`
- **Spec:** Response envelope `{ fromSnapshot: bool, warning: Option<String>, ... }`
- **Acceptance:** Test asserts envelope shape; viewer `/viewer` can render "served from cache" or "rebuild needed" banner

#### Phase 3 Exit criteria
- All 6 sub-items have unit + integration tests
- `crates/core/tests/graph_snapshot.rs` covers happy path + 25k-node refusal + force bypass

---

### Phase 4 — Smart-search Followup Diagnostic (3 days, ~24 h)

- **File:** `crates/core/src/search/smart_search.rs` (or `crates/core/src/recall.rs`)
- **Spec:**
  - When an agent issues a second `smart_search` within `MEMPALACE_FOLLOWUP_WINDOW_SECONDS` (default 30) and the new result set has zero overlap with the prior, count as directional "first results didn't satisfy" signal
  - Surface: OTEL counter `mempalace.smart_search.followup_within_window_total`, `GET /diagnostics/followup` REST endpoint, `mpr status` surface
  - Exclusion: viewer-source requests skip the counter (header `X-Mempalace-Source: viewer`)
- **Acceptance:**
  - Unit test: 2 sequential `smart_search` calls with disjoint result sets within 30s → counter +1
  - Unit test: disjoint result sets after 31s → counter unchanged
  - Unit test: viewer-source request → counter unchanged
  - Integration test: `GET /diagnostics/followup` returns `{ totalFollowups, recentFollowups: [{ at, queryA, queryB, overlap }] }`

---

### Phase 5 — Agent Adapters (5 days, ~40 h)

Per `install.sh` pattern, add 10 new adapters. For each:

| Adapter | Effort | Order |
|---|---|---|
| `antigravity` | S | 1 |
| `cline` | S | 2 |
| `continue` | S | 3 |
| `kiro` | S | 4 |
| `qwen` | S | 5 |
| `warp` | S | 6 |
| `zed` | S | 7 |
| `copilot-cli` | M (LSP-style + Windows wrapper) | 8 |
| `hermes` | M (Python plugin manifest) | 9 |
| `openclaw` | M | 10 |
| `openhuman` | M | 11 |

**Spec (per adapter):**
- Detect: function `detect() -> bool` checks known config paths
- Install: writes `${VAR:-default}` env block (per PR #650 lesson — never break config parse on unset required vars)
- Print `npx skills add` hint for native-skills install (where applicable)
- Test: `crates/cli/tests/connect_<adapter>.rs` with fake config dir

**Acceptance:** All 10 listed in `mpr connect --help`; `mpr connect <adapter>` succeeds in a fixture; no regression in 9 existing adapters

---

### Phase 6 — Sharded Index Persistence (defer to v0.3.1+ unless v0.3 timeline allows)

- **Source:** agentmemory PR #764 (957 LOC, 25 tests by @Rokurolize)
- **Spec:** Large BM25/vector snapshots save as bounded shards under a generation-scoped prefix, with a manifest published only after all shards commit. Rollback on shard-write failure, fail-closed on length mismatch, legacy snapshot load preserved for downgrade compat
- **Risk:** 957 LOC is large. **Recommend defer to v0.3.1** to keep v0.3.0 release tight
- **Acceptance (if shipped in v0.3):** Concurrent `add` + `remove` on a 100k-entry BM25 index; manifest always points at complete shard set; corrupted shard rejected on load

---

### Phase 7 — Skills System (2 days, ~16 h)

**Source:** agentmemory commit `45de643` (PR #854, 1802 insertions/100 deletions)

| Sub-task | Files | Output |
|---|---|---|
| Audit 8 existing action skills vs v0.9.27 SKILL.md format | `plugin/skills/{recall,recap,remember,forget,handoff,commit-context,commit-history,session-history}/SKILL.md` | gap matrix |
| Tiered format: `SKILL.md` < 100 lines, `EXAMPLES.md` (new), shared `_shared/TROUBLESHOOTING.md` | as above | restructured skills |
| Add 7 reference skills: `mempalace-agents`, `mempalace-architecture`, `mempalace-config`, `mempalace-hooks`, `mempalace-mcp-tools`, `mempalace-rest-api`, `write-mempalace-skill` | `plugin/skills/agentmemory-*` analogue, rename to `mempalace-*` | 7 new dirs |
| Add `scripts/skills/generate.ts` (or `scripts/skills/generate.py` if Python tooling) that extracts reference data tables from source | new file | auto-gen |
| Add `scripts/skills/check.ts` (or `.py`) CI guard against drift | new file | CI step |
| Add language identifiers to all opening code fences (markdownlint MD040) | all SKILL.md | docs |
| Correct recap/handoff REST fallback to the right path (avoid `recall` → `not registered route` pitfall) | `_shared/TROUBLESHOOTING.md` | docs |

**Acceptance:** All 8 action skills < 100 lines; 7 reference skills present; `mempalace` not `agentmemory` in every skill (rename); CI step `skills:check` (or `python scripts/skills/check.py`) blocks drift

---

### Phase 8 — LongMemEval Rust R@5 Reproduction (ongoing, separate epic)

- **Status:** Partially measured (0.083/0.143 single-session-user 12/500 vs naive) per `REMAINING.md`
- **Target:** ≥95.2% R@5 (agentmemory), 96.6% (Python mempalace)
- **Out of scope for v0.3.0**; tracked by separate epic `mr-longmemeval-s-baseline-t0ts`

---

## 4. Dependency Graph

```
Phase 0 (audit)
   │
   ├──► Phase 1.0 (SECURITY: AGENT_SCOPE) ──► Phase 1.1 ──► Phase 1.2 ──► Phase 1.3 ──► Phase 1.4
   │                                                                                       │
   │                                                                                       └─► Phase 1.5 (skip if N/A)
   │
   ├──► Phase 2.1 (--instance) ──► Phase 2.2 (port collisions) ──► Phase 2.3 (sessions summary)
   │
   ├──► Phase 3.4 (topDegrees) ──► Phase 3.5 (resetAt) ──► Phase 3.6 (fromSnapshot envelope)
   │                                                              │
   │                                                              ├─► Phase 3.1 (graph pagination)
   │                                                              └─► Phase 3.2 (snapshot-rebuild)
   │                                                                     │
   │                                                                     └─► Phase 3.3 (graph reset)
   │
   ├──► Phase 4 (followup diagnostic)         ── independent
   │
   ├──► Phase 5 (10 adapters)                 ── independent, 11 sub-streams can run in parallel
   │
   ├──► Phase 6 (sharded index)               ── DEFERRED to v0.3.1
   │
   └──► Phase 7 (skills system)               ── independent

Phase 8 (LongMemEval)                         ── separate epic
```

---

## 5. Estimated Total Effort

| Phase | Effort | Critical path? | v0.3.0 ship? |
|---|---|---|---|
| 0 — Pre-flight audit | 6 h | yes (blocks all) | ✅ |
| 1 — Security + fixes | 24 h | yes | ✅ |
| 2 — Multi-instance + sessions | 16 h | yes | ✅ |
| 3 — Graph snapshot/reset | 40 h | yes | ✅ |
| 4 — Followup diagnostic | 24 h | no | ✅ |
| 5 — 10 adapters | 40 h | no | ✅ |
| 6 — Sharded index | 80 h (estimate) | no | ❌ defer to v0.3.1 |
| 7 — Skills system | 16 h | no | ✅ |
| **Total v0.3.0** | **~166 h ≈ 4 weeks** | | |
| 6 (deferred) | +80 h | | v0.3.1 |

---

## 6. Test Strategy

### 6.1 Per-Phase Tests
- Phase 0: Manual audit + `phase0_audit.md` (no code tests)
- Phase 1: 24 isolation test cases + 4 fence tests + obsidian-export 1000-record batch + 5 LLM fallback tests
- Phase 2: `--instance` end-to-end + port-mapping truth table + sessions summary integration
- Phase 3: `crates/core/tests/graph_snapshot.rs` covering 6 sub-features; existing graph tests must remain green
- Phase 4: 5 followup-counter tests + `GET /diagnostics/followup` integration
- Phase 5: 10 adapter install/uninstall round-trip tests + dry-run assertions
- Phase 7: 8 skill format assertions + 7 reference skills presence + CI drift guard

### 6.2 Cross-Cutting Gates
```bash
# All must pass before v0.3.0 tag
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --doc --workspace
cargo test --release --workspace  # longer
bash install.sh --no-auto-mcp  # smoke test
mpr --version  # ensure v0.3.0
mpr status     # with empty palace
mpr init ~/tmp/test-palace && mpr mine ~/tmp/test-palace  # golden path
```

### 6.3 Parity Gate
- Update `REMAINING.md` scorecard with v0.3.0 numbers
- Update `PARITY_REPORT.md` (if exists) with new parity %
- New `docs/plan/POST_MIGRATION_PARITY_v0.3.md` summarizing what landed

---

## 7. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Phase 1.0 AGENT_SCOPE security fix breaks existing recall tests | High | High | Run full test suite in worktree first; gate on `cargo test --workspace` green before merge |
| Phase 3 graph snapshot 25k-node threshold is wrong for Rust's storage backend (SQLite vs iii file KV) | Medium | Medium | Phase 0 audit; tune threshold in `crates/core/src/rest_api.rs` env override `MEMPALACE_GRAPH_SNAPSHOT_CEILING` |
| Phase 5 adapter code paths differ from agentmemory (Rust install.sh vs TS `connect`) | Low | Low | Use existing 9 adapters in `install.sh` as template; minimum viable install = write `mcpServers` block + `${VAR:-default}` env |
| Phase 6 957 LOC scope creep | High | High | **Defer to v0.3.1**; create separate epic |
| Phase 7 skills rename `agentmemory` → `mempalace` breaks existing user references | Low | Low | Keep `agentmemory` alias files in `plugin/skills/agentmemory-*` as deprecated symlinks for 1 release |
| Renaming `agentmemory` references in code/docs | Low | Low | Search-replace `agentmemory` → `mempalace` only in new files; existing files already done in v0.2.0 |

---

## 8. Rollout

1. **Cut `v0.3.0-rc1` tag** after Phase 1 + Phase 2
2. **Cut `v0.3.0-rc2` tag** after Phase 3 + Phase 4
3. **Cut `v0.3.0-rc3` tag** after Phase 5 + Phase 7
4. **Cut `v0.3.0` tag** after full test gate (Section 6.2)
5. **Cut `v0.3.1-rc1` tag** when Phase 6 (sharded index) lands

---

## 9. Definition of Done (v0.3.0)

- [ ] All Phase 0-5 + Phase 7 phases complete
- [ ] Phase 6 deferred to v0.3.1, separate epic open
- [ ] All gates in Section 6.2 green
- [ ] `REMAINING.md` updated with v0.3.0 scorecard
- [ ] `docs/plan/POST_MIGRATION_PARITY_v0.3.md` published
- [ ] `CHANGELOG.md` v0.3.0 entry written
- [ ] Git tag `v0.3.0` cut
- [ ] `install.sh` smoke test green
- [ ] `mpr connect` works for all 19 adapters
- [ ] 5 new REST endpoints (`graph/snapshot-rebuild`, `graph/reset`, `diagnostics/followup`, plus `graph/query` pagination) registered
- [ ] AGENT_SCOPE isolation security test green
- [ ] `mpr status` shows followup counter when smart-search triggered
- [ ] `plugin/skills/` has 8 action + 7 reference skills, all `mempalace`-named, all auto-gen checkable

---

## 10. References

- agentmemory clone: `/tmp/agentmemory`
- Migration baseline epic: `mr-agentmemory-parity-m937` (closed 2026-05-26)
- v0.9.27 release: https://github.com/rohitg00/agentmemory/releases/tag/v0.9.27
- v0.2.0 mempalace_rust: see `CHANGELOG.md` § v0.2.0
- Parity report: `REMAINING.md`, `docs/plan/PARITY_REPORT.md` (if exists)
- Beads workflow: `br create`, `br ready`, `br audit` per AGENTS.md § "Task Tracking"
