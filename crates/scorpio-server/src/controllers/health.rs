//! `GET /health` — liveness probe with an OpenAPI-documented response.

use loco_openapi::prelude::*;
use loco_rs::prelude::*;
use serde::Serialize;

/// Response body returned by [`health`]. Annotated with [`ToSchema`] so the
/// utoipa-driven OpenAPI spec exposes the same JSON shape that callers see.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthStatus {
    /// Always `"ok"` while the server is accepting requests.
    pub status: &'static str,
}

/// Liveness probe.
///
/// Returns `200 OK` with `{"status": "ok"}` to indicate the process is up.
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Server is healthy", body = HealthStatus),
    ),
)]
#[debug_handler]
async fn health() -> Result<Response> {
    format::json(HealthStatus { status: "ok" })
}

/// Registers the health endpoint with the Loco router. The `openapi(...)`
/// wrapper from `loco_openapi` mirrors the route into the global OpenAPI spec
/// so it shows up in the Scalar visualizer.
pub fn routes() -> Routes {
    Routes::new().add("/health", openapi(get(health), routes!(health)))
}
