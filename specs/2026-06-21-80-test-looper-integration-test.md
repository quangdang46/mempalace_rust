# Planning Spec: Looper Integration Test for mempalace_rust

**Issue:** [#80](https://github.com/quangdang46/mempalace_rust/issues/80) — TEST: Looper integration test for mempalace_rust
**Created:** 2026-06-21
**Status:** Spec complete, pending review

---

## 1. Problem

The Looper autonomous development system needs a real-world GitHub repository to validate its full workflow — from issue pickup to spec creation, implementation, review, and PR delivery. This repository (mempalace_rust) serves as the test target.

The goal is to run Looper through its complete lifecycle against a known, real Rust project and verify every stage produces correct artifacts.

## 2. Goals

### Primary
- Verify Looper can pick up an issue labeled `looper:plan` and assigned to a user.
- Verify Looper's planner agent creates a valid spec document under `specs/`.
- Verify Looper can commit and push the spec to a matching branch.
- Verify Looper can open a PR from that branch.
- Validate that the project's CI passes for the spec-only change (trivial, since only a markdown file is added).

### Secondary (observable from the test)
- Confirm Looper respects the `agent_managed_with_fallback` PR lifecycle policy.
- Confirm Looper uses the correct branch naming convention (`looper/planner/<issue-number>-...`).
- Confirm Looper stamps commits with `Generated-By: looper ...` trailers.
- Confirm Looper creates PRs with appropriate target (`main`) and description.

### Non-goals
- This test does **not** implement any code changes to mempalace_rust itself.
- This test does **not** modify existing tests, source files, CI configurations, or any Rust code.
- This test does **not** alter `.beads/` or issue tracking state beyond what Looper's agent automatically does.

## 3. Scope

| In scope | Out of scope |
|---|---|
| Creating the spec document in `specs/` | Modifying `src/`, `crates/`, or `tests/` |
| Committing to `looper/planner/80-*` branch | Running cargo test / cargo build |
| Opening a PR targeting `main` | Changing CI workflows in `.github/` |
| Validating Looper commit trailers | Stamping disclosure footers on the PR body |
| Agent-managed PR lifecycle | Closing the source issue |

## 4. Approach

### Phase 1 — Spec Creation (current step)
1. Create this spec document at `specs/2026-06-21-80-test-looper-integration-test.md`.
2. Commit the spec on the `looper/planner/80-test-looper-integration-test` branch.
3. Push the branch and open a PR targeting `main`.

### Phase 2 — Review (Looper reviewer agent)
1. Looper's reviewer agent inspects the spec for completeness.
2. Validates all required sections (problem, goals, approach, risks, validation) are present.
3. Requests changes or approves.
4. If approved, Looper marks the spec ready.

### Phase 3 — Implementation (conditional)
1. If the issue body specifies implementation steps beyond spec creation, Looper's worker/fixer agents execute them.
2. For this test issue, the expected outcome is only that the spec PR is created and reviewed — no Rust code changes are needed.

### Phase 4 — Conclusion
1. PR is ready for human review.
2. The test verifies Looper completed the `looper:plan` → spec → PR pipeline successfully.

## 5. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Git worktree is detached or stale | Commit lands on wrong ref | Verify `git branch` and `git status` before each action |
| Branch already has a PR open | Duplicate PR or conflict | Check for existing PR first; reuse if present |
| CI fails on spec-only change | False negative | Spec files are markdown only — CI should pass trivially |
| Other agents modify the worktree simultaneously | Unrelated changes in diff | Follow the "never disturb other agents' work" principle from AGENTS.md |
| Looper can't authenticate with GitHub for push/PR | Pipeline stalls | Fall back to `agent_managed_with_fallback` — agent creates PR manually |
| Branch naming collision | Push rejected | Use the canonical `looper/planner/80-test-looper-integration-test` name; rebase if needed |

## 6. Validation

### Spec correctness
- [ ] Document exists at `specs/2026-06-21-80-test-looper-integration-test.md`
- [ ] Markdown renders without errors
- [ ] All sections (Problem, Goals, Scope, Approach, Risks, Validation) are present

### Git/PR validation
- [ ] Branch exists: `looper/planner/80-test-looper-integration-test`
- [ ] Commit contains the spec file and no other changes
- [ ] Commit message includes `Generated-By: looper ...` trailer
- [ ] PR is open against `main`
- [ ] PR title matches the issue title
- [ ] PR body references the issue (#80)

### Looper workflow validation
- [ ] Planner agent created the spec autonomously
- [ ] Spec was committed and pushed without human intervention
- [ ] PR was opened programmatically

## 7. Dependencies

- **GitHub access:** Write access to `quangdang46/mempalace_rust` (push + PR creation)
- **Looper runtime:** An active Looper instance with planner capabilities enabled
- **No code dependencies:** This spec is self-contained and does not depend on any Rust crate or build artifact
