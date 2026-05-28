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
        if let Some(provider) = self.provider.take() {
            // Manual flush per Langfuse's "Manual Flushing" guidance —
            // ensures any spans still buffered in the BatchSpanProcessor
            // (default flush interval is 5s) get sent before we trigger
            // shutdown. Without this, the trailing batch of a fast run
            // can be lost on teardown.
            if let Err(e) = provider.force_flush() {
                eprintln!("[observability] OTel force_flush error: {e}");
            }
            if let Err(e) = provider.shutdown() {
                // Best-effort log on shutdown — `tracing` subscriber may
                // already be torn down at this point, so use eprintln so
                // the operator can see flush failures during teardown.
                eprintln!("[observability] OTel provider shutdown error: {e}");
            }
        }
    }
}

impl TracingGuard {
    /// Explicit flush + shutdown for callers that want to drain spans
    /// before `main` returns (e.g. short-lived CLI invocations where
    /// relying on `Drop` order is fragile).
    ///
    /// Consumes the guard so it can't be double-flushed. After this call
    /// any further span emissions will be dropped at the
    /// `BatchSpanProcessor`.
    pub fn flush_and_shutdown(mut self) {
        if let Some(provider) = self.provider.take() {
            if let Err(e) = provider.force_flush() {
                eprintln!("[observability] OTel force_flush error: {e}");
            }
            if let Err(e) = provider.shutdown() {
                eprintln!("[observability] OTel provider shutdown error: {e}");
            }
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
/// via OpenTelemetry. Precedence: process env > `.env` file > `config.toml`.
///
/// Returns a [`TracingGuard`] that must be held until program exit so the
/// batch span processor flushes its final buffer.
pub fn init_tracing() -> TracingGuard {
    // Load .env before reading env vars so values set in .env are respected.
    // Silently ignored when no .env file exists (e.g. CI / production).
    dotenvy::dotenv().ok();

    // Fall back to ~/.scorpio-analyst/config.toml for SCORPIO_LANGFUSE_* values
    // that neither the process env nor .env supplied. This lets `scorpio setup`
    // persist Langfuse credentials without forcing users to also edit a .env file.
    apply_langfuse_config_fallback();

    if std::env::var("SCORPIO_LOG_FORMAT").as_deref() == Ok("pretty") {
        init_tracing_pretty()
    } else {
        init_tracing_json()
    }
}

/// Populate `SCORPIO_LANGFUSE_*` env vars from the persisted user config when
/// they are not already set. Existing env vars are never overwritten so that
/// process env and `.env` keep their precedence over the config file.
fn apply_langfuse_config_fallback() {
    let Ok(cfg) = crate::settings::load_user_config() else {
        return;
    };
    set_env_var_if_unset(
        "SCORPIO_LANGFUSE_PUBLIC_KEY",
        cfg.langfuse_public_key.as_deref(),
    );
    set_env_var_if_unset(
        "SCORPIO_LANGFUSE_SECRET_KEY",
        cfg.langfuse_secret_key.as_deref(),
    );
    set_env_var_if_unset(
        "SCORPIO_LANGFUSE_BASE_URL",
        cfg.langfuse_base_url.as_deref(),
    );
}

fn set_env_var_if_unset(name: &str, value: Option<&str>) {
    let Some(value) = value else { return };
    if std::env::var_os(name).is_some() {
        return;
    }
    // SAFETY: `init_tracing` is invoked once, synchronously, at the very top
    // of `main` before any tokio task or thread is spawned, so no other
    // thread can be reading the environment concurrently.
    unsafe {
        std::env::set_var(name, value);
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
        // Langfuse's OTel ingestion endpoint requires this header per
        // their docs (https://langfuse.com/integrations/native/opentelemetry).
        // Without it, spans may be silently dropped or routed to a legacy
        // ingestion path that doesn't surface them in the dashboard.
        // The `opentelemetry-langfuse` 0.6 crate does NOT add this header
        // automatically — we have to set it ourselves.
        .with_header("x-langfuse-ingestion-version", "4")
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

    // MUST use the Tokio-runtime BatchSpanProcessor (NOT
    // `with_batch_exporter`). The langfuse exporter uses an async
    // `reqwest::Client` and requires a tokio runtime to drive HTTP. The
    // default `with_batch_exporter` spawns a `std::thread` with no tokio
    // runtime in scope — the export future panics there, the worker
    // thread dies, and every subsequent span hits "BatchSpanProcessor.
    // OnEnd.AfterShutdown" (channel disconnected).
    //
    // Rig's published examples use `with_batch_exporter`, but that's
    // because they use `opentelemetry-otlp` with the blocking reqwest
    // client, which doesn't need a tokio runtime. We don't have that
    // option with langfuse — async reqwest only.
    //
    // `with_service_name` is the canonical helper for setting the Resource
    // `service.name` attribute Langfuse uses to identify the app.
    let provider = SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter, Tokio).build())
        .with_resource(
            Resource::builder()
                .with_service_name("scorpio-analyst")
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
    // Layer order mirrors rig's own otel examples: filter → fmt → otel.
    // The otel layer comes last so it sees spans after the fmt layer has
    // already attached its event hooks.
    if let Some(tracer) = tracer {
        tracing_subscriber::registry()
            .with(build_env_filter())
            .with(fmt::layer().json().flatten_event(true))
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
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
            .with(fmt::layer().pretty())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
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
