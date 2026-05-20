# scorpio-server Crate Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a new `scorpio-server` binary+library crate in the workspace exposing an axum HTTP server with a single `GET /health` endpoint, configurable host/port, and a Docker build, without depending on `scorpio-core`.

**Architecture:** A new crate at `crates/scorpio-server/` split into a thin `main.rs` (clap arg parsing, tokio runtime, bind, serve) and a `lib.rs` (router builder + health handler) so the router can be unit-tested via `tower::ServiceExt::oneshot` and embedded by future surfaces. The crate is autodiscovered by the existing `members = ["crates/*"]` glob — no root `Cargo.toml` member edits required, only one new workspace dependency entry (`axum`) plus a dev-dependency entry for `tower`.

**Tech Stack:** Rust 1.93+ / edition 2024, axum 0.8, tokio (workspace), clap derive (workspace), serde + serde_json (workspace), tracing + tracing-subscriber (workspace), anyhow (workspace), tower (dev, workspace).

**Source of truth:** `docs/superpowers/specs/2026-05-19-scorpio-server-setup-design.md`. The spec's Dockerfile lists `rust:1.83-slim`; this plan uses `rust:1.93-slim` because the workspace's `edition = "2024"` requires Rust ≥ 1.85 (CLAUDE.md states 1.93+).

---

## File Structure

**Created:**
- `crates/scorpio-server/Cargo.toml` — new crate manifest; library + binary targets; consumes workspace deps.
- `crates/scorpio-server/src/lib.rs` — `pub fn app() -> Router` + `async fn health() -> Json<Value>` + a `#[tokio::test]` unit test that hits `/health` via `tower::ServiceExt::oneshot`.
- `crates/scorpio-server/src/main.rs` — `#[derive(Parser)]` CLI args (`--host`, `--port` + `SCORPIO_SERVER_HOST` / `SCORPIO_SERVER_PORT` env), `#[tokio::main] async fn main() -> anyhow::Result<()>`, initialises tracing, binds a `TcpListener`, calls `axum::serve(listener, scorpio_server::app())`.
- `crates/scorpio-server/Dockerfile` — multi-stage build using `rust:1.93-slim` builder and `debian:bookworm-slim` runtime, exposing port 8088 and `ENTRYPOINT ["scorpio-server"]`.

**Modified:**
- `Cargo.toml` (workspace root) — append `axum = "0.8"` and `tower = { version = "0.5", default-features = false, features = ["util"] }` to `[workspace.dependencies]`. **No `members` edit** — the existing `members = ["crates/*"]` glob picks the new crate up automatically.

**Not modified:** No existing crate is touched. `scorpio-core`, `scorpio-cli`, and `scorpio-reporters` continue to build unchanged.

---

## Task 1: Add `axum` and `tower` to workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `axum` and dev-only `tower` entries to `[workspace.dependencies]`**

Open `Cargo.toml` at the workspace root and insert the following two lines into the `[workspace.dependencies]` table. Place them near the bottom of the table, immediately before the `# Dev-only` comment so they sit with general runtime/utility deps; the exact position is not load-bearing but keep the file tidy:

```toml
# HTTP server framework (scorpio-server crate)
axum = "0.8"

# Tower middleware ecosystem — dev-only for axum router unit tests (`oneshot`).
tower = { version = "0.5", default-features = false, features = ["util"] }
```

- [ ] **Step 2: Verify workspace still resolves**

Run: `cargo metadata --format-version 1 --no-deps > /dev/null`
Expected: exits 0 with no output. (We use `cargo metadata` here instead of `cargo build` because no crate consumes the new deps yet, so a build would be a no-op; metadata proves the workspace TOML still parses.)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "build(workspace): add axum and tower to workspace dependencies"
```

---

## Task 2: Create the `scorpio-server` crate manifest

**Files:**
- Create: `crates/scorpio-server/Cargo.toml`

- [ ] **Step 1: Create the crate directory**

Run: `mkdir -p crates/scorpio-server/src`
Expected: directory exists; no output.

- [ ] **Step 2: Write `crates/scorpio-server/Cargo.toml`**

Create the file with exactly this content. The `[lib]` and `[[bin]]` sections make both library and binary targets share the same crate name — `scorpio_server` (library, snake-case) and `scorpio-server` (binary, kebab-case). `publish = false` matches the other crates in this workspace.

```toml
[package]
name = "scorpio-server"
version.workspace = true
edition.workspace = true
description.workspace = true
license.workspace = true
repository.workspace = true
publish = false

[lib]
path = "src/lib.rs"

[[bin]]
name = "scorpio-server"
path = "src/main.rs"

[dependencies]
# HTTP framework
axum.workspace = true

# Async runtime (binary entry point + axum::serve)
tokio.workspace = true

# Serialization (Json response body)
serde.workspace = true
serde_json.workspace = true

# CLI argument parsing
clap.workspace = true

# Error handling (main returns anyhow::Result, matching scorpio-cli convention)
anyhow.workspace = true

# Observability
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
# `ServiceExt::oneshot` for unit-testing the axum Router without a TCP bind.
tower.workspace = true
```

- [ ] **Step 3: Verify the new crate is picked up by the workspace glob**

Run: `cargo metadata --format-version 1 --no-deps --manifest-path Cargo.toml | grep -o '"name":"scorpio-server"'`
Expected output (exactly): `"name":"scorpio-server"`

If empty, the `members = ["crates/*"]` glob did not pick up the new directory — re-check the path. Do **not** edit the root manifest to add an explicit member entry; the glob is the intended mechanism.

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-server/Cargo.toml
git commit -m "feat(scorpio-server): scaffold crate manifest with lib + bin targets"
```

---

## Task 3: Write the failing unit test for the `/health` endpoint

**Files:**
- Create: `crates/scorpio-server/src/lib.rs`

- [ ] **Step 1: Create `crates/scorpio-server/src/lib.rs` with the test only**

The library file starts with only the unit test — this is the failing-test step of TDD. The test imports `app` and `health` from the crate root via `super::*`, so the test will fail to compile (which is the expected red state) until Task 4 adds the implementation.

```rust
//! # scorpio-server
//!
//! HTTP API surface for scorpio. This crate exposes [`app`], a builder that
//! returns an [`axum::Router`] wired with the current set of HTTP endpoints,
//! and a thin `main` binary that binds a TCP listener and serves the router.
//!
//! The initial scope is a single `GET /health` endpoint. Future endpoints
//! will interact with the `scorpio-core` crate.

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_endpoint_returns_200_ok_with_status_ok_json() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body collects");
        let body_json: serde_json::Value =
            serde_json::from_slice(&body_bytes).expect("body is json");

        assert_eq!(body_json, serde_json::json!({"status": "ok"}));
    }
}
```

- [ ] **Step 2: Run the test and confirm it fails to compile**

Run: `cargo test -p scorpio-server --lib`
Expected: compilation error referencing `app` (e.g. `error[E0425]: cannot find function 'app' in this scope`). This is the intended red state.

If the error is anything other than "`app` is not defined" (or equivalent), stop and diagnose — that means the dev-dep wiring is wrong, not the missing implementation.

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/scorpio-server/src/lib.rs
git commit -m "test(scorpio-server): add failing /health endpoint test"
```

---

## Task 4: Implement `app()` and `health()` to make the test pass

**Files:**
- Modify: `crates/scorpio-server/src/lib.rs`

- [ ] **Step 1: Add the router builder and handler above the `#[cfg(test)] mod tests`**

Insert the following at the top of `crates/scorpio-server/src/lib.rs`, immediately after the module-level `//!` doc comment block and before the `#[cfg(test)]` block:

```rust
use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

/// Build the axum [`Router`] hosting every HTTP endpoint exposed by
/// scorpio-server. Returning a `Router` (rather than serving it directly)
/// keeps the router unit-testable via `tower::ServiceExt::oneshot` and lets
/// future surfaces embed it without re-binding TCP.
pub fn app() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}
```

After this edit, the full `lib.rs` reads (in order): the `//!` doc comment, the two `use` lines, `pub fn app()`, `async fn health()`, then the existing `#[cfg(test)] mod tests` block from Task 3.

- [ ] **Step 2: Run the test and confirm it passes**

Run: `cargo test -p scorpio-server --lib`
Expected: `test tests::health_endpoint_returns_200_ok_with_status_ok_json ... ok` and a `test result: ok. 1 passed; 0 failed` summary line.

- [ ] **Step 3: Run clippy on the new crate**

Run: `cargo clippy -p scorpio-server --all-targets -- -D warnings`
Expected: exits 0 with no warnings. (CI treats warnings as errors; catching them now avoids a separate fix-up commit.)

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-server/src/lib.rs
git commit -m "feat(scorpio-server): add app() router with /health endpoint"
```

---

## Task 5: Implement the `main.rs` binary entry point

**Files:**
- Create: `crates/scorpio-server/src/main.rs`

- [ ] **Step 1: Write `crates/scorpio-server/src/main.rs`**

Create the file with this content. Notes:
- `clap`'s `env` attribute reads `SCORPIO_SERVER_HOST` / `SCORPIO_SERVER_PORT` automatically when the corresponding flag is absent, satisfying the spec's "env var override" requirement without manual env lookups.
- `default_value_t = 8088` (vs `default_value`) keeps the port as a typed `u16` instead of going through a string round-trip.
- The address parse uses `format!("{host}:{port}")` then `.parse::<SocketAddr>()` so any malformed host bubbles up through `anyhow::Result` rather than panicking.
- `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))` mirrors the spec's "tracing-subscriber" dep usage and gives a sensible default when `RUST_LOG` is unset.

```rust
use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// scorpio-server — HTTP API surface for the scorpio analyst pipeline.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Bind address.
    #[arg(long, env = "SCORPIO_SERVER_HOST", default_value = "0.0.0.0")]
    host: String,

    /// Port to listen on.
    #[arg(long, env = "SCORPIO_SERVER_PORT", default_value_t = 8088)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid bind address {}:{}", args.host, args.port))?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    info!(%addr, "scorpio-server listening");

    axum::serve(listener, scorpio_server::app())
        .await
        .context("axum::serve returned an error")?;

    Ok(())
}
```

- [ ] **Step 2: Build the binary**

Run: `cargo build -p scorpio-server`
Expected: exits 0; produces `target/debug/scorpio-server`.

- [ ] **Step 3: Verify the binary's `--help` matches the spec's CLI interface**

Run: `cargo run -p scorpio-server -- --help`
Expected: output includes both `--host` (default `0.0.0.0`) and `--port` (default `8088`), plus an `Environment variables` section or per-flag `[env: SCORPIO_SERVER_HOST=...]` annotation produced by clap's `env` attribute.

If the env annotations are missing, the `clap` workspace entry is missing the `env` feature — confirm `Cargo.toml` workspace dep reads `clap = { version = "4", features = ["derive", "env"] }`. (Existing workspace already enables `env`; this is a sanity check.)

- [ ] **Step 4: Smoke-test the binary end-to-end**

Start the server in the background, hit `/health`, then stop it:

```bash
cargo run -p scorpio-server -- --port 18088 &
SERVER_PID=$!
sleep 2
curl -sf http://127.0.0.1:18088/health
EXIT=$?
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
test $EXIT -eq 0 && echo "SMOKE OK"
```

Expected: `curl` prints `{"status":"ok"}` (no trailing newline), followed by `SMOKE OK`. If the port is already taken, swap `18088` for another free high port and retry.

- [ ] **Step 5: Run the full crate test + clippy gate**

Run: `cargo test -p scorpio-server --all-targets && cargo clippy -p scorpio-server --all-targets -- -D warnings && cargo fmt -- --check`
Expected: all three exit 0.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-server/src/main.rs
git commit -m "feat(scorpio-server): add main binary with clap host/port args"
```

---

## Task 6: Add the multi-stage Dockerfile

**Files:**
- Create: `crates/scorpio-server/Dockerfile`

- [ ] **Step 1: Write `crates/scorpio-server/Dockerfile`**

The spec lists `rust:1.83-slim`, but the workspace uses `edition = "2024"` which requires Rust ≥ 1.85; CLAUDE.md states 1.93+. Using `rust:1.93-slim` keeps the Dockerfile in sync with what `cargo build` requires locally. The build context is the workspace root, so `COPY Cargo.toml Cargo.lock ./` plus `COPY crates/ crates/` reproduces the full workspace inside the image.

```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1.93-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release -p scorpio-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/scorpio-server /usr/local/bin/scorpio-server
EXPOSE 8088
ENTRYPOINT ["scorpio-server"]
```

- [ ] **Step 2: Build the image from the workspace root**

Run: `docker build -f crates/scorpio-server/Dockerfile -t scorpio-server:dev .`
Expected: ends with `Successfully tagged scorpio-server:dev` (or the buildkit equivalent `naming to docker.io/library/scorpio-server:dev`). Build time will be several minutes on a cold cache because the builder stage compiles the full workspace.

If Docker is not available in the development environment, skip this step and **note in the commit message that the Dockerfile is unverified**; do not block on this verification.

- [ ] **Step 3: Smoke-test the image**

```bash
docker run --rm -d -p 18088:8088 --name scorpio-server-smoke scorpio-server:dev
sleep 2
curl -sf http://127.0.0.1:18088/health
EXIT=$?
docker stop scorpio-server-smoke
test $EXIT -eq 0 && echo "DOCKER SMOKE OK"
```

Expected: `{"status":"ok"}` followed by `DOCKER SMOKE OK`. Skip with a note if Docker is unavailable, as in Step 2.

- [ ] **Step 4: Verify the env-var override path through the image**

```bash
docker run --rm -d -p 19000:9000 -e SCORPIO_SERVER_PORT=9000 --name scorpio-server-env scorpio-server:dev
sleep 2
curl -sf http://127.0.0.1:19000/health
EXIT=$?
docker stop scorpio-server-env
test $EXIT -eq 0 && echo "DOCKER ENV OK"
```

Expected: `{"status":"ok"}` followed by `DOCKER ENV OK`. This confirms `SCORPIO_SERVER_PORT` is wired through clap into the bind address. Skip with a note if Docker is unavailable.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-server/Dockerfile
git commit -m "build(scorpio-server): add multi-stage Dockerfile"
```

---

## Task 7: Full-workspace verification

**Files:** none modified unless Step 4 applies formatting fixes.

- [ ] **Step 1: Run the project-wide formatting check**

Run: `cargo fmt -- --check`
Expected: exits 0 with no output.

- [ ] **Step 2: Run the project-wide clippy gate (matches CI)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: exits 0 with no warnings. This mirrors `.github/workflows/tests.yml`.

- [ ] **Step 3: Run the full workspace test suite (matches CI)**

Run: `cargo test --workspace --all-features --locked --no-fail-fast`
Expected: exits 0; all tests across `scorpio-core`, `scorpio-cli`, `scorpio-reporters`, and the new `scorpio-server` pass.

If `cargo nextest` is installed (CI uses it), the equivalent invocation is `cargo nextest run --workspace --all-features --locked --no-fail-fast`. Either is acceptable here.

- [ ] **Step 4: Commit any formatting fixes if Step 1 caught any**

If Step 1 reported diffs, run `cargo fmt`, then:

```bash
git add -A
git commit -m "style: apply cargo fmt across new scorpio-server crate"
```

Otherwise skip this step.

---

## Acceptance Criteria (spec → task map)

| Spec requirement                                                          | Implemented by                                   |
|---------------------------------------------------------------------------|--------------------------------------------------|
| New `scorpio-server` binary crate in workspace                            | Task 2 (manifest + dir), Task 5 (bin)            |
| `GET /health` returns `{"status": "ok"}` with HTTP 200                    | Tasks 3 + 4 (test + impl)                        |
| CLI args `--host` / `--port` with defaults `0.0.0.0` / `8088`             | Task 5 (clap derive)                             |
| Env overrides `SCORPIO_SERVER_HOST` / `SCORPIO_SERVER_PORT`               | Task 5 (`clap` `env` attr) + Task 6 verification |
| Library + thin binary split (`lib.rs` exposes `app()`; `main.rs` thin)    | Tasks 2, 4, 5                                    |
| `axum = "0.8"` added; other deps reused from workspace                    | Tasks 1, 2                                       |
| `anyhow::Result<()>` from `main` matching `scorpio-cli` convention        | Task 5                                           |
| Unit test using `oneshot` for `/health`                                   | Tasks 3 + 4                                      |
| `members = ["crates/*"]` glob auto-includes the new crate; no member edit | Task 2 Step 3 (verification)                     |
| Multi-stage Dockerfile at `crates/scorpio-server/Dockerfile`              | Task 6                                           |
| `EXPOSE 8088`, `ENTRYPOINT ["scorpio-server"]`                            | Task 6                                           |
| Env-configurable port through Docker (`-e SCORPIO_SERVER_PORT=9000`)      | Task 6 Step 4                                    |
| No `scorpio-core` dependency (deferred to future work)                    | Task 2 (deps list excludes it)                   |
| No CORS / auth / rate-limit middleware (non-goal)                         | Task 4 (router has only `/health`)               |

---

## Out of Scope (per spec "Non-Goals")

The following are intentionally not addressed by this plan and must not be added by the implementer:

- Integration with `scorpio-core`.
- Authentication, CORS, rate limiting, or any tower-http middleware beyond what axum brings in by default.
- Endpoints other than `GET /health`.
- WebSocket or SSE support.
- TCP-level integration tests (the in-process `oneshot` test is sufficient for this scope).
