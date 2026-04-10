# Phase 02 - Align storage, search, miner, and project-mining semantics

## Context Links
- Rust miner core: `/home/quangdang/projects/mempalace_rust/crates/core/src/miner.rs:1-220`
- Python miner core: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/miner.py:1-220`
- Rust search entrypoints: `/home/quangdang/projects/mempalace_rust/crates/core/src/searcher.rs:41-115`
- Rust DB access surfaced through MCP search: `/home/quangdang/projects/mempalace_rust/crates/core/src/mcp_server.rs:402-438`
- Rust DB API: `/home/quangdang/projects/mempalace_rust/crates/core/src/palace_db.rs:27-162`
- Python MCP duplicate/add/delete expectations: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/mcp_server.py:750-796`

## Overview
- Priority: High
- Current status: Not started
- Brief description: Make project mining, drawer storage, duplicate behavior, and search semantics match the Python reference closely enough that CLI and MCP behavior are built on the same foundations.

## Key Insights
- Python mining semantics include `.gitignore`-aware traversal and explicit include overrides; Rust miner currently uses static skip lists and does not expose equivalent ignore behavior in the current parsed command surface.
- Python stores verbatim chunks and leans on collection semantics; Rust currently uses a SQLite-backed layer with behavior that must emulate reference outcomes, not just internal structure.
- MCP search parity will remain superficial until the underlying duplicate detection and retrieval semantics match.

## Requirements
- Match Python project file selection, skip rules, chunking thresholds, drawer ID semantics, and duplicate behavior.
- Ensure Rust search and duplicate-check flows return equivalent outcomes for the reference scenarios.
- Preserve exact user-visible project mining behavior before broadening internals.

## Architecture
- Storage foundation: Rust `palace_db` must emulate reference drawer lifecycle semantics.
- Ingest foundation: Rust `miner` must replicate Python file discovery, chunking, and metadata.
- Retrieval foundation: Rust `searcher` and duplicate checks must match Python expectations for CLI and MCP consumers.

## Related Code Files
- Modify:
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/miner.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/palace_db.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/searcher.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/normalize.rs` if normalization affects mined content parity
- Reference:
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/miner.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/mcp_server.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_miner.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_searcher.py`

## Implementation Steps
1. Port Python miner traversal rules into Rust, including file extension set, skip filenames, maximum file size handling, and `.gitignore` matching semantics.
2. Align chunking and drawer ID generation so the same source inputs produce equivalent chunk counts and stable identifiers.
3. Implement or refine duplicate-detection behavior to support both miner idempotency and MCP duplicate checking.
4. Validate search behavior against reference expectations, including wing/room filtering and result formatting constraints.

## Todo List
- [ ] Port `.gitignore` matcher and include-ignored semantics
- [ ] Match Python readable-extension and skip-file sets
- [ ] Match Python chunk-size, overlap, and minimum thresholds
- [ ] Match duplicate-check behavior used by CLI/MCP flows
- [ ] Add regression tests mirroring Python miner/search cases

## Success Criteria
- Running equivalent project-mining scenarios yields comparable files-processed, chunks-created, and idempotency behavior.
- Search and duplicate checks behave like the Python reference for the covered tests.
- No later phase needs to compensate for foundation mismatches here.

## Risk Assessment
- Risk: SQLite-backed internals may tempt semantic drift from Chroma-based reference behavior.  
  Mitigation: Verify user-observable outputs, not storage implementation details.
- Risk: Ignore rule parity is easy to under-implement.  
  Mitigation: Port exact rule precedence from Python before optimizing.

## Security Considerations
- Preserve existing input sanitation for file paths and content.
- Do not widen file traversal beyond reference behavior while adding ignore handling.

## Next Steps
- Once project mining and retrieval semantics are stable, Phase 03 can safely align CLI and conversation modes.
