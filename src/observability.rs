use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize the global tracing subscriber with structured JSON output.
///
/// The log level defaults to `info` but can be overridden via the `RUST_LOG` env var.
/// Call this once during application startup.
pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().json().flatten_event(true))
        .init();
}

/// Initialize tracing with a human-readable (non-JSON) format, primarily for local
/// development and test runs.
pub fn init_tracing_pretty() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().pretty())
        .init();
}
