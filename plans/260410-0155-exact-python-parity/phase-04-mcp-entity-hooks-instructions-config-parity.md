# Phase 04 - Align MCP tools, entity registry, hooks, instructions, and config/path behavior

## Context Links
- Rust MCP catalog/instructions: `/home/quangdang/projects/mempalace_rust/crates/core/src/mcp_server.rs:65-273`
- Rust MCP handlers snapshot: `/home/quangdang/projects/mempalace_rust/crates/core/src/mcp_server.rs:338-488`
- Python MCP catalog: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/mcp_server.py:590-829`
- Python MCP protocol/status text: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/mcp_server.py:169-195`
- Rust entity registry load/seed/lookup: `/home/quangdang/projects/mempalace_rust/crates/core/src/entity_registry.rs:119-240`
- Python entity registry semantics: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/entity_registry.py:1-240`
- Rust config load/save/init/path behavior: `/home/quangdang/projects/mempalace_rust/crates/core/src/config.rs:155-327`
- Python config behavior: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/config.py:115-209`
- Python hook runtime: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/hooks_cli.py:17-226`
- Python CLI wiring for hook/instructions/repair: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/cli.py:232-243`, `:492-567`

## Overview
- Priority: High
- Current status: Not started
- Brief description: Close the largest remaining feature gap by matching Python MCP behavior, registry semantics, hook/instruction commands, and config/path resolution rules.

## Key Insights
- Rust MCP currently exposes a different tool set and simplified behavior; Python exposes list_wings, taxonomy, duplicate/add/delete drawer, AAAK spec, KG, graph, and richer diary semantics.
- Rust config currently prefers XDG/project dirs; Python defaults to `~/.mempalace` with simpler precedence.
- Rust has no visible `hook` or `instructions` CLI parity despite Python providing both.
- Entity registry parity matters for onboarding, search interpretation, and later MCP correctness.

## Requirements
- Match the Python MCP tool surface and tool semantics, including read/write behavior, naming, inputs, and outputs.
- Match Python entity lookup/disambiguation behavior, wiki-backed unknown handling, and registry persistence model as closely as practical.
- Match Python hook and instruction command behavior, including session-start/stop/precompact entrypoints and instruction file output.
- Match Python config precedence and default paths unless an intentional deviation is approved.

## Architecture
- MCP parity should be implemented as a facade over the aligned storage/miner/registry/graph layers, not as one-off shortcuts.
- Config parity should be centralized in `config.rs`; downstream modules must stop making independent path assumptions.
- Hook and instruction support should be CLI-first features that use shared runtime helpers.

## Related Code Files
- Modify:
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/mcp_server.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/entity_registry.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/config.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/cli.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/knowledge_graph.rs`
  - `/home/quangdang/projects/mempalace_rust/crates/core/src/palace_graph.rs`
  - add Rust equivalents for hooks/instructions runtime if absent
- Reference:
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/mcp_server.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/entity_registry.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/config.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/hooks_cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/instructions_cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/mempalace/instructions/`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_mcp_server.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_entity_registry.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_config.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_config_extra.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_hooks_cli.py`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/tests/test_instructions_cli.py`

## Implementation Steps
1. Expand the Rust MCP catalog to match the Python tool names and behaviors, especially list_wings, taxonomy, duplicate/add/delete drawer, AAAK spec, graph, knowledge-graph, and diary contracts.
2. Align entity registry persistence, lookup, disambiguation, and unknown-word handling with the Python reference, including ambiguous/common-word logic and any approved external lookup behavior.
3. Add CLI-level `hook` and `instructions` parity, porting Python session-start/stop/precompact semantics and instruction-file output behavior.
4. Reconcile config precedence and path defaults so the Rust port resolves palace/config/people-map paths like Python, unless a documented exception is approved.

## Todo List
- [ ] Match Python MCP tool list and schemas
- [ ] Match MCP response shapes for read and write tools
- [ ] Match registry lookup/disambiguation semantics
- [ ] Add hook command parity
- [ ] Add instructions command parity
- [ ] Reconcile config/path precedence and defaults
- [ ] Add MCP/entity/config/hook/instructions tests

## Success Criteria
- The Rust MCP server exposes the same user-facing tool contract as Python for approved parity scope.
- Entity lookup outcomes for onboarding-known, ambiguous, and unknown terms match the reference expectations.
- `mempalace hook ...` and `mempalace instructions ...` behave like the Python CLI.
- Config file location, palace path resolution, and people-map behavior are reference-matched or explicitly approved deviations.

## Risk Assessment
- Risk: Python MCP write surface may conflict with current Rust read-only protections.  
  Mitigation: Require explicit approval if write-tool parity is intentionally constrained.
- Risk: Python’s wiki lookup may be undesirable in Rust parity.  
  Mitigation: Treat external lookup as a user confirmation point before implementation.

## Security Considerations
- Keep existing sanitization of names/content/paths unless the reference requires weaker behavior and the user explicitly approves that regression.
- If wiki/network lookup is ported, constrain timeout/error handling to reference-equivalent safe bounds.

## Next Steps
- Feed the aligned MCP/config/registry contracts into Phase 05 verification so parity tests assert the final public surface, not interim placeholders.
