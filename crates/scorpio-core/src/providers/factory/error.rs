//! Error conversion and sanitization utilities for the provider factory.
//!
//! - `map_prompt_error_with_context` — map `rig` prompt errors to [`TradingError`].
//! - `map_structured_output_error_with_context` — map structured-output errors,
//!   distinguishing schema violations from transport failures.
//! - [`sanitize_error_summary`] — redact credentials and truncate error strings for safe logging.

use rig::completion::{PromptError, StructuredOutputError};

use crate::{constants::MAX_ERROR_SUMMARY_CHARS, error::TradingError};

// ────────────────────────────────────────────────────────────────────────────
// Error mapping
// ────────────────────────────────────────────────────────────────────────────

pub(super) fn map_prompt_error_with_context(
    provider: &str,
    model_id: &str,
    err: PromptError,
) -> TradingError {
    TradingError::Rig(format!(
        "provider={provider} model={model_id} summary={}",
        sanitize_error_summary(&err.to_string())
    ))
}

pub(super) fn map_structured_output_error_with_context(
    provider: &str,
    model_id: &str,
    err: StructuredOutputError,
) -> TradingError {
    match err {
        StructuredOutputError::DeserializationError(_e) => {
            // Do not surface the raw serde error — it can contain a fragment of the
            // LLM's response text, which may include sensitive content.
            tracing::debug!(
                provider,
                model_id,
                "structured output deserialization failed"
            );
            TradingError::SchemaViolation {
                message: format!(
                    "provider={provider} model={model_id}: structured output could not be parsed"
                ),
            }
        }
        StructuredOutputError::EmptyResponse => TradingError::SchemaViolation {
            message: format!("provider={provider} model={model_id}: model returned empty response"),
        },
        StructuredOutputError::PromptError(e) => {
            map_prompt_error_with_context(provider, model_id, e)
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Sanitization utilities
// ────────────────────────────────────────────────────────────────────────────

/// Replace ASCII/Unicode control characters with a space.
pub(crate) fn replace_control_chars(s: &str) -> String {
    s.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

/// Redact known credential patterns (API key prefixes, auth headers, bearer tokens).
pub(crate) fn redact_credentials(s: &str) -> String {
    fn mask_prefixed_token(input: &str, prefix: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let bytes = input.as_bytes();
        let prefix_bytes = prefix.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i..].starts_with(prefix_bytes) {
                out.push_str("[REDACTED]");
                i += prefix_bytes.len();
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                        i += ch.len_utf8();
                    } else {
                        break;
                    }
                }
            } else {
                let ch = input[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }

        out
    }

    fn mask_assignment(input: &str, key: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let bytes = input.as_bytes();
        let key_bytes = key.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i..].starts_with(key_bytes) {
                out.push_str("[REDACTED]");
                i += key_bytes.len();
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch.is_whitespace() || matches!(ch, '&' | ',' | ';' | ')' | ']' | '}') {
                        break;
                    }
                    i += ch.len_utf8();
                }
            } else {
                let ch = input[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }

        out
    }

    fn mask_bearer(input: &str, prefix: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let bytes = input.as_bytes();
        let prefix_bytes = prefix.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i..].starts_with(prefix_bytes) {
                out.push_str("[REDACTED]");
                i += prefix_bytes.len();
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
                        break;
                    }
                    i += ch.len_utf8();
                }
            } else {
                let ch = input[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }

        out
    }

    let mut out = s.to_owned();
    for prefix in ["sk-ant-", "sk-", "AIza", "aiza"] {
        out = mask_prefixed_token(&out, prefix);
    }
    for key in ["api_key=", "api-key=", "apikey=", "token="] {
        out = mask_assignment(&out, key);
    }
    for prefix in ["Bearer ", "bearer ", "BEARER "] {
        out = mask_bearer(&out, prefix);
    }
    out = out.replace("Authorization:", "[REDACTED]");
    out = out.replace("authorization:", "[REDACTED]");
    out = out.replace("AUTHORIZATION:", "[REDACTED]");
    out
}

/// Truncate `s` to at most `max_chars` Unicode scalar values, appending `"..."` if trimmed.
pub(crate) fn truncate_to(s: &str, max_chars: usize) -> String {
    let truncated: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        format!("{truncated}...")
    } else {
        truncated
    }
}

pub fn sanitize_error_summary(input: &str) -> String {
    let sanitized = replace_control_chars(input);
    let sanitized = redact_credentials(&sanitized);
    truncate_to(&sanitized, MAX_ERROR_SUMMARY_CHARS)
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedLogBuffer {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).expect("valid utf8 logs")
        }
    }

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(Arc::clone(&self.0))
        }
    }

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // ── Error mapping ────────────────────────────────────────────────────

    #[test]
    fn map_prompt_error_produces_rig_variant() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "test error".to_owned(),
        ));
        let mapped = map_prompt_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::Rig(_)));
        assert!(mapped.to_string().contains("openai"));
        assert!(mapped.to_string().contains("gpt-4o-mini"));
    }

    #[test]
    fn map_structured_output_deserialization_error_produces_schema_violation() {
        let json_err = serde_json::from_str::<i32>("not a number").unwrap_err();
        let err = StructuredOutputError::DeserializationError(json_err);
        let mapped = map_structured_output_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn map_structured_output_empty_response_produces_schema_violation() {
        let err = StructuredOutputError::EmptyResponse;
        let mapped = map_structured_output_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
        assert!(mapped.to_string().contains("empty response"));
    }

    #[test]
    fn map_structured_output_prompt_error_falls_through_to_rig() {
        let inner = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "inner".to_owned(),
        ));
        let err = StructuredOutputError::PromptError(inner);
        let mapped = map_structured_output_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::Rig(_)));
    }

    #[test]
    fn sanitize_error_summary_redacts_secret_like_values() {
        let sanitized = sanitize_error_summary("authorization failed for sk-secret-value");
        assert!(!sanitized.contains("sk-secret-value"));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn sanitize_error_summary_redacts_openrouter_query_style_key() {
        let sanitized = sanitize_error_summary(
            "openrouter request failed: api_key=or-secret-value for model qwen/qwen3.6-plus-preview:free",
        );
        assert!(!sanitized.contains("or-secret-value"));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn sanitize_error_summary_flattens_multiline_input_for_single_line_logging() {
        let sanitized = sanitize_error_summary(
            "request failed\nAuthorization: Bearer secret-token\tapi_key=secret123",
        );

        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\r'));
        assert!(!sanitized.contains('\t'));
        assert!(!sanitized.contains("secret-token"));
        assert!(!sanitized.contains("secret123"));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn map_prompt_error_flattens_summary_before_embedding_in_error_message() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "first line\napi_key=secret123\tsecond line".to_owned(),
        ));

        let mapped =
            map_prompt_error_with_context("openrouter", "qwen/qwen3.6-plus-preview:free", err);
        let message = mapped.to_string();

        assert!(!message.contains('\n'));
        assert!(!message.contains('\r'));
        assert!(!message.contains('\t'));
        assert!(!message.contains("secret123"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn structured_output_deserialization_logs_do_not_include_raw_payload() {
        let logs = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .with_writer(logs.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let json_err = serde_json::from_str::<i32>("\"super-secret-payload\"").unwrap_err();
            let mapped = map_structured_output_error_with_context(
                "openrouter",
                "qwen/qwen3.6-plus-preview:free",
                StructuredOutputError::DeserializationError(json_err),
            );
            assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
        });

        let output = logs.contents();
        assert!(
            !output.contains("super-secret-payload"),
            "raw payload should not appear in debug logs: {output}"
        );
    }

    #[test]
    fn redacts_gemini_api_key_prefix() {
        let result = sanitize_error_summary("key=AIzaSyTest1234");
        assert!(
            !result.contains("AIza"),
            "Gemini key prefix must be redacted"
        );
        assert!(
            !result.contains("SyTest1234"),
            "Gemini key body must be redacted"
        );
    }

    #[test]
    fn redacts_bearer_token() {
        let result = sanitize_error_summary("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9");
        assert!(!result.contains("Bearer "), "Bearer token must be redacted");
        assert!(
            !result.contains("eyJhbGciOiJIUzI1NiJ9"),
            "Bearer token body must be redacted"
        );
    }

    #[test]
    fn redacts_api_key_eq() {
        let result = sanitize_error_summary("request failed: api_key=secret123");
        assert!(!result.contains("api_key="), "api_key= must be redacted");
        assert!(
            !result.contains("secret123"),
            "api_key value must be redacted"
        );
    }

    #[test]
    fn redacts_openai_style_key_body() {
        let result = sanitize_error_summary("provider said sk-live-abc123XYZ failed");
        assert!(!result.contains("sk-live-abc123XYZ"));
        assert!(!result.contains("abc123XYZ"));
    }
}
