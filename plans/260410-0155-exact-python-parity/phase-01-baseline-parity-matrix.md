# Phase 01 - Build baseline parity matrix

## Context Links
- Rust CLI surface: `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs:34-174`
- Rust CLI dispatch: `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs:980-1027`
- Python CLI surface: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/cli.py:39-258`
- Python CLI parser/dispatch: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/cli.py:430-570`
- Rust MCP tools: `/home/quangdang/projects/mempalace_rust/crates/core/src/mcp_server.rs:65-249`
- Python MCP tools: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/mcp_server.py:590-829`
- Rust config behavior: `/home/quangdang/projects/mempalace_rust/crates/core/src/config.rs:155-327`
- Python config behavior: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/config.py:115-209`

## Overview
- Priority: High
- Current status: Complete
- Brief description: Produce the authoritative gap matrix that maps each Rust module/command/tool to the Python reference behavior and marks exact parity blockers.

## Key Insights
- The largest parity risks are already visible without deeper excavation: conversation mining is stubbed, MCP surfaces differ, and config semantics diverge.
- The Rust CLI already contains non-reference surface area (`MineDevice`, `--embedding`) that needs an explicit parity decision.
- The plan should use the Python tests as the contract wherever feasible, not only source inspection.

## Requirements
- Create a module-by-module parity matrix covering CLI, miner, convo miner, MCP, onboarding, entity registry, config, compression, hooks, and instructions.
- Classify every gap as one of: missing, partial, divergent, extra, or blocked-by-foundation.
- Record exact file targets and expected reference behavior before implementation starts.

## Architecture
- Input layer: Python source + Python tests
- Comparison layer: Rust source + current behavior inventory
- Output layer: one parity matrix document that later phases can execute against without re-scoping

## Related Code Files
- Read:
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/config.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/convo_miner.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/miner.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/mcp_server.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/entity_registry.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/onboarding.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/dialect.rs`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/*.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/*.py`
- Create/update during execution:
  - parity matrix report under the active plan directory
  - task-specific follow-on notes if a reference behavior is ambiguous

## Implementation Steps
1. Inventory the Python reference public surface: CLI commands/options, MCP tools, config files, onboarding prompts, miner modes, hook entrypoints, and test coverage.
2. Inventory the Rust surface with the same schema and mark exact matches, missing behavior, divergent behavior, and Rust-only extras.
3. Build a dependency map that identifies foundation-first work, especially storage/miner/search before MCP/search parity.
4. Freeze the parity contract so later phases implement against a stable checklist rather than reinterpreting the reference.

## Todo List
- [x] Enumerate Python CLI commands and options
- [x] Enumerate Rust CLI commands and options
- [x] Enumerate Python MCP tools and write/read semantics
- [x] Enumerate Rust MCP tools and semantics
- [x] Map each Python test module to the corresponding Rust module
- [x] Publish a single parity matrix with blocker tags

## Authoritative Parity Matrix

Legend: `exact` = behavior already matches reference, `partial` = substantial implementation exists but user-visible behavior still diverges, `missing` = reference behavior absent, `divergent` = implemented but intentionally or accidentally different, `extra` = Rust-only surface not present in Python, `blocked-by-foundation` = should not be finalized until an earlier parity area lands.

| Area | Python reference | Rust surface | Status | Key gaps / exact notes | Owning bead(s) |
|---|---|---|---|---|---|
| CLI command surface | `mempalace/cli.py` commands: `init`, `mine`, `search`, `wake-up`, `split`, `status`, `repair`, `hook`, `instructions`, `mcp`, `compress` | `crates/core/src/cli.rs` commands: `init`, `mine`, `search`, `wake-up`, `compress`, `split`, `status`, `mine-device`, `mcp` | divergent | Missing Python `repair`, `hook`, `instructions`; Rust-only `mine-device`; `mcp` behavior differs (`show setup command` vs `run stdio server`) | `mr-nrt.7`, `mr-nrt.15`, policy bead `mr-nrt.1` |
| CLI option/default semantics | Python `mine` supports `--no-gitignore`, `--include-ignored`, `--agent`, `--limit`, `--dry-run`, `--extract`; search supports `--results`; init has `--yes` | Rust has `--agent`, `--limit`, `--dry-run`, `--extract`, `--results`, `--yes`, plus Rust-only `--embedding`; missing ignore controls | divergent | Rust parser and defaults do not expose Python ignore behavior and add non-reference search option | `mr-nrt.7`, `mr-nrt.3`, `mr-nrt.1` |
| Project traversal / scan semantics | Python `scan_project()` honors `.gitignore`, nested overrides, negation rules, include-ignored overrides, skip-dir exceptions | Rust `miner.rs` uses static skip dirs/files and readable-extension list only | missing | No `.gitignore` parser, no include-ignored override, no parity coverage for nested ignore precedence | `mr-nrt.3`, then `mr-nrt.6` |
| Project chunking / drawer lifecycle | Python miner + palace behavior define chunk counts, source metadata, mtime/idempotency expectations | Rust miner chunks text and batches inserts; drawer IDs are SHA-based; no mtime-aware already-mined contract surfaced | partial | Core chunking exists, but mtime/idempotency parity and metadata contract are not aligned to Python tests | `mr-nrt.4`, then `mr-nrt.6` |
| Search semantics | Python `search_memories()` returns structured results with wing/room filters and CLI `search()` prints formatted output; duplicate checks rely on vector query similarity | Rust `searcher.rs` and MCP search rely on local DB + naive similarity / substring behavior | partial | Basic retrieval exists, but result shape, ranking, filtering guarantees, and duplicate-threshold semantics diverge from Python contract | `mr-nrt.5`, then `mr-nrt.6` |
| Normalize / supported import formats | Python README/package advertises 5 core normalizers plus newer ecosystem support through source tree | Rust `normalize.rs` supports Claude Code, Claude.ai, ChatGPT, Slack, Codex, SoulForge, OpenCode SQLite, Aider, plain text | extra | Rust normalize surface is broader than Python baseline; parity work should preserve Python-observable formats while deciding whether extras remain documented deviations | `mr-nrt.2`, policy bead `mr-nrt.1` |
| Conversation mining pipeline | Python `convo_miner.py` scans transcript-like files, normalizes, chunks by exchange/fallback, routes rooms, supports `exchange` and `general` extraction | Rust `convo_miner.rs` is a 22-line stub returning zeroed counts | missing | Largest functional parity gap in user-facing ingestion | `mr-nrt.8`, `mr-nrt.9`, then `mr-nrt.11` |
| Split transcript workflow | Python `split_mega_files.py` is wired into CLI `split` flow and transcript-prep expectations | Rust `split_mega_files.rs` exists and CLI wiring exists | partial | Feature exists, but end-to-end parity must be validated against Python convo workflow and CLI surface | `mr-nrt.9`, then `mr-nrt.11` |
| Onboarding prompt flow | Python onboarding asks mode, people, projects, wings, optional auto-detect, ambiguity warning, then seeds registry and writes bootstrap artifacts | Rust onboarding implements mode enums, quick setup, bootstrap generation, auto-detect helper, but CLI `init` currently bypasses the full Python flow | partial | Major pieces exist, but prompt sequencing and generated artifacts are not yet reference-matched end to end | `mr-nrt.10`, then `mr-nrt.11` |
| Entity registry semantics | Python registry supports onboarding seed, aliases, ambiguous-word disambiguation, wiki-backed unknown research, learned entities, query extraction | Rust registry supports seed, aliases, ambiguous flags, rejection list, disambiguation, query extraction; no wiki/network research or learned-session flow | partial | Core semantics exist, but unknown handling and persistence model differ materially | `mr-nrt.14`, then `mr-nrt.17`, policy bead `mr-nrt.1` |
| Config precedence / default paths | Python precedence: env vars > `~/.mempalace/config.json` > defaults rooted in `~/.mempalace` | Rust adds XDG/project-dir logic and loads people map separately; defaults are not Python-style | divergent | Config/path behavior is intentionally extended today and blocks CLI/MCP parity claims | `mr-nrt.16`, then `mr-nrt.17`, policy bead `mr-nrt.1` |
| MCP tool catalog | Python exposes 19 named tools including taxonomy, duplicate check, drawer CRUD, AAAK spec, KG, graph, diary | Rust also exposes 19 tools, but with a different catalog (`list_drawers`, `mine`, `get_memory`, `set_config`, etc.) | divergent | Tool names, descriptions, schemas, and surface area do not match the Python client contract | `mr-nrt.12`, policy bead `mr-nrt.1` |
| MCP handler semantics | Python handlers return status + AAAK protocol/spec, search result objects, drawer CRUD, KG operations, diary entry flows, graph traversal/tunnels | Rust handlers provide simplified or placeholder behavior, different result shapes, and missing Python-specific handlers | partial | Server framework exists, but most tool contracts are still non-reference | `mr-nrt.13`, then `mr-nrt.17` |
| Knowledge graph | Python KG tooling supports add/query/invalidate/timeline/stats through MCP; core graph behavior is part of public contract | Rust `knowledge_graph.rs` is substantially implemented with temporal triples and auto-conflict resolution | partial | Core capability exists; public parity depends on MCP shape and exact test alignment | `mr-nrt.13`, `mr-nrt.17` |
| Palace graph / tunnels | Python MCP exposes `traverse`, `find_tunnels`, `graph_stats` | Rust `palace_graph.rs` implements traversal, tunnels, stats, but MCP does not expose the Python-equivalent facade | partial | Underlying graph exists; public contract still missing | `mr-nrt.13`, `mr-nrt.17` |
| AAAK compression semantics | Python `dialect.py` is lossy summarization with file/collection compression workflows and explicit stats framing | Rust `dialect.rs` is a lightweight reversible-ish string replacer with approximate token stats | divergent | Rust behavior contradicts the Python product contract and README framing | `mr-nrt.18`, then `mr-nrt.19` |
| Memory stack / wake-up | Python layers provide L0/L1/L2/L3 wake-up and search story | Rust `layers.rs` has implemented layer types and wake-up/retrieval/search paths | partial | Structural parity exists, but final acceptance still depends on storage/search/config parity below it | `mr-nrt.5`, `mr-nrt.19` |
| Doctor / health checks | Python roadmap/reference includes doctor command and health checks via command surface | Rust `doctor.rs` exists and CLI `doctor` is not wired in current CLI surface shown in README parity plan | extra | Rust has health-check implementation beyond current Python CLI contract; may remain as approved deviation or be hidden | policy bead `mr-nrt.1`, final ledger `mr-nrt.19` |
| Hook runtime | Python has `hooks_cli.py` with `session-start`, `stop`, and `precompact` flows wired through CLI `hook run` | Rust has no visible hook runtime or matching CLI entrypoint | missing | Entire hook flow absent from public Rust command surface | `mr-nrt.15`, then `mr-nrt.17` |
| Instructions runtime | Python has `instructions_cli.py` and packaged instruction outputs wired through CLI | Rust has no visible instructions runtime or assets | missing | Missing command + runtime + packaged outputs | `mr-nrt.15`, then `mr-nrt.17`, policy bead `mr-nrt.1` |
| Version/help/docs parity | Python README/help document a different public contract and benchmark framing than the Rust repo | Rust README intentionally diverges and advertises Rust-specific enhancements and extra providers | blocked-by-foundation | Docs should not be normalized until code-surface decisions and final parity gate land | `mr-nrt.20`, after `mr-nrt.19` |

## Python Test Contract → Rust Mapping

| Python test module | Contracted area | Rust module(s) / path(s) to validate | Primary owning bead |
|---|---|---|---|
| `test_cli.py` | command surface, parser, dispatch, user-visible output | `crates/core/src/cli.rs`, `crates/cli/src/main.rs` | `mr-nrt.7`, `mr-nrt.9`, `mr-nrt.11` |
| `test_config.py`, `test_config_extra.py` | config precedence, defaults, people-map persistence | `crates/core/src/config.rs` | `mr-nrt.16`, `mr-nrt.17` |
| `test_miner.py` | scan rules, `.gitignore`, include overrides, idempotency hooks | `crates/core/src/miner.rs`, `crates/core/src/palace_db.rs` | `mr-nrt.3`, `mr-nrt.4`, `mr-nrt.6` |
| `test_searcher.py` | search result structure, filters, CLI search errors | `crates/core/src/searcher.rs`, `crates/core/src/cli.rs`, `crates/core/src/palace_db.rs` | `mr-nrt.5`, `mr-nrt.6` |
| `test_convo_miner.py`, `test_convo_miner_unit.py` | transcript scanning, chunking, extract modes, room routing | `crates/core/src/convo_miner.rs`, `crates/core/src/general_extractor.rs`, `crates/core/src/normalize.rs` | `mr-nrt.8`, `mr-nrt.9`, `mr-nrt.11` |
| `test_onboarding.py` | prompt sequence, defaults, bootstrap artifacts, quick setup | `crates/core/src/onboarding.rs`, `crates/core/src/cli.rs` | `mr-nrt.10`, `mr-nrt.11` |
| `test_entity_registry.py` | seed, aliases, disambiguation, research/unknown handling | `crates/core/src/entity_registry.rs` | `mr-nrt.14`, `mr-nrt.17` |
| `test_entity_detector.py` | content detection signals and confidence behavior | `crates/core/src/entity_detector.rs` | `mr-nrt.10`, `mr-nrt.14` |
| `test_general_extractor.py` | general extraction into five memory types | `crates/core/src/general_extractor.rs`, `crates/core/src/convo_miner.rs` | `mr-nrt.8`, `mr-nrt.11` |
| `test_hooks_cli.py` | hook command behavior and hook phases | new Rust hook runtime + `crates/core/src/cli.rs` | `mr-nrt.15`, `mr-nrt.17` |
| `test_instructions_cli.py` | instruction outputs and command wiring | new Rust instructions runtime + `crates/core/src/cli.rs` | `mr-nrt.15`, `mr-nrt.17` |
| `test_mcp_server.py` | tool list, schemas, read/write payloads, KG, diary, duplicate checks | `crates/core/src/mcp_server.rs`, `crates/core/src/knowledge_graph.rs`, `crates/core/src/palace_graph.rs` | `mr-nrt.12`, `mr-nrt.13`, `mr-nrt.17` |
| `test_knowledge_graph.py`, `test_knowledge_graph_extra.py` | temporal facts, invalidation, stats, history | `crates/core/src/knowledge_graph.rs` | `mr-nrt.13`, `mr-nrt.17` |
| `test_layers.py` | wake-up context, L0/L1/L2/L3 behavior | `crates/core/src/layers.rs` | `mr-nrt.5`, `mr-nrt.19` |
| `test_dialect.py` | compress/decode/stats/entity/topic/emotion extraction | `crates/core/src/dialect.rs` | `mr-nrt.18`, `mr-nrt.19` |
| `test_normalize.py` | supported transcript/import formats and transcript normalization | `crates/core/src/normalize.rs` | `mr-nrt.8`, `mr-nrt.11` |
| `test_palace_graph.py` | traverse/tunnel/stats behavior | `crates/core/src/palace_graph.rs` | `mr-nrt.13`, `mr-nrt.17` |
| `test_room_detector_local.py` | room mapping heuristics | `crates/core/src/room_detector_local.rs` | `mr-nrt.3`, `mr-nrt.10` |
| `test_split_mega_files.py` | transcript splitting behavior | `crates/core/src/split_mega_files.rs` | `mr-nrt.9`, `mr-nrt.11` |
| `test_spellcheck.py`, `test_spellcheck_extra.py` | name-aware spell correction | `crates/core/src/spellcheck.rs` | final ledger `mr-nrt.19` unless surfaced earlier |
| `test_version_consistency.py` | release/version contract | workspace metadata + docs/release files | `mr-nrt.19`, `mr-nrt.20` |

## Foundation-First Dependency Freeze

1. **Policy decisions must be written first**: `mr-nrt.1` decides whether Rust-only CLI surface, XDG paths, MCP write restrictions, wiki lookup, and hook packaging remain as approved deviations or are removed for strict parity.
2. **This matrix is the execution contract**: every later bead should update code/tests against the row(s) above rather than re-scoping from scratch.
3. **Foundation sequence is fixed**:
   - `mr-nrt.3` → traversal / ignore semantics
   - `mr-nrt.4` → chunking / metadata / drawer lifecycle
   - `mr-nrt.5` → search + duplicate semantics
   - `mr-nrt.6` → lock Phase 02 with regression tests
4. **Public-surface work waits on the foundation contract**:
   - CLI / convo / onboarding: `mr-nrt.7`–`mr-nrt.11`
   - MCP / registry / hooks / config: `mr-nrt.12`–`mr-nrt.17`
   - AAAK / final parity gate / docs: `mr-nrt.18`–`mr-nrt.20`

## Approved-Deviation Questions To Resolve Before Later Phases

These are not implementation details; they change the parity target itself and therefore stay blocked on `mr-nrt.1` until decided:

1. Should Rust-only surfaces (`mine-device`, `--embedding`, `doctor`, broader normalizer coverage) be removed, hidden, or retained as explicit deviations?
2. Should path handling revert to Python-style `~/.mempalace` defaults, or is XDG behavior allowed to remain as a documented deviation?
3. Must the MCP write surface match Python exactly, or is the current safer/alternate Rust posture acceptable as an approved deviation?
4. Must Python’s external/wiki-backed entity research be ported, or can unknown handling remain local-only as a documented difference?
5. Does exact parity include Python hook/instruction packaging and emitted assets, or only runtime-equivalent behavior?

## Completion Note

This document is now the authoritative phase-01 matrix for the `mr-nrt` parity program. Later beads should reference specific matrix rows and test mappings when they claim parity progress.

## Success Criteria
- Every known high-level gap area from the audit is represented in the matrix.
- Every later phase can point back to an explicit parity contract entry.
- No major module remains unclassified as exact, partial, missing, divergent, or extra.

## Risk Assessment
- Risk: Hidden parity gaps outside the audited areas.  
  Mitigation: Use Python test modules as a second inventory source.
- Risk: Over-scoping into improvements.  
  Mitigation: Require each matrix row to cite the Python behavior being matched.

## Security Considerations
- No new security behavior should be designed here; only current reference behavior should be cataloged.
- Note any existing Rust hardening that would be weakened by parity, so the user can explicitly approve it.

## Next Steps
- Hand off the frozen matrix to Phase 02 foundation work.
- Escalate the three known parity decisions: Rust-only commands, MCP write surface, and XDG path behavior.
