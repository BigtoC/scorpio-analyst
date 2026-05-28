use opentelemetry::KeyValue;
use opentelemetry::global;
// `TracerProvider` is the trait that provides the `.tracer(name)` method
// on `SdkTracerProvider`. Must be in scope at the call site.
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_langfuse::ExporterBuilder;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// RAII guard that triggers a final OTel span flush on `Drop`.
///
/// `BatchSpanProcessor` buffers spans and flushes on a schedule (default
/// every 5 s) **and** on `provider.shutdown()`. Without an explicit
/// shutdown, the spans created in the last few seconds of a run can be
/// lost when the tokio runtime tears down before the next scheduled
/// flush. Hold this guard for the lifetime of `main` and the `Drop` impl
/// will run `provider.shutdown()` while tokio is still alive.
///
/// Bind it to a `_`-prefixed local so it lives until the end of scope:
///
/// ```ignore
/// let _tracing = scorpio_core::observability::init_tracing();
/// // ... run analysis ...
/// // _tracing drops here, final OTel flush fires
/// ```
#[must_use = "TracingGuard must be held until program exit; dropping it flushes pending OTel spans to Langfuse"]
pub struct TracingGuard {
    provider: Option<SdkTracerProvider>,
}

impl TracingGuard {
    fn empty() -> Self {
        Self { provider: None }
    }

    fn with_provider(provider: SdkTracerProvider) -> Self {
        Self {
            provider: Some(provider),
        }
    }
}

impl Drop for TracingGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take()
            && let Err(e) = provider.shutdown()
        {
            // Best-effort log on shutdown — `tracing` subscriber may
            // already be torn down at this point, so use eprintln so the
            // operator can see flush failures during teardown.
            eprintln!("[observability] OTel provider shutdown error: {e}");
        }
    }
}

/// Initialize tracing based on the `SCORPIO_LOG_FORMAT` environment variable.
///
/// - `SCORPIO_LOG_FORMAT=pretty` → human-readable output (local development)
/// - anything else (or unset) → structured JSON output (default)
///
/// If `SCORPIO_LANGFUSE_PUBLIC_KEY`, `SCORPIO_LANGFUSE_SECRET_KEY`, and
/// `SCORPIO_LANGFUSE_BASE_URL` are set, traces are also exported to Langfuse
/// via OpenTelemetry.
///
/// Returns a [`TracingGuard`] that must be held until program exit so the
/// batch span processor flushes its final buffer.
pub fn init_tracing() -> TracingGuard {
    // Load .env before reading env vars so values set in .env are respected.
    // Silently ignored when no .env file exists (e.g. CI / production).
    dotenvy::dotenv().ok();

    if std::env::var("SCORPIO_LOG_FORMAT").as_deref() == Ok("pretty") {
        init_tracing_pretty()
    } else {
        init_tracing_json()
    }
}

/// Build the log [`EnvFilter`], defaulting to `info` when `RUST_LOG` is unset.
fn build_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Outcome of [`init_langfuse_tracer`] — describes whether Langfuse export
/// is active, intentionally off, or failed to set up. Surfaced via a startup
/// log so operators can verify the integration without polling the
/// dashboard.
enum LangfuseStatus {
    /// All env vars present and the exporter built successfully.
    Enabled { host: String },
    /// At least one required env var is unset; Langfuse export is off by
    /// design (local dev / CI).
    Disabled { missing_var: &'static str },
    /// Env vars were set but the exporter builder returned an error.
    Failed { reason: String },
}

/// Set up the Langfuse OpenTelemetry tracer provider if all required
/// `SCORPIO_LANGFUSE_*` environment variables are present.
///
/// Returns the tracer plus a status describing what happened. The status
/// is logged after the subscriber is initialized so the operator sees one
/// of:
///
/// - `Langfuse tracing enabled` — spans will be exported
/// - `Langfuse tracing disabled — <var> not set` — intentional off
/// - `Langfuse tracing setup failed` — env vars present but exporter
///   could not be built (bad URL, malformed credentials, etc.)
fn init_langfuse_tracer() -> (
    Option<opentelemetry_sdk::trace::Tracer>,
    Option<SdkTracerProvider>,
    LangfuseStatus,
) {
    let public_key = match std::env::var("SCORPIO_LANGFUSE_PUBLIC_KEY") {
        Ok(v) => v,
        Err(_) => {
            return (
                None,
                None,
                LangfuseStatus::Disabled {
                    missing_var: "SCORPIO_LANGFUSE_PUBLIC_KEY",
                },
            );
        }
    };
    let secret_key = match std::env::var("SCORPIO_LANGFUSE_SECRET_KEY") {
        Ok(v) => v,
        Err(_) => {
            return (
                None,
                None,
                LangfuseStatus::Disabled {
                    missing_var: "SCORPIO_LANGFUSE_SECRET_KEY",
                },
            );
        }
    };
    let host = match std::env::var("SCORPIO_LANGFUSE_BASE_URL") {
        Ok(v) => v,
        Err(_) => {
            return (
                None,
                None,
                LangfuseStatus::Disabled {
                    missing_var: "SCORPIO_LANGFUSE_BASE_URL",
                },
            );
        }
    };

    let exporter = match ExporterBuilder::new()
        .with_host(&host)
        .with_basic_auth(&public_key, &secret_key)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            return (
                None,
                None,
                LangfuseStatus::Failed {
                    reason: e.to_string(),
                },
            );
        }
    };

    // Batch processor backed by the tokio runtime — required for async apps.
    // `SimpleSpanProcessor` blocks the reactor on each export and routinely
    // drops spans during process teardown; `BatchSpanProcessor` buffers and
    // flushes on `shutdown` / `Drop` so spans actually reach Langfuse.
    //
    // `service.name` on the Resource is what Langfuse uses to identify the
    // app; without it traces may be filtered out at ingestion.
    let provider = SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter, Tokio).build())
        .with_resource(
            Resource::builder()
                .with_attributes(vec![KeyValue::new("service.name", "scorpio-analyst")])
                .build(),
        )
        .build();

    let tracer = provider.tracer("scorpio-analyst");
    // Clone before moving into global so the guard can call shutdown()
    // while the tokio runtime is still alive. The Arc<TracerProviderInner>
    // inside SdkTracerProvider means both references point at the same
    // underlying state.
    let provider_for_guard = provider.clone();
    global::set_tracer_provider(provider);

    (
        Some(tracer),
        Some(provider_for_guard),
        LangfuseStatus::Enabled { host },
    )
}

/// Emit the Langfuse startup status. Called once after the tracing
/// subscriber is initialized so the log actually reaches the configured
/// sink (stdout / JSON).
fn log_langfuse_status(status: &LangfuseStatus) {
    match status {
        LangfuseStatus::Enabled { host } => {
            tracing::info!(
                target: "scorpio_core::observability",
                langfuse_host = %host,
                "Langfuse tracing enabled — exporting OTLP spans"
            );
        }
        LangfuseStatus::Disabled { missing_var } => {
            tracing::info!(
                target: "scorpio_core::observability",
                missing_var,
                "Langfuse tracing disabled — env var not set"
            );
        }
        LangfuseStatus::Failed { reason } => {
            tracing::warn!(
                target: "scorpio_core::observability",
                reason = %reason,
                "Langfuse tracing setup failed — running without it"
            );
        }
    }
}

/// Initialize the global tracing subscriber with structured JSON output.
///
/// The log level defaults to `info` but can be overridden via the `RUST_LOG` env var.
/// Call this once during application startup.
pub fn init_tracing_json() -> TracingGuard {
    let (tracer, provider, status) = init_langfuse_tracer();
    if let Some(tracer) = tracer {
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
    log_langfuse_status(&status);
    match provider {
        Some(p) => TracingGuard::with_provider(p),
        None => TracingGuard::empty(),
    }
}

/// Initialize tracing with a human-readable (non-JSON) format, primarily for local
/// development and test runs.
pub fn init_tracing_pretty() -> TracingGuard {
    let (tracer, provider, status) = init_langfuse_tracer();
    if let Some(tracer) = tracer {
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
    log_langfuse_status(&status);
    match provider {
        Some(p) => TracingGuard::with_provider(p),
        None => TracingGuard::empty(),
    }
}
