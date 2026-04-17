# Module Map

Complete source file reference for the MemPalace codebase.

## Project Structure

```
mempalace_rust/
├── README.md                  ← project documentation
├── src/
│   ├── main.rs                ← CLI entry point
│   ├── mcp_server.rs          ← MCP server (29 tools)
│   ├── commands/
│   │   ├── init.rs            ← init command
│   │   ├── mine.rs            ← mine command
│   │   ├── search.rs          ← search command
│   │   ├── split.rs           ← split command
│   │   ├── compress.rs        ← compress command
│   │   ├── wakeup.rs          ← wake-up command
│   │   ├── repair.rs          ← repair command
│   │   ├── status.rs          ← status command
│   │   ├── hook.rs            ← hook logic
│   │   └── instructions.rs    ← skill instructions
│   ├── palace/
│   │   ├── mod.rs             ← palace core
│   │   ├── drawer.rs          ← drawer management
│   │   ├── search.rs          ← semantic search (embedvec)
│   │   ├── layers.rs          ← 4-layer memory stack
│   │   └── embedvec.rs        ← SQLite vector store
│   ├── knowledge_graph/
│   │   ├── mod.rs             ← knowledge graph core
│   │   ├── entities.rs        ← entity management
│   │   └── triples.rs         ← temporal triples
│   ├── graph/
│   │   ├── mod.rs             ← navigation graph
│   │   ├── rooms.rs           ← room management
│   │   └── tunnels.rs         ← cross-wing tunnels
│   ├── mining/
│   │   ├── mod.rs             ← mining core
│   │   ├── project_miner.rs   ← project file ingest
│   │   ├── convo_miner.rs      ← conversation ingest
│   │   ├── normalizer.rs       ← format converter
│   │   └── extractor.rs       ← memory type extraction
│   ├── detection/
│   │   ├── mod.rs             ← detection core
│   │   ├── entity.rs          ← entity detection
│   │   ├── registry.rs        ← entity registry
│   │   └── rooms.rs           ← room detection
│   ├── dialect/
│   │   ├── mod.rs             ← AAAK compression
│   │   └── abbrev.rs          ← abbreviation system
│   ├── config.rs              ← configuration loading
│   └── split.rs               ← transcript splitting
├── hooks/                     ← Claude Code auto-save hooks
│   ├── mempal_save_hook.sh    ← save every N messages
│   └── mempal_precompact_hook.sh ← emergency save
├── examples/                  ← usage examples
│   ├── basic_mining.sh
│   ├── convo_import.sh
│   └── mcp_setup.md
└── Cargo.toml                 ← package config
```

## Core Modules

### `main.rs` — CLI Entry Point

Clap-based CLI with subcommands: `init`, `mine`, `split`, `search`, `compress`, `wake-up`, `repair`, `status`, `hook`, `instructions`. Dispatches to the corresponding module.

### `mcp_server.rs` — MCP Server

JSON-RPC over stdin/stdout. Implements the MCP protocol with 29 tools covering palace read/write, knowledge graph, navigation, and agent diary operations. Includes the Memory Protocol and AAAK Spec in status responses.

### `search.rs` — Semantic Search

Two functions: `search()` for CLI output and `search_memories()` for programmatic use. Both query embedvec with optional wing/room filters and return verbatim drawer content with similarity scores.

### `layers.rs` — Memory Stack

Four structs (`Layer0` through `Layer3`) and the unified `MemoryStack`. Layer 0 reads identity, Layer 1 auto-generates from top drawers, Layer 2 does filtered retrieval, Layer 3 does semantic search.

### `knowledge_graph/` — Temporal KG

SQLite-backed entity-relationship graph with temporal validity windows. Supports add, invalidate, query, timeline, and stats. Auto-creates entities on triple insertion.

### `graph/` — Navigation Graph

Builds a graph from embedvec metadata where nodes = rooms and edges = tunnels (rooms spanning multiple wings). Supports BFS traversal and tunnel finding.

### `dialect/` — AAAK Compression

Lossy abbreviation system with entity encoding, emotion detection, topic extraction, and flag identification. Works on both plain text and structured zettel data.

## Ingest Modules

### `mining/` — Project Ingest

Scans project directories for code and doc files. Respects `.gitignore`. Files content as drawers tagged with wing/room metadata.

### `convo_miner.rs` — Conversation Ingest

Imports conversation exports (Claude, ChatGPT, Slack, Markdown, plaintext). Chunks by exchange pair. Supports `exchange` and `general` extraction modes.

### `normalizer.rs` — Format Converter

Converts 5 chat formats to a standard transcript format before mining.

### `extractor.rs` — Memory Type Extraction

Classifies conversation content into decisions, preferences, milestones, problems, and emotional context.

## Detection Modules

### `entity.rs` — Entity Detection

Scans file content to auto-detect people and projects using regex patterns and heuristics.

### `registry.rs` — Entity Registry

Manages entity name → code mappings for AAAK dialect.

### `rooms.rs` — Room Detection

Detects rooms from folder structure during `mpr init`.

## Utility Modules

### `config.rs` — Configuration

Loads settings from `~/.mempalace/config.json` and environment variables.

### `split.rs` — Transcript Splitting

Splits concatenated transcripts into per-session files based on session boundary detection.
