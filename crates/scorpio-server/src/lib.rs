//! # scorpio-server
//!
//! HTTP API surface for scorpio. This crate exposes [`app`], a builder that
//! returns an [`axum::Router`] wired with the current set of HTTP endpoints,
//! and a thin `main` binary that binds a TCP listener and serves the router.
//!
//! The initial scope is a single `GET /health` endpoint. Future endpoints
//! will interact with the `scorpio-core` crate.

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
