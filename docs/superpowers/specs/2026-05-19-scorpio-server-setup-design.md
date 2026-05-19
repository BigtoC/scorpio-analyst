# scorpio-server Crate Setup — Design Spec

## Summary

Create a new `scorpio-server` binary crate in the workspace that provides an HTTP API surface using axum. This initial scope covers only the crate scaffolding and a single health check endpoint. Future work will add endpoints that interact with `scorpio-core`.

## Goals

- Establish a new axum-based HTTP server crate following workspace conventions
- Provide `GET /health` returning `{"status": "ok"}`
- CLI args for host/port configuration
- Library + thin binary split for testability and future embeddability

## Non-Goals (This Task)

- Integration with `scorpio-core` (future task)
- Authentication, CORS, rate limiting middleware
- Multiple endpoints beyond health check
- WebSocket or SSE support

## Crate Structure

```
crates/scorpio-server/
├── Cargo.toml
└── src/
    ├── lib.rs          # Exposes app() -> Router, health handler
    └── main.rs         # clap args, bind, serve
```

### `lib.rs`

- `pub fn app() -> Router` — builds and returns the axum router
- `async fn health() -> Json<Value>` — returns `{"status": "ok"}`
- Unit test using axum test utilities to verify health endpoint

### `main.rs`

- Clap `#[derive(Parser)]` struct with `--port` and `--host` args
- Default port: `8088`, default host: `0.0.0.0`
- Env var overrides: `SCORPIO_SERVER_PORT`, `SCORPIO_SERVER_HOST`
- `#[tokio::main] async fn main()` → parse args → call `app()` → bind → serve
- Returns `anyhow::Result<()>` for error propagation

## Dependencies

```toml
[dependencies]
axum = "0.8"
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
clap.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

Only `axum` is a new dependency. All others are already defined in workspace deps.

## Health Endpoint

```
GET /health

Response: 200 OK
Content-Type: application/json

{"status": "ok"}
```

## CLI Interface

```
scorpio-server [--port <PORT>] [--host <HOST>]

Options:
  --port    Port to listen on (default: 8088)
  --host    Bind address (default: 0.0.0.0)

Environment variables:
  SCORPIO_SERVER_PORT    Override --port
  SCORPIO_SERVER_HOST    Override --host
```

## Error Handling

- `main.rs` returns `anyhow::Result<()>` matching `scorpio-cli` convention
- Bind/serve errors propagated via `?`
- No custom error types needed for health-only scope

## Testing

- **Unit test in `lib.rs`**: Call `app()`, use `axum::body::to_bytes` + `tower::ServiceExt::oneshot` to verify `GET /health` returns 200 with correct JSON body
- No TCP-level integration test needed for initial scope

## Workspace Integration

- `members = ["crates/*"]` glob in root `Cargo.toml` automatically includes `crates/scorpio-server`
- No changes to existing crates required
- Follows same `version.workspace = true`, `edition.workspace = true` pattern as siblings

## Docker Build

Multi-stage Dockerfile at `crates/scorpio-server/Dockerfile`:

```dockerfile
FROM rust:1.83-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release -p scorpio-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/scorpio-server /usr/local/bin/
EXPOSE 8088
ENTRYPOINT ["scorpio-server"]
```

Build: `docker build -f crates/scorpio-server/Dockerfile -t scorpio-server .`
Run: `docker run -p 8088:8088 scorpio-server`

Port and host configurable via env: `docker run -p 9000:9000 -e SCORPIO_SERVER_PORT=9000 scorpio-server`

## Future Work

- Add `scorpio-core` dependency for domain data access
- Additional endpoints (analysis triggers, state queries, etc.)
- `tower-http` middleware (CORS, request logging, tracing)
- Consider extracting `lib.rs` router for embedding in `scorpio-cli` as a subcommand
