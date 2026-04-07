# MemPalace SKILL

## Purpose

MemPalace gives your AI agent a persistent, searchable memory palace. Mine projects, conversations, and notes into a local knowledge base that persists across sessions.

## Setup

```bash
# Install via curl
curl -fsSL https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh | sh

# Or build from source
cargo build --release
cp target/release/mpr ~/bin/mpr  # or any directory in PATH

# Initialize the palace (auto-detects folder structure)
mpr init

# Mine a project into the palace
mpr mine ./path/to/project

# Start the MCP server for Claude Code / other AI tools
mpr mcp
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `mpr init` | Detect rooms from folder structure and create config |
| `mpr mine <path>` | Mine files into the palace |
| `mpr search <query>` | Search palace entries by text |
| `mpr wake-up` | Show L0 (identity) + L1 (essential story) context |
| `mpr compress` | Compress drawers using AAAK dialect (~30x reduction) |
| `mpr status` | Show palace statistics |
| `mpr mcp` | Start the MCP server for AI tool integration |

## MCP Server

MemPalace provides an MCP server for zero-configuration integration with Claude Code and other AI tools.

### Auto-Installation (Recommended)

```bash
mpr install-mcp
```

This auto-detects and installs into: Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, Droid.

### Manual MCP Setup

For Claude Code (`~/.claude.json`):

```json
{
  "mcpServers": {
    "mempalace": {
      "command": "/path/to/mpr",
      "args": ["mcp"]
    }
  }
}
```

For Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.mempalace]
command = "/path/to/mpr"
args = ["mcp"]
```

### MCP Tools (20+ tools)

**Read Tools:**
- `mempalace_status` — Palace overview
- `mempalace_list_drawers` — List all wings/halls
- `mempalace_list_rooms` — List rooms in a drawer
- `mempalace_search` — Search by text, drawer, room
- `mempalace_full_search` — Full-text search
- `mempalace_get_memory` — Get entries by key
- `mempalace_diary_read` — Read diary entries
- `mempalace_list_entities` — List known entities
- `mempalace_get_entity` — Get entity details
- `mempalace_graph_dump` — Knowledge graph dump
- `mempalace_get_config` — Get configuration
- `mempalace_get_people` — Get people map

**Write Tools:**
- `mempalace_mine` — Mine a file into palace
- `mempalace_write_memory` — Write a key-value entry
- `mempalace_diary_write` — Append diary entry
- `mempalace_set_config` — Update configuration
- `mempalace_set_people` — Update people map

**Compression Tools:**
- `mempalace_compress` — Compress text using AAAK dialect
- `mempalace_decompress` — Decompress AAAK shorthand

**Utility:**
- `mempalace_onboard` — Run onboarding flow
- `mempalace_doctor` — Run diagnostics

## Memory Layers

MemPalace uses a 4-layer memory architecture:

- **L0 (Identity)**: ~100 tokens — Who you are, key facts about you
- **L1 (Essential Story)**: ~500-800 tokens — Top palace moments, pre-computed
- **L2 (On-Demand)**: ~200-500 tokens — Wing/room filtered retrieval
- **L3 (Deep Search)**: Unlimited — Full semantic search across all memory

Use `mpr wake-up` to get L0 + L1 context for your agent's system prompt.

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `MEMPALACE_PALACE_PATH` | Palace storage path | `~/.mempalace/palace` |
| `MEMPALACE_NONINTERACTIVE` | Skip prompts (CI mode) | `false` |
| `MEMPALACE_READONLY` | Disable write tools | `false` |
| `XDG_CONFIG_HOME` | Config directory | `~/.config` |
| `XDG_DATA_HOME` | Data directory | `~/.local/share` |

## Data Storage

- Config: `~/.config/mempalace/config.json` (or `~/.mempalace/config.json`)
- Palace: `~/.local/share/mempalace/palace/` (or `~/.mempalace/palace/`)
- Knowledge Graph: SQLite at `palace/knowledge_graph.db`

## AAAK Compression

AAAK (AI Agent Acquisition Knowledge) is a shorthand dialect for ~30x token reduction:

```bash
# Compress drawers
mpr compress

# MCP tool response includes:
# {
#   "original": "...",
#   "compressed": "...",
#   "stats": {
#     "original_tokens": 1234,
#     "compressed_tokens": 42,
#     "ratio": 29.4
#   }
# }
```

## Troubleshooting

```bash
# Run diagnostics
mpr doctor

# Check palace status
mpr status

# Reset to defaults
rm -rf ~/.mempalace && mpr init
```