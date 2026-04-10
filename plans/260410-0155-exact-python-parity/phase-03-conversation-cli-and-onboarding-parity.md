# Phase 03 - Align conversation mining, CLI surface, and onboarding flows

## Context Links
- Rust conversation miner is stubbed: `/home/quangdang/projects/mempalace_rust/crates/core/src/convo_miner.rs:1-22`
- Python conversation miner behavior: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/convo_miner.py:39-260`
- Rust CLI command definitions: `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs:34-174`
- Rust CLI dispatch: `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs:980-1027`
- Python CLI command handlers: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/cli.py:39-258`
- Python CLI parser: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/cli.py:430-570`
- Rust onboarding: `/home/quangdang/projects/mempalace_rust/crates/core/src/onboarding.rs:22-239`
- Python onboarding: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/onboarding.py:24-240`

## Overview
- Priority: High
- Current status: Not started
- Brief description: Match the Python CLI contract and conversation ingestion flow, then align onboarding prompts and output artifacts with the reference.

## Key Insights
- Rust currently exposes a different command surface than Python, including Rust-only `MineDevice` and missing Python `hook`, `instructions`, and `repair` commands.
- Python conversation mining includes exchange-based and general extraction modes; Rust currently returns zeros.
- Onboarding exists in Rust but must be matched to Python prompt flow, defaults, generated files, and entity-seeding behavior.

## Requirements
- Match Python CLI subcommands, options, defaults, and help-contract semantics unless explicitly approved otherwise.
- Implement Python conversation file scanning, chunking, room detection, and extract-mode behavior in Rust.
- Align onboarding prompts, default wings, bootstrap artifacts, and registry seeding to Python reference behavior.

## Architecture
- CLI is the orchestration surface; it should call parity-matched underlying modules rather than reimplement logic inline.
- Conversation mining should mirror Python phases: scan → normalize → chunk/extract → route to room → file drawers.
- Onboarding should mirror Python’s mode/person/projects/wings flow and seed the same downstream registry facts.

## Related Code Files
- Modify:
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/convo_miner.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/onboarding.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/general_extractor.rs` if required by `--extract general`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/split_mega_files.rs`
- Reference:
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/convo_miner.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/onboarding.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_convo_miner.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_convo_miner_unit.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_onboarding.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_cli.py`

## Implementation Steps
1. Reconcile the CLI contract: add missing Python commands/options, remove or hide Rust-only extras that break exact parity, and match Python defaults/help text behavior.
2. Port Python conversation scanning and chunking semantics, including file extensions, file-size cap, exchange chunking, paragraph fallback, topic-room detection, and extract-mode branching.
3. Wire conversation mining into the CLI `mine --mode convos` path so the Rust path matches Python command behavior end to end.
4. Align onboarding interaction flow, wing defaults, people/project capture, auto-detection, and bootstrap artifact generation to the Python reference outputs.

## Todo List
- [ ] Match Python subcommand set and option defaults
- [ ] Implement convo scan/chunk/room/extract behavior
- [ ] Match split-before-mine expectations for transcript workflows
- [ ] Match onboarding prompt sequence and generated artifacts
- [ ] Add CLI/convo/onboarding regression tests

## Success Criteria
- Equivalent CLI commands produce equivalent control flow and user-visible output.
- Conversation mining no longer returns stubbed zeros and passes parity cases.
- Onboarding produces reference-aligned seed artifacts and registry initialization behavior.

## Risk Assessment
- Risk: Exact CLI parity may conflict with existing Rust additions.  
  Mitigation: Treat extras as removable unless the user explicitly preserves them.
- Risk: Interactive onboarding tests may be brittle.  
  Mitigation: Match prompt sequencing and non-interactive defaults first, then snapshot the outputs.

## Security Considerations
- Preserve path and content validation for user-provided directories and names.
- Avoid introducing shell-execution behavior while aligning command flow.

## Next Steps
- Hand off registry/MCP/config dependencies to Phase 04 once CLI and onboarding call sites are stable.
