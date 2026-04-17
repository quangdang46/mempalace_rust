# Gemini CLI

MemPalace works natively with [Gemini CLI](https://github.com/google/gemini-cli), which handles the MCP server and save hooks automatically.

## Prerequisites

- Rust 1.75+
- Gemini CLI installed and configured

## Installation

### One-Line Install (Recommended)

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/mempalace_rust/main/install.sh?$(date +%s)" | bash
```

This automatically builds `mpr` and configures MCP for Gemini CLI.

### Manual Installation

```bash
git clone https://github.com/quangdang46/mempalace_rust.git
cd mempalace_rust
cargo build --release
cargo install --path .
```

## Initialize the Palace

```bash
mpr init .
```

### Identity and Project Configuration (Optional)

You can optionally create or edit:

- **`~/.mempalace/identity.txt`** — plain text describing your role and focus
- **`~/.mempalace/wing_config.json`** — per-project MemPalace configuration created by `mpr init`
- **`~/.mempalace/entity_registry.json`** — entity mappings used by AAAK compression

## Connect to Gemini CLI

Register MemPalace as an MCP server:

```bash
gemini mcp add mempalace -- mpr mcp
```

## Enable Auto-Saving

Add a `PreCompress` hook to `~/.gemini/settings.json`:

```json
{
  "hooks": {
    "PreCompress": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/mempalace_rust/hooks/mempal_precompact_hook.sh"
          }
        ]
      }
    ]
  }
}
```

Make sure the hook scripts are executable:
```bash
chmod +x hooks/*.sh
```

## Usage

Once connected, Gemini CLI will automatically:
- Start the MemPalace server on launch
- Use `mpr_search` to find relevant past discussions
- Use the `PreCompress` hook to save memories before context compression

### Manual Mining

Mine existing code or docs:
```bash
mpr mine /path/to/your/project
```

### Verification

In a Gemini CLI session:
- `/mcp list` — verify `mempalace` is `CONNECTED`
- `/hooks panel` — verify the `PreCompress` hook is active
