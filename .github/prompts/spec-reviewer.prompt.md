---
description: Review and improve an OpenSpec proposal.
---

Review the existing OpenSpec proposal for the requested spec.

<SpecName>
  $ARGUMENTS
</SpecName>

Follow `@AGENTS.md`, `@PRD.md`, `@docs/architect-plan.md`, and `@/openspec/AGENTS.md`.

Check the OpenSpec docs for `<SpecName>`:
- locate the matching change or proposal
- review `proposal.md`, `tasks.md`, `design.md` if present, and all spec deltas
- verify alignment with product requirements and architect plan
- update the docs if anything is missing, unclear, inconsistent, or invalid
- Check if cross-owner modifications are needed (e.g. config changes, new core types, provider API additions) and if so, add them to the proposal and link to the relevant files in the codebase
- run strict validation after fixes
- report the gaps you found and what you updated
