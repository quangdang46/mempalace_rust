<div align="center">

<img src="assets/mempalace_logo.png" alt="MemPalace" width="280">

# MemPalace

### The highest-scoring AI memory system ever benchmarked. Now in Rust.

<br>

Every conversation you have with an AI — every decision, every debugging session, every architecture debate — disappears when the session ends. Six months of work, gone. You start over every time.

Other memory systems let AI decide what's worth remembering. They extract "user prefers Postgres" and throw away the conversation where you explained *why*. MemPalace takes a different approach: **store everything, then make it findable.**

**The Palace** — Ancient Greek orators memorized entire speeches by placing ideas in rooms of an imaginary building. Walk through the building, find the idea. MemPalace applies the same principle to AI memory: your conversations are organized into wings (people and projects) and rooms (specific ideas). No AI decides what matters — you keep every word, and the structure makes it searchable. That structure alone improves retrieval by 34%.

**AAAK** — A lossy shorthand dialect designed for AI agents. Roughly **5–10× token reduction** via lossy summarisation, optimised for LLM readability. The original prose can't be reconstructed from AAAK — the verbatim drawers stay the source of truth, AAAK is the compact index your agent reads first. Your AI loads months of context in ~120 tokens. And because AAAK is just structured text, it works with **any model that reads text** — Claude, GPT, Gemini, Llama, Mistral. No decoder, no fine-tuning, no cloud API required.

**Local, open, adaptable** — Zero external API calls by default. Everything runs on your machine. No subscription, no cloud dependency, no telemetry.

<br>

[![][version-shield]][release-link]
[![][rust-shield]][rust-link]
[![][license-shield]][license-link]

<br>

[Quick Start](#quick-start) · [The Palace](#the-palace) · [AAAK Dialect](#aaak-compression) · [Benchmarks](#benchmarks) · [MCP Tools](#mcp-server)

<br>

<table>
<tr>
<td align="center"><strong>96.6%</strong><br><sub>LongMemEval R@5<br>Zero API calls</sub></td>
<td align="center"><strong>100%</strong><br><sub>LongMemEval R@5<br>with Claude Haiku rerank</sub></td>
<td align="center"><strong>+34%</strong><br><sub>Retrieval boost<br>from palace structure</sub></td>
<td align="center"><strong>$0</strong><br><sub>No subscription<br>Local only. Always.</sub></td>
</tr>
</table>

<sub>Benchmark scores from the <a href="https://github.com/MemPalace/mempalace">reference Python implementation</a>. Rust port aims to match or exceed these.</sub>

</div>

---

## Quick Start

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash
```

Or build from source:

```bash
cargo install mempalace
```

Or install manually via [latest release](https://github.com/quangdang46/mempalace_rust/releases):

```bash
# Download the binary for your platform from releases
chmod +x mpr && sudo mv mpr /usr/local/bin/
```

### Use

```bash
mpr init ~/projects/myapp                    # guided onboarding
mpr mine ~/projects/myapp                     # mine project files
mpr mine ~/projects/myapp --mode convos       # mine conversation exports
mpr search "auth decision"                    # search everything
mpr wake-up                                   # load 170-token context for your AI
```

The installer auto-detects and configures MCP for Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, and Droid. After install, your AI tool already has MemPalace available:

```bash
# Already configured — just use your AI tool
# Or manually:
claude mcp add mpr -- mpr mcp
```

That's it. Your AI has 19+ MCP tools. Ask it anything — it searches your palace automatically.

---

## How It Works

### The Palace

```
  ┌─────────────────────────────────────────────────────────────┐
  │  WING: Person                                              │
  │                                                            │
  │    ┌──────────┐  ──hall──  ┌──────────┐                    │
  │    │  Room A  │            │  Room B  │                    │
  │    └────┬─────┘            └──────────┘                    │
  │         │                                                  │
  │         ▼                                                  │
  │    ┌──────────┐      ┌──────────┐                          │
  │    │  Closet  │ ───▶ │  Drawer  │                          │
  │    └──────────┘      └──────────┘                          │
  └─────────┼──────────────────────────────────────────────────┘
            │
          tunnel
            │
  ┌─────────┼──────────────────────────────────────────────────┐
  │  WING: Project                                             │
  │         │                                                  │
  │    ┌────┴─────┐  ──hall──  ┌──────────┐                    │
  │    │  Room A  │            │  Room C  │                    │
  │    └────┬─────┘            └──────────┘                    │
  │         │                                                  │
  │         ▼                                                  │
  │    ┌──────────┐      ┌──────────┐                          │
  │    │  Closet  │ ───▶ │  Drawer  │                          │
  │    └──────────┘      └──────────┘                          │
  └─────────────────────────────────────────────────────────────┘
```

- **Wings** — people and projects. As many as you need.
- **Rooms** — specific topics within a wing (auth, billing, deploy).
- **Halls** — connections between related rooms *within* the same wing.
- **Tunnels** — connections *between* wings (same room name → auto-linked).
- **Closets** — AAAK-compressed summaries pointing to verbatim content.
- **Drawers** — the original verbatim files, never summarized.

### Why Structure Matters

Tested on 22,000+ real conversation memories:

```
Search all closets:          60.9%  R@10
Search within wing:          73.1%  (+12%)
Search wing + hall:          84.8%  (+24%)
Search wing + room:          94.8%  (+34%)
```

The palace structure is the product — a **34% retrieval improvement** over flat search.

### The Memory Stack

| Layer | What | Size | When |
|-------|------|------|------|
| **L0** | Identity — who is this AI? | ~50 tokens | Always loaded |
| **L1** | Critical facts — team, projects, preferences | ~120 tokens (AAAK) | Always loaded |
| **L2** | Room recall — recent sessions, current project | On demand | When topic comes up |
| **L3** | Deep search — semantic query across all closets | On demand | When explicitly asked |

Your AI wakes up with L0 + L1 (~170 tokens) and knows your world.

### AAAK Compression

AAAK is a lossy shorthand dialect — ~5–10× token reduction via summarisation, readable by any LLM. No decoder required.

**English (~1000 tokens)** → **AAAK (~120 tokens):**

```
TEAM: PRI(lead) | KAI(backend,3yr) SOR(frontend) MAY(infra) LEO(junior,new)
PROJ: DRIFTWOOD(saas.analytics) | SPRINT: auth.migration→clerk
DECISION: KAI.rec:clerk>auth0(pricing+dx) | ★★★★
```

Works with Claude, GPT, Gemini, Llama, Mistral — any model that reads text. The original verbatim content is never replaced; AAAK is the index, not the source.

---

## Knowledge Graph

Temporal entity-relationship triples — SQLite-backed, local, free.

```rust
use mempalace::knowledge_graph::KnowledgeGraph;

let mut kg = KnowledgeGraph::open("~/.mempalace/knowledge.db")?;
kg.add_triple("Kai", "works_on", "Orion", valid_from="2025-06-01")?;
kg.add_triple("Maya", "assigned_to", "auth-migration", valid_from="2026-01-15")?;

// What's true today?
kg.query_entity("Kai")?;
// → [Kai → works_on → Orion (current)]

// What was true in January?
kg.query_entity("Maya", as_of="2026-01-20")?;
// → [Maya → assigned_to → auth-migration (active)]

// Timeline
kg.timeline("Orion")?;
// → chronological story of the project
```

Facts have validity windows. Auto-conflict resolution invalidates old triples when new facts replace them. Historical queries still see the full timeline.

---

## MCP Server

```bash
# Run as MCP stdio server for any MCP-compatible tool
mpr mcp

# Or configure for Claude Code:
claude mcp add mpr -- mpr mcp
```

**Palace tools:** status, list_wings, list_rooms, get_taxonomy, search, check_duplicate, traverse, find_tunnels, graph_stats, add_drawer, delete_drawer  
**Knowledge Graph tools:** kg_query, kg_add, kg_invalidate, kg_timeline, kg_stats  
**Agent Diary tools:** diary_write, diary_read

Plus observing, mining, hallway management, and collaboration tools — 80+ MCP tools in total.

---

## Auto-Save Hooks

Two hooks for Claude Code that automatically save memories during work:

**Save Hook** — every 15 messages, triggers a structured save with topics, decisions, code changes.
**PreCompact Hook** — emergency save before context window shrinks.

```json
{
  "hooks": {
    "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "mpr hook stop"}]}],
    "PreCompact": [{"matcher": "", "hooks": [{"type": "command", "command": "mpr hook precompact"}]}]
  }
}
```

Can be disabled per-session via `MEMPALACE_HOOKS_AUTO_SAVE=false`.

---

## Benchmarks

| Benchmark | Mode | Score | API Calls |
|-----------|------|-------|-----------|
| **LongMemEval R@5** | Raw (vector DB only) | **96.6%** | Zero |
| **LongMemEval R@5** | Hybrid + Haiku rerank | **100%** | Optional |
| **Palace structure impact** | Wing+room filtering | **+34%** R@10 | Zero |

<sub>From the <a href="https://github.com/MemPalace/mempalace">reference Python implementation</a> on LongMemEval-S. Rust port targets parity.</sub>

### vs Published Systems

| System | LongMemEval R@5 | API Required | Cost |
|--------|----------------|--------------|------|
| **MemPalace (hybrid)** | **100%** | Optional | Free |
| **MemPalace (raw)** | **96.6%** | **None** | **Free** |
| Mastra | 94.87% | Yes (GPT) | API costs |
| Mem0 | ~85% | Yes | $19–249/mo |
| Zep | ~85% | Yes | $25/mo+ |

---

## Configuration

**Global** (`~/.mempalace/config.json`): palace_path, collection_name, people_map, LLM provider, consolidation, chunk sizes, weights, and more.

**Environment variables** (all optional):

| Variable | Default | Purpose |
|----------|---------|---------|
| `MEMPALACE_PALACE_PATH` | `~/.mempalace/palace` | Override palace location |
| `MEMPALACE_NONINTERACTIVE` | unset | Skip prompts (CI/CD, agents) |
| `MEMPALACE_READONLY` | unset | Block all mutation MCP tools |
| `MEMPALACE_EMBED_MODEL` | `ONNXMiniLM_L6_V2` | Embedding model |
| `MEMPALACE_MAX_CHUNKS_PER_FILE` | `50000` | Per-file chunk cap (0 = disable) |
| `MEMPALACE_HOOKS_AUTO_SAVE` | `true` | Disable auto-save hooks |
| `MEMPALACE_MAX_BACKUPS` | `10` | Backup retention cap |
| `MEMPALACE_LLM_CONSENT` | unset | Opt in to external LLM providers |
| `OPENAI_EMBEDDING_API_KEY` | — | Embedding-specific API key |

---

## Rust Enhancements

The Rust port adds features beyond the original Python implementation:

- **Sharded index persistence** with manifest commit/rollback for crash safety
- **Metric-aware distance conversion** — cosine, L2, and inner product all map to [0, 1] similarity
- **Embedder-identity contract** (RFC 001) — three-state strict/lenient enforcement on palace open
- **Privacy consent gate** — blocking guard for env-fallback LLM API keys
- **External LLM warning** — detects public vs. private endpoints (Tailscale CGNAT-safe)
- **9 i18n locales** — French, Spanish, German, Chinese, Japanese, Korean, Hindi, Russian, Belarusian
- **Multilingual embedding** — embeddinggemma-300m ONNX stub for cross-lingual search
- **Background task runner** — auto-forget, consolidation, retention sweep, lesson decay
- **Per-target PID guard** — atomic lock claim with O_EXCL for concurrent safety
- **Staging rebuild** — crash-safe repair via temp directory + atomic swap
- **Backup retention** — env-configurable cap on stale backups (default 10)

---

## License

MIT — see [LICENSE](LICENSE).

[version-shield]: https://img.shields.io/github/v/release/quangdang46/mempalace_rust?style=for-the-badge
[release-link]: https://github.com/quangdang46/mempalace_rust/releases
[rust-shield]: https://img.shields.io/badge/Rust-2021-orange?style=for-the-badge&logo=rust
[rust-link]: https://www.rust-lang.org
[license-shield]: https://img.shields.io/github/license/quangdang46/mempalace_rust?style=for-the-badge&color=blue
[license-link]: https://github.com/quangdang46/mempalace_rust/blob/main/LICENSE
