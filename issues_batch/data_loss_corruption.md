## Description

**3 separate bugs in palace_db.rs that can cause silent data loss or corruption.**

### Bug 1: Corrupted JSON collection → silent data loss

`palace_db.rs` lines 1204-1209 (and 1895-1907 for open_with_embedder):

```rust
let documents: HashMap<String, DocumentEntry> = if docs_path.exists() {
    let content = std::fs::read_to_string(&docs_path)?;
    serde_json::from_str(&content).unwrap_or_default()  // silent failure
} else {
    HashMap::new()
};
```

If `mempalace_drawers.json` is corrupted (partial write, disk error, cross-version incompatibility), `unwrap_or_default()` returns an empty HashMap. The palace appears empty. If `flush()` is then called (on shutdown, auto-save, or any write operation), the empty state **overwrites** the corrupted file with an empty collection — **permanent data loss**.

### Bug 2: classify_palace also silently accepts corrupted JSON

`palace_db.rs` line 85:

```rust
let docs: HashMap<String, DocumentEntry> = serde_json::from_str(&content).unwrap_or_default();
```

A corrupted palace is classified as `Empty` instead of reporting an error. The user sees "no drawers yet" when in reality the data is corrupted.

### Bug 3: repair.rs — silent drop of misaligned document entries

`repair.rs` lines 241-246:

```rust
let id = entry.ids.get(i).cloned().unwrap_or_default();
if id.is_empty() { continue; }
let meta = entry.metadatas.get(i).cloned().unwrap_or_default();
```

If `ids`, `documents`, and `metadatas` vectors are misaligned (different lengths — should never happen but can due to bugs), entries are silently dropped during `mpr repair rebuild` without any warning.

### Fix

1. **JSON parse failure → error, not silence**:
```rust
let documents: HashMap<String, DocumentEntry> = if docs_path.exists() {
    let content = std::fs::read_to_string(&docs_path)?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", docs_path.display()))?
} else {
    HashMap::new()
};
```

2. **classify_palace**: Return `Result<PalaceState>` for parse errors, or at minimum log a `warn!` on parse failure.

3. **repair.rs**: Add alignment validation:
```rust
if entry.ids.len() != entry.documents.len() || entry.ids.len() != entry.metadatas.len() {
    warn!("repair: misaligned document entry (ids={}, docs={}, meta={}) — skipping",
        entry.ids.len(), entry.documents.len(), entry.metadatas.len());
    continue;
}
```
