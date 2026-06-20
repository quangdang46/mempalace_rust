## Description

`connect/json_mcp.rs:write_mcp_config()` writes MCP server configs to third-party agent config files (Cursor, Claude Code, Codex, Gemini, etc.). If `serde_json::to_vec_pretty` fails, `unwrap_or_default()` returns empty `[]`, which is then atomically written to the config file — **truncating the user's entire agent configuration**.

### Location

`crates/core/src/connect/json_mcp.rs`, line 97:

```rust
let json_bytes = serde_json::to_vec_pretty(&obj).unwrap_or_default();
```

### Impact

This is invoked by 12+ `mpr connect` commands that write to agent configs:

| Agent | Config File |
|---|---|
| Claude Code | `~/.claude.json` |
| Codex | `~/.codex/config.toml` (JSON-based) |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| VS Code | `.vscode/mcp.json` |
| Gemini CLI | `~/.gemini/settings.json` |
| Cline | MCP config |
| Continue | `~/.continue/config.json` |
| Zed | `~/.config/zed/settings.json` |
| Warp | MCP config |
| Kiro | MCP config |
| OpenHuman | MCP config |

If serialization fails for any reason (extremely large config, circular reference in unexpected data, memory pressure), the agent's config file is silently replaced with `[]`. The user's entire agent setup (MCP servers, custom tools, API keys configured in env vars through MCP args, etc.) is **permanently lost**.

### Fix

Replace `unwrap_or_default()` with proper error propagation:

```rust
let json_bytes = serde_json::to_vec_pretty(&obj)?;
```

Also add a size validation before writing to prevent writing empty/truncated data:

```rust
if json_bytes.is_empty() {
    anyhow::bail!("serialization produced empty config — refusing to write");
}
```
