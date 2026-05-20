# scorpio-server

HTTP API surface for the scorpio analyst pipeline, built on [Loco](https://loco.rs).

The crate exposes a single endpoint today (`GET /health`); future endpoints
will call into [`scorpio-core`](../scorpio-core). OpenAPI documentation is
generated from `#[utoipa::path]` annotations and served via the
[Scalar](https://github.com/scalar/scalar) visualizer.

## Endpoints

| Method | Path                   | Description                                           |
|--------|------------------------|-------------------------------------------------------|
| `GET`  | `/health`              | Liveness probe — returns `200 {"status": "ok"}`       |
| `GET`  | `/scalar`              | Scalar UI for the auto-generated OpenAPI spec         |
| `GET`  | `/scalar/openapi.json` | Machine-readable OpenAPI 3.1 document                 |
| `GET`  | `/_ping`               | Loco built-in ping                                    |
| `GET`  | `/_health`             | Loco built-in deep health check                       |
| `GET`  | `/_readiness`          | Loco built-in readiness probe                         |

## Build and run

```sh
# Build
cargo build -p scorpio-server

# Start the server (loads config/development.yaml by default)
cargo run -p scorpio-server -- start

# Override binding/port without editing config
PORT=8088 BINDING=0.0.0.0 cargo run -p scorpio-server -- start

# Production profile (loads config/production.yaml)
LOCO_ENV=production cargo run -p scorpio-server -- start

# Print every mounted route
cargo run -p scorpio-server -- routes

# Validate configuration
cargo run -p scorpio-server -- doctor
```

`scorpio-server --help` lists every Loco subcommand.

## Configuration

Loco resolves `config/<LOCO_ENV>.yaml` relative to the current working
directory. The crate ships three profiles:

| Environment             | File                      | Defaults                                                                |
|-------------------------|---------------------------|-------------------------------------------------------------------------|
| `development` (default) | `config/development.yaml` | binding `0.0.0.0:8088`, log level `debug`                               |
| `production`            | `config/production.yaml`  | binding `0.0.0.0:8088`, log level `info`, JSON logs                     |
| `test`                  | `config/test.yaml`        | binding `localhost:5555`, logging disabled, OpenAPI initializer skipped |

Environment variables consulted by the YAML templates:

| Variable    | Default          | Meaning                                           |
|-------------|------------------|---------------------------------------------------|
| `LOCO_ENV`  | `development`    | Selects the config file to load                   |
| `PORT`      | `8088`           | TCP port                                          |
| `BINDING`   | `0.0.0.0`        | Bind interface                                    |
| `LOG_LEVEL` | `debug` / `info` | `trace` \| `debug` \| `info` \| `warn` \| `error` |

## OpenAPI

`loco-openapi` is enabled with the `scalar` feature only. The
`OpenapiInitializerWithSetup` registered in
[`src/app.rs`](src/app.rs) collects every handler annotated with
`#[utoipa::path]` and mounts the Scalar UI at `/scalar`.

Add a new documented endpoint by:

1. Annotating the handler with `#[utoipa::path(...)]` and `#[debug_handler]`.
2. Deriving `ToSchema` on the response struct.
3. Wrapping the route with `openapi(get(handler), routes!(handler))` when
   adding it to `Routes`.

See [`src/controllers/health.rs`](src/controllers/health.rs) for the canonical example.

## Default Endpoints
> Loco built-in health check endpoints
### Health check endpoints
There are three default health check endpoints that are automatically registered in the application:
- `_ping` and `_health`: Can be used by startup probe and liveness probe, they only confirm the server is running (simple 200 OK).
- `_readiness`: Can be used by readiness probe, tt checks dependencies (DB, Cache, Storage).
   - If you configure a queue, it will check if the queue is reachable.
   - If you enable `with-db` feature, it'll also check the database connection.
   - If you enable `cache_inmem` or `cache_redis` features, it'll also check the cache connection.

Why we separate these endpoints?
- **Best practices**: Aligns with Kubernetes patterns to avoid removing healthy servers from rotation when dependencies fail temporarily.
- **Load Balancer Clarity**: A Clear distinction helps load balancers make accurate routing decisions without conflating server and dependency health.
- **Flexibility**: Splitting endpoints gives users more control to decide which checks to monitor based on their needs (e.g., prioritizing liveness for basic uptime or readiness for full system health).
- **Debugging**: Separate endpoints make it easier to diagnose issues (e.g., server up but S3 down).

## Tests

```sh
cargo test -p scorpio-server
```

The integration test in [`tests/health.rs`](tests/health.rs) boots the full
Loco app via `loco_rs::testing::request::<App>` and hits `/health` end-to-end.
The OpenAPI initializer is intentionally skipped in the `Test` environment to
avoid the global-state bleed documented upstream.

## Docker

The crate ships a multi-stage [`Dockerfile`](Dockerfile) that builds a release
binary and copies the `config/` tree into the runtime image:

```sh
docker build -f crates/scorpio-server/Dockerfile -t scorpio-server .
docker run --rm -p 8088:8088 scorpio-server
```

`LOCO_ENV=production` is set in the image, so the production profile loads by
default. Override `PORT` and `BINDING` at `docker run` time to change the
listen address.

## Adding a new endpoint

| Step                                 | File                        |
|--------------------------------------|-----------------------------|
| Create the controller module         | `src/controllers/<name>.rs` |
| Re-export it                         | `src/controllers/mod.rs`    |
| Wire its `routes()` into `AppRoutes` | `src/app.rs`                |
| Add an integration test              | `tests/<name>.rs`           |
