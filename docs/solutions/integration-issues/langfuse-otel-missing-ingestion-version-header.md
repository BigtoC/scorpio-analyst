---
title: "Langfuse OpenTelemetry traces silently dropped (missing `x-langfuse-ingestion-version` header)"
date: 2026-05-29
category: integration-issues
module: observability
problem_type: integration_issue
component: tooling
symptoms:
  - "Langfuse dashboard remains empty despite successful application runs"
  - "Project info API authentication succeeds (valid project data returned)"
  - "`BatchSpanProcessor` emits no export errors with `RUST_LOG=opentelemetry_sdk=debug`"
  - "Spans appear to flush cleanly on shutdown but never surface in Langfuse UI"
  - "No HTTP 4xx/5xx errors visible from the OTel exporter despite traces being rejected"
root_cause: incomplete_setup
resolution_type: config_change
severity: high
related_components:
  - tracing
  - opentelemetry
  - langfuse
tags: [langfuse, opentelemetry, otel, observability, tracing, rust, silent-failure, integration]
---

# Langfuse OpenTelemetry traces silently dropped (missing `x-langfuse-ingestion-version` header)

## Problem

Langfuse OpenTelemetry traces were silently dropped from a Rust tokio CLI — the dashboard at `jp.cloud.langfuse.com` remained empty despite no 4xx errors, no panics, and successful basic-auth handshakes. The proximate cause was a missing `x-langfuse-ingestion-version: 4` HTTP header that the `opentelemetry-langfuse 0.6.1` crate does not set automatically; getting there also required fixing four orthogonal foot-guns (OTel crate-version skew, swapped env-var values, wrong span-processor variant for tokio, and `std::process::exit` bypassing the `Drop`-based flush).

## Symptoms

- Dashboard at `jp.cloud.langfuse.com` shows zero traces under the correct project for the correct time range, despite the run completing normally and the auth env vars being set.
- `curl https://jp.cloud.langfuse.com/api/public/projects -u "$PUB:$SEC"` returns `200 OK` — auth itself is fine.
- No 4xx/5xx export errors with `RUST_LOG=opentelemetry=debug,opentelemetry_sdk=debug` — the SDK reports successful HTTP POSTs to `/api/public/otel/v1/traces`.
- Intermittent (depending on which broken variant is in use) spam of `BatchSpanProcessor.OnEnd.AfterShutdown` warnings once the background exporter worker dies and the span channel becomes disconnected.
- Compile-time failure when crate majors drift: `multiple different versions of crate "opentelemetry_sdk"` / trait-identity mismatch between the langfuse `SpanExporter` impl and the `SdkTracerProvider` builder.
- Final batch of spans missing on any failure-path exit (non-zero exit code), even when earlier spans from the same run made it through.

## What Didn't Work

- **Bumping just `opentelemetry_sdk` to a newer pin** while leaving `opentelemetry = "0.32"` / `tracing-opentelemetry = "0.33"` — `opentelemetry-langfuse 0.6.1` is locked to the `opentelemetry 0.31` family, so two majors of `opentelemetry` ended up in the dep graph and the `SpanExporter` trait identity didn't line up with the builder's expected trait.
- **`with_simple_exporter(exporter)`** — fires a synchronous export per span; on a tokio app it blocks the reactor and the spans created in the last ~5 seconds of the run get lost during runtime teardown.
- **`with_batch_exporter(exporter)`** — this is the default OTel batch processor, which spawns a `std::thread`. The langfuse exporter uses an async `reqwest::Client` and needs a tokio runtime to drive HTTP; the export future panics on the bare std thread with "no reactor running", the worker dies, the span channel becomes disconnected, and every subsequent span trips an `OnEnd.AfterShutdown` warning. (Rig's published examples use `with_batch_exporter` because they pair it with `opentelemetry-otlp`'s blocking reqwest client — different exporter, doesn't apply here.)
- **Relying on the `TracingGuard::Drop` impl alone** to flush at program exit — the CLI's failure branch calls `std::process::exit(exit_code)`, which does not run destructors, so the trailing batch of spans (still buffered inside `BatchSpanProcessor`'s 5 s scheduled-delay window) never gets shipped.
- **Trusting that `opentelemetry-langfuse` would set every required HTTP header** — reading the crate source revealed it only attaches the basic-auth header; the documented `x-langfuse-ingestion-version: 4` header per [Langfuse's OTel integration docs](https://langfuse.com/integrations/native/opentelemetry) is the caller's responsibility, and without it the server silently drops the spans (no 4xx, no log).
- **Assuming the keys were correct because the crate accepted them** — `with_basic_auth(public, secret)` happily runs even with the values swapped; the actual mistake was `SCORPIO_LANGFUSE_PUBLIC_KEY` holding the `sk-l...` secret. Server returned 401, but only on the export call, which the SDK swallowed.

## Solution

**1. Align the OTel ecosystem on the 0.31 family** so the trait identities match across crates. In the root `Cargo.toml`:

```toml
# [workspace.dependencies]
# opentelemetry-langfuse 0.6 only supports the opentelemetry 0.31 family.
# Pin the rest of the OTel ecosystem to matching majors so the SpanExporter
# trait identities line up (langfuse exporter → SdkTracerProvider builder).
# tracing-opentelemetry 0.32.x is the version compatible with opentelemetry 0.31.
tracing-opentelemetry = "0.32"
opentelemetry         = "0.31"
opentelemetry_sdk     = { version = "=0.31.0", features = ["trace", "rt-tokio"] }
opentelemetry-langfuse = "0.6"
```

**2. Build the exporter with the version header, and use the tokio-runtime `BatchSpanProcessor`.** In `crates/scorpio-core/src/observability.rs`:

```rust
// `TracerProvider` is the trait that exposes `.tracer(name)` on SdkTracerProvider.
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_langfuse::ExporterBuilder;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;

let exporter = ExporterBuilder::new()
    .with_host(&host)
    .with_basic_auth(&public_key, &secret_key)
    // REQUIRED by Langfuse OTel ingestion; the 0.6.1 crate does NOT set
    // this automatically. Without it, the server silently drops the spans
    // (no 4xx, no log).
    .with_header("x-langfuse-ingestion-version", "4")
    .build()?;

// MUST use the Tokio-runtime BatchSpanProcessor — NOT `with_batch_exporter`,
// which spawns a std::thread that can't drive the langfuse exporter's
// async reqwest::Client.
let provider = SdkTracerProvider::builder()
    .with_span_processor(BatchSpanProcessor::builder(exporter, Tokio).build())
    .with_resource(
        Resource::builder()
            .with_service_name("scorpio-analyst")
            .build(),
    )
    .build();

let tracer = provider.tracer("scorpio-analyst");
// Clone the provider into a TracingGuard (next snippet) before moving it
// into opentelemetry::global::set_tracer_provider — the clone lets the
// guard call shutdown() while tokio is still alive.
```

**3. Give the guard an explicit `flush_and_shutdown()` so callers can drain before `std::process::exit`.** `Drop` delegates to the same flush+shutdown sequence for the happy path. Same file:

```rust
#[must_use = "TracingGuard must be held until program exit; \
              dropping it flushes pending OTel spans to Langfuse"]
pub struct TracingGuard {
    provider: Option<SdkTracerProvider>,
}

impl TracingGuard {
    /// Explicit drain for exit paths that bypass `Drop`
    /// (e.g. `std::process::exit`).
    pub fn flush_and_shutdown(mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.force_flush();
            let _ = provider.shutdown();
        }
    }
}

// `impl Drop for TracingGuard` runs the same `force_flush()` + `shutdown()`
// sequence on the happy path. Omitted here for brevity.
```

**4. Wire it in `main.rs` with a named binding and explicit flush before every exit.** In `crates/scorpio-cli/src/main.rs`:

```rust
#[tokio::main]
async fn main() {
    // Named binding — NOT `_tracing_guard` (which still drops normally,
    // but the failure branch never reaches Drop because of process::exit).
    let tracing_guard = init_tracing();

    // ... cli dispatch ...

    let exit_code = if let Err(e) = result { eprintln!("{e:#}"); 1 } else { 0 };

    // Flush BEFORE any process::exit. Drop is not enough — process::exit
    // bypasses destructors. The success path benefits too: drains while
    // tokio is fully healthy, avoiding any Drop-vs-runtime-teardown race.
    tracing_guard.flush_and_shutdown();

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
```

**5. Verify env-var orientation.** When debugging an empty dashboard, dump key prefixes once:

```rust
eprintln!(
    "[langfuse-auth] host={host:?}, public.len={}, public={:.4}…{}, secret.len={}",
    public_key.len(),
    &public_key,
    &public_key[public_key.len() - 4..],
    secret_key.len(),
);
```

`SCORPIO_LANGFUSE_PUBLIC_KEY` must start with `pk-l`, `SCORPIO_LANGFUSE_SECRET_KEY` with `sk-l`. If they're swapped, `with_basic_auth(public, secret)` will send `secret:public` and the export will 401.

## Why This Works

**The version header is the actual delivery key.** Langfuse's OTel ingestion endpoint (`/api/public/otel/v1/traces`) uses `x-langfuse-ingestion-version` to route spans into the modern ingestion pipeline that surfaces them in the dashboard. With the header missing, the server accepts the POST (returns 2xx), logs nothing, and discards the spans — there's no client-side signal that anything went wrong. The `opentelemetry-langfuse 0.6.1` crate only attaches the basic-auth header, so the caller has to add the version header explicitly via the generic `.with_header()` builder method. This is the single line that "unbroke" the dashboard.

**The Tokio runtime variant of `BatchSpanProcessor` is mandatory for async HTTP exporters.** OTel ships two batch processor variants. The default `with_batch_exporter` spawns a plain `std::thread` to run the export loop — fine for blocking exporters (`opentelemetry-otlp` with the blocking reqwest client, which is what most rig examples use). The langfuse crate, however, uses async `reqwest::Client`, and `reqwest`'s async client requires a tokio reactor on the calling thread. From a bare `std::thread`, the first await panics with "no reactor running"; the worker dies, the bounded span channel becomes disconnected, and every subsequent span emission trips `OnEnd.AfterShutdown`. `span_processor_with_async_runtime::BatchSpanProcessor::builder(exporter, Tokio).build()` instead spawns the export loop as a tokio task on the existing runtime, which can drive `reqwest::Client::post(...).send().await` correctly.

**Explicit `flush_and_shutdown()` is required because `std::process::exit` bypasses `Drop`.** `BatchSpanProcessor` buffers spans and ships them on a 5 s scheduled delay (or on `force_flush`/`shutdown`). On a happy `main` return, the `TracingGuard::Drop` impl runs, calls `force_flush()` then `shutdown()`, and the final batch ships before the tokio runtime tears down. But our CLI maps `Err` results to a non-zero exit by calling `std::process::exit(exit_code)` — and per the Rust reference, `process::exit` terminates the process without running destructors. Holding the guard in a named binding does not help: the binding's `Drop` never fires on that branch. The fix is to consume the guard explicitly with `flush_and_shutdown()` before any `process::exit` call. Doing it on the success path too is symmetric and removes a subtler race between `Drop` order and tokio runtime teardown.

**The version pins are non-negotiable because `opentelemetry-langfuse 0.6` is the only published version and it requires the `opentelemetry 0.31` family.** OTel crates pin tightly across the ecosystem: an `SpanExporter` impl from `opentelemetry_sdk 0.31` is a different trait object from one in `0.32`, even though the source looks identical. If the workspace pulls in both 0.31 (transitively via langfuse) and 0.32 (directly via the wrong `opentelemetry` / `tracing-opentelemetry` versions), `SdkTracerProvider::builder()` from one major refuses to accept the exporter from the other — compile-time trait-identity mismatch. Aligning `opentelemetry = "0.31"`, `opentelemetry_sdk = "=0.31.0"`, `tracing-opentelemetry = "0.32"` (the version targeting opentelemetry 0.31), and `opentelemetry-langfuse = "0.6"` puts everything on the same trait set. The one extra one-liner — `use opentelemetry::trace::TracerProvider as _;` — brings the trait's `.tracer(name)` method into scope at the call site, since `SdkTracerProvider`'s `tracer` is provided through the trait rather than as an inherent method.

## Prevention

- **When integrating any OTel exporter crate, read the exporter's source for required HTTP headers.** Vendors document required headers (`x-langfuse-ingestion-version`, `x-honeycomb-team`, etc.) in their integration guides but rarely set them inside the published exporter crate. Don't assume "the crate handles it" — `grep` for `with_header` / `HeaderMap::insert` in the exporter source and cross-reference against the vendor's OTel docs.
- **When the dashboard is empty and there are no export errors, enable SDK-level OTel logging FIRST.** `RUST_LOG=opentelemetry=debug,opentelemetry_sdk=debug,opentelemetry_langfuse=debug` shows the actual HTTP status codes and payload sizes returned by the ingestion endpoint. Empty dashboard + `200 OK` exports points squarely at a header / version / routing mismatch on the server side.
- **When mixing OTel crates in a workspace, lock all of them to the same major and let the most-constrained crate dictate the version.** `opentelemetry-langfuse` (or any other vendor exporter) is usually the lagging crate; align `opentelemetry`, `opentelemetry_sdk`, and `tracing-opentelemetry` to whatever family it requires, not the latest major.
- **Auth diagnostic recipe for `with_basic_auth`-style APIs:** temporarily `eprintln!` the first 4 + last 4 characters of each credential plus its length right before the builder call. This catches both env-var swaps (public key holds a `sk-` prefix) and silent whitespace contamination (trailing `\n` from `cat`-ing a secret into `.env`).
- **For async LLM apps using Langfuse (or any async-reqwest-based exporter): ALWAYS use `BatchSpanProcessor::builder(exporter, Tokio).build()` via `span_processor_with_async_runtime`, NEVER `with_batch_exporter` and NEVER `with_simple_exporter`.** The first blocks the reactor per span; the second spawns a std::thread that can't drive async HTTP. The tokio-runtime variant is the only correct choice.
- **Wire the tracing guard's explicit `flush_and_shutdown()` before EVERY `std::process::exit` call site.** `Drop` is not a substitute. As a workspace rule: grep for `process::exit` whenever you add observability, and make sure each one is preceded by an explicit flush of any RAII guards holding buffered I/O. Consider a `clippy.toml` `disallowed-methods` rule for `std::process::exit` to force the audit on each addition.

## Related Issues

- Langfuse OTel integration docs: <https://langfuse.com/integrations/native/opentelemetry> (canonical source of the required `x-langfuse-ingestion-version: 4` header).
- Rig observability concepts: <https://docs.rig.rs/docs/concepts/observability> (rig auto-emits `gen_ai.*` semantic-convention spans for `invoke_agent`, `chat`, `execute_tool`; no caller changes needed once the exporter pipeline works).
- Key file references in this repo:
  - `crates/scorpio-core/src/observability.rs` — `TracingGuard`, `init_langfuse_tracer`, `BatchSpanProcessor` wiring, version header.
  - `crates/scorpio-cli/src/main.rs` — named guard binding + explicit `flush_and_shutdown()` before `std::process::exit`.
  - `Cargo.toml` (workspace deps) — aligned OTel 0.31-family pins.
