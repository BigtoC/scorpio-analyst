<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

## Rust Guidelines

Detailed Rust coding conventions are in `.github/instructions/rust.instructions.md`. Key points:
- Prefer borrowing (`&T`) over cloning; use `&str` over `String` for function params when ownership isn't needed.
- Use `serde` for serialization, `thiserror`/`anyhow` for errors.
- Async code uses `tokio` runtime with `async/await`.
- Implement common traits (`Debug`, `Clone`, `PartialEq`) on public types.
- Use enums over flags/booleans for type safety.
- Warnings are treated as errors in CI (`-D warnings`).

<!-- OPENSPEC:END -->