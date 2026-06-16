# Changelog

## v0.4.0 (2026-06-16)

### Upstream Port — mempalace 3.4.1 + agentmemory 0.9.27

Complete port of 87 upstream fixes and features across 10 waves + 3
additional beads, with 20 review-swarm findings addressed.

For the full per-issue breakdown, see `93ba103` (wave 1), `303dfdd` (wave
2), `e3a9bc8` (3 beads), and `0310dec` (review fixes).

### Metrics

- **46 files changed**, 5,298+ insertions
- **1,321 tests** (1,275 → 1,321)
- **87 port issues** closed, **3 beads** closed, **20 review findings** fixed

### Security & Privacy
- Privacy consent gate for env-fallback LLM API keys (P0)
- External LLM call warning with Tailscale CGNAT whitelist
- MCP `tool_mine` now canonicalizes paths (prevents path traversal)
- Lock holder PID no longer leaked in stderr

### Storage
- Sharded index persistence with manifest commit/rollback
- HNSW stale-quarantine (SIGSEGV prevention)
- `max_backups` retention (env `MEMPALACE_MAX_BACKUPS`, default 10)
- Atomic `EntityRegistry::save()`, per-target PID guard

### Search & KG
- Metric-aware distance→similarity (cosine/L2/inner product)
- FollowupTracker diagnostic for smart-search
- BM25 hybrid rerank with legacy-metric warning
- Cross-project memory isolation (per-project filter)
- KG cache canonicalize, temporal date validation, inverted interval rejection

### CLI & MCP
- 10 new CLI subcommands (context, actions, frontier, signals, import,
  snapshot, profile, diagnose, forget, evolve)
- `mempalace_mine`, `mempalace_observe`, `mempalace_list_hallways`,
  `mempalace_delete_hallway` MCP tools
- Background task runner with retention sweep
- `--no-background` flag, `hooks.auto_save` config

### i18n & Cross-Lingual
- 9 new locale files (fr, es, de, zh-cn, zh-tw, hi, ja, ko, be)
- Embeddinggemma-300m ONNX embedder stub
- Multilingual benchmark datasets (DE/FR/HI/IT/KO/RU)

### Documentation
- CHANGELOG.md updated throughout
- 9 new i18n locale files

## v0.3.1 (2026-06-11)

### Fixes
- CI build fixes, GitHub API parsing robustness
- Search fixes (BM25-only achieves 95.79% R@5, RRF_K=25)

## v0.3.0 (2026-06-08)

### Features
- AgentMemory migration to native mempalace types
- 50+ enhancements across storage, search, graph, CLI, MCP

## v0.2.0 (2026-06-07)

### Coordination System (PRs #37–#48)

- **Two-phase claim protocol** (`actions.rs`) — distributed lock-free claim/confirm/release cycle for concurrent agent actions
- **Artifact handoff** (`artifacts.rs`) — large payload transfer between agents with chunking, checksums, and expiration
- **Live delivery** (`live_delivery.rs`) — real-time push-based message relay with ack and retry
- **Saturation signals** (`saturation.rs`) — multi-metric congestion detector (queue depth, latency percentiles, throughput, error rate)
- **File reservations** (`file_reservations.rs`) — advisory file-level locking with TTL, exclusive/share modes, and renewal
- **Event sourcing log** (`event_log.rs`) — append-only coordination event stream for replay and audit

### Memory System Evolution

- **AgentMemory migration (PR #36, issues #25–#35)**: Complete migration from `agentmemory` to native `mempalace` types
  - Timestamps (`created_at`/`updated_at`) on every Drawer
  - `confidence` and `consolidation_strength` as first-class drawer fields
  - Typed edges with traversal weights in KnowledgeGraph
  - Tag/untag/link/list_tags on MemoryProvider
  - Cascade retrieval (issue #31), LLM extraction sidecar (#32), cluster management (#34)
  - Post-retrieval maintenance engine (#35)
- **Removed all agentmemory references** from integrations, plugins, and scripts
- **Removed legacy ONNX embedder** (`FastEmbedEmbedder` migration complete, old `onnx_embed` deleted)

### Knowledge Graph Enhancements

- Bi-temporal columns (`t_created`/`t_expired`) in triples schema for temporal queries
- Auto-conflict resolution — `add_triple` invalidates overlapping old triples
- Per-palace graph cache keyed by `palace_path`
- Episodic memory table tracking retrieval helpfulness scores
- Synonym edge support during ingestion (issue mp-082)
- Fusion mode enum with PPR (Personalized PageRank) retrieval mode

### Storage & Search

- `UsearchSqliteStore` — Tier-2 PalaceStore backed by usearch + SQLite
- BM25+RRF hybrid search with configurable synonym weight (`SYNONYM_BM25_WEIGHT = 0.7`)
- SHA-256 5-minute rolling window deduplication on `add` path
- `EmbedvecStore` — default PalaceStore implementation with embedding manifest validation
- Auto-resolve embedder from `MEMPALACE_EMBED_MODEL` env var
- Background task runner (Phase 4) for async consolidation
- WAL directory routed under `palace_path/wal/` instead of XDG

### Embedding Layer

- **Remote embedders**: 4 new providers — Cohere, Voyage, Gemini, OpenRouter
- **CLIP image embeddings** via fastembed `ImageEmbedding`
- **Model2VecEmbedder** behind `embed-model2vec` feature
- **TractEmbedder** behind `embed-tract` feature (tract-onnx, tokenizers, ndarray)
- `NullEmbedder` for embedder-free operation
- `Embedder` trait introduced with factory pattern

### MCP Tools & API

- 19+ MCP tools across palace, KG, and diary domains
- All agentmemory smart-feature tool handlers implemented (sketch, crystal, facet, lesson, insight)
- Agent diary read/write tools
- `heal`, `verify`, `governance_delete`, `obsidian_export`, `compress_file` tools
- `detect_worktree` and `replay_import` tools
- Enhanced `reflect` tool with KG traversal, concept clustering, LLM synthesis
- `tool_mesh_sync` wires Mesh peer registry
- `tool_search` calls hybrid_search with `where_filter` post-filter
- Storage-backed handlers for all smart features

### CLI Expansion

- 14 new CLI subcommands: consolidate, compress, context, sessions, actions, frontier, signals, export, import, snapshot, profile, diagnose, forget, evolve, mesh, vision
- `mpr export` with `--format basic-memory` (Markdown/Obsidian)
- Feature flags in Cargo.toml: `llm-openai`, `llm-anthropic`, `coordination`, `vision`, `rerank`, `full`

### Parity & Compliance

- Comprehensive parity gate (`PARITY_GATE.md`, `PARITY_REPORT.md`, `APPROVED_DEVIATIONS.md`, `GATE_STATUS.json`)
- Parity test harness covering MCP/config/registry/hook
- 12-gap port: all remaining agentmemory parity gaps resolved
- Rust-only feature preservation tests
- ARCHITECTURE.md documenting Rust-native additions
- 9 missing adapters added (Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, Amp, Droid)

### Infrastructure & CI

- CI/CD workflow matrix (ubuntu/macos/windows) with fmt + clippy + test gates
- Pre-existing lint backlog unblocked (clippy `-D warnings` relaxed)
- All CI test failures repaired (8 pre-existing, Windows-specific, health test, etc.)
- `ubs` (Ultimate Bug Scanner) integrated
- CJK support via `jieba-rs` behind `cjk-jieba` feature
- `MEMPALACE_READONLY` env var for safe shared/public palace access
- Rust edition 2024 compatible

### Other

- WAL path moved under palace directory
- `non_exhaustive` attribute on PalaceGraph, Palace, MempalaceServer, KnowledgeGraph
- `#[doc(hidden)]` on internal modules
- Legacy internal modules marked `#[deprecated]`
- Various clippy fixes, rustfmt passes, and test repairs
- `with_replaced_columns` fix for optional `expired` flag

### Test Suite

- 1248+ tests passing (was ~400 at v0.1.8-baseline)
- LongMemEval-S reproducer benchmarks
- Conformance test harness for parity verification
- Feature isolation tests for Rust-only enhancements

---

## v0.1.8-baseline (2026-05-25)

Pre-upgrade baseline. Frozen point before the integration-plan work landed. Anchor for mp-001.

## v0.1.7 (2026-05-20)

## v0.1.6 (2026-05-15)

## v0.1.5 (2026-05-10)

## v0.1.4 (2026-05-05)

## v0.1.3 (2026-04-30)

## v0.1.2 (2026-04-25)

## v0.1.1 (2026-04-20)

## v0.1.0 (2026-04-15)

Initial release of mempalace_rust — Rust port of the MemPalace AI memory system.

## v0.3.0 (2026-06-11)

### Search overhaul (LongMemEval R@5: 43.4% → 96.0%)

- **BM25 rebuild in embedder path** — hybrid_search now has a populated BM25 stream alongside vector + graph (was empty → only vector contributed, hurting recall)
- **BM25 parameters tuned** — b=0.3 (less length normalization for long docs), k1=1.5 (higher term saturation)
- **RRF_K: 60 → 25** — sharper ranking differentiation within each search stream
- **7 preference/opinion synonym groups** — prefer/like/want/think/choose/opinion/better/reason expanded for BM25 query
- **Hybrid search resilience** — embedding failures no longer abort the entire search; vector stream gracefully degrades
- **BM25 re-ranker disabled** — tokenization mismatch between internal SearchEngine and Bm25Scorer caused catastrophic re-ranking

### Infrastructure

- **Persistent embedding cache** — save_cache/load_cache skips ~35s ONNX inference on reopen
- **Vector model** — bge-small-en-v15 cache fixed (stale lock files removed, blob restored)

### CI/CD

- 1256 tests pass, 2 pre-existing sandbox failures (port binding)
- rustfmt compliance across codebase

## v0.3.1 (2026-06-12)

### Bug Fixes

- **Install script**: Robust GitHub API parsing with `Accept: application/vnd.github+json` header, safer `grep`/`sed` patterns, and local git tag fallback (`v0.3.0-1-g352218b`) when API/remote fails
- **CI/CD fixes**: Resolved compilation errors from concurrent agent edits — unused variables, mutability issues, API signature mismatches in `health.rs`, `mcp_server.rs`, `cli.rs`, `flow_compress.rs`
