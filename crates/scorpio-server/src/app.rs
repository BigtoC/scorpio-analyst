//! Loco [`Hooks`] implementation for scorpio-server.

use async_trait::async_trait;
use loco_openapi::prelude::*;
use loco_rs::{
    Result,
    app::{AppContext, Hooks, Initializer},
    bgworker::Queue,
    boot::{BootResult, StartMode, create_app},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
};

use crate::controllers;

/// Marker type that Loco's generics anchor on. All app-level wiring lives in
/// the [`Hooks`] impl below.
pub struct App;

#[async_trait]
impl Hooks for App {
    fn app_version() -> String {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("BUILD_SHA")
                .or(option_env!("GITHUB_SHA"))
                .unwrap_or("dev")
        )
    }

    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self>(mode, environment, config).await
    }

    async fn initializers(ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        let mut initializers: Vec<Box<dyn Initializer>> = vec![];

        // The OpenAPI initializer relies on process-global state for automatic
        // route collection. Loading it inside test runs causes routes from
        // separate tests to bleed together, so we follow the upstream
        // recommendation and skip it in the Test environment.
        if ctx.environment != Environment::Test {
            initializers.push(Box::new(loco_openapi::OpenapiInitializerWithSetup::new(
                |_ctx| {
                    #[derive(OpenApi)]
                    #[openapi(info(
                        title = "scorpio-server",
                        description = "HTTP API surface for the scorpio analyst pipeline.",
                    ))]
                    struct ApiDoc;

                    ApiDoc::openapi()
                },
                None,
            )));
        }

        Ok(initializers)
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes().add_route(controllers::health::routes())
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    fn register_tasks(_tasks: &mut Tasks) {}
}
