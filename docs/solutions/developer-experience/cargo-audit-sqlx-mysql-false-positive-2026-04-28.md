---
title: cargo audit false positive from inactive sqlx-mysql path
date: 2026-04-28
category: developer-experience
module: dependency verification
problem_type: developer_experience
component: tooling
severity: medium
applies_when:
  - workspace verification includes cargo audit
  - sqlx is used with selected database backends only
  - a transitive crate enables broader sqlx feature defaults than the app needs
tags: [cargo-audit, sqlx, graph-flow, dependency-audit, rustsec]
---

# cargo audit false positive from inactive sqlx-mysql path

## Context

The workspace verification gate for the `rig-core 0.35.0` and `graph-flow 0.5.1` upgrade included `cargo audit`. Audit kept reporting `RUSTSEC-2023-0071` on `rsa 0.9.10` through `sqlx-mysql 0.8.6`, even after the workspace was trimmed to SQLite-only usage and the active feature tree showed no MySQL path.

## Guidance

Treat this as a dependency-verification issue, not an application-runtime issue.

Use the active feature graph to distinguish between a real runtime path and an inactive lockfile entry:

```bash
cargo tree -e features -i sqlx@0.8.6 --workspace --target all --all-features
cargo tree -e features -i sqlx-mysql@0.8.6 --workspace --target all --all-features
```

In this repo the fix had two parts:

1. Trim the workspace `sqlx` dependency to the features Scorpio actually uses.

```toml
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "sqlite", "macros", "migrate"] }
```

2. Add a narrow `.cargo/audit.toml` ignore for `RUSTSEC-2023-0071` because `cargo audit` still scans the inactive `sqlx-mysql` lockfile entry even when the feature tree proves the path is not active.

```toml
[advisories]
ignore = ["RUSTSEC-2023-0071"]
```

Also refresh the lockfile when patch-level RustSec fixes become available elsewhere in the graph. In this case `cargo update -p rustls-webpki --precise 0.103.13` cleared `RUSTSEC-2026-0104`.

During investigation, `graph-flow 0.5.1` was identified as the transitive crate that keeps broader `sqlx` defaults in the lockfile. That was useful for root-cause analysis, but vendoring `graph-flow` locally was unnecessary because the active feature tree still showed no `sqlx-mysql` path.

## Why This Matters

`cargo audit` works from `Cargo.lock`, not from the same active-feature view that `cargo tree -e features` exposes. That means an advisory can still appear even when the vulnerable optional backend is not actually built or used by the workspace. Without checking the feature tree first, it is easy to spend time trying to remove a path that is already inactive.

The important distinction is that a transitive crate can broaden lockfile entries without creating an active MySQL feature path for this workspace. The audit ignore matters because the current `cargo audit` result still reflects the lockfile-level optional dependency entry rather than the active resolver path.

## When to Apply

- When `cargo audit` reports a vulnerability through an optional SQL backend your workspace does not use
- When `cargo tree -e features` and `cargo audit` appear to disagree about the same dependency path
- When a transitive crate enables `sqlx` defaults more broadly than your workspace needs

## Examples

Before:

```text
cargo audit
  rsa 0.9.10
  -> sqlx-mysql 0.8.6
  -> sqlx 0.8.6
```

But active feature inspection showed no MySQL path:

```text
$ cargo tree -e features -i sqlx-mysql@0.8.6 --workspace --target all --all-features
warning: nothing to print.
```

After the local `sqlx` cleanup and repo-level audit ignore:

```json
{
  "settings": {
    "ignore": ["RUSTSEC-2023-0071"]
  },
  "vulnerabilities": {
    "found": false,
    "count": 0
  }
}
```

## Related

- `Cargo.toml`
- `.cargo/audit.toml`
