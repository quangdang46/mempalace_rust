## Description

### 1. FTS5 opens separate SQLite connection — race condition

`search_strategy/fts5.rs:44`:

```rust
let conn = Connection::open(db.path().join("mempalace.db"))?;
```

The FTS5 search strategy opens a **brand new** SQLite connection instead of using PalaceDb's existing connection. Since `mempalace.db` doesn't exist in v0.6.0 (data is stored in `mempalace_drawers.json`), this creates an empty database that always returns 0 results. The FTS5 strategy then silently falls back to NaiveJaccard.

Even if `mempalace.db` existed, opening a separate connection bypasses WAL-mode synchronization, creating a race condition: if PalaceDb writes while this FTS5 query runs, the query may see a stale snapshot or fail with `SQLITE_BUSY`.

### 2. UTF-8 char boundary panics (6 new locations, plus already-reported #66)

These byte-slice operations panic if the content contains multi-byte UTF-8 (CJK, emoji, accented chars):

| File | Line | Code | Context |
|---|---|---|---|
| `normalize.rs` | 92 | `&cmd[..200]` | Bash command truncation |
| `normalize.rs` | 133 | `&args_str[..200]` | Args truncation |
| `normalize.rs` | 188 | `&text[..2048]` | Generic text truncation |
| `mcp/context_inject.rs` | 76 | `&result.text[..200]` | Context preview |
| `convo_miner.rs` | 367 | `name[..50]` | Filename display |
| `diary_ingest.rs` | 77 | `&s[..10]` | Date string extraction |

**Example (normalize.rs:92)**:
```rust
let cmd_trimmed = if cmd.len() > 200 { &cmd[..200] } else { &cmd };
```
This panics if byte 200 falls in the middle of a multi-byte character.

### 3. notes/mod.rs — I/O errors silently swallowed

`notes/mod.rs:110`:

```rust
let agent = fs::read_to_string(&self.agent_path).unwrap_or_default();
let user = fs::read_to_string(&self.user_path).unwrap_or_default();
```

If `AGENT.md` or `USER.md` cannot be read (permissions, racing deletion, filesystem error), an empty string is silently returned. The user sees empty notes with no indication something went wrong.

### 4. sync.rs + coordination/*.rs — database corruption via silent fixup

Multiple coordination modules silently "fix" corrupted database values instead of reporting errors:

**Pattern** (e.g. `actions.rs:256`):
```rust
status: row.get::<_, String>("status")?.parse().unwrap_or(ActionStatus::Pending),
```

Same pattern for dates (`unwrap_or_else(|_| Utc::now())`) and JSON fields (`unwrap_or_default()`). Database corruption silently degrades data instead of surfacing errors.

### Fix

1. **FTS5**: Pass `PalaceDb`'s connection reference or check `mempalace_drawers.json` instead of hardcoded `mempalace.db`
2. **UTF-8 slicing**: Replace with `.char_indices()` or `.chars().take(N).collect()`:
```rust
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    let mut end = s.len();
    for (i, _) in s.char_indices() {
        if i >= max_chars { end = i; break; }
    }
    &s[..end]
}
```
3. **notes**: `fs::read_to_string(&path).with_context(|| format!("failed to read {:?}", path))?`
4. **Coordination**: Return `Result` instead of silently fixing corrupted data
