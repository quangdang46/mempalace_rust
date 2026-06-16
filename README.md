<div align="center">

<img src="assets/mempalace_logo.png" alt="MemPalace" width="280">

# MemPalace

### The highest-scoring AI memory system ever benchmarked. Now in Rust.

<br>

Every conversation you have with an AI — every decision, every debugging session, every architecture debate — disappears when the session ends. Six months of work, gone. You start over every time.

Other memory systems let AI decide what's worth remembering. They extract "user prefers Postgres" and throw away the conversation where you explained *why*. MemPalace takes a different approach: **store everything, then make it findable.**

**The Palace** — Ancient Greek orators memorized entire speeches by placing ideas in rooms of an imaginary building. Walk through the building, find the idea. MemPalace applies the same principle to AI memory: your conversations are organized into wings (people and projects), halls (types of memory), rooms (specific topics), closets (AAAK-compressed summaries), and drawers (verbatim content). No AI decides what matters — you keep every word, and the structure makes it searchable. That structure alone improves retrieval by 34%.

**AAAK** — A lossy shorthand dialect designed for AI agents. Roughly **5–10× token reduction** via lossy summarisation, optimised for LLM readability. The original prose can't be reconstructed from AAAK — the verbatim drawers stay the source of truth, AAAK is the compact index your agent reads first. Your AI loads months of context in ~120 tokens. And because AAAK is just structured text, it works with **any model that reads text** — Claude, GPT, Gemini, Llama, Mistral. No decoder, no fine-tuning, no cloud API required.

**Local, open, adaptable** — Zero external API calls by default. Everything runs on your machine. No subscription, no cloud dependency, no telemetry.

<br>

[![][version-shield]][release-link]
[![][rust-shield]][rust-link]
[![][license-shield]][license-link]

<br>

[Quick Start](#quick-start) · [Install](#install) · [The Palace](#the-palace) · [AAAK Dialect](#aaak-compression) · [Knowledge Graph](#knowledge-graph) · [Search](#search) · [Benchmarks](#benchmarks) · [MCP Tools](#mcp-server) · [All Commands](#all-commands) · [Configuration](#configuration) · [Rust Enhancements](#rust-enhancements)

<br>

<table>
<tr>
<td align="center"><strong>96.6%</strong><br><sub>LongMemEval R@5<br>Zero API calls</sub></td>
<td align="center"><strong>100%</strong><br><sub>LongMemEval R@5<br>with Claude Haiku rerank</sub></td>
<td align="center"><strong>+34%</strong><br><sub>Retrieval boost<br>from palace structure</sub></td>
<td align="center"><strong>$0</strong><br><sub>No subscription<br>Local only. Always.</sub></td>
</tr>
</table>

<sub>Benchmark scores from the <a href="https://github.com/MemPalace/mempalace">reference Python implementation</a> on LongMemEval-S. Rust port targets parity.</sub>

</div>

---

## Quick Start

```bash
# 1. Install
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash

# 2. One-command setup: init + mine a project
mpr init ~/projects/myapp --auto-mine
mpr mine ~/projects/myapp --mode convos          # conversation exports too

# 3. Search everything
mpr search "auth decision"

# 4. Your AI already has MemPalace (auto-MCP after install)
# Just ask it: "What did we decide about auth?"
```

The installer auto-detects Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, and Droid — no manual MCP configuration.

---

## Install

### Linux / macOS / WSL

```bash
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash
```

Single binary `mpr` installed to `/usr/local/bin/`. The installer auto-detects your AI tools and wires the MCP server — no manual config editing.

### Windows (native PowerShell)

```powershell
irm https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.ps1 | iex
```

Mặc định cài vào `%USERPROFILE%\.mempalace\bin\mpr.exe` và thêm vào PATH. Hỗ trợ biến môi trường:

```powershell
$env:MPR_VERSION = "v0.4.0"   # pin version
$env:MPR_PREFIX  = "D:\tools" # thay đổi thư mục cài
```

Hoặc [tải tay từ releases](https://github.com/quangdang46/mempalace_rust/releases): chọn `mpr-windows-x86_64.zip`, giải nén, chạy `mpr.exe`.

Chạy native trên Windows 10/11 x86_64. ARM cần WSL2 (chưa có native ARM build).

### From source

### From source

```bash
cargo install mempalace
```

### Manual (from releases)

```bash
# Download the binary for your platform from the releases page
chmod +x mpr && sudo mv mpr /usr/local/bin/
```

### System requirements

- **OS**: Linux, macOS, Windows (via WSL2)
- **Arch**: x86_64, aarch64 (ARM Mac, ARM Linux)
- **Disk**: ~500 MB for embedding models (downloaded on first use)
- **RAM**: ~2 GB for vector search indexes on large palaces

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
          tunnel  (connects wings)
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

**Wings** — people and projects. As many as you need. Each wing is a namespace for its content.
**Rooms** — specific topics within a wing. Auth, billing, deploy, architecture — endless rooms.
**Halls** — connections between related rooms *within* the same wing.
**Tunnels** — connections *between* wings. When Person A and a Project both have a room about "auth," a tunnel cross-references them automatically.
**Closets** — AAAK-compressed summaries that point to verbatim content. Fast for AI to read.
**Drawers** — the original verbatim files. Never summarized, never paraphrased.

**Halls** are memory types — the same in every wing, acting as corridors:
- `hall_facts` — decisions made, choices locked in
- `hall_events` — sessions, milestones, debugging
- `hall_discoveries` — breakthroughs, new insights
- `hall_preferences` — habits, likes, opinions
- `hall_advice` — recommendations and solutions

**Rooms** are named ideas — `auth-migration`, `graphql-switch`, `ci-pipeline`. When the same room appears in different wings, it creates a tunnel:

```
wing_kai       / hall_events / auth-migration  → "Kai debugged the OAuth token refresh"
wing_driftwood / hall_facts  / auth-migration  → "team decided to migrate auth to Clerk"
wing_priya     / hall_advice / auth-migration  → "Priya approved Clerk over Auth0"
```

Same room. Three wings. The tunnel connects them.

### Why Structure Matters

Tested on 22,000+ real conversation memories:

```
Search all closets:          60.9%  R@10
Search within wing:          73.1%  (+12%)
Search wing + hall:          84.8%  (+24%)
Search wing + room:          94.8%  (+34%)
```

Wings and rooms aren't cosmetic. They're a **34% retrieval improvement**. The palace structure is the product.

### The Memory Stack

| Layer | What | Size | When |
|-------|------|------|------|
| **L0** | Identity — who is this AI? | ~50 tokens | Always loaded |
| **L1** | Critical facts — team, projects, preferences | ~120 tokens (AAAK) | Always loaded |
| **L2** | Room recall — recent sessions, current project | On demand | When topic comes up |
| **L3** | Deep search — semantic query across all closets | On demand | When explicitly asked |

Your AI wakes up with L0 + L1 (~170 tokens) and knows your world. Searches only fire when needed.

### AAAK Compression

AAAK is a lossy shorthand dialect — ~5–10× token reduction via summarisation, readable by any LLM without a decoder. It works with **Claude, GPT, Gemini, Llama, Mistral** — any model that reads text. Run it against a local Llama model and your whole memory stack stays offline.

**English (~1000 tokens)**:

```
Priya manages the Driftwood team: Kai (backend, 3 years), Soren (frontend),
Maya (infrastructure), and Leo (junior, started last month). They're building
a SaaS analytics platform. Current sprint: auth migration to Clerk.
Kai recommended Clerk over Auth0 based on pricing and DX.
```

**AAAK (~120 tokens)**:

```
TEAM: PRI(lead) | KAI(backend,3yr) SOR(frontend) MAY(infra) LEO(junior,new)
PROJ: DRIFTWOOD(saas.analytics) | SPRINT: auth.migration→clerk
DECISION: KAI.rec:clerk>auth0(pricing+dx) | ★★★★
```

Same information. 8× fewer tokens. Your AI learns AAAK automatically from the MCP server — no manual setup.

The original prose can't be reconstructed from AAAK; the verbatim drawers remain the source of truth, AAAK is the compact index your agent reads first.

### Contradiction Detection

MemPalace catches mistakes before they reach you:

```
Input:  "Soren finished the auth migration"
Output: 🔴 AUTH-MIGRATION: attribution conflict — Maya was assigned, not Soren

Input:  "Kai has been here 2 years"
Output: 🟡 KAI: wrong_tenure — records show 3 years (started 2023-04)
```

Facts checked against the knowledge graph. Ages, dates, and tenures calculated dynamically — not hardcoded.

---

## Knowledge Graph

Temporal entity-relationship triples — like Zep's Graphiti, but SQLite instead of Neo4j. Local and free.

```rust
use mempalace::knowledge_graph::KnowledgeGraph;

let mut kg = KnowledgeGraph::open("~/.mempalace/knowledge.db")?;
kg.add_triple("Kai", "works_on", "Orion", valid_from="2025-06-01")?;
kg.add_triple("Maya", "assigned_to", "auth-migration", valid_from="2026-01-15")?;
kg.add_triple("Maya", "completed", "auth-migration", valid_from="2026-02-01")?;

// What's Kai working on?
kg.query_entity("Kai")?;
// → [Kai → works_on → Orion (current), Kai → recommended → Clerk (2026-01)]

// What was true in January?
kg.query_entity("Maya", as_of="2026-01-20")?;
// → [Maya → assigned_to → auth-migration (active)]

// Timeline
kg.timeline("Orion")?;
// → chronological story of the project
```

Facts have validity windows. When something stops being true, invalidate it:

```rust
kg.invalidate("Kai", "works_on", "Orion", ended="2026-03-01")?;
```

Now queries for Kai's current work won't return Orion. Historical queries still will.

### Auto-resolving Conflicts

When a new fact contradicts an existing one, the knowledge graph automatically invalidates the old triple — no manual cleanup needed:

```rust
kg.add_triple("Alice", "works_at", "Acme Corp", valid_from="2024-01")?;
// months later...
kg.add_triple("Alice", "works_at", "NewCo", valid_from="2025-06")?;
// → "Acme Corp" triple auto-invalidated, timeline shows both
```

### Episodic Memory

The palace learns what's useful over time. When a memory is retrieved and confirmed or denied, that signal is recorded:

```
retrieve("auth migration") → drawer #42
user says "yes, exactly"  → drawer #42 helpfulness +1
user says "no, wrong"     → drawer #42 helpfulness -1
```

Future retrievals blend semantic similarity with historical helpfulness — memories that consistently help rank higher, misleading ones fade.

| Feature | MemPalace | Zep (Graphiti) |
|---------|-----------|----------------|
| Storage | SQLite (local) | Neo4j (cloud) |
| Cost | Free | $25/mo+ |
| Temporal validity | Yes | Yes |
| Auto-resolve conflicts | Yes | No |
| Episodic feedback | Yes | No |
| Privacy | Everything local | SOC 2, HIPAA |

---

## Search

Three retrieval streams fused by Reciprocal Rank Fusion (RRF):

| Stream | What it catches |
|--------|----------------|
| **BM25** | Exact keyword matches with synonym expansion (SYNONYM_BM25_WEIGHT = 0.7) |
| **Vector (384-d)** | Semantic similarity via FastEmbed (cosine, L2, or inner product) |
| **Knowledge Graph** | Entity relationships, BFS traversal across wings/rooms |

RRF fuses them: a hit in any 2 of 3 ranks higher than 3 hits in 1. The search pipeline:

```bash
mpr search "rust async"                          # hybrid BM25 + vector + KG
mpr search "rust async" --bm25                   # add BM25 rerank
mpr search "rust async" --wing driftwood         # scope to a wing
mpr search "rust async" --room auth-migration    # scope to a room
```

### Metric-aware distance

The distance metric (cosine, L2, or inner product) is detected automatically from the vector index config and mapped to a correct [0, 1] similarity score:

| Metric | Conversion | Range |
|--------|------------|-------|
| Cosine | `1 - distance` | [0, 2] → [0, 1] |
| L2 | `1 / (1 + distance)` | [0, ∞) → [0, 1] |
| Inner product | `-distance` | [-1, 1] → [0, 1] |

---

## Memory Storage Pipeline

```
                     ┌─────────────────────────────────────────────┐
                     │               SOURCE STREAMS               │
                     ├──────────┬──────────┬──────────┬───────────┤
                     │ mpr mine │ MCP tool │ hooks/*  │ REST API  │
                     └────┬─────┴────┬─────┴────┬─────┴─────┬─────┘
                          │          │          │           │
                          └──────────┴────┬─────┴───────────┘
                                          ▼
                     ┌────────────────────────────────────────┐
                     │          AAAK COMPRESSION              │
                     │  LLM → facts / narrative / concepts    │
                     └──────────────────┬─────────────────────┘
                                        ▼
                     ┌────────────────────────────────────────┐
                     │           STORAGE LAYERS               │
                     ├────────────────────────────────────────┤
                     │ SQLite (drawers) │ Vector (FastEmbed)  │
                     │ KG (triples)     │ BM25 (keyword idx)  │
                     └──────────────────┬─────────────────────┘
                                        ▼
                     ┌────────────────────────────────────────┐
                     │       RETRIEVAL (RRF fusion)           │
                     │  BM25 ⨯ Vector ⨯ KG → ranked results  │
                     └────────────────────────────────────────┘
```

| Stage | What happens | Where |
|-------|-------------|-------|
| **Source** | Code, MCP calls, agent hooks, REST writes | `mpr mine`, `mempalace_add_drawer`, hooks |
| **AAAK** | LLM extracts facts, narrative, concepts, importance | `compress.rs`, `compress_synthetic.rs` |
| **Storage** | 3 indexes — SQLite, vector, KG | `palace_db.rs`, `palace/store/`, `knowledge_graph.rs` |
| **Retrieval** | BM25 + vector + graph → RRF fusion | `palace_db.rs::hybrid_search`, `search/rrf.rs` |
| **Serve** | Top-K returned via MCP tool or REST | `mcp_server.rs`, `rest_api.rs` |

---

## MCP Server

```bash
# Run as MCP stdio server for any MCP-compatible tool
mpr mcp

# Or configure for Claude Code:
claude mcp add mpr -- mpr mcp
```

### Palace tools (read)

| Tool | What |
|------|------|
| `mpr_status` | Palace overview + AAAK spec + memory protocol |
| `mpr_list_wings` | Wings with drawer counts |
| `mpr_list_rooms` | Rooms within a wing |
| `mpr_get_taxonomy` | Full wing → room → count tree |
| `mpr_search` | Semantic search with wing/room filters |
| `mpr_check_duplicate` | Check before filing |
| `mpr_traverse` | Walk the graph from a room across wings |
| `mpr_find_tunnels` | Find rooms bridging two wings |
| `mpr_graph_stats` | Graph connectivity overview |

### Palace tools (write)

| Tool | What |
|------|------|
| `mpr_add_drawer` | File verbatim content |
| `mpr_delete_drawer` | Remove by ID |
| `mpr_mine` | Mine a directory into the palace |

### Knowledge Graph tools

| Tool | What |
|------|------|
| `mpr_kg_query` | Entity relationships with time filtering |
| `mpr_kg_add` | Add temporal facts |
| `mpr_kg_invalidate` | Mark facts as ended |
| `mpr_kg_timeline` | Chronological entity story |
| `mpr_kg_stats` | Graph overview |

### Hallway tools

| Tool | What |
|------|------|
| `mpr_list_hallways` | List inter-room connections |
| `mpr_delete_hallway` | Remove a hallway |

### Diary tools

| Tool | What |
|------|------|
| `mpr_diary_write` | Write AAAK diary entry |
| `mpr_diary_read` | Read recent diary entries |

The AI learns AAAK and the memory protocol automatically from `mpr_status`. No manual configuration.

### Supported MCP Providers

`install.sh` auto-detects these:

| Provider | Config Path |
|----------|------------|
| Claude Code | `~/.claude.json` |
| Codex | `~/.codex/config.toml` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| VS Code | `.vscode/mcp.json` |
| Gemini | `~/.gemini/settings.json` |
| OpenCode | `~/.opencode.json` |
| Amp | `~/.config/amp/settings.json` |
| Droid | `~/.factory/mcp.json` |

---

## Auto-Save Hooks

Two hooks for Claude Code that automatically save memories during work:

**Save Hook** — every 15 messages, triggers a structured save. Topics, decisions, quotes, code changes. Also regenerates the critical facts layer.

**PreCompact Hook** — fires before context compression. Emergency save before the window shrinks.

```json
{
  "hooks": {
    "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "mpr hook stop"}]}],
    "PreCompact": [{"matcher": "", "hooks": [{"type": "command", "command": "mpr hook precompact"}]}]
  }
}
```

Can be disabled via `MEMPALACE_HOOKS_AUTO_SAVE=false`.

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

## All Commands

```bash
# Setup
mpr init <dir>                              # guided onboarding + AAAK bootstrap
mpr init <dir> --auto-mine                  # init + immediate mine

# Mining
mpr mine <dir>                              # mine project files
mpr mine <dir> --mode convos                # mine conversation exports
mpr mine <dir> --mode convos --wing myapp   # tag with a wing name
mpr mine <dir> --max-chunks-per-file 0      # disable per-file chunk cap
mpr mine-device                             # scan machine for all AI tool sessions

# Splitting
mpr split <dir>                             # split concatenated transcripts
mpr split <dir> --dry-run                   # preview without splitting

# Search
mpr search "query"                          # search everything
mpr search "query" --wing myapp             # within a wing
mpr search "query" --room auth-migration    # within a room
mpr search "query" --bm25                   # with BM25 rerank

# Context
mpr wake-up                                 # load L0 + L1 context (~170 tokens)
mpr wake-up --wing driftwood                # project-specific
mpr context                                 # full context build

# Compression
mpr compress --wing myapp                   # AAAK compress a wing
mpr consolidate                             # run LLM consolidation pipeline

# Health
mpr doctor                                  # palace health check (6 checks)
mpr status                                  # palace overview
mpr diagnose                                # deep diagnostics

# Knowledge graph
mpr kg stats                                # KG overview
mpr kg timeline "Kai"                       # entity timeline

# Actions & signals
mpr actions                                 # list coordination actions
mpr frontier                                # unblocked action frontier
mpr signals send --to agent --data "msg"    # send signal to agent
mpr signals list --agent agent              # read signals

# Profile
mpr profile                                 # compute palace profile

# Snapshot
mpr snapshot                                # snapshot palace state

# Export / Import
mpr export --format basic-memory            # export as markdown
mpr import --json file.json                 # import JSON data

# Visual
mpr vision                                  # vision search

# Repair
mpr repair scan                             # scan for corrupt IDs
mpr repair prune --confirm                  # delete corrupt IDs
mpr repair rebuild                          # rebuild palace index
mpr repair cleanup-pid                      # clean stale PID file

# Migrate
mpr migrate-wings --palace <path>           # normalize legacy wing names

# Forgetting
mpr forget --older-than 30                  # forget old memories

# Server mode
mpr mcp                                     # run as MCP stdio server
mpr serve                                   # run as HTTP + REST server
mpr serve --no-background                   # server without background tasks

# Evolution
mpr evolve                                  # evolve palace state

# Mesh
mpr mesh status                             # mesh peer status
mpr mesh connect --peer peer                # connect to peer
```

All commands accept `--palace <path>` to override the default palace location.

---

## Configuration

### Global config (`~/.mempalace/config.json`)

```json
{
  "palace_path": "/custom/path/to/palace",
  "collection_name": "mpr_drawers",
  "people_map": {"Kai": "KAI", "Priya": "PRI"},
  "llm_provider": "openai",
  "embedding_model": "embeddinggemma",
  "hooks_auto_save": true,
  "max_backups": 10,
  "llm_external_warn": true,
  "embedder_identity_strict": true,
  "languages": ["en"]
}
```

### Environment variables

All optional. Defaults are sensible for local single-user use.

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
| `MEMPALACE_LLM_EXTERNAL_WARN` | `true` | Warn on external LLM calls |
| `MEMPALACE_AGENT_ID` | unset | Multi-agent identity |
| `MEMPALACE_AGENT_SCOPE` | unset | Multi-agent isolation |
| `MEMPALACE_DISABLE_HOOK` | unset | Kill switch for all hooks |
| `OPENAI_EMBEDDING_API_KEY` | — | Embedding-specific API key |
| `OPENAI_EMBEDDING_BASE_URL` | — | Embedding-specific base URL |

### XDG directory layout

| Platform | Config | Data |
|----------|--------|------|
| Linux | `$XDG_CONFIG_HOME/mempalace/` | `$XDG_DATA_HOME/mempalace/` |
| macOS | `~/Library/Application Support/mempalace/` | same |
| Windows | `%APPDATA%/mempalace/` | same |
| Fallback | `~/.mempalace/` | `~/.mempalace/` |

Backward-compatible — if `~/.mempalace/` exists, it's used. Migration from old path supported.

---

## Rust Enhancements

Beyond the original Python features, the Rust port includes:

### Security & Privacy
- **Privacy consent gate** — blocking guard for env-fallback LLM API keys; no LLM call without explicit consent
- **External LLM warning** — detects public vs. private endpoints; Tailscale CGNAT (100.64.0.0/10) treated as local
- **Read-only MCP mode** via `MEMPALACE_READONLY` — write tools are disabled
- **MCP path sandboxing** — `mpr_mine` canonicalizes paths and restricts traversal
- **No shell injection vectors** — Rust's `Command::new` vs Python's `os.system`

### Storage
- **Sharded index persistence** with manifest commit/rollback — crash-safe save, no orphan data on failure
- **Backup retention** — `MEMPALACE_MAX_BACKUPS=10` prunes oldest backups after new ones are written
- **Per-target PID guard** — atomic lock claim with `O_EXCL` for concurrent safety
- **Staging rebuild** — crash-safe repair via temp directory + atomic swap

### Search & KG
- **Metric-aware distance conversion** — cosine, L2, and inner product all map correctly to [0, 1] similarity
- **FollowupTracker diagnostic** — detects when smart-search results don't satisfy the user
- **Legacy-metric warning** — detects palaces created before cosine was consistently set
- **BM25 hybrid rerank** — 70% vector + 30% BM25 weighted combination

### i18n & Cross-lingual
- **9 locale files** — French, Spanish, German, Simplified Chinese, Traditional Chinese, Hindi, Japanese, Korean, Belarusian (in addition to English, Russian, and Brazilian Portuguese)
- **Script-aware detection** — Cyrillic, Devanagari, CJK word boundaries for entity detection
- **Case-insensitive BCP 47 resolution** — `zh-cn`, `zh-CN`, `ZH-CN` all resolve to Simplified Chinese
- **Embeddinggemma-300m ONNX** — multilingual embedder stub for cross-lingual search

### Reliability
- **Background task runner** — auto-forget, consolidation, retention sweep, lesson decay all run on schedule
- **Init idempotency** — re-running init on an existing palace is safe
- **Graceful shutdown** — SIGINT handling for long operations with PID file guard
- **Stale-PID detection** — age-based stale-PID cleanup with fallback
- **JSON-RPC null payload safety** — null/empty bodies rejected with `-32600`
- **Post-rebuild FTS5 cleanup** — FTS5 integrity check + VACUUM after repair

### Embedder ecosystem

| Embedder | Dim | Feature flag | Status |
|----------|-----|-------------|--------|
| FastEmbed (ONNX) | 384-d | `embed-fastembed` (default) | ✅ |
| OpenAI-compatible | 1536/3072 | `embed-openai` | ✅ |
| Model2Vec | variable | `embed-model2vec` | ✅ |
| Tract (ONNX) | variable | `embed-tract` | ✅ |
| Cohere | 1024/4096 | `embed-cohere` | ✅ |
| Voyage | 1024/2048 | `embed-voyage` | ✅ |
| Gemini | variable | `embed-gemini` | ✅ |
| OpenRouter | variable | `embed-openrouter` | ✅ |
| EmbeddingGemma (ONNX) | 384 (MRL) | `embed-embeddinggemma` | 🔧 stub |
| Null (BM25-only) | 0 | None required | ✅ |

---

## Download formats

The normalizer supports 9+ chat export formats, auto-detected from file structure:

| Format | Source | Auto-detected by |
|--------|--------|-----------------|
| Claude Code JSONL | `~/.claude/projects/` | JSONL with role/content |
| Claude.ai JSON | Claude.ai export | JSON with chat_messages |
| ChatGPT JSON | `conversations.json` | JSON with mapping |
| Slack JSON | Slack export | JSON with channel/messages |
| Codex CLI JSONL | `~/.codex/sessions/` | session_meta header |
| SoulForge JSONL | SoulForge export | segments/toolCalls/durationMs |
| OpenCode SQLite | OpenCode sessions DB | session table with dir column |
| Gemini CLI JSONL | Gemini sessions | role + content fields |
| Plain text | Any `.txt` | Fallback |

---

## License

MIT — see [LICENSE](LICENSE).

[version-shield]: https://img.shields.io/github/v/release/quangdang46/mempalace_rust?style=for-the-badge
[release-link]: https://github.com/quangdang46/mempalace_rust/releases
[rust-shield]: https://img.shields.io/badge/Rust-2021-orange?style=for-the-badge&logo=rust
[rust-link]: https://www.rust-lang.org
[license-shield]: https://img.shields.io/github/license/quangdang46/mempalace_rust?style=for-the-badge&color=blue
[license-link]: https://github.com/quangdang46/mempalace_rust/blob/main/LICENSE
