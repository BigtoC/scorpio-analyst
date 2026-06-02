---
title: Langfuse generation spans missing Input/Output text
date: 2026-06-02
category: integration-issues
module: providers/factory
problem_type: integration_issue
component: tooling
severity: medium
symptoms:
  - Langfuse dashboard shows blank Input and Output columns for all LLM generation spans
  - Token counts and model name appear correctly but prompt text and completion text are absent
  - span.record calls for gen_ai.prompt and gen_ai.completion execute without error but have no visible effect
root_cause: incomplete_setup
resolution_type: code_fix
tags:
  - langfuse
  - opentelemetry
  - tracing
  - gen-ai
  - span-attributes
  - rust
  - observability
  - silent-failure
related_components:
  - observability
---

# Langfuse generation spans missing Input/Output text

## Problem

After establishing the Langfuse OTel pipeline, all LLM generation spans appeared in Langfuse
with model names and token counts but with blank Input and Output fields. The `gen_ai.prompt`
and `gen_ai.completion` attributes were never set because they were not declared upfront in the
`tracing::info_span!` macro — a call to `span.record()` on an undeclared field is silently
ignored with no error or warning.

## Symptoms

- Langfuse dashboard shows blank Input and Output columns for all LLM generation spans
- Token counts and model name appear correctly but prompt text and completion text are absent
- `span.record` calls for `gen_ai.prompt` and `gen_ai.completion` execute without error but have no visible effect

## What Didn't Work

- **Adding `span.record("gen_ai.prompt", prompt)` without first declaring the field**: silently
  a no-op. The `tracing` crate does not surface this failure — the span is exported without the
  field as if the call never happened.
- **`gen_ai.server.time_to_first_token` as a span attribute**: this is an OTel Histogram metric
  instrument only; Langfuse does not map it to generation spans.
- **Setting `langfuse.observation.completion_start_time` equal to the response end time for
  non-streaming calls**: triggers Langfuse bug #3554, which corrupts the generation record. This
  field must be omitted entirely for non-streaming architectures.

## Solution

### 1. Declare every post-call field as `tracing::field::Empty` upfront

```rust
fn llm_span(&self, operation: &str) -> tracing::Span {
    tracing::info_span!(
        "llm_generation",
        otel.name = operation,
        gen_ai.system = self.provider_name(),
        gen_ai.request.model = self.model_id(),
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.prompt = tracing::field::Empty,      // ← must be declared here
        gen_ai.completion = tracing::field::Empty,  // ← must be declared here
    )
}
```

### 2. Replace `record_usage` with `record_generation`

```rust
fn record_generation(
    span: &tracing::Span,
    prompt: &str,
    completion: Option<&str>,  // None for typed/structured-output calls
    usage: &rig::completion::Usage,
) {
    span.record("gen_ai.prompt", prompt);
    if let Some(c) = completion {
        span.record("gen_ai.completion", c);
    }
    span.record("gen_ai.usage.input_tokens", usage.input_tokens);
    span.record("gen_ai.usage.output_tokens", usage.output_tokens);
}
```

`completion` is `Option<&str>` because typed/structured-output calls return `T: DeserializeOwned`
which is not necessarily `Serialize` — adding a bound to satisfy observability would be wrong
coupling. Omit it; usage attribution still works.

Call sites for text-returning methods:

```rust
record_generation(&span, prompt, Some(&response.output), &response.usage);
```

Call sites for typed methods:

```rust
record_generation(&span, prompt, None, &response.usage);
```

### 3. Delegate simple String-returning methods to their `_details` counterparts

`prompt` and `chat` returned `String` with no way to capture usage or content. Delegating to
`prompt_details` / `chat_details` gives them full telemetry through the detail path:

```rust
pub async fn prompt(&self, prompt: &str) -> Result<String, PromptError> {
    Ok(self.prompt_details(prompt).await?.output)
}

pub async fn chat(&self, prompt: &str, chat_history: Vec<Message>) -> Result<String, PromptError> {
    let mut history = chat_history;
    Ok(self.chat_details(prompt, &mut history).await?.output)
}
```

### 4. Set `langfuse.release` as a resource attribute

```rust
Resource::builder()
    .with_service_name("scorpio-analyst")
    .with_attribute(KeyValue::new("langfuse.release", env!("CARGO_PKG_VERSION")))
    .build()
```

Populates Langfuse's release filter automatically from `Cargo.toml`.

## Why This Works

**`tracing::field::Empty` reserves the slot.** The tracing crate builds a static metadata table
at compile time from the `info_span!` invocation. `span.record("field", value)` looks up the
field by name in that table. If the field was not declared, the lookup silently fails and the
value is discarded — there is no dynamic field addition in tracing.

**`gen_ai.request.model` triggers Langfuse generation classification.** Any OTel span with this
attribute is automatically promoted from a plain observation to a "generation" in Langfuse — no
other SDK or configuration needed.

**`gen_ai.prompt` and `gen_ai.completion` populate Langfuse's Input/Output fields.** Langfuse
maps these OTel attributes directly to the Input and Output display fields in the generation
detail view.

**Explicit token counts beat tokenizer inference.** Langfuse's built-in tokenizer is inaccurate
for Claude 3+ models. Providing `gen_ai.usage.input_tokens` and `gen_ai.usage.output_tokens`
from the provider response ensures accurate cost attribution.

## Prevention

- **Every field filled via `span.record()` must be pre-declared as `tracing::field::Empty` in
  `info_span!`.** Treat the two as an inseparable pair — adding a `span.record()` call without
  the matching `tracing::field::Empty` in the macro is a silent no-op, with no compile error and
  no runtime warning.
- **Use `record_generation()` for all LLM call sites.** The helper ensures prompt text,
  completion text, and token counts are recorded consistently. Pass `None` for completion on
  typed/structured calls.
- **Delegate simple wrapper methods to detail variants** so the telemetry path runs once in the
  detail implementation. Methods returning `String` have no inline telemetry path.
- **Do not set `langfuse.observation.completion_start_time` on non-streaming calls.** For
  non-streaming architectures (all current calls in this repo), leave TTFT unset — Langfuse bug
  #3554 is triggered when it equals the end time.

## Related Issues

- Prerequisite: [langfuse-otel-missing-ingestion-version-header.md](langfuse-otel-missing-ingestion-version-header.md)
  establishes the OTel exporter pipeline this doc builds on. **Correction:** that doc contains
  the claim "Rig auto-emits `gen_ai.*` semantic-convention spans; no caller changes needed once
  the exporter pipeline works." This is incorrect — caller-side `gen_ai.*` attribute wiring is
  required, as documented here.
- OTel GenAI Semantic Conventions: https://opentelemetry.io/docs/specs/semconv/gen-ai/
- Langfuse OTel integration: https://langfuse.com/integrations/native/opentelemetry
