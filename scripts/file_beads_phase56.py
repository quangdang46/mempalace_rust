#!/usr/bin/env python3
"""
Continue filing beads issues: phase 5 + phase 6 (22 issues) with CORRECT priorities.

Bugfix: previous run conflated phase number with priority.
"""

from __future__ import annotations
import json
import subprocess
import sys
from pathlib import Path

REPO = Path("/data/projects/mempalace_rust")

# (slug, priority, size, phase, title, description, labels-extra)
REMAINING = [
    # ---------- PHASE 5 (14 issues) ----------
    ("mp-080", 0, "M", 5, "Bi-temporal columns in KnowledgeGraph + back-compat migration",
     "Add `t_created`, `t_expired`, `t_valid_from`, `t_valid_to` columns. Existing rows "
     "default `t_created = valid_from`. ADR-5; report 02 §1187.",
     "kg,schema"),
    ("mp-081", 1, "L", 5, "Personalised PageRank retrieval mode + FusionMode::Ppr",
     "PPR over wing/room/tunnel/entity graph with query-derived seeds. Single biggest "
     "expected lift on multi-hop recall. Report 02 §7 (HippoRAG2).",
     "retrieval,kg"),
    ("mp-082", 1, "M", 5, "Synonymy edges (cosine > 0.85) created at ingestion",
     "Auto-link semantically similar rooms across wings. Solves "
     "auth-migration <-> oauth-migration. Report 02 §7 (HippoRAG2 §1242 Tier 2 #7).",
     "retrieval,kg"),
    ("mp-083", 1, "L", 5, "Sleep-time consolidation worker (mpr daemon + library hook)",
     "Background worker periodically refines closets, generates new tunnels, computes "
     "per-hall reflections. Off the critical path. Report 01 §6.2; report 02 §1242 #8.",
     "consolidation"),
    ("mp-084", 2, "L", 5, "A-MEM evolution loop (re-evaluate sibling closets on new drawer)",
     "When a new drawer enters a populated room, re-summarise siblings to incorporate "
     "the new context. NeurIPS-2025 architecture. Report 01 §6.2; report 02 §2.",
     "consolidation"),
    ("mp-085", 1, "M", 5, "UsearchSqliteStore Tier-2 PalaceStore implementation",
     "Mmap'd HNSW + SQLite payload. 5k–100k drawer range. ADR-2; report 05 §B.3.",
     "storage,tier-2"),
    ("mp-086", 1, "M", 5, "LancedbStore Tier-3 PalaceStore implementation",
     "Apache Arrow columnar, async, SQL-ish payload. 100k+ drawer range. ADR-2; "
     "report 05 §B.3.",
     "storage,tier-3"),
    ("mp-087", 1, "M", 5, "mpr doctor advises Tier promotion based on drawer count",
     "Recommendation only — no auto-promotion. Report 05 §B.5.",
     "doctor"),
    ("mp-088", 1, "M", 5, "Tantivy-backed BM25 leg for Tier 2/3 hybrid search",
     "Replace in-memory `bm25.rs` with `tantivy` for persistent corpus-level statistics. "
     "RRF fuses tantivy + ANN. Report 05 §B.4; report 06 §3.3.",
     "retrieval,bm25"),
    ("mp-089", 1, "M", 5, "Per-(wing, room) sub-indexes so payload filtering is unnecessary",
     "Each (wing, room) gets its own ANN; cross-wing global index for unscoped queries. "
     "Smaller indexes, faster recall, simpler code. Report 01 §1.3; report 05 §B.3.",
     "retrieval,storage"),
    ("mp-090", 1, "M", 5, "Reproduce LongMemEval-S in CI; capture Phase-5 lift per moat",
     "Bench fixture; record baseline (post-Phase 4) and per-moat deltas (PPR, sleep-time, "
     "A-MEM, tier 2/3). Document in docs/research/06_phase5_benchmark_results.md.",
     "bench"),
    ("mp-091", 1, "M", 5, "Tier-promotion logic in sleep-time consolidation",
     "Promote drawers Working -> Episodic -> Semantic -> Procedural with Ebbinghaus "
     "decay + reinforcement. ADR-13; report 06 §3.2.",
     "consolidation"),
    ("mp-092", 1, "S", 5, "SearchScope.max_per_session post-RRF filter (default 3)",
     "Cap fused results to 3 per session so a single productive session can't drown "
     "out other context. ADR-14; report 06 §3.3.",
     "retrieval"),
    ("mp-093", 1, "M", 5, "Reproduce agentmemory's 95.2% R@5 LongMemEval-S in crates/bench",
     "Match the bar set by `rohitg00/agentmemory`. Phase-5 release gated on this. "
     "Report 06 §1.",
     "bench"),

    # ---------- PHASE 6 (7 issues) ----------
    ("mp-100", 1, "M", 6, "CI release pipeline produces statically-linked binaries (5 targets)",
     "Including ORT-static builds for the FastEmbed default. Mirrors current matrix.",
     "release,ci"),
    ("mp-101", 1, "S", 6, "MCP tool naming alignment with @modelcontextprotocol/server-memory",
     "Aliases for canonical names (`create_entities`, `search_nodes`, etc.) so any "
     "host wired for the official MCP memory server gets mempalace as a drop-in. "
     "Old names aliased for one minor release. Report 01 §6.1.",
     "mcp"),
    ("mp-102", 1, "M", 6, "mpr export --format basic-memory (Markdown / Obsidian)",
     "Generate Obsidian-compatible folder per palace so users can open the palace in "
     "their note editor. Report 01 §6.1, §4.4.",
     "export"),
    ("mp-103", 1, "M", 6, "Integration docs + migration guide",
     "`docs/integration_jcode.md`, `docs/integration_third_party.md`, "
     "`docs/migration_v0_to_v1.md`. Plan §4 Phase 6.",
     "docs"),
    ("mp-104", 0, "M", 6, "Cut mempalace-core 1.0 to crates.io",
     "Final SemVer commitment. jcode pins to it from this point on.",
     "release"),
    ("mp-105", 1, "M", 6, "Standalone CLI ships full hook scripts for Claude Code/Codex/OpenCode",
     "SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, "
     "PreCompact, Stop, SessionEnd. Matches agentmemory's auto-capture surface. "
     "ADR-11; report 06 §3.1.",
     "hooks,install"),
    ("mp-106", 2, "S", 6, "Privacy-filter UX: configurable allow-list + redaction stats",
     "`mpr config privacy.allow <pattern>`; `mpr doctor` reports redaction hits per "
     "palace so users can audit. ADR-12; report 06 §3.4.",
     "privacy,doctor"),
]


def br_create(slug, priority, title, description, labels):
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
            f"[fail] {slug}: rc={res.returncode}\n  stderr: {res.stderr.strip()}\n"
        )
        raise SystemExit(1)
    return res.stdout.strip()


def br_dep_add(blocker_id, blocked_id):
    cmd = ["br", "dep", "add", blocked_id, blocker_id]
    res = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True)
    if res.returncode != 0:
        sys.stderr.write(
            f"[dep fail] {blocked_id} -> {blocker_id}: rc={res.returncode}\n  stderr: {res.stderr.strip()}\n"
        )
        return False
    return True


def main():
    print(f"Filing {len(REMAINING)} remaining issues (Phase 5 + 6)...", flush=True)
    slug_to_id = {}
    for slug, prio, size, phase, title, desc, extra in REMAINING:
        labels = f"mempal-upgrade,phase-{phase},size-{size.lower()}"
        if extra:
            labels += "," + extra
        tagged_title = f"[{slug}] {title}"
        full_desc = (
            desc
            + f"\n\nUpgrade-plan ref: docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md"
            f" (phase {phase}, slug {slug}, P{prio}, size {size})."
        )
        bid = br_create(slug, prio, tagged_title, full_desc, labels)
        slug_to_id[slug] = bid
        print(f"  {slug} -> {bid}", flush=True)

    # Wire dep edges:
    #   - phase 5 anchor (mp-080, P0) is blocked by phase 4 anchor (mp-060)
    #   - every phase-5 issue (except mp-080) blocked by mp-080
    #   - phase 6 anchor (mp-104, P0) blocked by phase-5 anchor (mp-080)
    #   - every phase-6 issue (except mp-104) blocked by mp-104
    print("\nWiring phase 5/6 dependency anchors...")
    # Resolve already-created phase-4 anchor from prior run via `br list`.
    # We anchor on mp-060 (first P0 in phase 4): mr-mp-060-9v5 (from prior run output)
    PHASE4_ANCHOR = "mr-mp-060-9v5"

    PHASE5_ANCHOR = slug_to_id["mp-080"]
    PHASE6_ANCHOR = slug_to_id["mp-104"]

    br_dep_add(PHASE4_ANCHOR, PHASE5_ANCHOR)
    br_dep_add(PHASE5_ANCHOR, PHASE6_ANCHOR)

    n_p5 = 0
    for slug, _, _, phase, *_ in REMAINING:
        if phase == 5 and slug != "mp-080":
            br_dep_add(PHASE5_ANCHOR, slug_to_id[slug])
            n_p5 += 1
    n_p6 = 0
    for slug, _, _, phase, *_ in REMAINING:
        if phase == 6 and slug != "mp-104":
            br_dep_add(PHASE6_ANCHOR, slug_to_id[slug])
            n_p6 += 1
    print(f"  Phase 4 -> 5 anchor: 1 edge")
    print(f"  Phase 5 -> 6 anchor: 1 edge")
    print(f"  Within-phase 5 edges: {n_p5}")
    print(f"  Within-phase 6 edges: {n_p6}")

    # Also wire within-phase fanout for phases 0..4 from the previous run.
    # Phase anchors known from prior output:
    PRIOR_ANCHORS_AND_DEPS = {
        # phase: (anchor_id, [slugs and ids in phase])
        0: ("mr-mp-001-fg1", [
            "mr-mp-001-fg1", "mr-mp-002-nwp", "mr-mp-003-ktw", "mr-mp-004-epm",
            "mr-mp-005-goh", "mr-mp-006-ncy", "mr-mp-007-z6h",
        ]),
        1: ("mr-mp-010-c8k", [
            "mr-mp-010-c8k", "mr-mp-011-rh2", "mr-mp-012-5cs", "mr-mp-013-qsh",
            "mr-mp-014-urn", "mr-mp-015-tcg", "mr-mp-016-xcl", "mr-mp-017-lpq",
            "mr-mp-018-48p", "mr-mp-019-gy6", "mr-mp-031-n3d", "mr-mp-032-aei",
        ]),
        2: ("mr-mp-020-zwj", [
            "mr-mp-020-zwj", "mr-mp-021-qx6", "mr-mp-022-n6f", "mr-mp-023-dij",
            "mr-mp-024-t8t", "mr-mp-025-wv7", "mr-mp-026-dic", "mr-mp-027-21f",
            "mr-mp-028-7lv", "mr-mp-051-gtq", "mr-mp-052-t59",
        ]),
        3: ("mr-mp-040-vb1", [
            "mr-mp-040-vb1", "mr-mp-041-uts", "mr-mp-042-s50", "mr-mp-043-v8z",
            "mr-mp-044-u53", "mr-mp-045-soj", "mr-mp-046-uav",
        ]),
        4: ("mr-mp-060-9v5", [
            "mr-mp-060-9v5", "mr-mp-061-he9", "mr-mp-062-dl4", "mr-mp-063-zvb",
            "mr-mp-064-x1p", "mr-mp-065-p82", "mr-mp-066-64i", "mr-mp-067-ann",
            "mr-mp-068-g46", "mr-mp-069-7gm",
        ]),
    }

    # Fanout: every non-anchor in phase X depends on phase X anchor.
    print("\nWiring within-phase fanout for phases 0..4...")
    for phase, (anchor, members) in PRIOR_ANCHORS_AND_DEPS.items():
        for m in members:
            if m == anchor:
                continue
            br_dep_add(anchor, m)
    # Cross-phase: anchor X+1 blocked by anchor X.
    print("Wiring cross-phase anchor edges 0->1->2->3->4...")
    anchors_chain = [
        PRIOR_ANCHORS_AND_DEPS[0][0],
        PRIOR_ANCHORS_AND_DEPS[1][0],
        PRIOR_ANCHORS_AND_DEPS[2][0],
        PRIOR_ANCHORS_AND_DEPS[3][0],
        PRIOR_ANCHORS_AND_DEPS[4][0],
    ]
    for i in range(len(anchors_chain) - 1):
        br_dep_add(anchors_chain[i], anchors_chain[i + 1])

    Path("/tmp/mp_ids_phase56.json").write_text(json.dumps(slug_to_id, indent=2))
    print("\nDone. /tmp/mp_ids_phase56.json saved.")


if __name__ == "__main__":
    main()
