<p align="center">
  <img src="../../assets/banner.png" alt="mempalace" width="640" />
</p>

<h1 align="center">
  &nbsp;mempalace for pi
</h1>

<p align="center">
  <strong>Your pi sessions remember everything. No more re-explaining.</strong><br/>
  <sub>Persistent cross-session memory via <a href="https://github.com/rohitg00/mempalace">mempalace</a> — shared with Claude Code, Codex CLI, Gemini CLI, Hermes, OpenClaw, and more.</sub>
</p>

---

## Quick setup

Start the mempalace server in a separate terminal:

```bash
npx @mempalace/mempalace
```

Copy this folder into pi's global extensions directory:

```bash
mkdir -p ~/.pi/agent/extensions/mempalace
cp integrations/pi/index.ts ~/.pi/agent/extensions/mempalace/index.ts
```

Then enable it in `~/.pi/agent/settings.json` if you prefer explicit loading:

```json
{
  "extensions": ["~/.pi/agent/extensions/mempalace"]
}
```

If you place it under `~/.pi/agent/extensions/mempalace/`, pi will also auto-discover it and `/reload` can hot-reload it.

## What it adds

- `memory_health` — confirm the shared memory server is reachable
- `memory_search` — search prior decisions, bugs, workflows, and preferences
- `memory_save` — write durable facts back to long-term memory
- `/mempalace-status` — check health from inside pi
- `before_agent_start` recall — injects relevant memories into the prompt
- `agent_end` capture — saves completed conversation turns back to mempalace

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `MEMPALACE_URL` | `http://localhost:3111` | mempalace server URL |
| `MEMPALACE_SECRET` | (none) | Bearer token for protected instances |
| `MEMPALACE_REQUIRE_HTTPS` | (off) | When set to `1`, refuse to send a bearer token over plaintext HTTP to a non-loopback host. Sends the token only when `MEMPALACE_URL` is `https://...` or points at `localhost`/`127.0.0.1`/`::1`. With this off, the plugin warns once but still sends. |

## Smoke test

Run pi and ask it to use the `memory_health` tool, or call the command directly:

```text
/mempalace-status
```

You should see `mempalace healthy` and a footer status like `🧠 mempalace`.

## Notes

- This extension uses pi's extension API, not MCP, so it can hook directly into the agent lifecycle.
- One local mempalace server can be shared across pi, pi2, Hermes, OpenClaw, Claude Code, Codex CLI, and Gemini CLI.

## See also

- [mempalace main README](../../README.md)
- [Hermes integration](../hermes/README.md)
- [OpenClaw integration](../openclaw/README.md)
