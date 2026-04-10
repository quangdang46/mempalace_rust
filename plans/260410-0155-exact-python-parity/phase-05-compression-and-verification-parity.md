# Phase 05 - Align AAAK compression semantics and parity verification suite

## Context Links
- Rust dialect implementation: `/home/quangdang/projects/mempalace_rust/crates/core/src/dialect.rs:6-186`
- Python dialect hotspots from reference grep:
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/dialect.py:545`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/dialect.py:761`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/dialect.py:771`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/dialect.py:951-959`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/dialect.py:1073`
- Python dialect tests: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_dialect.py`
- Python MCP tests: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_mcp_server.py`
- Python config tests: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_config.py`
- Python version/CLI/onboarding/miner tests under `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/`

## Overview
- Priority: High
- Current status: Not started
- Brief description: Match Python AAAK semantics and build the verification layer that proves the Rust port has reached exact parity for approved scope.

## Key Insights
- Rust dialect is currently a lightweight string replacer with reversible decompression semantics; Python treats AAAK as lossy summarization and exposes broader compress-file/compress-all/stat behavior.
- Exact parity cannot be claimed without explicit test mapping from Python reference cases to Rust equivalents.
- Verification must focus on observable outcomes, not implementation language differences.

## Requirements
- Match Python AAAK compression behavior and terminology, especially lossy-summary framing and stats output semantics.
- Build Rust parity tests mirroring the Python reference suite for config, CLI, MCP, onboarding, entity registry, miner, convo miner, and dialect behavior.
- Define a final parity gate that lists approved deviations explicitly and fails on unapproved ones.

## Architecture
- Behavioral parity layer: test vectors and expected outputs copied from Python scenarios.
- Module-level regression layer: Rust unit/integration tests mapped one-for-one from Python test intent.
- Final acceptance layer: one command/report that summarizes parity coverage and open deviations.

## Related Code Files
- Modify:
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/dialect.rs`
  - Rust test modules under the relevant crates
- Reference:
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/dialect.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_dialect.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_convo_miner.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_entity_registry.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_hooks_cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_instructions_cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_mcp_server.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_miner.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_onboarding.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_searcher.py`

## Implementation Steps
1. Rework Rust AAAK behavior to match Python’s lossy summarization model, stats language, and any file/collection compression workflows that are part of the public contract.
2. Translate the Python test suite into Rust parity tests, preserving scenario intent and expected outcomes rather than language-specific mechanics.
3. Add a parity report step that enumerates passed modules, failed modules, and approved deviations.
4. Freeze the parity gate so future changes cannot silently reintroduce divergence.

## Todo List
- [ ] Match Python AAAK semantics and stats framing
- [ ] Port dialect tests
- [ ] Port or mirror config/CLI/miner/convo/MCP/onboarding/entity tests
- [ ] Add final parity report artifact
- [ ] Document approved deviations, if any

## Success Criteria
- Rust AAAK outputs and stats match Python expectations for covered cases.
- Each major Python test area has a Rust parity equivalent.
- Final parity report can state which areas are exact, which are approved deviations, and which still fail.

## Risk Assessment
- Risk: Compression parity may uncover broader metadata differences.  
  Mitigation: Test end-to-end CLI/MCP compression flows, not only helper functions.
- Risk: Translating Python tests mechanically may miss user-visible behavior.  
  Mitigation: Keep each translated test tied to a reference scenario statement.

## Security Considerations
- Do not weaken existing content sanitization while matching AAAK formatting semantics.
- If compressed outputs are stored separately, keep storage paths and overwrite behavior explicit.

## Next Steps
- If all parity gates pass, proceed to Phase 06 for conditional docs and release acceptance.
