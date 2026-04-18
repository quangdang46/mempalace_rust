# MCP Integration

MemPalace provides 19 tools through the [Model Context Protocol (MCP)](https://modelcontextprotocol.io/), giving any MCP-compatible AI full read/write access to your palace.

## Setup

### Setup Helper

MemPalace includes a setup helper that prints the exact configuration commands for your environment:

```bash
mpr mcp
```

### Manual Connection

```bash
claude mcp add mempalace -- mpr mcp
```

### With Custom Palace Path

```bash
claude mcp add mempalace -- mpr mcp --palace /path/to/palace
```

Now your AI has all 19 tools available. Ask it anything:

> *"What did we decide about auth last month?"*

Claude calls `mpr_search` automatically, gets verbatim results, and answers you.

## Compatible Tools

MemPalace works with any tool that supports MCP:

- **Claude Code** — via manual MCP
- **OpenClaw** — via official skill, see [OpenClaw Skill](/guide/openclaw)
- **ChatGPT** — via MCP bridge
- **Cursor** — native MCP support
- **Gemini CLI** — see [Gemini CLI guide](/guide/gemini-cli)

## Memory Protocol

When the AI first calls `mpr_status`, it receives the **Memory Protocol** — a behavior guide that teaches it to:

1. **On wake-up**: Call `mpr_status` to load the palace overview
2. **Before responding** about any person, project, or past event: search first, never guess
3. **If unsure**: Say "let me check" and query the palace
4. **After each session**: Write diary entries to record what happened
5. **When facts change**: Invalidate old facts, add new ones

This protocol is what turns storage into memory — the AI knows to verify before speaking.

## Tool Overview

### Palace (read)

| Tool | What |
|------|------|
| `mpr_status` | Palace overview + AAAK spec + memory protocol |
| `mpr_list_wings` | Wings with counts |
| `mpr_list_rooms` | Rooms within a wing |
| `mpr_get_taxonomy` | Full wing → room → count tree |
| `mpr_search` | Semantic search with wing/room filters |
| `mpr_check_duplicate` | Check before filing |
| `mpr_get_aaak_spec` | AAAK dialect reference |

### Palace (write)

| Tool | What |
|------|------|
| `mpr_add_drawer` | File verbatim content |
| `mpr_delete_drawer` | Remove by ID |

### Knowledge Graph

| Tool | What |
|------|------|
| `mpr_kg_query` | Entity relationships with time filtering |
| `mpr_kg_add` | Add facts |
| `mpr_kg_invalidate` | Mark facts as ended |
| `mpr_kg_timeline` | Chronological entity story |
| `mpr_kg_stats` | Graph overview |

### Navigation

| Tool | What |
|------|------|
| `mpr_traverse` | Walk the graph from a room across wings |
| `mpr_find_tunnels` | Find rooms bridging two wings |
| `mpr_graph_stats` | Graph connectivity overview |

### Agent Diary

| Tool | What |
|------|------|
| `mpr_diary_write` | Write AAAK diary entry |
| `mpr_diary_read` | Read recent diary entries |

For detailed schemas and parameters, see [MCP Tools Reference](/reference/mcp-tools).
