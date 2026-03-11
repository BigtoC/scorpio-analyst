---
description: Run a multi-angle code review for an OpenSpec change.
---

Review the implementation for the requested spec.

<SpecName>
  $ARGUMENTS
</SpecName>

Follow `@AGENTS.md`, `@PRD.md`, `@docs/architect-plan.md`, `@/openspec/AGENTS.md`, and `.github/instructions/rust.instructions.md`.

Review the corresponding OpenSpec proposal and implementation together from 5 perspectives:
- requirements fulfillment
- security implications
- performance impact
- code quality and maintainability
- test coverage

Return:
- key findings grouped by review perspective
- severity for each issue
- missing tests or edge cases
- a final go or no-go recommendation
