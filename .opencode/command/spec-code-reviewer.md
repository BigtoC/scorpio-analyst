---
agent: build
description: Run a multi-angle code review for an OpenSpec change.
---
Review the implementation for the requested spec.

<SpecName>
  $ARGUMENTS
</SpecName>

Follow `@AGENTS.md`, `@PRD.md`, `@docs/architect-plan.md`, `@/openspec/AGENTS.md`, and `.github/instructions/rust.instructions.md`.

Create an agent team to review `<SpecName>` and report findings from 5 perspectives:
- requirements fulfillment
- security implications
- performance impact
- code quality and maintainability
- test coverage

Review the corresponding OpenSpec proposal and implementation together.
Return:
- key findings by reviewer
- severity for each issue
- missing tests or edge cases
- a final go/no-go recommendation
