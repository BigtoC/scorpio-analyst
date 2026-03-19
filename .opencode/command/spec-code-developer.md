---
agent: build
description: Implement an approved OpenSpec change.
---

Implement the approved OpenSpec change for the requested spec using a TDD workflow and a small agent team.

`<SpecName>`: `$ARGUMENTS`

Follow `@PRD.md`, `@docs/architect-plan.md`.

---

**Input**: Optionally specify a change name via `$ARGUMENTS`. If omitted, check if it can be inferred from conversation context. If vague or ambiguous, prompt for available changes.

**Steps**

1. **Select the change**

   If a name is provided, use it. Otherwise:
   - Infer from conversation context if the user mentioned a change
   - Auto-select if only one active change exists
   - If ambiguous, run `openspec list --json` to get available changes and use the **AskUserQuestion tool** to let the user select

   Always announce: "Using change: <name>"

2. **Check status and understand the schema**
   ```bash
   openspec status --change "<name>" --json
   ```
   Parse the JSON to understand:
   - `schemaName`: the workflow being used (e.g., `"spec-driven"`)
   - Which artifact contains the tasks

3. **Get apply instructions**
   ```bash
   openspec instructions apply --change "<name>" --json
   ```
   This returns:
   - `contextFiles`: file paths to read (proposal, specs, design, tasks — varies by schema)
   - `progress`: total, complete, remaining task counts
   - `tasks`: list with status
   - `state`: `"blocked"` | `"all_done"` | `"proceed"`
   - `instruction`: dynamic guidance for current state

   **Handle states:**
   - If `state: "blocked"` (missing artifacts): show message, suggest using `/opsx-continue`
   - If `state: "all_done"`: congratulate and suggest archive
   - Otherwise: proceed to implementation

4. **Read context files and confirm approval**

   Read all files listed in `contextFiles` from the apply instructions output.
   - Confirm the proposal exists and is approved before coding
   - Check for active related changes that could conflict before editing
   - If the proposal is missing, not approved, blocked by cross-owner approval, or contradicted by current specs — **stop and explain why**

5. **Show current progress**

   Display:
   - Schema being used
   - Progress: "N/M tasks complete"
   - Remaining tasks overview
   - Dynamic instruction from CLI

6. **Form the agent team**

   Create a 4-role agent team to execute the change:
   - **Coordinator**: owns the plan, maps tasks to work slices, keeps changes minimal, and enforces the approval/spec gate
   - **Test Writer**: writes or updates failing tests first for the current task slice and does not edit production code
   - **Code Writer**: edits implementation code only after the new tests fail for the expected reason, then makes the smallest change to pass them
   - **Validator**: runs formatting, lint, and targeted tests; verifies task completion matches reality before any checklist updates

7. **Implement tasks in strict TDD slices (loop until done or blocked)**

   For each unchecked item in `tasks.md`:

   a. Translate the task into a concrete red → green → refactor slice
   b. **Test Writer**: write or update tests that define expected behavior first
   c. Run tests to confirm the new test fails for the expected reason
   d. **Code Writer**: implement the smallest production change to make the test pass
   e. Refactor only if the test suite stays green and the edit stays focused on the approved change
   f. **Validator**: run `cargo fmt -- --check`, `cargo clippy`, targeted `cargo test` (and broader tests when warranted)
   g. Mark task complete: `- [ ]` → `- [x]` in `tasks.md` only after code and tests are actually complete

   **Pause if:**
   - Task is unclear → ask for clarification
   - Implementation reveals a design issue → suggest updating artifacts
   - Error or blocker encountered → report and wait for guidance
   - User interrupts

8. **On completion or pause, show status**

   Display tasks completed this session, overall progress, and next steps.

**Testing Requirements**
- Prefer mocked or deterministic test seams over live APIs or live model calls
- Follow the repository testing strategy: unit tests for isolated behavior, integration tests with deterministic stubs, and property-based tests where the spec calls for them
- For agent work, keep test-writing and code-writing responsibilities separate even if one assistant performs them serially
- Do not skip the failing-test step unless the task is documentation-only or cannot be meaningfully exercised by an automated test; if skipped, explain why

**Output During Implementation**

```
## Implementing: <change-name> (schema: <schema-name>)

Working on task 3/7: <task description>
[...implementation happening...]
✓ Task complete

Working on task 4/7: <task description>
[...implementation happening...]
✓ Task complete
```

**Output On Completion**

```
## Implementation Complete

**Change:** <change-name>
**Schema:** <schema-name>
**Progress:** 7/7 tasks complete ✓

### Completed This Session
- [x] Task 1
- [x] Task 2
...

All tasks complete! You can archive this change with `/opsx-archive`.
```

**Output On Pause (Issue Encountered)**

```
## Implementation Paused

**Change:** <change-name>
**Schema:** <schema-name>
**Progress:** 4/7 tasks complete

### Issue Encountered
<description of the issue>

**Options:**
1. <option 1>
2. <option 2>
3. Other approach

What would you like to do?
```

**Guardrails**
- Keep going through tasks until done or blocked
- Always read context files from `openspec instructions apply` output before starting — do not assume specific file names
- Confirm proposal is approved before writing any code
- If task is ambiguous, pause and ask before implementing
- If implementation reveals issues, pause and suggest artifact updates
- Keep code changes minimal and scoped to each task
- Update task checkbox immediately after completing each task
- Pause on errors, blockers, or unclear requirements — don't guess
- Do not skip the red → green step; record a reason if unavoidable

**Fluid Workflow Integration**
- **Can be invoked anytime**: before all artifacts are done (if tasks exist), after partial implementation, interleaved with other actions
- **Allows artifact updates**: if implementation reveals design issues, suggest updating artifacts — not phase-locked, work fluidly
