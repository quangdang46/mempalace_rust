# Rust API

High-level overview of the key Rust interfaces you'd use to integrate MemPalace into your application.

## Search

The primary way to query the palace programmatically.

```rust
use mempalace::searcher::search_memories;

let results = search_memories(
    query="why did we switch to GraphQL",
    palace_path="~/.mempalace/palace",
    wing=Some("myapp".to_string()),           // optional filter
    room=Some("architecture".to_string()),       // optional filter
    n_results=5,
)?;
```

## Memory Stack

The 4-layer memory system with a unified interface.

```rust
use mempalace::layers::MemoryStack;

let stack = MemoryStack::new("~/.mempalace/palace".to_string());

// Wake-up: L0 (identity) + L1 (essential story)
let context = stack.wake_up(wing="myapp")?;  // ~170-900 tokens

// On-demand: L2 retrieval
let recall = stack.recall(wing="myapp", room="auth", n_results=10)?;

// Deep search: L3 semantic search
let results = stack.search("pricing change", wing="myapp")?;

// Status
let status = stack.status()?;
```

## Knowledge Graph

Temporal entity-relationship graph built on SQLite.

```rust
use mempalace::knowledge_graph::KnowledgeGraph;

let kg = KnowledgeGraph::open("~/.mempalace/knowledge.db")?;

// Write
kg.add_triple("Kai", "works_on", "Orion", valid_from="2025-06-01")?;
kg.invalidate("Kai", "works_on", "Orion", ended="2026-03-01")?;

// Read
let facts = kg.query_entity("Kai", as_of=Some("2026-01-15"), direction="both")?;
let relationships = kg.query_relationship("works_on")?;
let timeline = kg.timeline("Orion")?;
let stats = kg.stats()?;
```

## Palace Graph

Room-based navigation graph built from metadata.

```rust
use mempalace::palace_graph::{build_graph, traverse, find_tunnels, graph_stats};

// Build the graph
let (nodes, edges) = build_graph("~/.mempalace/palace")?;

// Navigate
let path = traverse("auth-migration", max_hops=2)?;
let tunnels = find_tunnels(wing_a="wing_code", wing_b="wing_team")?;
let stats = graph_stats()?;
```

## AAAK Dialect

Lossless compression for token density at scale.

```rust
use mempalace::dialect::Dialect;

// Basic
let dialect = Dialect::new();
let text = "We decided to use GraphQL because REST was too chatty for our dashboard.";
let compressed = dialect.compress(text);

// With entity mappings
let dialect = Dialect::with_entities(entities)?;
let compressed = dialect.compress_with_metadata(text, metadata)?;

// Stats
let stats = dialect.compression_stats(text, &compressed);
```

## Configuration

```rust
use mempalace::config::Config;

let config = Config::load()?;
println!("{}", config.palace_path);       // ~/.mempalace/palace
println!("{}", config.collection_name);   // mpr_drawers
```

For detailed parameter documentation, see [API Reference](/reference/api-reference).
