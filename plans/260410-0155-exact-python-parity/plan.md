# Exact Python Parity Plan

## Goal
Reach exact functional parity for the Rust port against the Python reference in `/home/quangdang/projects/mempalace_rust/references/mempalace-main`, prioritizing matching behavior over Rust-specific improvements.

## Scope
- Rust target: `/home/quangdang/projects/mempalace_rust/crates/core/src/` and `/home/quangdang/projects/mempalace_rust/crates/cli/src/main.rs`
- Python reference: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/`
- Validation target: reference CLI, MCP, onboarding, mining, registry, config/path, hooks/instructions, and compression behavior

## Key parity facts
- Rust conversation mining is still a stub: `crates/core/src/convo_miner.rs:1-22`
- Rust CLI surface diverges from Python: `crates/core/src/cli.rs:34-174`, `:980-1027` vs `references/.../cli.py:39-258`, `:430-570`
- Rust MCP tool catalog diverges materially from Python: `crates/core/src/mcp_server.rs:65-249`, `:338-438` vs `references/.../mcp_server.py:590-829`
- Rust config/path behavior is XDG-extended, while Python defaults to `~/.mempalace`: `crates/core/src/config.rs:155-327` vs `references/.../config.py:115-209`
- Rust onboarding/entity/compression implementations exist but are not yet reference-matched: `onboarding.rs`, `entity_registry.rs`, `dialect.rs`

## Phases
1. [Phase 01 - Build baseline parity matrix](./phase-01-baseline-parity-matrix.md)
2. [Phase 02 - Align storage, search, miner, and project-mining semantics](./phase-02-storage-search-and-project-miner-parity.md)
3. [Phase 03 - Align conversation mining, CLI surface, and onboarding flows](./phase-03-conversation-cli-and-onboarding-parity.md)
4. [Phase 04 - Align MCP tools, entity registry, hooks, instructions, and config/path behavior](./phase-04-mcp-entity-hooks-instructions-config-parity.md)
5. [Phase 05 - Align AAAK compression semantics and parity verification suite](./phase-05-compression-and-verification-parity.md)
6. [Phase 06 - Conditional docs sync and release acceptance](./phase-06-conditional-docs-sync-and-release-acceptance.md)

## Dependencies
- Phase 01 defines the gap matrix and parity contract for all later phases.
- Phase 02 must land before MCP/search verification can be trusted.
- Phase 03 and Phase 04 can proceed in parallel once Phase 01 is approved.
- Phase 05 depends on Phases 02-04.
- Phase 06 is final and conditional on user-visible deltas.

## Guardrails
- Match Python reference behavior exactly unless a deviation is explicitly approved.
- Do not keep Rust-only UX/API additions if they break 1:1 parity.
- Prefer porting reference semantics/tests over redesigning architecture.
- Docs updates are conditional; only sync docs when implementation changes user-visible behavior.

## Open confirmations before execution
- Whether exact parity requires removing or hiding Rust-only commands/options such as `MineDevice`, `--embedding`, and XDG path migration behavior.
- Whether Python’s write-capable MCP tool set must be matched exactly, or whether read-only safety policy is allowed to remain as a deliberate deviation.
- Whether “exact parity” includes Python hook/instruction packaging and hook shell assets, or only equivalent runtime behavior.

## Done when
- Rust command behavior, config/path resolution, MCP tool surface, mining behavior, onboarding flow, registry lookups, and AAAK outputs match Python reference behavior for covered cases.
- A Rust parity test suite demonstrates equivalent outcomes for the Python reference scenarios.
- Any approved intentional deviations are documented explicitly and kept minimal.
