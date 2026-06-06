# @mempalace/fs-watcher

Filesystem connector for mempalace. Watches one or more directories and emits an observation to the running mempalace server every time a file changes.

Part of the data-source-connectors effort tracked in issue #62.

## Install

```bash
npm install -g @mempalace/fs-watcher
```

Or run without installing:

```bash
npx @mempalace/fs-watcher ~/work/my-repo
```

## Usage

```bash
# CLI args win over env.
mempalace-fs-watcher ~/work/my-repo ~/notes

# Or set env once in your shell.
export MEMPALACE_FS_WATCH_DIRS=~/work/my-repo,~/notes
export MEMPALACE_URL=http://localhost:3111
export MEMPALACE_SECRET=...   # only if the server requires auth
mempalace-fs-watcher
```

Every file change inside the watched roots becomes a `post_tool_use` observation whose `data.changeKind` is `file_change` or `file_delete`. The first 4 KB of each text file is included as `data.content` so retrieval can match by substring; larger files are truncated with `data.truncated: true`. Binary files are not read (set `MEMPALACE_FS_WATCH_ALLOW_BINARY=1` to override).

Session id and project are required by the observe endpoint — set them via env, or the watcher generates a per-process `fs-watcher-<ts>-<rand>` session id and uses the first root's directory name as the project.

Requires Node.js **>=20 LTS**. Recursive `fs.watch` needs Node 19.1.0+ on Linux; Node 20 is the minimum supported LTS line.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `MEMPALACE_FS_WATCH_DIRS` | — | Comma-separated list of directories to watch |
| `MEMPALACE_FS_WATCH_IGNORE` | — | Comma-separated regex patterns to ignore (applied to relative paths) |
| `MEMPALACE_FS_WATCH_ALLOW_BINARY` | `0` | `1` to include binary files in the preview read |
| `MEMPALACE_URL` | `http://localhost:3111` | mempalace server URL |
| `MEMPALACE_SECRET` | — | Bearer token, required if the server has `MEMPALACE_SECRET` set |
| `MEMPALACE_PROJECT` | — | Optional project label attached to each observation |
| `MEMPALACE_SESSION_ID` | — | Optional session id to attribute observations to |

## Defaults

Ignored out of the box: `.git/`, `node_modules/`, `dist/`, `build/`, `.next/`, `.turbo/`, `coverage/`, `.DS_Store`, `*.log`, `*.lock`. Extend with `MEMPALACE_FS_WATCH_IGNORE`.

Text extensions read for preview: common source, config, and docs (`.ts/.js/.py/.go/.rs/.md/.yaml/...`). Unknown extensions are recorded as a path-only observation without content.

Writes are debounced 500 ms per path so a stream of saves from your editor becomes a single observation.

## Notes

- Uses Node's built-in `fs.watch` with `{ recursive: true }`. Works natively on macOS, Linux, and Windows 10+. No native deps.
- If `fs.watch` errors on a specific root (permission, platform quirk), the watcher logs and continues on the others.
- The process must keep running. Use a process manager (`launchd`, `systemd`, `pm2`) to supervise it.
- This connector is intentionally one-way: it writes observations and never reads the mempalace store.
