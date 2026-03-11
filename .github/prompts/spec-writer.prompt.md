---
description: Create a new OpenSpec proposal for a planned spec.
---

Write a new spec that is planned in `docs/architect-plan.md`.

<SpecName>
  $ARGUMENTS
</SpecName>

Follow `@AGENTS.md`, `@PRD.md`, `@docs/architect-plan.md`, and `@/openspec/AGENTS.md`.

Create an OpenSpec change proposal for `<SpecName>`:
- review existing OpenSpec context first
- choose a unique verb-led change id
- create `proposal.md`, `tasks.md`, and `design.md` when needed
- create the required spec delta files under `openspec/changes/<change-id>/specs/`
- validate with `openspec validate <change-id> --strict`
- if the request is ambiguous, ask only the minimum clarifying question needed
