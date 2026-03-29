use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize tracing based on the `SCORPIO_LOG_FORMAT` environment variable.
///
/// - `SCORPIO_LOG_FORMAT=pretty` → human-readable output (local development)
/// - anything else (or unset) → structured JSON output (default)
pub fn init_tracing() {
    // Load .env before reading SCORPIO_LOG_FORMAT so values set in .env are respected.
    // Silently ignored when no .env file exists (e.g. CI / production).
    dotenvy::dotenv().ok();

    if std::env::var("SCORPIO_LOG_FORMAT").as_deref() == Ok("pretty") {
        init_tracing_pretty();
    } else {
        init_tracing_json();
    }
}

/// Build the log [`EnvFilter`], defaulting to `info` when `RUST_LOG` is unset.
fn build_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialize the global tracing subscriber with structured JSON output.
///
/// The log level defaults to `info` but can be overridden via the `RUST_LOG` env var.
/// Call this once during application startup.
pub fn init_tracing_json() {
    tracing_subscriber::registry()
        .with(build_env_filter())
        .with(fmt::layer().json().flatten_event(true))
        .init();
}

/// Initialize tracing with a human-readable (non-JSON) format, primarily for local
/// development and test runs.
pub fn init_tracing_pretty() {
    tracing_subscriber::registry()
        .with(build_env_filter())
        .with(fmt::layer().pretty())
        .init();
}
