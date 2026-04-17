# API Reference

Comprehensive parameter-level documentation for all public Rust APIs.

## `mempalace::searcher`

### `search_memories(query, palace_path, wing, room, n_results) → Result<SearchResults, SearchError>`

Programmatic search returning a structured result. Used by the MCP server.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `query` | `&str` | — | Search query text |
| `palace_path` | `&str` | — | Path to palace directory |
| `wing` | `Option<String>` | `None` | Filter by wing name |
| `room` | `Option<String>` | `None` | Filter by room name |
| `n_results` | `usize` | `5` | Maximum number of results |

**Returns `Result<SearchResults, SearchError>`:**

```rust
pub struct SearchResults {
    pub query: String,
    pub filters: Filters,
    pub results: Vec<SearchHit>,
}

pub struct SearchHit {
    pub text: String,           // verbatim drawer content
    pub wing: String,          // wing name
    pub room: String,          // room name
    pub source_file: String,    // original file basename
    pub similarity: f32,        // 0.0 to 1.0
}
```

On error: `SearchError` with descriptive message.

---

## `mempalace::layers`

### `struct MemoryStack`

Unified 4-layer interface.

```rust
use mempalace::layers::MemoryStack;

let stack = MemoryStack::new(palace_path.to_string())?;
```

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `wake_up(wing)` | `&str` | `Result<String>` | L0 + L1 context (~170–900 tokens) |
| `recall(wing, room, n_results)` | `&str, &str, usize` | `Result<String>` | L2 on-demand retrieval |
| `search(query, wing, room, n_results)` | `&str, &str, &str, usize` | `Result<String>` | L3 deep search |
| `status()` | — | `Result<Status>` | All layer status info |

### `struct Layer0`

Identity layer (~50 tokens). Reads from `~/.mempalace/identity.txt`.

| Method | Returns | Description |
|--------|---------|-------------|
| `render()` | `String` | Identity text or default message |
| `token_estimate()` | `usize` | Approximate token count |

### `struct Layer1`

Essential story layer (~500–800 tokens). Auto-generated from top drawers.

| Attribute | Type | Description |
|-----------|------|-------------|
| `MAX_DRAWERS` | `usize` | Max moments in wake-up (15) |
| `MAX_CHARS` | `usize` | Hard cap on L1 text (3200) |

---

## `mempalace::knowledge_graph`

### `struct KnowledgeGraph`

```rust
use mempalace::knowledge_graph::KnowledgeGraph;

let kg = KnowledgeGraph::open("~/.mempalace/knowledge.db")?;
```

Default path: `~/.mempalace/knowledge.db`

#### Write Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `add_triple(subject, predicate, obj, valid_from)` | `&str, &str, &str, &str` | `Result<String>` | Add relationship triple |
| `invalidate(subject, predicate, obj, ended)` | `&str, &str, &str, &str` | `Result<()>` | Mark relationship as ended |

#### Query Methods

| Method | Parameters | Returns |
|--------|-----------|---------|
| `query_entity(name, as_of, direction)` | `&str, Option<&str>, &str` | `Result<Vec<EntityFact>>` |
| `query_relationship(predicate, as_of)` | `&str, Option<&str>` | `Result<Vec<Relationship>>` |
| `timeline(entity_name)` | `Option<&str>` | `Result<Vec<TimelineEvent>>` |
| `stats()` | — | `Result<KgStats>` |

**Direction values:** `"outgoing"` (entity→?), `"incoming"` (?→entity), `"both"`

---

## `mempalace::palace_graph`

### `build_graph(palace_path) -> Result<(HashMap<String, RoomNode>, Vec<Edge>), PalaceGraphError>`

Build the palace graph from palace metadata.

### `traverse(start_room, palace_path, max_hops) -> Result<Vec<TraversalResult>, PalaceGraphError>`

BFS graph traversal from a room across wings.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `start_room` | `&str` | — | Room slug to start from |
| `max_hops` | `usize` | `2` | Max connection depth |

### `find_tunnels(wing_a, wing_b, palace_path) -> Result<Vec<Tunnel>, PalaceGraphError>`

Find rooms spanning multiple wings.

### `graph_stats(palace_path) -> Result<GraphStats, PalaceGraphError>`

---

## `mempalace::dialect`

### `struct Dialect`

```rust
use mempalace::dialect::Dialect;

let dialect = Dialect::new();
let dialect = Dialect::with_entities(entities)?;
```

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `compress(text)` | `&str` | `String` | AAAK-formatted summary |
| `encode_entity(name)` | `&str` | `Option<String>` | 3-letter entity code |
| `compression_stats(original, compressed)` | `&str, &str` | `CompressionStats` | Compression ratio stats |

---

## `mempalace::config`

### `struct Config`

Reads from `~/.mempalace/config.json` and environment variables.

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| `palace_path` | `String` | `~/.mempalace/palace` | Palace storage path |
| `collection_name` | `String` | `mpr_drawers` | Collection name |

```rust
use mempalace::config::Config;

let config = Config::load()?;
```
