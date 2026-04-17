# OpenClaw Skill

MemPalace provides an official skill for [OpenClaw](https://github.com/openclaw/openclaw), making it trivial to give your ClawHub agents complete access to the palace's declarative memory and knowledge graph.

## Installation

The skill is built right into the `integrations/openclaw` directory of MemPalace.

You can add MemPalace as an MCP server to OpenClaw via the CLI:

```bash
openclaw mcp set mempalace '{"command":"mpr","args":["mcp"]}'
```

Or by directly editing your OpenClaw configuration:

```json
{
  "mcpServers": {
    "mempalace": {
      "command": "mpr",
      "args": ["mcp"]
    }
  }
}
```

## How It Works

Once connected, OpenClaw agents receive all 14 MCP tools along with the **Memory Protocol**—a strict behavioral guide indicating they should:

1. **Never guess**: Query `mpr_search` or `mpr_kg_query` before confidently answering.
2. **Keep an agent diary**: Maintain continuity between sessions by writing to `mpr_diary_write`.
3. **Manage the Knowledge Graph**: Update declarative facts when things change using `mpr_kg_add` and `mpr_kg_invalidate`.

By connecting OpenClaw to MemPalace, you get both autonomous code execution and persistent, high-recall memory in the same workflow.
