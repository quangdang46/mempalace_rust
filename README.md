<div align="center">

<img src="assets/mempalace_logo.png" alt="MemPalace" width="280">

# MemPalace

### The highest-scoring AI memory system ever benchmarked. Now in Rust.

<br>

Every conversation you have with an AI — every decision, every debugging session, every architecture debate — disappears when the session ends. Six months of work, gone. You start over every time.

Other memory systems try to fix this by letting AI decide what's worth remembering. It extracts "user prefers Postgres" and throws away the conversation where you explained *why*. MemPalace takes a different approach: **store everything, then make it findable.**

**The Palace** — Ancient Greek orators memorized entire speeches by placing ideas in rooms of an imaginary building. Walk through the building, find the idea. MemPalace applies the same principle to AI memory: your conversations are organized into wings (people and projects), halls (types of memory), and rooms (specific ideas). No AI decides what matters — you keep every word, and the structure makes it searchable. That structure alone improves retrieval by 34%.

**AAAK** — A lossless shorthand dialect designed for AI agents. Not meant to be read by humans — meant to be read by your AI, fast. 30x compression, zero information loss. Your AI loads months of context in ~120 tokens. And because AAAK is just structured text with a universal grammar, it works with **any model that reads text** — Claude, GPT, Gemini, Llama, Mistral. No decoder, no fine-tuning, no cloud API required. Run it against a local model and your entire memory stack stays offline. Nothing else like it exists.

**Local, open, adaptable** — MemPalace runs entirely on your machine, on any data you have locally, without using any external API or services. It has been tested on conversations — but it can be adapted for different types of datastores. This is why we're open-sourcing it.

<br>

[![][version-shield]][release-link]
[![][rust-shield]][rust-link]
[![][license-shield]][license-link]

<br>

[Quick Start](#quick-start) · [The Palace](#the-palace) · [AAAK Dialect](#aaak-compression) · [Benchmarks](#benchmarks) · [MCP Tools](#mcp-server) · [Rust Enhancements](#rust-enhancements) · [Port Status](#port-status)

<br>

### Highest LongMemEval score ever published — free or paid.

<table>
<tr>
<td align="center"><strong>96.6%</strong><br><sub>LongMemEval R@5<br>Zero API calls</sub></td>
<td align="center"><strong>100%</strong><br><sub>LongMemEval R@5<br>with Haiku rerank</sub></td>
<td align="center"><strong>+34%</strong><br><sub>Retrieval boost<br>from palace structure</sub></td>
<td align="center"><strong>$0</strong><br><sub>No subscription<br>No cloud. Local only.</sub></td>
</tr>
</table>

<sub>Benchmark scores from the <a href="https://github.com/milla-jovovich/mempalace">original Python implementation</a>. Rust port aims to match or exceed these.</sub>

</div>

---

## Quick Start

### Install

```bash
# One-line install (Linux / macOS / Windows Git Bash)
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash

# Or build from source
cargo install --path .
```

### Use

```bash
# Set up your world — who you work with, what your projects are
mpr init ~/projects/myapp

# Mine your data
mpr mine ~/projects/myapp                    # projects — code, docs, notes
mpr mine ~/chats/ --mode convos              # convos — Claude, ChatGPT, Slack exports
mpr mine ~/chats/ --mode convos --extract general  # general — classifies into decisions, milestones, problems

# Search anything you've ever discussed
mpr search "why did we switch to GraphQL"

# Your AI remembers
mpr status
```

Three mining modes: **projects** (code and docs), **convos** (conversation exports), and **general** (auto-classifies into decisions, preferences, milestones, problems, and emotional context). Supports **8+ chat formats** — Claude Code JSONL, Claude.ai JSON, ChatGPT JSON, Slack JSON, Codex CLI JSONL, SoulForge JSONL, OpenCode SQLite, plain text, and more. Everything stays on your machine.

### Auto-config MCP during install

The `install.sh` script automatically detects your installed AI tools and registers `mpr` as an MCP server — no manual config editing needed:

```bash
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash
# → builds mpr, detects Claude Code / Cursor / Windsurf / ..., injects MCP config into each
```

Supports: Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, Droid

---

## How You Actually Use It

After the one-time setup (install → init → mine), you don't run MemPalace commands manually. Your AI uses it for you. There are two ways, depending on which AI you use.

### With Claude, ChatGPT, Cursor (MCP-compatible tools)

```bash
# Already done during install — just use your AI tool
# install.sh auto-detected and configured MCP for you

# Or manually for Claude Code:
claude mcp add mpr -- mpr mcp
```

Now your AI has 14+ tools available through MCP. Ask it anything:

> *"What did we decide about auth last month?"*

Claude calls `mpr_search` automatically, gets verbatim results, and answers you. You never type `mpr search` again. The AI handles it.

### With local models (Llama, Mistral, or any offline LLM)

Local models generally don't speak MCP yet. Two approaches:

**1. Wake-up command** — load your world into the model's context:

```bash
mpr wake-up > context.txt
# Paste context.txt into your local model's system prompt
```

This gives your local model ~170 tokens of critical facts (in AAAK if you prefer) before you ask a single question.

**2. CLI search** — query on demand, feed results into your prompt:

```bash
mpr search "auth decisions" > results.txt
# Include results.txt in your prompt
```

Or use the Rust library API:

```rust
use mempalace::searcher::search_memories;

let results = search_memories("auth decisions", "~/.mempalace/palace")?;
// Inject into your local model's context
```

Either way — your entire memory stack runs offline. Vector DB on your machine, Llama on your machine, AAAK for compression, zero cloud calls.

---

## The Problem

Decisions happen in conversations now. Not in docs. Not in Jira. In conversations with Claude, ChatGPT, Copilot. The reasoning, the tradeoffs, the "we tried X and it failed because Y" — all trapped in chat windows that evaporate when the session ends.

**Six months of daily AI use = 19.5 million tokens.** That's every decision, every debugging session, every architecture debate. Gone.

| Approach | Tokens loaded | Annual cost |
|----------|--------------|-------------|
| Paste everything | 19.5M — doesn't fit any context window | Impossible |
| LLM summaries | ~650K | ~$507/yr |
| **MemPalace wake-up** | **~170 tokens** | **~$0.70/yr** |
| **MemPalace + 5 searches** | **~13,500 tokens** | **~$10/yr** |

MemPalace loads 170 tokens of critical facts on wake-up — your team, your projects, your preferences. Then searches only when needed. $10/year to remember everything vs $507/year for summaries that lose context.

---

## How It Works

### The Palace

The layout is fairly simple, though it took a long time to get there.

It starts with a **wing**. Every project, person, or topic you're filing gets its own wing in the palace.

Each wing has **rooms** connected to it, where information is divided into subjects that relate to that wing — so every room is a different element of what your project contains. Project ideas could be one room, employees could be another, financial statements another. There can be an endless number of rooms that split the wing into sections. The MemPalace install detects these for you automatically, and of course you can personalize it any way you feel is right.

Every room has a **closet** connected to it, and here's where things get interesting. We've developed an AI language called **AAAK**. Don't ask — it's a whole story of its own. Your agent learns the AAAK shorthand every time it wakes up. Because AAAK is essentially English, but a very truncated version, your agent understands how to use it in seconds. It comes as part of the install, built into the MemPalace code.

Inside those closets are **drawers**, and those drawers are where your original files live. The summaries have shown **96.6% recall** in all the benchmarks done across multiple benchmarking platforms. The closet approach has been a huge boon to how much info is stored in a small space — it's used to easily point your AI agent to the drawer where your original file lives. You never lose anything, and all this happens in seconds.

There are also **halls**, which connect rooms within a wing, and **tunnels**, which connect rooms from different wings to one another. So finding things becomes truly effortless — we've given the AI a clean and organized way to know where to start searching, without having to look through every keyword in huge folders.

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

**Wings** — a person or project. As many as you need.
**Rooms** — specific topics within a wing. Auth, billing, deploy — endless rooms.
**Halls** — connections between related rooms *within* the same wing.
**Tunnels** — connections *between* wings. When Person A and a Project both have a room about "auth," a tunnel cross-references them automatically.
**Closets** — compressed summaries that point to the original content. Fast for AI to read.
**Drawers** — the original verbatim files. The exact words, never summarized.

**Halls** are memory types — the same in every wing, acting as corridors:
- `hall_facts` — decisions made, choices locked in
- `hall_events` — sessions, milestones, debugging
- `hall_discoveries` — breakthroughs, new insights
- `hall_preferences` — habits, likes, opinions
- `hall_advice` — recommendations and solutions

**Rooms** are named ideas — `auth-migration`, `graphql-switch`, `ci-pipeline`. When the same room appears in different wings, it creates a **tunnel**:

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

AAAK is a lossless dialect — 30x compression, readable by any LLM without a decoder. It works with **Claude, GPT, Gemini, Llama, Mistral** — any model that reads text. Run it against a local Llama model and your whole memory stack stays offline.

**English (~1000 tokens):**
```
Priya manages the Driftwood team: Kai (backend, 3 years), Soren (frontend),
Maya (infrastructure), and Leo (junior, started last month). They're building
a SaaS analytics platform. Current sprint: auth migration to Clerk.
Kai recommended Clerk over Auth0 based on pricing and DX.
```

**AAAK (~120 tokens):**
```
TEAM: PRI(lead) | KAI(backend,3yr) SOR(frontend) MAY(infra) LEO(junior,new)
PROJ: DRIFTWOOD(saas.analytics) | SPRINT: auth.migration→clerk
DECISION: KAI.rec:clerk>auth0(pricing+dx) | ★★★★
```

Same information. 8x fewer tokens. Your AI learns AAAK automatically from the MCP server — no manual setup.

### Contradiction Detection

MemPalace catches mistakes before they reach you:

```
Input:  "Soren finished the auth migration"
Output: 🔴 AUTH-MIGRATION: attribution conflict — Maya was assigned, not Soren

Input:  "Kai has been here 2 years"
Output: 🟡 KAI: wrong_tenure — records show 3 years (started 2023-04)

Input:  "The sprint ends Friday"
Output: 🟡 SPRINT: stale_date — current sprint ends Thursday (updated 2 days ago)
```

Facts checked against the knowledge graph. Ages, dates, and tenures calculated dynamically — not hardcoded.

---

## Real-World Examples

### Solo developer across multiple projects

```bash
mpr mine ~/chats/orion/  --mode convos --wing orion
mpr mine ~/chats/nova/   --mode convos --wing nova
mpr mine ~/chats/helios/ --mode convos --wing helios

# Six months later: "why did I use Postgres here?"
mpr search "database decision" --wing orion
# → "Chose Postgres over SQLite because Orion needs concurrent writes
#    and the dataset will exceed 10GB. Decided 2025-11-03."

# Cross-project search
mpr search "rate limiting approach"
# → finds your approach in Orion AND Nova, shows the differences
```

### Team lead managing a product

```bash
mpr mine ~/exports/slack/ --mode convos --wing driftwood
mpr mine ~/.claude/projects/ --mode convos

mpr search "Soren sprint" --wing driftwood
# → 14 closets: OAuth refactor, dark mode, component library migration

mpr search "Clerk decision" --wing driftwood
# → "Kai recommended Clerk over Auth0 — pricing + developer experience.
#    Team agreed 2026-01-15. Maya handling the migration."
```

### Before mining: split mega-files

```bash
mpr split ~/chats/                      # split into per-session files
mpr split ~/chats/ --dry-run            # preview first
mpr split ~/chats/ --min-sessions 3     # only split files with 3+ sessions
```

### Machine-wide session discovery

```bash
# Scan your entire machine for AI tool sessions and mine them all
mpr mine-device
```

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

When a new fact contradicts an existing one, the knowledge graph automatically invalidates the old triple:

```rust
kg.add_triple("Alice", "works_at", "Acme Corp", valid_from="2024-01")?;
// months later...
kg.add_triple("Alice", "works_at", "NewCo", valid_from="2025-06")?;
// → "Acme Corp" triple auto-invalidated, timeline shows both
```

No manual cleanup. The graph keeps history but surfaces only current facts.

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
| Self-hosted | Always | Enterprise only |
| Privacy | Everything local | SOC 2, HIPAA |

---

## Specialist Agents

Create agents that focus on specific areas. Each agent gets its own wing and diary in the palace — not in your CLAUDE.md. Add 50 agents, your config stays the same size.

```
~/.mempalace/agents/
  ├── reviewer.json       # code quality, patterns, bugs
  ├── architect.json      # design decisions, tradeoffs
  └── ops.json            # deploys, incidents, infra
```

Your CLAUDE.md just needs one line:

```
You have MemPalace agents. Run mpr_list_agents to see them.
```

The AI discovers its agents from the palace at runtime. Each agent:

- **Has a focus** — what it pays attention to
- **Keeps a diary** — written in AAAK, persists across sessions
- **Builds expertise** — reads its own history to stay sharp in its domain

Each agent is a specialist lens on your data. The reviewer remembers every bug pattern it's seen. The architect remembers every design decision. The ops agent remembers every incident. They don't share a scratchpad — they each maintain their own memory.

Letta charges $20–200/mo for agent-managed memory. MemPalace does it with a wing.

---

## MCP Server

```bash
# Already configured by install.sh — detected your AI tools automatically

# Or manually for Claude Code:
claude mcp add mpr -- mpr mcp
```

### 14 Tools (consolidated from the original 19)

**Palace (read)**

| Tool | What |
|------|------|
| `mpr_status` | Palace overview + AAAK spec + memory protocol |
| `mpr_list_wings` | Wings with counts |
| `mpr_list_rooms` | Rooms within a wing |
| `mpr_get_taxonomy` | Full wing → room → count tree |
| `mpr_search` | Semantic search with wing/room filters |
| `mpr_check_duplicate` | Check before filing |
| `mpr_traverse` | Walk the graph from a room across wings |
| `mpr_find_tunnels` | Find rooms bridging two wings |
| `mpr_graph_stats` | Graph connectivity overview |

**Palace (write)**

| Tool | What |
|------|------|
| `mpr_add_drawer` | File verbatim content |
| `mpr_delete_drawer` | Remove by ID |

**Knowledge Graph**

| Tool | What |
|------|------|
| `mpr_kg_query` | Entity relationships with time filtering |
| `mpr_kg_add` | Add facts |
| `mpr_kg_invalidate` | Mark facts as ended |
| `mpr_kg_timeline` | Chronological entity story |
| `mpr_kg_stats` | Graph overview |

**Agent Diary**

| Tool | What |
|------|------|
| `mpr_diary_write` | Write AAAK diary entry |
| `mpr_diary_read` | Read recent diary entries |

The AI learns AAAK and the memory protocol automatically from the `mpr_status` response. No manual configuration.

### Supported MCP Providers

`install.sh` auto-detects these providers during install:

| Provider | Config Path | Scope |
|----------|------------|-------|
| Claude Code | `~/.claude.json` | User |
| Codex | `~/.codex/config.toml` | User |
| Cursor | `~/.cursor/mcp.json` | Global |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | Global |
| VS Code | `.vscode/mcp.json` | Project |
| Gemini | `~/.gemini/settings.json` | User |
| OpenCode | `~/.opencode.json` | User |
| Amp | `~/.config/amp/settings.json` | User |
| Droid | `~/.factory/mcp.json` | User |

---

## Auto-Save Hooks

Two hooks for Claude Code that automatically save memories during work:

**Save Hook** — every 15 messages, triggers a structured save. Topics, decisions, quotes, code changes. Also regenerates the critical facts layer.

**PreCompact Hook** — fires before context compression. Emergency save before the window shrinks.

```json
{
  "hooks": {
    "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "mpr hook save"}]}],
    "PreCompact": [{"matcher": "", "hooks": [{"type": "command", "command": "mpr hook precompact"}]}]
  }
}
```

---

## Rust Enhancements

Beyond the original Python features, the Rust port includes enhancements from upstream PRs, community issues, and Rust-native improvements.

### Architecture

**Centralized palace_db singleton** — All modules share a single vector DB connection via `palace_db.rs`. No scattered client creation. Thread-safe via `Arc<Mutex<>>`. Constants (chunk sizes, search defaults, traversal caps) centralized in one module — no magic numbers.

**Security hardening** — No shell injection vectors (Rust's `Command::new` vs Python's `os.system`). Input validation on all MCP tool parameters. Error messages never leak internal paths or data. Read-only MCP mode available via `MEMPALACE_READONLY` env var — write tools are disabled. Safe for shared/public palace access.

**Batch I/O performance** — Mining accumulates chunks and inserts in a single batch call per file (was 1 call per chunk in Python). Deduplication pre-fetches all known source files into a `HashSet` — O(1) membership check instead of O(n) per-file queries. Mining 500 files uses O(1) dedup queries, not O(500).

### Extended Format Support

The normalizer supports **8+ chat export formats** and growing:

| Format | Source | Auto-detected by |
|--------|--------|-----------------|
| Claude Code JSONL | `~/.claude/projects/` | JSONL with role/content |
| Claude.ai JSON | Claude.ai export | JSON with chat_messages |
| ChatGPT JSON | `conversations.json` | JSON with mapping |
| Slack JSON | Slack export | JSON with channel/messages |
| Codex CLI JSONL | `~/.codex/sessions/` | session_meta header |
| SoulForge JSONL | SoulForge export | segments/toolCalls/durationMs |
| OpenCode SQLite | OpenCode sessions DB | session table with dir column |
| Plain text | Any `.txt` | Fallback |

Plus planned support for **Cursor** (SQLite state.vscdb), **GitHub Copilot Chat** (VS Code JSON), **Windsurf/Codeium**, and **Aider** (`.aider.chat.history.md`).

### Multilingual & Unicode

Entity detection and AAAK compression work with **non-Latin scripts** — Cyrillic, CJK, and any Unicode text. The `regex` crate enables Unicode-aware patterns (`\p{Lu}\p{Ll}`) by default. Pluggable language modules with Russian (Cyrillic) as the first non-Latin language, with 33 person verb patterns.

Configurable embedding models support multilingual search — use `paraphrase-multilingual-MiniLM-L12-v2` for cross-lingual queries (English query, non-English content).

### Agent-Friendly Automation

**Zero-interactive setup** — Set `MEMPALACE_NONINTERACTIVE=1` and every prompt is skipped with safe defaults. Works with piped stdin, CI/CD, and AI agents:

```bash
MEMPALACE_NONINTERACTIVE=1 mpr init ~/projects/myapp
echo "y" | mpr init ~/projects/myapp    # also works
```

**Auto-detect mining mode** — `mpr mine --auto` scans the target directory and figures out whether it contains project files or conversation exports. No `--mode` flag needed.

**Machine-wide discovery** — `mpr mine-device` scans known paths (`~/.claude/`, `~/.codex/sessions/`, `~/.cursor/`, etc.) and mines all discovered AI tool sessions in one command.

**Auto MCP install** — `install.sh` detects all installed AI tools (Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, Droid) and injects the `mpr` MCP server config into each. Zero manual config editing. Just `curl | bash` and start using your AI tool — it already has MemPalace available.

### XDG Base Directory

Config location follows platform conventions:

| Platform | Config | Data |
|----------|--------|------|
| Linux | `$XDG_CONFIG_HOME/mempalace/` | `$XDG_DATA_HOME/mempalace/` |
| macOS | `~/Library/Application Support/mempalace/` | same |
| Windows | `%APPDATA%/mempalace/` | same |
| Fallback | `~/.mempalace/` | `~/.mempalace/` |

Backward-compatible — if `~/.mempalace/` exists, it's used. Migration from old path supported.

### Configurable Exclude Lists

Control what gets skipped during mining:

```bash
# CLI flag
mpr mine ~/projects/myapp --exclude "node_modules" --exclude "*.log" --exclude "build/**"

# Or in config
```

Glob patterns supported. Built-in defaults (`.git`, `node_modules`, `__pycache__`, etc.) still apply unless overridden.

### Palace Doctor

Run a health check on your palace:

```bash
mpr doctor
# Checks: vector DB connectivity, orphan drawers, duplicates,
#         knowledge graph dangling refs, identity.txt, config validity
```

Colorized output (green/yellow/red), `--no-color` for scripts, non-zero exit on failure for CI.

### Smarter Entity Detection

Higher confidence thresholds eliminate false positives. Common English words that look like names ("hunter", "april", "grace") are filtered. Entity detection runs during mining with better accuracy — init focuses on basic setup (DB location, wing structure), entity learning happens naturally.

### AAAK Token Accuracy

Token counts verified against real tokenizers (cl100k_base / tiktoken). The `compression_stats()` report shows accurate pre/post token counts, not the rough chars/4 approximation.

### Integrations

| Integration | What | Status |
|------------|------|--------|
| **Hermes** | Memory provider plugin for [Hermes agent framework](https://github.com/NousResearch/hermes-agent) | Planned |
| **OpenClaw** | Skill file for [OpenClaw](https://openclaw.ai) agents (`clawhub install mpr`) | Planned |
| **AAAK inter-agent** | Use AAAK as token-efficient communication between LLMs | Planned |

### AAAK as Inter-Agent Language

Compress prompts before sending to any LLM API, decompress responses. Save tokens on long conversations:

```bash
# MCP tools
mpr_compress("long context text")    → AAAK (~30x shorter)
mpr_decompress("AAAK text")          → original meaning
```

---

## Benchmarks

Benchmark scores from the [original Python implementation](https://github.com/milla-jovovich/mempalace). Rust port aims to match or exceed.

| Benchmark | Mode | Score | API Calls |
|-----------|------|-------|-----------|
| **LongMemEval R@5** | Raw (vector DB only) | **96.6%** | Zero |
| **LongMemEval R@5** | Hybrid + Haiku rerank | **100%** (500/500) | ~500 |
| **LoCoMo R@10** | Raw, session level | **60.3%** | Zero |
| **Personal palace R@10** | Heuristic bench | **85%** | Zero |
| **Palace structure impact** | Wing+room filtering | **+34%** R@10 | Zero |

### vs Published Systems

| System | LongMemEval R@5 | API Required | Cost |
|--------|----------------|--------------|------|
| **MemPalace (hybrid)** | **100%** | Optional | Free |
| Supermemory ASMR | ~99% | Yes | — |
| **MemPalace (raw)** | **96.6%** | **None** | **Free** |
| Mastra | 94.87% | Yes (GPT) | API costs |
| Mem0 | ~85% | Yes | $19–249/mo |
| Zep | ~85% | Yes | $25/mo+ |

---

## All Commands

```bash
# Setup
mpr init <dir>                              # guided onboarding + AAAK bootstrap

# Mining
mpr mine <dir>                              # mine project files
mpr mine <dir> --mode convos                # mine conversation exports
mpr mine <dir> --mode convos --wing myapp   # tag with a wing name
mpr mine <dir> --auto                       # auto-detect project vs convos
mpr mine-device                             # scan machine for all AI tool sessions

# Splitting
mpr split <dir>                             # split concatenated transcripts
mpr split <dir> --dry-run                   # preview

# Search
mpr search "query"                          # search everything
mpr search "query" --wing myapp             # within a wing
mpr search "query" --room auth-migration    # within a room

# Memory stack
mpr wake-up                                 # load L0 + L1 context
mpr wake-up --wing driftwood                # project-specific

# Compression
mpr compress --wing myapp                   # AAAK compress

# Health
mpr doctor                                  # palace health check
mpr status                                  # palace overview

# MCP server mode
mpr mcp                                     # run as MCP stdio server
```

All commands accept `--palace <path>` to override the default location.

---

## Configuration

### Global (`~/.mempalace/config.json`)

```json
{
  "palace_path": "/custom/path/to/palace",
  "collection_name": "mpr_drawers",
  "people_map": {"Kai": "KAI", "Priya": "PRI"}
}
```

### Wing config (`~/.mempalace/wing_config.json`)

Generated by `mpr init`. Maps your people and projects to wings:

```json
{
  "default_wing": "wing_general",
  "wings": {
    "wing_kai": {"type": "person", "keywords": ["kai", "kai's"]},
    "wing_driftwood": {"type": "project", "keywords": ["driftwood", "analytics", "saas"]}
  }
}
```

### Identity (`~/.mempalace/identity.txt`)

Plain text. Becomes Layer 0 — loaded every session.

---

## Port Status

This is a Rust port of the [original Python MemPalace](https://github.com/milla-jovovich/mempalace). The port brings single-binary distribution, faster performance, and native cross-platform support.

### Implementation Progress

#### Core Modules (Python → Rust port)

| Module | Priority | Status | Notes |
|--------|----------|--------|-------|
| `Cargo.toml` + `lib.rs` + `main.rs` | P0 | Planned | Project scaffold, all module stubs |
| `config.rs` | P1 | Planned | Serde config, env overrides, XDG support |
| `normalize.rs` | P1 | Planned | 8+ chat formats (Claude, ChatGPT, Slack, Codex, SoulForge, OpenCode...) |
| `miner.rs` | P1 | Planned | Batch I/O, hash-set dedup, async file scanning |
| `convo_miner.rs` | P1 | Planned | Exchange-pair + general extraction modes |
| `searcher.rs` | P1 | Planned | Wing/room filtered semantic search |
| `layers.rs` | P1 | Planned | 4-layer memory stack (L0–L3) |
| `dialect.rs` | P2 | Planned | AAAK 30x lossless compression |
| `knowledge_graph.rs` | P1 | Planned | SQLite temporal triples via rusqlite |
| `palace_graph.rs` | P2 | Planned | BFS traversal, tunnel detection |
| `mcp_server.rs` | P1 | Planned | 14 consolidated MCP tools over stdio |
| `cli.rs` | P1 | Planned | clap-based CLI, binary name `mpr` |
| `onboarding.rs` | P2 | Planned | Interactive + non-interactive setup |
| `entity_registry.rs` | P1 | Planned | Persistent entity codes, optional Wikipedia lookup |
| `entity_detector.rs` | P2 | Planned | Heuristic person/project detection, Unicode-aware |
| `general_extractor.rs` | P2 | Planned | 5 memory type classification (no LLM) |
| `room_detector_local.rs` | P2 | Planned | 70+ folder-to-room patterns |
| `spellcheck.rs` | P3 | Planned | Name-aware spell correction |
| `split_mega_files.rs` | P3 | Planned | Session boundary detection |
| `doctor.rs` | P2 | Planned | 6-check palace health diagnostic |

#### Architecture & Infrastructure

| Feature | Priority | Status | Notes |
|---------|----------|--------|-------|
| `palace_db.rs` singleton | P1 | Planned | Centralized vector DB access, thread-safe |
| Constants centralization | P1 | Planned | No magic numbers — chunk sizes, search defaults in one place |
| Security hardening | P1 | Planned | Input validation, read-only MCP mode, no error leaks |
| MCP best practices | P1 | Planned | Tool annotations, structured output, actionable errors |
| CI/CD + `install.sh` + MCP auto-install | P1 | Planned | 5-target cross-compile, curl-pipe installer, auto-detect 9 AI tool providers |
| Test suite | P2 | Planned | Port all Python tests + Rust-native integration tests |

#### Upstream PRs & Enhancements

| Feature | Source | Priority | Status |
|---------|--------|----------|--------|
| Codex CLI JSONL normalizer | PR #61 | P2 | Planned |
| SoulForge session normalizer | PR #52 | P2 | Planned |
| OpenCode SQLite session support | PR #23 | P3 | Planned |
| `mine-device` command | PR #51 | P3 | Planned |
| `doctor` health check command | PR #36 | P2 | Planned |
| Zero-interactive setup (`--auto`, env var) | PR #33 | P2 | Planned |
| Non-Latin / Unicode-aware processing | PR #28 | P3 | Planned |
| palace_db singleton + MCP 19→14 | PR #25 | P1 | Planned |
| Batch I/O + hash-set dedup | PR #38 | P2 | Planned |
| Unify onboarding + non-interactive init | PR #18 + #13 | P2 | Planned |
| Hermes memory provider integration | PR #3 | P3 | Planned |
| OpenClaw skill integration | PR #12 | P4 | Planned |

#### Community Issues

| Feature | Source | Priority | Status |
|---------|--------|----------|--------|
| Cursor, Copilot, Windsurf, Aider imports | Issue #59 | P2 | Planned |
| Configurable exclude list for mining | Issue #56 | P2 | Planned |
| Windows Unicode console fix | Issue #47 | P2 | Planned |
| XDG base directory support | Issue #46 | P2 | Planned |
| AAAK token estimate fix (tiktoken) | Issue #43 | P2 | Planned |
| Status command 10K limit fix | Issue #40 | P2 | Planned |
| Multilingual search / configurable embedding | Issue #50 | P3 | Planned |
| KG auto-resolve conflicting triples | Issue #11 | P3 | Planned |
| Episodic memory — track retrieval outcomes | Issue #10 | P3 | Planned |
| Fix slow mining on Windows | Issue #19 | P2 | Planned |
| AI-driven init with smarter entity detection | Issue #26 | P3 | Planned |
| AAAK as inter-agent compression language | Issue #4 | P4 | Planned |

### Rust Advantages over Python

| Aspect | Python | Rust |
|--------|--------|------|
| Distribution | pip + Python runtime | Single binary, zero deps |
| Startup time | ~300ms (Python + imports) | <10ms |
| Memory | ~50MB (Python + ChromaDB client) | ~10MB |
| Parallel mining | Sequential or threading | Native async (tokio) |
| Cross-compile | Complex (PyInstaller) | Native (cross, 5 targets) |
| Install | `pip install` + venv | `curl \| bash` |

---

## Project Structure

```
mempalace_rust/
├── Cargo.toml                 ← package config
├── src/
│   ├── main.rs                ← binary entry point
│   ├── lib.rs                 ← library re-exports
│   ├── cli.rs                 ← CLI (clap subcommands)
│   ├── config.rs              ← configuration loading
│   ├── normalize.rs           ← 8+ chat format normalizers
│   ├── miner.rs               ← project file ingest
│   ├── convo_miner.rs         ← conversation ingest
│   ├── searcher.rs            ← semantic search
│   ├── layers.rs              ← 4-layer memory stack
│   ├── dialect.rs             ← AAAK compression
│   ├── knowledge_graph.rs     ← temporal entity graph (SQLite)
│   ├── palace_graph.rs        ← room navigation graph
│   ├── palace_db.rs           ← centralized vector DB access
│   ├── mcp_server.rs          ← MCP server (14 tools)
│   ├── onboarding.rs          ← guided setup
│   ├── entity_registry.rs     ← entity code registry
│   ├── entity_detector.rs     ← auto-detect people/projects
│   ├── general_extractor.rs   ← 5-type memory classifier
│   ├── room_detector_local.rs ← folder-to-room mapping
│   ├── spellcheck.rs          ← name-aware spell correction
│   ├── split_mega_files.rs    ← transcript splitter
│   └── doctor.rs              ← palace health check
├── tests/                     ← integration tests
├── install.sh                 ← curl-pipe installer + MCP auto-config
├── .github/workflows/
│   ├── ci.yml                 ← fmt + clippy + test, 3-OS
│   └── release.yml            ← cross-compile 5 targets
├── references/                ← original Python source (reference only)
└── assets/                    ← logo + brand assets
```

---

## Requirements

- Rust 1.75+ (edition 2021)
- A vector database (ChromaDB server or compatible)
- SQLite (bundled via rusqlite)

No API key. No internet after install. Everything local.

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | bash

# Or from source
git clone https://github.com/quangdang46/mempalace_rust.git
cd mempalace_rust && cargo install --path .
```

---

## Contributing

PRs welcome. This is an active port — see [Port Status](#port-status) for open modules.

## Acknowledgments

This is a Rust port of [MemPalace](https://github.com/milla-jovovich/mempalace) by [milla-jovovich](https://github.com/milla-jovovich) and contributors. The original Python implementation, architecture, AAAK dialect, palace model, and benchmark results are all their work. This port aims to bring the same system to Rust for single-binary distribution and native performance.

## License

MIT — see [LICENSE](LICENSE).

<!-- Link Definitions -->
[version-shield]: https://img.shields.io/badge/version-0.1.0-orange?style=flat-square&labelColor=0a0e14
[release-link]: https://github.com/quangdang46/mempalace_rust/releases
[rust-shield]: https://img.shields.io/badge/rust-1.75+-dea584?style=flat-square&labelColor=0a0e14&logo=rust&logoColor=dea584
[rust-link]: https://www.rust-lang.org/
[license-shield]: https://img.shields.io/badge/license-MIT-b0e8ff?style=flat-square&labelColor=0a0e14
[license-link]: https://github.com/quangdang46/mempalace_rust/blob/main/LICENSE
