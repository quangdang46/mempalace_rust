# `rohitg00/agentmemory` Deep-Dive — Architectural Findings for the MemPalace Upgrade

> Source: <https://github.com/rohitg00/agentmemory> (README + DESIGN.md probe).
> Snapshot: 2026-05-25.
> Stars: 17.7k. Forks: 1.5k. License: Apache-2.0.
> Stack: TypeScript (81 %), built on `iii-engine` v0.11.2 (Node-based runtime).
> Last release: v0.9.21 (May 19, 2026).

This is the **single most-relevant external repo** for the mempalace_rust upgrade. It is the
state-of-the-art "drop-in persistent memory for any coding agent" product — same niche,
mature TypeScript implementation, broad agent compatibility. We adopt several of its ideas;
we deliberately *do not* adopt others.

---

## 1. Headline numbers (verify before citing externally)

| Metric | Claim | Reproducibility |
|---|---|---|
| LongMemEval-S R@5 | **95.2 %** | Reproducer in `eval/` ([benchmark/LONGMEMEVAL.md](https://github.com/rohitg00/agentmemory/blob/main/benchmark/LONGMEMEVAL.md)) |
| LongMemEval-S R@10 | 98.6 % | Same |
| LongMemEval-S MRR | 88.2 % | Same |
| In-house "coding-agent-life-v1" P@5 | 0.578 | Sandbox-reproducible |
| Token cost | ~170k tokens/year (≈ $10 with API embed; $0 with local) | Self-reported |
| Tests | 950+ | Repo CI |
| MCP tools | 53 (51 routed + 2 internal); shim has 7-tool fallback when no server reachable | README §MCP Server |

**Comparison to the bar in our report 02 (snapshot, end-2025):**

| System | LongMemEval-S R@5 |
|---|---|
| **agentmemory** | **95.2 %** |
| Mem0 (Nov 2025 algo) | 94.4 % |
| Vectorize Hindsight | ≥ 90 % (first OSS) |
| MemPalace (Python original) | 96.6 % (vendor) |
| MemPalace (Rust port) | not yet reproduced |

So agentmemory is the **#1 OSS with a public reproducer** and a meaningful margin over Mem0.
The original MemPalace claim of 96.6 % is higher but unverified at the Rust-port level
(see report 02 §14 and our Phase-0 backlog item `mp-003`). Our Phase-5 success criterion
should include "match or beat agentmemory's 95.2 % on the same fixture."

---

## 2. Architecture in 60 seconds

```
                        ┌──────────────────────────┐
                        │  iii-engine (Node-based) │  ← runtime substrate
                        │  Functions / Triggers /  │     (replaces Express, Postgres,
                        │  KV State / Streams /    │      pgvector, SSE, pm2,
                        │  OTEL                    │      Prometheus)
                        └─────────────┬────────────┘
                                      │
            ┌─────────────────────────┼─────────────────────────┐
            │                         │                         │
   ┌────────▼────────┐       ┌────────▼────────┐       ┌────────▼────────┐
   │  Capture        │       │  Memory         │       │  Retrieval      │
   │  ─ 12 hooks      │       │  ─ Working       │       │  ─ BM25          │
   │    (Claude Code) │       │  ─ Episodic      │       │  ─ Vector        │
   │  ─ 6  hooks      │       │  ─ Semantic      │       │  ─ Graph (BFS)   │
   │    (Codex)       │       │  ─ Procedural    │       │  ─ RRF (k=60)    │
   │  ─ 22 hooks      │       │  ─ Knowledge     │       │  ─ Session       │
   │    (OpenCode)    │       │     graph        │       │     diversified  │
   │  ─ Privacy filter│       │  ─ Ebbinghaus    │       │                  │
   │     (secrets)    │       │     decay        │       │                  │
   │  ─ SHA-256 dedup │       │  ─ Auto-forget   │       │                  │
   │     (5-min win)  │       │     (TTL +       │       │                  │
   │                  │       │      contradict.)│       │                  │
   └─────────────────┘       └─────────────────┘       └─────────────────┘
            │                         │                         │
            └─────────────────────────┼─────────────────────────┘
                                      │
                       ┌──────────────▼─────────────┐
                       │  Surfaces                   │
                       │  ─ MCP server (53 tools)   │
                       │  ─ REST (124 endpoints)    │
                       │  ─ Real-time viewer :3113  │
                       │  ─ iii console     :3114   │
                       │  ─ Session replay timeline │
                       │  ─ Plugins (per-agent)     │
                       └────────────────────────────┘
```

The runtime substrate (`iii-engine`) is the single most opinionated design choice. It gives
agentmemory observability, durable queues, KV state, scheduled triggers, hot-pluggable
workers — all "for free." We do **not** want to adopt that substrate (Node-based, not what
mempalace or jcode is built on). We can absorb the *pattern* (memory ops as named functions
emitting OTEL traces) without taking the dependency.

---

## 3. Concrete ideas worth borrowing (ranked)

### 3.1 Auto-capture via lifecycle hooks (P0 — highest ROI for jcode)

agentmemory's defining UX win is "zero manual save/recall calls." It hooks **every** agent
lifecycle event:

| Hook | Captures |
|---|---|
| `SessionStart` | Project path, session ID; **injects** retrieved context (~1–2 K chars) |
| `UserPromptSubmit` | User prompts (after privacy filter) |
| `PreToolUse` | File access patterns, enriched context |
| `PostToolUse` | Tool name, input, output |
| `PostToolUseFailure` | Error context (so future sessions know what *not* to do) |
| `PreCompact` | Re-injects memory before the agent compacts its context |
| `SubagentStart/Stop` | Sub-agent lifecycle |
| `Stop` | End-of-session summary, kicks off graph + slot reflection if enabled |
| `SessionEnd` | Final marker |

Compare to mempalace today: only `mempal_save_hook.sh` and `mempal_precompact_hook.sh`. Two
hooks, no `PreToolUse` / `PostToolUse` / `UserPromptSubmit` / `SessionStart` injection.
That's the missing UX layer.

**Implication for the upgrade plan:** *Add auto-capture hooks for jcode and the standalone
host.* This becomes a new ADR and several new beads issues.

### 3.2 4-tier memory consolidation (Working / Episodic / Semantic / Procedural) (P1)

Inspired by sleep-stage consolidation in cognitive science. Each tier has a different
lifecycle:

| Tier | What | Decay/Promotion |
|---|---|---|
| Working | Raw observations from PostToolUse | Promoted to Episodic on `Stop` |
| Episodic | Compressed session summaries | Decays unless reinforced |
| Semantic | Extracted facts and patterns (KG) | Bi-directionally synced with `MEMORY.md` |
| Procedural | Workflows, decision patterns | Reinforced when the agent re-uses the pattern |

This is **finer-grained than mempalace's halls** (`hall_facts`, `hall_events`, etc.). Halls
are write-time categories; tiers are *lifecycle* categories. They are **complementary**:
- Halls = where it goes (the room).
- Tiers = what stage of memory it's in (raw → consolidated → semantic).

**Implication:** add a `tier: WorkingMemory | Episodic | Semantic | Procedural` field to
`Drawer`, with a state machine for promotion and decay. Maps cleanly to MemoryBank
(report 02 §5) and Generative Agents (02 §6) — we're consolidating two prior architecture
ideas into one mechanism.

### 3.3 Reciprocal Rank Fusion (RRF, k=60) + session diversification (P1)

Confirms our ADR-2 hybrid-search plan. agentmemory specifically uses:
- BM25 + Vector + Graph (entity-match BFS) → three rank lists
- RRF with `k=60`
- "Session-diversified": **at most 3 results per session** in the final top-K

Session diversification is something we hadn't called out — without it, "find me what we
talked about with X" returns 10 hits all from the same one productive session and hides
prior context. mempalace today has wing/room as filters but no per-session cap.

**Implication:** add `max_per_session: usize` to `SearchScope` (default 3); apply after
fusion.

### 3.4 Privacy filter at ingest (P0)

agentmemory strips **API keys, secrets, and `<private>` tags** before storage. mempalace
today stores everything verbatim — which is by design ("never lose anything"), but is a
**security blocker for jcode**: jcode runs in user terminals and tool-result blocks routinely
contain `OPENAI_API_KEY=sk-...` after a misclick.

**Implication:** new ADR + module `crates/core/src/privacy.rs`. Strip well-known secret
patterns (`OPENAI_*`, `ANTHROPIC_*`, `AWS_*`, `GH_*`, JWTs, bearer tokens, RSA keys, etc.)
at `add_drawer`. Configurable allow-list. *Drawer content* is the verbatim source minus
secrets; the original is not stored anywhere by default.

### 3.5 SHA-256 dedup window (P1)

Before storing an observation, SHA-256 it and check a 5-minute rolling window. Skip if seen.
mempalace has dedup but it's offline (`sweeper.rs`, runs over a whole palace). Online
windowed dedup is a small addition.

**Implication:** `Palace::add_drawer` gains an in-memory LRU of recent hashes; configurable
window (default 300s).

### 3.6 Multi-agent leases / signals / actions / routines (P2 — defer)

agentmemory exposes:
- `memory_lease` — exclusive action leases for multi-agent coordination.
- `memory_signal_send` / `memory_signal_read` — inter-agent messaging with receipts.
- `memory_action_create` / `memory_action_update` / `memory_frontier` / `memory_next` —
  work-item DAG with dependencies and "next ready" semantics.
- `memory_routine_run` — instantiate a workflow.
- `memory_sentinel_create` — event-driven watchers.

This **overlaps massively with jcode's MCP Agent Mail / Beads workflow** documented in
`AGENTS.md`. We should NOT re-implement leases/signals/actions in mempalace; we should let
jcode's existing tools own that surface. mempalace just stores the data.

**Implication:** *not an upgrade item.* We document the boundary in `docs/integration_jcode.md`
(Phase 6).

### 3.7 Citation provenance (P1)

Every memory traces back to source observations. agentmemory exposes `memory_verify` to
walk the chain.

mempalace today has `provenance: Option<DrawerId>` on triples but no equivalent for
drawer-level reasoning ("this AAAK summary was synthesised from drawers X, Y, Z"). This is
a small addition: `Drawer.derived_from: Vec<DrawerId>`.

**Implication:** new field on `Drawer`; populate during AAAK compression and general
extraction.

### 3.8 Real-time viewer + iii console (DEFER)

agentmemory ships a viewer on `:3113` (live observation stream, session explorer, KG
visualisation) and an iii console on `:3114` (OTEL traces, KV browser, function invocation).
mempalace's website is a docs site, not a viewer.

For jcode integration, a viewer is **redundant** — jcode has its own TUI memory widget
(`info_widget_memory_render.rs`). Standalone users might want one, but it's a Phase 7+
follow-up (post-1.0).

### 3.9 Bi-directional `MEMORY.md` sync (DEFER)

agentmemory keeps a Markdown file in sync with the Semantic tier so users can hand-edit and
the changes flow back. Cute UX, complex to maintain consistency. mempalace's existing
`mpr export --format basic-memory` (in our backlog item `mp-102`) covers the same need
with one-way export. Defer bi-directional sync to v1.x.

### 3.10 Team memory (namespaced shared/private) (DEFER)

Multi-tenant write semantics; explicitly out of scope per our open question 6. Leave to
post-1.0.

---

## 4. Things we deliberately do NOT adopt

### 4.1 iii-engine substrate

It's the right call for a TypeScript product (you get traces / queues / KV / streams for
free), but the cost is:
- Node.js runtime requirement.
- iii-engine v0.11.2 pin (and the upstream is moving to a sandbox model that breaks them).
- Engine binary install per-platform.

mempalace and jcode are Rust single-binary projects. Adopting iii-engine would *reverse*
the entire native-Rust direction of our plan. **Reject.**

We *do* take the architectural pattern: memory operations as named functions, each emitting
OTEL spans. But we implement it with `tracing` + `opentelemetry-rust` directly, not iii.

### 4.2 Node.js MCP shim (`@agentmemory/mcp`)

A small Node package that JSON-RPC-proxies to the running server. mempalace already does
this in pure Rust with `mpr mcp`. Confirms our ADR-4 hybrid integration mode but no code
borrowing.

### 4.3 Claude-subscription fallback (`@anthropic-ai/claude-agent-sdk`)

agentmemory has an opt-in mode that spawns Claude Pro/Max via the agent SDK to do LLM-backed
compression when no API key is present. There's a documented Stop-hook recursion footgun
(`#149` follow-up). Bad idea to copy. mempalace already supports configurable LLM providers
via `MEMPALACE_LLM_*`.

### 4.4 53-tool MCP surface

agentmemory's surface is enormous (`memory_sentinel_*`, `memory_routine_*`, `memory_action_*`,
`memory_signal_*`, `memory_facet_*`, `memory_verify`, `memory_consolidate`, …). mempalace
has 19. We do *not* expand to 53; we **align tool naming with the canonical
`@modelcontextprotocol/server-memory`** so any host already wired for the official server
gets mempalace as a drop-in (already in our backlog as `mp-101`).

The 53-tool surface is what comes from owning the multi-agent coordination layer (4.6
above). We're letting jcode's Agent Mail + Beads own that layer, so we don't need 53 tools.

---

## 5. Updates to the master plan

### 5.1 New ADR-11 — Auto-capture lifecycle hooks

**Context.** agentmemory's defining UX is hook-driven auto-capture; mempalace today only
has `Stop` + `PreCompact`. jcode has its own per-turn pipeline (`memory_agent.rs`) but
no `PostToolUse`/`PreToolUse` capture.

**Options.**
- A. Mempalace ships standalone hook scripts only (current state).
- B. Mempalace exposes an `EventCapture` trait; jcode's agent runtime calls it on every
  tool use; standalone CLI ships hook scripts that invoke `mpr observe ...`.
- C. Mempalace owns its own hook lifecycle in the standalone CLI, plus the trait for
  hosts.

**Decision.** **C.** New `pub trait EventCapture` in `mempalace-core`. Standalone install
ships `SessionStart` / `UserPromptSubmit` / `PreToolUse` / `PostToolUse` / `PreCompact` /
`Stop` hook scripts for Claude Code, Codex, OpenCode (matching agentmemory's surface). jcode
adapter implements `EventCapture` via the existing `memory_agent.rs` runtime — wiring in
Phase 4.

**Consequences.** Adds three new beads issues to Phase 4 (jcode side) and Phase 6 (CLI
hook scripts). Sets up direct UX competition with agentmemory; a user choosing between the
two now has feature parity on auto-capture.

### 5.2 New ADR-12 — Privacy filter at ingest

**Context.** mempalace stores raw verbatim. jcode tool-result blocks regularly contain
secrets (API keys, OAuth tokens, JWTs). Storing them is a P0 security blocker for jcode
adoption.

**Options.**
- A. Don't filter; document it.
- B. Strip well-known patterns; configurable.
- C. Strip + reversible-encrypt with a per-palace key (recoverable by user).

**Decision.** **B.** New `crates/core/src/privacy.rs` module with built-in patterns
(`sk-*`, `gh[opsr]_*`, `xox[bpars]-*`, AWS keys, JWT structure, RSA private blocks, BEGIN
PRIVATE KEY, base64-encoded credentials over 32 chars in `=` boundary contexts). Allow-list
override per palace. Stripped tokens replaced with `<REDACTED:type>` placeholders so AAAK
compression is still meaningful.

**Consequences.** New beads issues in Phase 1 (must land before any jcode integration in
Phase 3). Requires a careful test suite — false positives over-redact, false negatives leak
secrets.

### 5.3 New ADR-13 — Memory tier (Working / Episodic / Semantic / Procedural)

**Context.** agentmemory's 4-tier model is a meaningful UX upgrade and aligns with the
research we surveyed (MemoryBank decay, Generative Agents reflection).

**Options.**
- A. Don't add tiers; keep halls only.
- B. Add tiers as a new Drawer field; promotion/decay handled in sleep-time consolidation.
- C. Replace halls with tiers.

**Decision.** **B.** Tiers and halls are complementary. New `Drawer.tier` field. Promotion
rules are part of Phase 5's sleep-time consolidation worker. Halls stay as the *taxonomic*
axis (what kind of memory); tiers are the *lifecycle* axis (how raw/consolidated).

**Consequences.** Adds one beads issue to Phase 2 (schema field) and one to Phase 5
(promotion logic).

### 5.4 New ADR-14 — Session diversification in retrieval

**Context.** agentmemory caps results at 3 per session post-RRF; mempalace doesn't.

**Options.**
- A. Skip; deliver as-is.
- B. Add `max_per_session: usize` to `SearchScope` (default 3), enforced after fusion.

**Decision.** **B.** One-line addition to `SearchScope`, post-RRF filter.

**Consequences.** One small beads issue in Phase 5.

### 5.5 New beads issues to file (deltas from the original 41)

These are added to the Phase 1 / 2 / 4 / 5 sections of the master plan. They will be
filed alongside the original 41.

| ID | Phase | Pri | Size | Title |
|---|---|---|---|---|
| **mp-031** | 1 | P0 | M | Privacy filter at ingest — strip API keys/JWT/RSA before `add_drawer` (ADR-12) |
| **mp-032** | 1 | P1 | S | SHA-256 5-min dedup window in `Palace::add_drawer` |
| **mp-051** | 2 | P1 | S | Add `tier: MemoryTier` field to `Drawer` (Working/Episodic/Semantic/Procedural) (ADR-13) |
| **mp-052** | 2 | P1 | S | Add `derived_from: Vec<DrawerId>` to `Drawer` for citation provenance |
| **mp-068** | 4 | P0 | M | jcode: implement `EventCapture` trait against `memory_agent.rs` runtime (ADR-11) |
| **mp-069** | 4 | P0 | M | jcode: capture `PostToolUse` and `UserPromptSubmit` events into mempalace |
| **mp-091** | 5 | P1 | M | Tier-promotion logic in sleep-time consolidation (working → episodic → semantic → procedural) |
| **mp-092** | 5 | P1 | S | `SearchScope.max_per_session` (default 3) post-RRF filter (ADR-14) |
| **mp-093** | 5 | P1 | M | Reproduce agentmemory's 95.2 % R@5 LongMemEval-S in our `crates/bench` |
| **mp-105** | 6 | P1 | M | Standalone CLI: ship `SessionStart`/`UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`PreCompact`/`Stop` hook scripts for Claude Code, Codex, OpenCode |
| **mp-106** | 6 | P2 | S | Privacy filter — configurable allow-list and per-pattern severity in `mpr config` |

**Total new issues: 11. Total backlog: 41 + 11 = 52.**

---

## 6. Bottom line for filing beads issues

Two things changed by reading agentmemory's repo:

1. **Auto-capture hooks become a P0 deliverable**, not an afterthought. We bumped the
   relevant items from Phase 6 to Phase 4 (jcode side) so that adopting mempalace doesn't
   *regress* jcode's existing per-turn pipeline.
2. **Privacy filter is a hard prerequisite for jcode adoption.** Without it, jcode's
   security posture is worse with mempalace than without. This is now a Phase 1 P0.

Everything else from agentmemory is either confirmed (RRF fusion, hybrid search, decay) or
correctly out of scope (iii-engine substrate, multi-agent coordination, bi-directional
`MEMORY.md` sync). The original 41-issue plan is intact; we layer 11 new issues on top.

---

*End of report.*
