# Phase 06 - Conditional docs sync and release acceptance

## Context Links
- Rust README: `/home/quangdang/projects/mempalace_rust/README.md`
- Rust install script: `/home/quangdang/projects/mempalace_rust/install.sh`
- Python README: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/README.md`
- Python hooks dir: `/home/quangdang/projects/mempalace_rust/references/mempalace-main/hooks/`

## Overview
- Priority: Medium
- Current status: Not started
- Brief description: Sync docs only where parity work changed user-visible behavior, then perform final release-readiness acceptance against the Python contract.

## Key Insights
- The request explicitly says docs sync only if implementation would affect docs.
- CLI/MCP/config/path parity work is likely to affect README/install/help examples, but internal refactors should not trigger docs churn.
- Final acceptance should confirm both behavior and user-facing packaging.

## Requirements
- Update docs only when parity work changes user-visible commands, options, path behavior, hook setup, MCP setup, or onboarding guidance.
- Verify install/setup instructions remain correct after parity changes.
- Publish a short final acceptance summary with exact parity status and any approved deviations.

## Architecture
- Docs pass is conditional and narrow: README, install/setup snippets, and hook/MCP examples only if needed.
- Acceptance pass consumes outputs from Phase 05 parity tests and the final gap matrix.

## Related Code Files
- Update only if needed:
  - `/home/quangdang/projects/mempalace_rust/README.md`
  - `/home/quangdang/projects/mempalace_rust/install.sh`
  - any future docs under `/home/quangdang/projects/mempalace_rust/docs/` if created later
- Reference:
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/README.md`
  - `/home/quangdang/projects/mempalace_rust/references/mempalace-main/hooks/README.md` if relevant

## Implementation Steps
1. Compare post-parity Rust user-facing behavior against current Rust docs to determine whether documentation drift exists.
2. Update only the sections affected by approved parity changes, especially CLI examples, MCP setup, hook usage, and path/config behavior.
3. Run a final release-acceptance pass that checks docs/examples against the implemented behavior and the Phase 05 parity gate.
4. Publish a concise final summary: exact parity reached, approved deviations, remaining blockers if any.

## Todo List
- [ ] Diff current docs against final parity behavior
- [ ] Update only user-visible drift
- [ ] Recheck install/MCP/hook examples
- [ ] Publish final acceptance summary

## Success Criteria
- No user-facing command or setup example in Rust docs contradicts the implemented parity behavior.
- Final summary clearly states whether exact parity was achieved and what, if anything, remains intentionally different.

## Risk Assessment
- Risk: Unnecessary docs churn obscures the actual parity work.  
  Mitigation: Limit edits to user-visible behavior changes only.
- Risk: Install/setup examples drift from the final command surface.  
  Mitigation: Validate examples against the final CLI and MCP behavior.

## Security Considerations
- Ensure docs do not encourage unsafe paths, leaked secrets, or unsupported write behavior.
- Keep hook/install examples aligned with the final approved safety posture.

## Next Steps
- If acceptance is clean, hand off to implementation execution.
- If any blocker remains, reopen the relevant phase rather than patching around it in docs.
