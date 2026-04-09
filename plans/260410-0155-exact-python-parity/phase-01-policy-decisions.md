# Phase 01 - Exact-Parity Policy Decisions

## Status
- Priority: High
- Current status: Complete
- Bead: `mr-nrt.1`
- Purpose: Freeze the policy choices that define the parity target so downstream implementation beads do not make conflicting assumptions.

## Decision 1 — Rust-only CLI and public surface

### Decision
For exact-parity work, the public CLI and MCP surfaces should match the Python reference by default. Rust-only public surface area that changes user-visible behavior should be removed, hidden, or treated as non-parity internal capability until the parity program finishes.

### Applies to
- CLI command `mine-device`
- CLI option `search --embedding`
- Any other user-visible command/flag not present in the Python reference
- Rust-only MCP/public affordances that are not part of the Python contract

### Required downstream behavior
- `mr-nrt.7` must make the default/public CLI surface match Python.
- `mr-nrt.12` must make the MCP catalog match Python tool names and schemas.
- Rust-only capabilities may remain as internal code paths if they are not exposed through the parity-facing public contract.

### Why this choice
The plan goal is exact Python parity, not “Python parity plus Rust conveniences.” Keeping extra public commands/options would create a second contract and invalidate 1:1 parity claims.

---

## Decision 2 — Path semantics and config defaults

### Decision
The parity target is the Python path contract: default config and palace behavior should resolve through `~/.mempalace` semantics, with Python-style env/config precedence. XDG behavior should not remain the default parity behavior.

### Applies to
- Config file default location
- Palace path default location
- People-map default location
- Any caller currently assuming XDG/project-dir fallbacks as the primary behavior

### Required downstream behavior
- `mr-nrt.16` must make Python-style path/default behavior the parity-facing default.
- Compatibility helpers for XDG migration may remain only if they do not alter the default contract and are documented as implementation detail rather than parity behavior.

### Why this choice
The Python reference is explicit and simpler. Allowing XDG as the default would preserve a meaningful behavioral divergence across CLI, MCP, onboarding, and persistence.

---

## Decision 3 — MCP write surface

### Decision
The parity target includes the Python write-capable MCP tool surface. Rust should expose the same write/read behavior at the public-contract level for parity mode.

### Applies to
- Drawer add/delete operations
- KG mutation tools
- Diary write operations
- Any current Rust-only restriction that removes a Python write capability from the parity-facing MCP contract

### Required downstream behavior
- `mr-nrt.12` must align the advertised tool catalog and schemas to the Python contract.
- `mr-nrt.13` must align handler outputs and side effects to Python behavior.
- Existing sanitization, validation, and error-hardening may remain as long as they do not change the public contract for valid inputs.
- Optional read-only/safe modes may exist only as explicitly non-parity operational modes, not as the default parity contract.

### Why this choice
Exact parity cannot exclude Python write tools while still claiming the same MCP surface. Safety features are allowed only when they are additive and do not change behavior for valid, parity-covered flows.

---

## Decision 4 — External entity lookup behavior

### Decision
The parity target includes Python-style unknown-entity research behavior. Rust should port the external/wiki-backed lookup path used by the Python registry flow.

### Applies to
- Unknown-word/entity research in entity registry logic
- Cache/confirmation behavior associated with researched entities

### Required downstream behavior
- `mr-nrt.14` must implement Python-equivalent research handling, including persistence semantics and user-confirmable outcomes.
- Network behavior should use bounded timeout/error handling and degrade gracefully, but the presence of the research path itself is part of the parity contract.

### Why this choice
Without the external lookup path, Rust would keep a materially different entity-resolution model for unknown terms. That is a real behavioral gap, not an implementation detail.

---

## Decision 5 — Hooks, instructions, and packaging scope

### Decision
The parity target requires runtime-equivalent hook and instructions behavior, plus any packaged instruction assets needed to satisfy the Python CLI/runtime contract. It does not require byte-for-byte shell-script packaging if equivalent runtime behavior and user-visible outputs are preserved.

### Applies to
- `mempalace hook ...` CLI behavior and supported phases
- `mempalace instructions ...` CLI behavior and emitted content
- Instruction assets referenced by the runtime/CLI contract
- Hook shell assets and packaging conventions

### Required downstream behavior
- `mr-nrt.15` must implement the Python hook phases and instructions outputs in the Rust CLI/runtime.
- If Python examples or documented outputs depend on packaged instruction content, equivalent assets must ship in Rust.
- Shell implementation details may differ if the resulting behavior, outputs, and user-facing contract match Python.

### Why this choice
The behavioral contract matters more than identical packaging internals. Requiring byte-for-byte shell packaging would over-constrain implementation without improving parity for users or tests.

---

## Approved downstream assumptions

All later beads should assume the following without reopening scope:

1. **Public contract first** — Python-visible CLI/MCP behavior wins over Rust-only convenience.
2. **Python default paths win** — `~/.mempalace` semantics define parity.
3. **Write-capable MCP is in scope** — parity includes Python read/write flows.
4. **External entity research is in scope** — Rust must port it.
5. **Runtime-equivalent hooks/instructions are required** — exact packaging internals are not.

## Reference points for later beads

- Use this file together with `phase-01-baseline-parity-matrix.md`.
- If a later bead finds an implementation tradeoff, it must preserve these decisions rather than redefining parity scope.
- The final parity ledger (`mr-nrt.19`) should cite these decisions as the approved baseline rather than restating them from scratch.
