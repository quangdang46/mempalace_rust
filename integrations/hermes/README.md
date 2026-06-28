<p align="center">
  <img src="../../assets/banner.png" alt="mempalace" width="640" />
</p>

<h1 align="center">
  <img src="https://github.com/NousResearch.png?size=80" alt="Hermes Agent" width="28" height="28" align="center" />
  &nbsp;mempalace for Hermes Agent
</h1>

<p align="center">
  <strong>Your Hermes agent remembers everything. No more re-explaining.</strong><br/>
  <sub>Persistent cross-session memory via <a href="https://github.com/rohitg00/mempalace">mempalace</a> — 95.2% retrieval accuracy on <a href="https://arxiv.org/abs/2410.10813">LongMemEval-S</a>. Cross-agent shared with Claude Code, Cursor, OpenCode, and more.</sub>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/MCP-43_tools-1f6feb?style=flat-square" alt="43 MCP tools" />
  <img src="https://img.shields.io/badge/Hooks-6_lifecycle-1f6feb?style=flat-square" alt="6 lifecycle hooks" />
  <img src="https://img.shields.io/badge/R@5-95.2%25-00875f?style=flat-square" alt="95.2% R@5" />
  <img src="https://img.shields.io/badge/Self--hosted-yes-00875f?style=flat-square" alt="Self-hosted" />
  <img src="https://img.shields.io/badge/License-Apache_2.0-blue?style=flat-square" alt="Apache 2.0" />
</p>

---

## Install it in 30 seconds

**Paste this prompt into Hermes** and it does the whole setup for you:

```text
Install mempalace for Hermes. Run `mpr serve` in a separate terminal to
start the memory server. Then add this to `~/.hermes/config.yaml` so Hermes
can use mempalace as a memory provider:

memory:
  provider: mempalace

mcp_servers:
  mempalace:
    command: mpr
    args: ["serve"]

Verify it's working with `hermes memory status` — it should return
Provider: mempalace.

If I want deeper integration — pre-LLM context injection, turn-level
capture, memory-write mirroring, and system prompt block injection — copy
`integrations/hermes` from the mempalace repo to `~/.hermes/plugins/mempalace`
instead. That gives me the 6-hook memory provider plugin on top of the MCP
server.
```

That's it. Hermes handles the rest.

## Quick setup

### Option 1: MCP server (zero code)

Add to `~/.hermes/config.yaml`:

```yaml
mcp_servers:
  mempalace:
    command: mpr
    args: ["serve"]

memory:
  provider: mempalace
```

This gives Hermes access to all MCP tools and enables the mempalace memory provider. Start the server separately:

```bash
mpr serve
```

### Option 2: Memory provider plugin (deeper integration)

Copy this folder to your Hermes plugins directory:

```bash
cp -r integrations/hermes ~/.hermes/plugins/mempalace
```

Start the mempalace server:

```bash
mpr serve --http
```

The plugin auto-detects the running server and hooks into the Hermes agent loop. Make sure `memory.provider` is set to `mempalace` in `~/.hermes/config.yaml`:

- `prefetch()` injects relevant memories before each LLM call
- `sync_turn()` captures every conversation turn in the background
- `on_session_end()` marks sessions complete for summarization
- `on_pre_compress()` re-injects context before compaction
- `on_memory_write()` mirrors MEMORY.md writes to mempalace
- `system_prompt_block()` injects project profile at session start

### Environment variables

| Variable | Default | Description |
|---|---|---|
| `MEMPALACE_URL` | `http://localhost:3111` | mempalace server URL |
| `MEMPALACE_SECRET` | (none) | Auth token for protected instances |
| `MEMPALACE_REQUIRE_HTTPS` | (off) | When set to `1`, refuse to send the bearer token over plaintext HTTP |

The plugin reads `~/.mempalace/.env` (or `$XDG_CONFIG_HOME/mempalace/.env`) at import time and populates any missing values into the process environment via `os.environ.setdefault`. Anything you set in the shell takes precedence; the file is only used to fill gaps. This means `hermes memory status` reports the plugin as available even when the mempalace service is launched by systemd or another process manager that loads `~/.mempalace/.env` directly without exporting it to the Hermes CLI shell.

## What Hermes gets

- 95.2% retrieval accuracy (LongMemEval-S, ICLR 2025)
- Hybrid search: BM25 + vector + knowledge graph
- Memory versioning, decay, and auto-forget
- Cross-agent: memories from Claude Code, Cursor, Gemini CLI all accessible
- Real-time viewer at http://localhost:3113

## How it works

Hermes has two memory files (MEMORY.md, USER.md) and SQLite full-text search. mempalace adds structured memory on top:

| Hermes built-in | mempalace adds |
|---|---|
| MEMORY.md (flat text) | Structured observations with facts, concepts, files |
| USER.md (preferences) | Project profiles with top patterns and conventions |
| SQLite FTS5 (session search) | BM25 + vector + knowledge graph (95.2% R@5) |
| Skills (self-improving) | Skill extraction from completed sessions |
| Single agent | Cross-agent memory via MCP + REST |
