# CLI Commands

All commands accept `--palace <path>` to override the default palace location.

## `mpr init`

Scan a project directory for people, projects, and rooms, and set up the palace.

```bash
mpr init <dir>                 # <dir> is required
mpr init <dir> --yes           # non-interactive mode
mpr init ~/projects/myapp      # example
mpr init .                     # initialize from the current directory
```

| Option  | Description                                                                  |
|---------|------------------------------------------------------------------------------|
| `<dir>` | **Required.** Project directory to scan. Pass `.` for the current directory. |
| `--yes` | Auto-accept all detected entities                                            |

What it does:

1. Scans `<dir>` for people and projects in file content
2. Detects rooms from `<dir>`'s folder structure
3. Saves detected entities to `<dir>/entities.json`
4. Ensures the global `~/.mempalace/` config directory exists

Running `mpr init` with no argument will exit with
`error: the following arguments are required: dir`.

## `mpr mine`

Mine files into the palace.

```bash
mpr mine <dir>
mpr mine <dir> --mode convos
mpr mine <dir> --mode convos --extract general
mpr mine <dir> --wing myapp
```

| Option | Default | Description |
|--------|---------|-------------|
| `<dir>` | — | Directory to mine |
| `--mode` | `projects` | `projects` for code/docs, `convos` for chat exports |
| `--wing` | directory name | Wing name override |
| `--agent` | `mempalace` | Agent name tag |
| `--limit` | `0` (all) | Max files to process |
| `--dry-run` | — | Preview without filing |
| `--extract` | `exchange` | `exchange` or `general` (for convos mode) |
| `--no-gitignore` | — | Don't respect .gitignore |
| `--include-ignored` | — | Always scan these paths even if ignored |

## `mpr search`

Find anything by semantic search.

```bash
mpr search "query"
mpr search "query" --wing myapp
mpr search "query" --wing myapp --room auth
mpr search "query" --results 10
```

| Option | Default | Description |
|--------|---------|-------------|
| `"query"` | — | What to search for |
| `--wing` | all | Filter by wing |
| `--room` | all | Filter by room |
| `--results` | `5` | Number of results |

## `mpr split`

Split concatenated transcript mega-files into per-session files.

```bash
mpr split <dir>
mpr split <dir> --dry-run
mpr split <dir> --min-sessions 3
mpr split <dir> --output-dir ~/split-output/
```

| Option | Default | Description |
|--------|---------|-------------|
| `<dir>` | — | Directory with transcript files |
| `--output-dir` | same dir | Write split files here |
| `--dry-run` | — | Preview without writing |
| `--min-sessions` | `2` | Only split files with N+ sessions |

## `mpr wake-up`

Show L0 + L1 wake-up context (~170–900 tokens).

```bash
mpr wake-up
mpr wake-up --wing driftwood
```

| Option | Description |
|--------|-------------|
| `--wing` | Project-specific wake-up |

## `mpr compress`

Compress drawers using AAAK Dialect.

```bash
mpr compress --wing myapp
mpr compress --wing myapp --dry-run
mpr compress --config entities.json
```

| Option | Description |
|--------|-------------|
| `--wing` | Wing to compress (default: all) |
| `--dry-run` | Preview without storing |
| `--config` | Entity config JSON file |

## `mpr status`

Show what's been filed — drawer count, wing/room breakdown.

```bash
mpr status
```

## `mpr repair`

Rebuild palace vector index from stored data. Fixes segfaults after database corruption.

```bash
mpr repair
```

Creates a backup at `<palace_path>.backup` before rebuilding.

## `mpr mcp`

Helper command that outputs setup syntax (like `claude mcp add...`) to connect MemPalace to your AI client, automatically handling paths.

```bash
mpr mcp
mpr mcp --palace ~/.custom-palace
```

## `mpr hook`

Run hook logic for Claude Code / Codex integration.

```bash
mpr hook run --hook stop --harness claude-code
mpr hook run --hook precompact --harness claude-code
mpr hook run --hook session-start --harness codex
```

| Option | Values | Description |
|--------|--------|-------------|
| `--hook` | `session-start`, `stop`, `precompact` | Hook name |
| `--harness` | `claude-code`, `codex` | Harness type |

## `mpr instructions`

Output skill instructions to stdout.

```bash
mpr instructions init
mpr instructions search
mpr instructions mine
mpr instructions help
mpr instructions status
```
