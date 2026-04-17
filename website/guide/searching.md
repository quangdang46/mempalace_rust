# Searching Memories

MemPalace uses semantic vector search to find relevant memories. When you search, you get **verbatim text** — the exact words, never summaries.

## CLI Search

```bash
# Search everything
mpr search "why did we switch to GraphQL"

# Filter by wing (project)
mpr search "database decision" --wing myapp

# Filter by room (topic)
mpr search "auth decisions" --room auth-migration

# Filter by both
mpr search "pricing" --wing driftwood --room costs

# More results
mpr search "deploy process" --results 10
```

## How Search Works

1. Your query is embedded using the ONNX embedding model (`ONNXMiniLM_L6_V2` with 384 dimensions).
2. The embedding is compared against all drawers using cosine similarity.
3. Optional wing/room filters narrow the search scope — standard metadata filtering.
4. Results are returned with similarity scores and source metadata.

### Why Scoping Matters

Wing/room filtering is useful when a single palace contains many unrelated projects or people. Narrowing the search to a specific wing (or wing + room) means the vector store only scores candidates inside that scope, which keeps retrieval predictable as the palace grows.

This is a metadata-filter feature, not a novel retrieval mechanism. Treat it as an operational convenience: clear scoping rules that a human or an agent can apply predictably.

## Programmatic Search

Use the Rust API for integration:

```rust
use mempalace::searcher::search_memories;

let results = search_memories(
    query="auth decisions",
    palace_path="~/.mempalace/palace",
    wing=Some("myapp".to_string()),
    room=Some("auth".to_string()),
    n_results=5,
)?;

for hit in &results.results {
    println!("[{:.3}] {}/{}", hit.similarity, hit.wing, hit.room);
    println!("  {}", &hit.text[..hit.text.len().min(200)]);
}
```

The `search_memories()` function returns:

```rust
pub struct SearchResults {
    pub query: String,
    pub filters: Filters,
    pub results: Vec<SearchHit>,
}

pub struct SearchHit {
    pub text: String,           // "We decided to migrate auth to Clerk because..."
    pub wing: String,            // "myapp"
    pub room: String,           // "auth-migration"
    pub source_file: String,     // "session_2026-01-15.md"
    pub similarity: f32,        // 0.892
}
```

## MCP Search

When connected via MCP, your AI searches automatically:

> *"What did we decide about auth last month?"*

The AI calls `mpr_search` behind the scenes. You never type a search command.

See [MCP Integration](/guide/mcp-integration) for setup.

## Wake-Up Context

Instead of searching, you can load a compact context of your world:

```bash
# Load identity + top memories (~600-900 tokens in typical use)
mpr wake-up

# Project-specific context
mpr wake-up --wing driftwood
```

This loads Layer 0 (identity) and Layer 1 (essential story) as bounded startup context before the first retrieval call.

See [Memory Stack](/concepts/memory-stack) for details on the 4-layer architecture.
