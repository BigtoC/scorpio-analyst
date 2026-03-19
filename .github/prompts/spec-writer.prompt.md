---
description: Create a new OpenSpec proposal for a planned spec.
---

Write a new spec that is planned in `docs/architect-plan.md`.

`<SpecName>`: `$ARGUMENTS`

Read `@PRD.md`, `@docs/architect-plan.md`.

I'll create a change with artifacts:
- proposal.md (what & why)
- design.md (how)
- tasks.md (implementation steps)
- spec delta files under `openspec/changes/<change-id>/specs/`

When ready to implement, run `/opsx:apply`.

---

**Input**: The argument is the spec name (kebab-case from `docs/architect-plan.md`), OR a description of what to build.

**Steps**

1. **If no input provided, ask which spec to build**

   Use the **AskUserQuestion tool** (open-ended, no preset options) to ask:
   > "Which planned spec from `docs/architect-plan.md` do you want to work on? Describe what you want to build or fix."

   From their description, derive a kebab-case name (e.g., "add researcher debate" → `add-researcher-debate`).

   **IMPORTANT**: Do NOT proceed without understanding what the user wants to build.

2. **Review existing context**

   Before creating anything:
   - Review `docs/architect-plan.md` to understand the planned spec and its scope
   - Review existing changes in `openspec/changes/` to avoid duplication
   - Choose a unique verb-led change ID

3. **Create the change directory**
   ```bash
   openspec new change "<name>"
   ```
   This creates a scaffolded change at `openspec/changes/<name>/` with `.openspec.yaml`.

4. **Get the artifact build order**
   ```bash
   openspec status --change "<name>" --json
   ```
   Parse the JSON to get:
   - `applyRequires`: array of artifact IDs needed before implementation (e.g., `["tasks"]`)
   - `artifacts`: list of all artifacts with their status and dependencies

5. **Create artifacts in sequence until apply-ready**

   Use the **TodoWrite tool** to track progress through the artifacts.

   Loop through artifacts in dependency order (artifacts with no pending dependencies first):

   a. **For each artifact that is `ready` (dependencies satisfied)**:
      - Get instructions:
        ```bash
        openspec instructions <artifact-id> --change "<name>" --json
        ```
      - The instructions JSON includes:
        - `context`: Project background (constraints for you - do NOT include in output)
        - `rules`: Artifact-specific rules (constraints for you - do NOT include in output)
        - `template`: The structure to use for your output file
        - `instruction`: Schema-specific guidance for this artifact type
        - `outputPath`: Where to write the artifact
        - `dependencies`: Completed artifacts to read for context
      - Read any completed dependency files for context
      - Create the artifact file using `template` as the structure
      - Apply `context` and `rules` as constraints - but do NOT copy them into the file
      - Show brief progress: "Created <artifact-id>"

   b. **Continue until all `applyRequires` artifacts are complete**
      - After creating each artifact, re-run `openspec status --change "<name>" --json`
      - Check if every artifact ID in `applyRequires` has `status: "done"` in the artifacts array
      - Stop when all `applyRequires` artifacts are done

   c. **If an artifact requires user input** (unclear context):
      - Use **AskUserQuestion tool** to clarify
      - Then continue with creation

6. **Create spec delta files**

   Create the required spec delta files under `openspec/changes/<change-id>/specs/` based on the design artifact.

7. **Validate the change**
   ```bash
   openspec validate <change-id> --strict
   ```

8. **Show final status**
   ```bash
   openspec status --change "<name>"
   ```

**Output**

After completing all artifacts, summarize:
- Change name and location
- List of artifacts created with brief descriptions
- What's ready: "All artifacts created! Ready for implementation."
- Prompt: "Run `/opsx:apply` to start implementing."

**Artifact Creation Guidelines**

- Follow the `instruction` field from `openspec instructions` for each artifact type
- The schema defines what each artifact should contain - follow it
- Read dependency artifacts for context before creating new ones
- Use `template` as the structure for your output file - fill in its sections
- **IMPORTANT**: `context` and `rules` are constraints for YOU, not content for the file
  - Do NOT copy `<context>`, `<rules>`, `<project_context>` blocks into the artifact
  - These guide what you write, but should never appear in the output

**Guardrails**
- Create ALL artifacts needed for implementation (as defined by schema's `apply.requires`)
- Always read dependency artifacts before creating a new one
- If context is critically unclear, ask the user - but prefer making reasonable decisions to keep momentum
- If a change with that name already exists, ask if user wants to continue it or create a new one
- Verify each artifact file exists after writing before proceeding to next
- If the request is ambiguous, ask only the minimum clarifying question needed
