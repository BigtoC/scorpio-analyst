---
agent: build
description: Implement an approved OpenSpec change.
---
Implement the approved OpenSpec change for the requested spec using a TDD workflow and a small agent team.

`<SpecName>`: `$ARGUMENTS`

Follow `@AGENTS.md`, `@PRD.md`, `@docs/architect-plan.md`, and `@/openspec/AGENTS.md`.

For `<SpecName>`:
- find the corresponding OpenSpec change and confirm the proposal exists and is approved before coding
- review `openspec/project.md`, the relevant capability spec(s), `proposal.md`, `design.md` if present, and `tasks.md`
- check for active related changes that could conflict before editing

Create an agent team with 4 roles to execute the change:
- **Coordinator**: owns the plan, maps tasks to work slices, keeps changes minimal, and enforces the approval/spec gate
- **Test Writer**: writes or updates failing tests first for the current task slice and does not edit production code
- **Code Writer**: edits implementation code only after the new tests fail for the expected reason, then makes the smallest change to pass them
- **Validator**: runs formatting, lint, and targeted tests; verifies task completion matches reality before any checklist updates

Execute the change in strict TDD slices:
1. choose the next unchecked item in `tasks.md`
2. translate it into a concrete red → green → refactor slice
3. have the **Test Writer** add or update tests that define the expected behavior first
4. run the relevant tests to confirm the new test fails for the expected reason
5. have the **Code Writer** implement the smallest production change needed to make that test pass
6. refactor only if the test suite stays green and the edit remains focused on the approved change
7. repeat sequentially for each remaining unchecked task

Testing requirements:
- prefer mocked or deterministic test seams over live APIs or live model calls
- follow the repository testing strategy: unit tests for isolated behavior, integration tests with deterministic stubs, and property-based tests where the spec calls for them
- for agent work, keep test-writing and code-writing responsibilities separate even if one assistant performs them serially
- do not skip the failing-test step unless the task is documentation-only or cannot be meaningfully exercised by an automated test; if skipped, explain why

Completion rules:
- update `tasks.md` only after the corresponding code and tests are actually complete
- mark completed items as `- [x]` and leave untouched work unchecked
- run relevant validation (`cargo fmt -- --check`, `cargo clippy`, targeted `cargo test`, and broader tests when warranted by the change)
- report what changed, which tests were added first, which code changes made them pass, and the final validation results
- if the proposal is missing, not approved, blocked by cross-owner approval, or contradicted by the current specs, stop and explain why
