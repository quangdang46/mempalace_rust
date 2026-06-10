# mempalace Rust port → Hermes Agent integration guide

[`mempalace_rust`](https://github.com/quangdang46/mempalace_rust) implements
all Hermes-facing APIs. This guide covers both integration layers for the Rust
binary (`mpr`).

## Prerequisites

Build or install `mpr`:

```bash
cargo build --release --features http-server
# Binary in ./target/release/mpr
```

The `--features http-server` flag enables the axum-based HTTP REST API server
needed by the Hermes memory-provider plugin. Without it, only the stdio MCP
server is available.

---

## Two integration layers

### Layer 1: MCP tools (stdio)

The simplest integration. The stdio MCP server exposes ~73 `mempalace_*` tools
plus ~53 `memory_*` aliases — search, remember, observe, context-build, session
lifecycle, and more. Hermes connects via its `mcp_servers` config.

**Start the server:**

```bash
mpr serve
```

**Hermes config (`~/.hermes/config.yaml`):**

```yaml
mcp_servers:
  mempalace:
    command: mpr
    args: ["serve"]

memory:
  provider: mempalace
```

This gives Hermes the full tool surface immediately. The memory provider plugin
(Layer 2) is optional — it adds prefetch, turn-level sync, and system-prompt
block injection on top of the MCP tools.

> **Tip:** For a custom palace path:
> ```yaml
>   mempalace:
>     command: mpr
>     args: ["--palace", "/path/to/palace", "serve"]
> ```

#### Verifying MCP tools work

```bash
hermes memory status
```

### Layer 2: Memory provider plugin (deep integration)

Copy this folder to the Hermes plugins directory:

```bash
cp -r integrations/hermes ~/.hermes/plugins/mempalace
```

The plugin auto-detects the running HTTP server and hooks into the Hermes agent
loop. **You must have the HTTP server running** — the plugin calls REST
endpoints, not MCP stdio.

**Start the HTTP server:**

```bash
mpr serve --http
```

This starts the axum-based REST API on `http://0.0.0.0:3111` (port configurable
via `MEMPALACE_HTTP_PORT`).

**Hermes config:**

```yaml
memory:
  provider: mempalace
```

The plugin provides these hooks:
- **`prefetch()`** — injects relevant memories before each LLM call via
  `POST /mempalace/smart-search`
- **`sync_turn()`** — captures every conversation turn in the background
- **`on_session_end()`** — marks sessions complete for summarization
- **`on_pre_compress()`** — re-injects context before compaction
- **`on_memory_write()`** — mirrors MEMORY.md writes to mempalace
- **`system_prompt_block()`** — injects project profile at session start

---

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `MEMPALACE_HTTP_PORT` | `3111` | Port for the `--http` REST API server |
| `MEMPALACE_URL` | `http://localhost:3111` | Hermes plugin server URL — automatically derived from `MEMPALACE_HTTP_PORT` if not set |
| `MEMPALACE_SECRET` | (none) | Auth token for protected instances |
| `MEMPALACE_REQUIRE_HTTPS` | (off) | When set to `1`, refuse to send the bearer token over plaintext HTTP to a non-loopback host |

---

## REST API endpoints

All routes are under the `/mempalace/` prefix. The axum server implements the
same schema as the upstream Python mempalace server. Key endpoints used by the
Hermes plugin:

| Endpoint | Hermes hook | Description |
|---|---|---|
| `POST /mempalace/session/start` | `initialize()` | Start a new session |
| `POST /mempalace/session/end` | `on_session_end()` | End and summarize a session |
| `POST /mempalace/context` | `prefetch()` | Get L0+L1 wake-up context |
| `POST /mempalace/smart-search` | `prefetch()`, `memory_search()` | Hybrid search (vector + BM25 + KG) |
| `POST /mempalace/search` | `memory_recall()` | Basic keyword/vector search |
| `POST /mempalace/remember` | `sync_turn()` | Store an observation |
| `POST /mempalace/observe` | `sync_turn()` | Record an ephemeral observation |

Additional endpoints exposed by the Rust REST API (viewer, governance,
insights, lessons, etc.) are available but not required by the Hermes plugin.

---

## Quick-start checklist

1. [ ] Build: `cargo build --release --features http-server`
2. [ ] Start HTTP server: `mpr serve --http`
3. [ ] Copy plugin: `cp -r integrations/hermes ~/.hermes/plugins/mempalace`
4. [ ] Set memory provider in `~/.hermes/config.yaml`
5. [ ] Verify: `hermes memory status`

---

## Python bridge (legacy workaround)

The file `bridge.py` provides a pure-Python HTTP server that wraps `mpr` CLI
calls as REST endpoints. It is a **stop-gap** for when the native HTTP server
is not yet built (pre-#49). Use only if `mpr serve --http` is unavailable:

```bash
python3 integrations/hermes/bridge.py
```

The bridge has known limitations:
- No proper structured JSON results (parses `mpr search` plain text)
- `remember` and `observe` are no-ops
- Session lifecycle is stubbed
- Subprocess per request (slow)

Once `mpr serve --http` is built, the bridge is **fully superseded**.

---

## Troubleshooting

**"HTTP server not available" error:**
Rebuild with the `http-server` feature:
```bash
cargo build --release --features http-server
```

**Plugin reports "no server":**
Ensure `mpr serve --http` is running and reachable on the port matching
`MEMPALACE_HTTP_PORT` (default 3111).
```bash
curl http://localhost:3111/mempalace/health
```

**No results from smart-search:**
The Rust port defaults to the `naive` embedding model on first run. Mine some
content first:
```bash
mpr mine /path/to/project
```
Then re-run the search.
