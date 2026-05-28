use opentelemetry::global;
// `TracerProvider` is the trait that provides the `.tracer(name)` method
// on `SdkTracerProvider`. Must be in scope at the call site.
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_langfuse::ExporterBuilder;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize tracing based on the `SCORPIO_LOG_FORMAT` environment variable.
///
/// - `SCORPIO_LOG_FORMAT=pretty` → human-readable output (local development)
/// - anything else (or unset) → structured JSON output (default)
///
/// If `SCORPIO_LANGFUSE_PUBLIC_KEY`, `SCORPIO_LANGFUSE_SECRET_KEY`, and
/// `SCORPIO_LANGFUSE_BASE_URL` are set, traces are also exported to Langfuse
/// via OpenTelemetry.
pub fn init_tracing() {
    // Load .env before reading env vars so values set in .env are respected.
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

/// Set up the Langfuse OpenTelemetry tracer provider if all required
/// `SCORPIO_LANGFUSE_*` environment variables are present.
///
/// Returns `Some(tracer)` on success, `None` if env vars are missing or
/// initialization fails. This allows running without Langfuse in local dev
/// or CI environments.
fn init_langfuse_tracer() -> Option<opentelemetry_sdk::trace::Tracer> {
    let public_key = std::env::var("SCORPIO_LANGFUSE_PUBLIC_KEY").ok()?;
    let secret_key = std::env::var("SCORPIO_LANGFUSE_SECRET_KEY").ok()?;
    let host = std::env::var("SCORPIO_LANGFUSE_BASE_URL").ok()?;

    let exporter = ExporterBuilder::new()
        .with_host(&host)
        .with_basic_auth(&public_key, &secret_key)
        .build()
        .ok()?;

    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();

    let tracer = provider.tracer("scorpio-analyst");
    global::set_tracer_provider(provider);

    Some(tracer)
}

/// Initialize the global tracing subscriber with structured JSON output.
///
/// The log level defaults to `info` but can be overridden via the `RUST_LOG` env var.
/// Call this once during application startup.
pub fn init_tracing_json() {
    if let Some(tracer) = init_langfuse_tracer() {
        tracing_subscriber::registry()
            .with(build_env_filter())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(fmt::layer().json().flatten_event(true))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(build_env_filter())
            .with(fmt::layer().json().flatten_event(true))
            .init();
    }
}

/// Initialize tracing with a human-readable (non-JSON) format, primarily for local
/// development and test runs.
pub fn init_tracing_pretty() {
    if let Some(tracer) = init_langfuse_tracer() {
        tracing_subscriber::registry()
            .with(build_env_filter())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(fmt::layer().pretty())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(build_env_filter())
            .with(fmt::layer().pretty())
            .init();
    }
}
