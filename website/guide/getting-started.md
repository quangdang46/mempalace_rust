# Getting Started

## Installation

### One-Line Install (Recommended)

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh?$(date +%s)" | bash
```

This script automatically:
- Builds the `mpr` binary
- Detects your installed AI tools (Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, Droid)
- Configures MCP for each detected tool

### From Source

```bash
git clone https://github.com/quangdang46/mempalace_rust.git
cd mempalace_rust
cargo build --release
cargo install --path .
```

### Requirements

- **Rust 1.75+** (edition 2021)
- **ONNX runtime** for embeddings (auto-installed by `install.sh`)

No API key required. Everything runs locally.

## Quick Start

Three steps: **init**, **mine**, **search**.

### 1. Initialize Your Palace

```bash
mpr init ~/projects/myapp
```

This scans your project directory and:
- Detects people and projects from file content
- Creates rooms from your folder structure
- Ensures the `~/.mempalace/` config directory exists

### 2. Mine Your Data

```bash
# Mine project files (code, docs, notes)
mpr mine ~/projects/myapp

# Mine conversation exports (Claude, ChatGPT, Slack)
mpr mine ~/chats/ --mode convos

# Mine with auto-classification into memory types
mpr mine ~/chats/ --mode convos --extract general
```

Three mining modes:
- **projects** — code and docs, auto-detected rooms
- **convos** — conversation exports, chunked by exchange pair
- **general extraction** — `--extract general` option for conversation mining that classifies content into decisions, preferences, milestones, problems, and emotional context

Supports **8+ chat formats** — Claude Code JSONL, Claude.ai JSON, ChatGPT JSON, Slack JSON, Codex CLI JSONL, SoulForge JSONL, OpenCode SQLite, plain text.

### 3. Search

```bash
mpr search "why did we switch to GraphQL"
```

That gives you a working local memory index.

## What Happens Next

After the one-time setup, you don't run MemPalace commands manually. Your AI uses it for you through [MCP integration](/guide/mcp-integration) or a [Claude Code plugin](/guide/claude-code).

Ask your AI anything:

> *"What did we decide about auth last month?"*

It calls `mpr_search` automatically, gets verbatim results, and answers you. You never type `mpr search` again.

## Next Steps

- [Mining Your Data](/guide/mining) — deep dive into mining modes
- [MCP Integration](/guide/mcp-integration) — connect to Claude, ChatGPT, Cursor, Gemini
- [The Palace](/concepts/the-palace) — understand wings, rooms, halls, and tunnels
