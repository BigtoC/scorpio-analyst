//! Integration test for the `GET /health` endpoint.
//!
//! Boots the full Loco app via `request::<App>(...)`, hitting the same router
//! that ships in production minus the OpenAPI initializer (skipped in the Test
//! environment, per `app::initializers`). `#[serial]` keeps Loco's
//! process-wide tracing/init guards from racing across parallel tests.

use loco_rs::testing::prelude::*;
use scorpio_server::app::App;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn health_endpoint_returns_200_ok_with_status_ok_json() {
    request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/health").await;

        assert_eq!(response.status_code(), 200);
        assert_eq!(
            response.json::<serde_json::Value>(),
            serde_json::json!({"status": "ok"})
        );
    })
    .await;
}
