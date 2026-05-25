#!/usr/bin/env python3
"""
File the 52 mempalace_rust upgrade-plan issues into beads with dependency edges.

Source of truth: docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md
This script:
  1) Calls `br create` per issue, capturing the generated ID.
  2) Adds a "blocks" dependency from every issue in phase N+1 onto a representative
     anchor issue in phase N (the first P0 of that phase, or first listed if no P0).
  3) Writes a slug -> ID mapping to /tmp/mp_ids.json for the user's records.

Run from repo root: python3 scripts/file_beads_upgrade_plan.py
"""

from __future__ import annotations
import json
import shlex
import subprocess
import sys
from pathlib import Path

REPO = Path("/data/projects/mempalace_rust")

# (slug, priority, size, phase, title, description, labels-extra)
# phase is the integer phase 0..6
ISSUES = [
    # ---------- PHASE 0 ----------
    ("mp-001", 0, "S", 0, "Tag mempalace-rust 0.1.0 as pre-upgrade baseline",
     "Tag the current main branch as 0.1.0 to freeze the pre-upgrade state. "
     "All subsequent upgrade work lives behind feature flags or separate branches "
     "until 1.0. See 00_UPGRADE_AND_INTEGRATION_PLAN.md §4 Phase 0.",
     ""),
    ("mp-002", 0, "S", 0, "Add criterion benches for searcher/palace_db/miner",
     "Add criterion benches for `searcher::search_memories`, `palace_db::add`, and "
     "`miner::mine_dir` on a 5k-drawer fixture. Run on CI on every PR. "
     "See plan §4 Phase 0; gap report 04 P2 #18.",
     "perf"),
    ("mp-003", 0, "M", 0, "LongMemEval-S reproducer in crates/bench + capture Rust baseline",
     "Add a reproducer for LongMemEval-S to `crates/bench` and capture the current "
     "Rust port's R@5 / R@10 / MRR. README will quote the Rust number, not the "
     "Python original. See plan §4 Phase 0; report 02 §1306; report 06 §1.",
     "bench"),
    ("mp-004", 0, "S", 0, "Update README + dialect.rs marketing to match code (AAAK is lossy)",
     "Update README from \"30x lossless compression\" to \"lossy summarisation, ~5–10x "
     "token reduction, optimised for LLM readability.\" Aligns marketing with the "
     "existing dialect.rs docstring. ADR-9, report 02 §15.",
     "docs"),
    ("mp-005", 0, "S", 0, "Add #[non_exhaustive] to every public enum and config struct",
     "Mark every public enum / config struct with `#[non_exhaustive]` so future "
     "additions are non-breaking. Plan §4 Phase 0; report 04 P2 #14.",
     "api-stability"),
    ("mp-006", 0, "S", 0, "Mark internal modules #[doc(hidden)]",
     "Hide internals (palace_graph statics, embedding internals, hooks_cli) with "
     "`#[doc(hidden)]` so docs.rs only shows the curated public surface. "
     "Report 04 P2 #19.",
     "api-stability"),
    ("mp-007", 0, "S", 0, "Document current public API in docs/PUBLIC_API.md",
     "Snapshot the existing public surface so Phase 2's `Palace` facade can show a "
     "clean diff. Plan §4 Phase 0.",
     "docs"),

    # ---------- PHASE 1 ----------
    ("mp-010", 1, "M", 1, "Define pub trait Embedder in crates/core/src/embed/mod.rs",
     "Trait shape per ADR-1 / ADR-8: `dim()`, `fingerprint()`, `embed()`, `embed_batch()`. "
     "`async_trait` based; `Send + Sync + 'static`. Report 03 §A; report 05 §A.5.",
     "trait"),
    ("mp-011", 1, "S", 1, "Add NullEmbedder for tests",
     "`NullEmbedder { dim }` returns zero-vectors so tests can compile without an "
     "embedder. Used by trait dyn-compat smoke tests. Report 05 §C.2 PR 1.",
     "testing"),
    ("mp-012", 1, "M", 1, "Implement FastEmbedEmbedder behind feature embed-fastembed",
     "Adapter wrapping `fastembed = \"4\"`. Default feature on. Maps "
     "`MEMPALACE_EMBED_MODEL` env var to `fastembed::EmbeddingModel`. ADR-1; "
     "report 05 §A.4, §C.2 PR 2.",
     "embed"),
    ("mp-013", 1, "M", 1, "Implement Model2VecEmbedder behind feature embed-model2vec",
     "Lightweight static-embedding adapter for low-power machines. Sub-ms inference. "
     "Report 05 §A.4 fallback.",
     "embed"),
    ("mp-014", 1, "M", 1, "Implement TractEmbedder behind feature embed-tract (pure Rust)",
     "Pure-Rust path using `tract-onnx` + `tokenizers` (mirrors `jcode-embedding`). "
     "Required for sandboxed/exotic platforms. Report 05 §A.4 fallback; §C.2 PR 4.",
     "embed"),
    ("mp-015", 1, "S", 1, "Add palace embedding.json manifest (model, dim, fingerprint)",
     "Persist model identity to `<palace>/embedding.json` on first write so future "
     "`Palace::open` calls can validate. ADR-8.",
     "schema"),
    ("mp-016", 1, "M", 1, "Validate embedding.json at Palace::open with actionable error",
     "Fail loud on dim/fingerprint mismatch (\"re-embed with `mpr migrate --re-embed`\"). "
     "Prevents silent vector-space corruption when the user swaps models. ADR-8.",
     "schema"),
    ("mp-017", 1, "M", 1, "Delete onnx_embed.rs + Python subprocess + pip install step",
     "Remove `onnx_embed.rs`, `onnx_embed_python.py`, `__pycache__`, and the "
     "`pip install` step from `install.sh` and `install.ps1`. Single-binary promise "
     "restored. ADR-1; report 04 P0 #4; report 05 §C.2 PR 3.",
     "embed,install"),
    ("mp-018", 1, "S", 1, "Wire MEMPALACE_EMBED_MODEL to fastembed::EmbeddingModel enum",
     "Map env-var values (`bge-small`, `multilingual-e5-small`, etc.) to the "
     "fastembed enum; surface invalid values with a list of accepted names. ADR-1.",
     "config"),
    ("mp-019", 1, "S", 1, "mpr doctor reports active embedder, dim, fingerprint",
     "Add embedder block to `mpr doctor` output: backend, model name, dim, fingerprint, "
     "manifest match status. Plan §4 Phase 1.",
     "doctor"),
    ("mp-031", 1, "M", 1, "Privacy filter at ingest (strip API keys / JWT / RSA / OAuth)",
     "New `crates/core/src/privacy.rs` module. Strip well-known secret patterns "
     "(`sk-*`, `gh[opsr]_*`, `xox[bpars]-*`, AWS keys, JWT structure, BEGIN PRIVATE KEY) "
     "before `add_drawer`. Replace tokens with `<REDACTED:type>`. Configurable allow-list. "
     "ADR-12; report 06 §3.4. Hard prerequisite for jcode adoption.",
     "security,privacy"),
    ("mp-032", 1, "S", 1, "SHA-256 5-min rolling-window dedup in Palace::add_drawer",
     "Online windowed dedup (LRU of recent SHA-256 hashes, default 300s) to skip "
     "duplicate observations within a session. Complements offline `sweeper.rs`. "
     "Report 06 §3.5.",
     "ingestion"),

    # ---------- PHASE 2 ----------
    ("mp-020", 2, "L", 2, "Define MemoryProvider/PalaceStore traits and Palace facade",
     "Public traits and `Palace`/`PalaceBuilder` per plan §3. `MemoryProvider` is the "
     "host-facing surface; `PalaceStore` is the swappable storage; `Embedder` is BYO. "
     "ADR-3; report 03 §A.",
     "trait,api"),
    ("mp-021", 2, "M", 2, "Refactor PalaceDb into JaccardJsonStore + EmbedvecStore impls",
     "Two PalaceStore impls: `JaccardJsonStore` (legacy), `EmbedvecStore` (current "
     "embedvec wrapper). Both behind the trait. ADR-2; report 05 §C.2 PR 5.",
     "refactor"),
    ("mp-022", 2, "M", 2, "Migrate _GRAPH_CACHE/_GRAPH_BUILD_VERSION/SHUTDOWN_REQUESTED off statics",
     "Move from process-global statics to per-`Palace` fields so jcode can hold "
     "multiple palaces concurrently. ADR-7; report 04 P1 #6.",
     "refactor"),
    ("mp-023", 2, "M", 2, "Wrap KnowledgeGraph Connection in Arc<Mutex> for Send+Sync",
     "rusqlite::Connection is `!Sync`. Wrap at the facade layer so callers don't need "
     "to know. ADR-5; report 04 P1 #7.",
     "refactor"),
    ("mp-024", 2, "M", 2, "Move all internal call sites onto Palace facade",
     "searcher.rs, miner.rs, layers.rs, sweeper.rs, convo_miner.rs, mcp_server.rs, "
     "and CLI commands all consume the trait via `Palace`. No semantic change. "
     "Report 04 §2.",
     "refactor"),
    ("mp-025", 2, "M", 2, "Mark old pub mod re-exports #[deprecated]",
     "Keep them re-exported for one minor release with `#[deprecated]` notes pointing "
     "to the new public surface. ADR-3.",
     "deprecation"),
    ("mp-026", 2, "S", 2, "Add crates/sample/ external consumer that proves the public API",
     "External demo crate consumes `Palace::builder().embedder(custom).open()` so "
     "compile failures surface immediately when the API regresses.",
     "testing"),
    ("mp-027", 2, "S", 2, "Cargo features cli/mcp gate clap and rmcp dependencies",
     "Library consumers shed `clap`, `rmcp`, `directories`, `signal-hook` when they "
     "use `default-features = false`. Report 04 P2 #17.",
     "deps"),
    ("mp-028", 2, "S", 2, "Add PalaceConfig (replaces global config reads from library code)",
     "Library never reads `$XDG_CONFIG_HOME` on its own. CLI loads global, passes "
     "`PalaceConfig` into `Palace::open`. ADR-7.",
     "config"),
    ("mp-051", 2, "S", 2, "Add tier: MemoryTier field to Drawer (Working/Episodic/Semantic/Procedural)",
     "Schema field for ADR-13's 4-tier model. Default Working on raw observations, "
     "Episodic on mined session summaries. Promotion logic lands in Phase 5. "
     "Report 06 §3.2.",
     "schema"),
    ("mp-052", 2, "S", 2, "Add derived_from: Vec<DrawerId> to Drawer for provenance",
     "Citation-provenance field. Populated by AAAK compression and general extraction. "
     "Powers a future `mpr verify` command. Report 06 §3.7.",
     "schema"),

    # ---------- PHASE 3 ----------
    ("mp-040", 3, "M", 3, "[jcode] Define pub trait MemoryProvider in jcode-memory-types",
     "Trait shape per report 03 §A. Allows swapping `MemoryManager` for a mempalace-backed "
     "provider behind a config flag. Cross-repo work in /data/projects/jcode.",
     "jcode-side"),
    ("mp-041", 3, "M", 3, "[jcode] Convert MemoryManager -> JcodeLocalProvider impl",
     "Existing `MemoryManager` implements the new trait, renamed to `JcodeLocalProvider`. "
     "No behaviour change. Report 03 §C.",
     "jcode-side,refactor"),
    ("mp-042", 3, "M", 3, "[jcode] AgentsConfig.memory_backend enum + provider() factory",
     "`MemoryBackend::{Local, Mempalace}` with default Local. New `provider()` factory "
     "function returns `Arc<dyn MemoryProvider>`. Report 03 §D.",
     "jcode-side,config"),
    ("mp-043", 3, "M", 3, "[jcode] Migrate 12 MemoryManager::new() call sites to provider() accessor",
     "Touch sites listed in report 03 §C: turn_memory.rs, turn_execution.rs, prompting.rs, "
     "tool/memory.rs, memory_agent.rs, etc.",
     "jcode-side,refactor"),
    ("mp-044", 3, "M", 3, "Create jcode-mempalace-adapter crate (or module)",
     "Adapter type `MempalaceProvider` impls jcode's `MemoryProvider` against `Palace`. "
     "Maps `SearchHit` <-> `MemoryEntry`. ADR-10.",
     "adapter"),
    ("mp-045", 3, "M", 3, "Adapter writes to Local backend, reads through Mempalace (read-only mode)",
     "Phase-3 dark launch: queries flow through mempalace, inserts still go to the "
     "JSON store. Report 03 §F Phase 2.",
     "adapter"),
    ("mp-046", 3, "S", 3, "[jcode] Cargo feature mempalace-backend (off by default)",
     "Feature gate so the adapter ships dark; flip on via `agents.memory_backend = mempalace`.",
     "jcode-side,feature-flag"),

    # ---------- PHASE 4 ----------
    ("mp-060", 4, "M", 4, "Adapter add_drawer/remember/upsert write through to mempalace",
     "Make mempalace authoritative for inserts. Report 03 §F Phase 3.",
     "adapter"),
    ("mp-061", 4, "M", 4, "Adapter extract_from_transcript routes to add_drawer + add_triple",
     "Per-extracted-memory: `Palace::add_drawer` for the content, "
     "`KnowledgeGraph::add_triple` for the entity facts. Report 03 §F Phase 3.",
     "adapter"),
    ("mp-062", 4, "M", 4, "Map jcode episode-feedback to mempalace helpfulness_score",
     "When jcode's post-retrieval maintenance marks a memory verified or rejected, "
     "call mempalace's helpfulness +/- API. Report 03 §4.7.",
     "adapter,feedback"),
    ("mp-063", 4, "M", 4, "Mining lock + PID guard scoped per-palace",
     "Replace global `mine_palace_lock` with per-palace `<palace>/mine.pid` so jcode "
     "and the standalone CLI don't fight over a single lock. Report 04 P1 #12.",
     "concurrency"),
    ("mp-064", 4, "M", 4, "Route WAL writes under <palace>/wal/...",
     "Drop the global XDG WAL log; per-palace WAL directory. Required for per-project "
     "palaces in jcode. Report 04 P1 #13.",
     "wal"),
    ("mp-065", 4, "L", 4, "[jcode] One-shot migration: jcode memory migrate --backend mempalace",
     "Read existing `~/.jcode/memory/` JSON store, write each entry into a per-project "
     "mempalace palace, verify counts. Report 03 §F Phase 3.",
     "jcode-side,migration"),
    ("mp-066", 4, "M", 4, "[jcode] Run all 30+ memory tests against Mempalace backend in CI",
     "Parametrise the test suite over backend; CI runs both `Local` and `Mempalace`. "
     "Report 03 §9.1.",
     "jcode-side,testing"),
    ("mp-067", 4, "M", 4, "[jcode] MEMORY_BUDGET regression suite includes mempalace path",
     "Existing regression budget (jcode's MEMORY_BUDGET.md) covers both backends.",
     "jcode-side,perf"),
    ("mp-068", 4, "M", 4, "Define pub trait EventCapture in mempalace-core",
     "Trait with `on_session_start`, `on_user_prompt`, `on_pre_tool_use`, "
     "`on_post_tool_use`, `on_pre_compact`, `on_stop`, `on_session_end`. ADR-11; "
     "report 06 §3.1.",
     "trait,hooks"),
    ("mp-069", 4, "M", 4, "[jcode] Wire EventCapture from memory_agent.rs into mempalace",
     "Existing per-turn pipeline calls into `Palace::on_post_tool_use` etc. so "
     "auto-capture parity with agentmemory is preserved. ADR-11; report 06 §3.1.",
     "jcode-side,hooks"),

    # ---------- PHASE 5 ----------
    ("mp-080", 5, "M", 5, "Bi-temporal columns in KnowledgeGraph + back-compat migration",
     "Add `t_created`, `t_expired`, `t_valid_from`, `t_valid_to` columns. Existing rows "
     "default `t_created = valid_from`. ADR-5; report 02 §1187.",
     "kg,schema"),
    ("mp-081", 5, "L", 5, "Personalised PageRank retrieval mode + FusionMode::Ppr",
     "PPR over wing/room/tunnel/entity graph with query-derived seeds. Single biggest "
     "expected lift on multi-hop recall. Report 02 §7 (HippoRAG2).",
     "retrieval,kg"),
    ("mp-082", 5, "M", 5, "Synonymy edges (cosine > 0.85) created at ingestion",
     "Auto-link semantically similar rooms across wings. Solves "
     "auth-migration <-> oauth-migration. Report 02 §7 (HippoRAG2 §1242 Tier 2 #7).",
     "retrieval,kg"),
    ("mp-083", 5, "L", 5, "Sleep-time consolidation worker (mpr daemon + library hook)",
     "Background worker periodically refines closets, generates new tunnels, computes "
     "per-hall reflections. Off the critical path. Report 01 §6.2; report 02 §1242 #8.",
     "consolidation"),
    ("mp-084", 5, "L", 5, "A-MEM evolution loop (re-evaluate sibling closets on new drawer)",
     "When a new drawer enters a populated room, re-summarise siblings to incorporate "
     "the new context. NeurIPS-2025 architecture. Report 01 §6.2; report 02 §2.",
     "consolidation"),
    ("mp-085", 5, "M", 5, "UsearchSqliteStore Tier-2 PalaceStore implementation",
     "Mmap'd HNSW + SQLite payload. 5k–100k drawer range. ADR-2; report 05 §B.3.",
     "storage,tier-2"),
    ("mp-086", 5, "M", 5, "LancedbStore Tier-3 PalaceStore implementation",
     "Apache Arrow columnar, async, SQL-ish payload. 100k+ drawer range. ADR-2; "
     "report 05 §B.3.",
     "storage,tier-3"),
    ("mp-087", 5, "M", 5, "mpr doctor advises Tier promotion based on drawer count",
     "Recommendation only — no auto-promotion. Report 05 §B.5.",
     "doctor"),
    ("mp-088", 5, "M", 5, "Tantivy-backed BM25 leg for Tier 2/3 hybrid search",
     "Replace in-memory `bm25.rs` with `tantivy` for persistent corpus-level statistics. "
     "RRF fuses tantivy + ANN. Report 05 §B.4; report 06 §3.3.",
     "retrieval,bm25"),
    ("mp-089", 5, "M", 5, "Per-(wing, room) sub-indexes so payload filtering is unnecessary",
     "Each (wing, room) gets its own ANN; cross-wing global index for unscoped queries. "
     "Smaller indexes, faster recall, simpler code. Report 01 §1.3; report 05 §B.3.",
     "retrieval,storage"),
    ("mp-090", 5, "M", 5, "Reproduce LongMemEval-S in CI; capture Phase-5 lift per moat",
     "Bench fixture; record baseline (post-Phase 4) and per-moat deltas (PPR, sleep-time, "
     "A-MEM, tier 2/3). Document in docs/research/06_phase5_benchmark_results.md.",
     "bench"),
    ("mp-091", 5, "M", 5, "Tier-promotion logic in sleep-time consolidation",
     "Promote drawers Working -> Episodic -> Semantic -> Procedural with Ebbinghaus "
     "decay + reinforcement. ADR-13; report 06 §3.2.",
     "consolidation"),
    ("mp-092", 5, "S", 5, "SearchScope.max_per_session post-RRF filter (default 3)",
     "Cap fused results to 3 per session so a single productive session can't drown "
     "out other context. ADR-14; report 06 §3.3.",
     "retrieval"),
    ("mp-093", 5, "M", 5, "Reproduce agentmemory's 95.2% R@5 LongMemEval-S in crates/bench",
     "Match the bar set by `rohitg00/agentmemory`. Phase-5 release gated on this. "
     "Report 06 §1.",
     "bench"),

    # ---------- PHASE 6 ----------
    ("mp-100", 6, "M", 6, "CI release pipeline produces statically-linked binaries (5 targets)",
     "Including ORT-static builds for the FastEmbed default. Mirrors current matrix.",
     "release,ci"),
    ("mp-101", 6, "S", 6, "MCP tool naming alignment with @modelcontextprotocol/server-memory",
     "Aliases for canonical names (`create_entities`, `search_nodes`, etc.) so any "
     "host wired for the official MCP memory server gets mempalace as a drop-in. "
     "Old names aliased for one minor release. Report 01 §6.1.",
     "mcp"),
    ("mp-102", 6, "M", 6, "mpr export --format basic-memory (Markdown / Obsidian)",
     "Generate Obsidian-compatible folder per palace so users can open the palace in "
     "their note editor. Report 01 §6.1, §4.4.",
     "export"),
    ("mp-103", 6, "M", 6, "Integration docs + migration guide",
     "`docs/integration_jcode.md`, `docs/integration_third_party.md`, "
     "`docs/migration_v0_to_v1.md`. Plan §4 Phase 6.",
     "docs"),
    ("mp-104", 6, "M", 6, "Cut mempalace-core 1.0 to crates.io",
     "Final SemVer commitment. jcode pins to it from this point on.",
     "release"),
    ("mp-105", 6, "M", 6, "Standalone CLI ships full hook scripts for Claude Code/Codex/OpenCode",
     "SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, "
     "PreCompact, Stop, SessionEnd. Matches agentmemory's auto-capture surface. "
     "ADR-11; report 06 §3.1.",
     "hooks,install"),
    ("mp-106", 6, "S", 6, "Privacy-filter UX: configurable allow-list + redaction stats",
     "`mpr config privacy.allow <pattern>`; `mpr doctor` reports redaction hits per "
     "palace so users can audit. ADR-12; report 06 §3.4.",
     "privacy,doctor"),
]

# Phase-level dependency anchors: each phase's first P0 (or first issue if no P0)
# blocks the next phase's first issue.

def find_phase_anchor(phase: int) -> str:
    """Return the slug that anchors phase `phase` (first P0 of that phase)."""
    for slug, prio, _, p, *_ in ISSUES:
        if p == phase and prio == 0:
            return slug
    for slug, _, _, p, *_ in ISSUES:
        if p == phase:
            return slug
    raise ValueError(f"no issues in phase {phase}")


def br_create(slug: str, priority: int, title: str, description: str, labels: str) -> str:
    """Create one issue, return its full beads ID."""
    cmd = [
        "br", "create",
        "--slug", slug,
        "--type", "task",
        "--priority", str(priority),
        "--description", description,
        "--labels", labels,
        "--silent",
        title,
    ]
    res = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True)
    if res.returncode != 0:
        sys.stderr.write(
            f"[fail] {slug}: rc={res.returncode}\n"
            f"  stderr: {res.stderr.strip()}\n"
            f"  stdout: {res.stdout.strip()}\n"
        )
        raise SystemExit(1)
    return res.stdout.strip()


def br_dep_add(blocker_id: str, blocked_id: str) -> None:
    """`br dep add` — make `blocked_id` depend on `blocker_id`."""
    cmd = ["br", "dep", "add", blocked_id, blocker_id]
    res = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True)
    if res.returncode != 0:
        sys.stderr.write(
            f"[dep fail] {blocked_id} -> {blocker_id}: rc={res.returncode}\n"
            f"  stderr: {res.stderr.strip()}\n"
            f"  stdout: {res.stdout.strip()}\n"
        )


def main() -> int:
    print(f"Filing {len(ISSUES)} issues into beads at {REPO}/.beads/", flush=True)
    slug_to_id: dict[str, str] = {}
    for slug, prio, size, phase, title, desc, extra_labels in ISSUES:
        labels = f"mempal-upgrade,phase-{phase},size-{size.lower()}"
        if extra_labels:
            labels += "," + extra_labels
        # Title gets a [phase/pri/size] tag prefix for at-a-glance sorting in `br ready`.
        tagged_title = f"[{slug}] {title}"
        full_desc = (
            desc
            + f"\n\nUpgrade-plan ref: docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md"
            f" (phase {phase}, slug {slug}, P{prio}, size {size})."
        )
        bid = br_create(slug, prio, tagged_title, full_desc, labels)
        slug_to_id[slug] = bid
        print(f"  {slug} -> {bid}", flush=True)

    # Cross-phase dependency edges: every phase-N+1 issue is blocked by the phase-N anchor.
    print("\nWiring phase-level dependency anchors...", flush=True)
    anchors: dict[int, str] = {p: find_phase_anchor(p) for p in range(0, 7)}
    for slug, _, _, phase, *_ in ISSUES:
        if phase == 0:
            continue
        prior_anchor_slug = anchors[phase - 1]
        blocker_id = slug_to_id[prior_anchor_slug]
        blocked_id = slug_to_id[slug]
        br_dep_add(blocker_id, blocked_id)
    print(f"  Wired {sum(1 for _, _, _, p, *_ in ISSUES if p > 0)} phase-1+ deps.")

    out = REPO / "/tmp/mp_ids.json"
    json_path = "/tmp/mp_ids.json"
    Path(json_path).write_text(json.dumps(slug_to_id, indent=2))
    print(f"\nSaved slug->id map to {json_path}.")
    print("Run `br ready --json | jq` and `br sync --flush-only` next.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
