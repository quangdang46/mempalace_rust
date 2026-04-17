# Mining Your Data

MemPalace ingests your data by **mining** — scanning files and filing their content as verbatim drawers in the palace.

## Mining Modes

### Projects Mode (default)

Scans code, docs, and notes. Respects `.gitignore` by default.

```bash
mpr mine ~/projects/myapp
```

Each file becomes a drawer, tagged with a wing (project name) and room (topic). Rooms are auto-detected from your folder structure during `mpr init`.

Options:
```bash
# Override wing name
mpr mine ~/projects/myapp --wing myapp

# Ignore .gitignore rules
mpr mine ~/projects/myapp --no-gitignore

# Include specific ignored paths
mpr mine ~/projects/myapp --include-ignored dist,build

# Limit number of files
mpr mine ~/projects/myapp --limit 100

# Preview without filing
mpr mine ~/projects/myapp --dry-run
```

### Conversations Mode

Indexes conversation exports from Claude, ChatGPT, Slack, and other tools. Chunks by exchange pair (human + assistant turns).

```bash
mpr mine ~/chats/ --mode convos
```

Supports five chat formats automatically:
- Claude JSON exports
- ChatGPT exports
- Slack exports
- Markdown conversations
- Plain text transcripts

### General Extraction

Auto-classifies conversation content into five memory types:

```bash
mpr mine ~/chats/ --mode convos --extract general
```

Memory types:
- **Decisions** — choices made, options rejected
- **Preferences** — habits, likes, opinions
- **Milestones** — sessions completed, goals reached
- **Problems** — bugs, blockers, issues encountered
- **Emotional context** — reactions, concerns, excitement

## Splitting Mega-Files

Some transcript exports concatenate multiple sessions into one huge file. Split them first:

```bash
# Preview what would be split
mpr split ~/chats/ --dry-run

# Split files with 2+ sessions (default)
mpr split ~/chats/

# Only split files with 3+ sessions
mpr split ~/chats/ --min-sessions 3

# Output to a different directory
mpr split ~/chats/ --output-dir ~/chats-split/
```

::: tip
Always run `mpr split` before mining conversation files. It's a no-op if files don't need splitting.
:::

## Multi-Project Setup

Mine each project into its own wing:

```bash
mpr mine ~/chats/orion/  --mode convos --wing orion
mpr mine ~/chats/nova/   --mode convos --wing nova
mpr mine ~/chats/helios/ --mode convos --wing helios
```

Six months later:
```bash
# Project-specific search
mpr search "database decision" --wing orion

# Cross-project search
mpr search "rate limiting approach"
# → finds your approach in Orion AND Nova, shows the differences
```

## Team Usage

Mine Slack exports and AI conversations for team history:

```bash
mpr mine ~/exports/slack/ --mode convos --wing driftwood
mpr mine ~/.claude/projects/ --mode convos
```

Then search across people and projects:
```bash
mpr search "Soren sprint" --wing driftwood
# → 14 closets: OAuth refactor, dark mode, component library migration
```

## Agent Tag

Every drawer is tagged with the agent that filed it:

```bash
# Default agent name
mpr mine ~/data/ --agent mempalace

# Custom agent name
mpr mine ~/data/ --agent reviewer
```

This is used by [Specialist Agents](/concepts/agents) to partition memories.


---
