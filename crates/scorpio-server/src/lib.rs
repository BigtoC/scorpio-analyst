//! # scorpio-server
//!
//! Loco-based HTTP API surface for scorpio. The crate exposes [`app::App`] —
//! the Loco [`Hooks`](loco_rs::app::Hooks) implementation that owns routing,
//! initializers, and lifecycle hooks — alongside the per-endpoint controllers
//! it composes.
//!
//! The initial scope is a single `GET /health` endpoint; future endpoints will
//! interact with the `scorpio-core` crate. OpenAPI documentation is generated
//! from `#[utoipa::path]` annotations on each handler and served via the Scalar
//! visualizer at `/scalar` (see `config/*.yaml`).

pub mod app;
pub mod controllers;
