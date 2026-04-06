---
agent: build
description: Run a multi-angle code review using ce:review, extended with OpenSpec requirements fulfillment.
---
Review the implementation for the requested spec.

`<SpecName>`: `$ARGUMENTS`

## Step 1: OpenSpec Requirements Fulfillment

Read `@PRD.md`, `@docs/architect-plan.md`, and `.github/instructions/rust.instructions.md`.

Find the OpenSpec proposal for `<SpecName>` under `openspec/changes/`. Read the proposal and its tasks. For each requirement and task, check whether the implementation addresses it and report:
- met
- partially addressed (with what's missing)
- not addressed

## Step 2: Code Review

Run `/ce:review <SpecName>`. The spec tasks from Step 1 serve as the requirements — pass them via `plan:` only if a plan document also exists under `docs/plans/`. This covers security, performance, code quality, maintainability, and test coverage.

## Return

Combine both steps into a single report:
- OpenSpec requirements fulfillment checklist
- `ce:review` findings (severity, route, reviewer)
- Missing tests or edge cases
- A final go/no-go recommendation
