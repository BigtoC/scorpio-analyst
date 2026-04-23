//! Focused edge-case tests for secret redaction and error/timeout behavior.

use std::time::Duration;

use scorpio_core::config::{ApiConfig, Config, ProviderSettings, ProvidersConfig};
use scorpio_core::error::{RetryPolicy, TradingError, check_analyst_degradation};
use secrecy::SecretString;

// ── Secret redaction ───────────────────────────────────────────────

#[test]
fn api_config_debug_never_leaks_secrets() {
    let api = ApiConfig {
        finnhub_api_key: Some(SecretString::from("ct_finnhub_key")),
        fred_api_key: None,
    };
    let debug = format!("{api:?}");

    // No secret value should appear anywhere in the debug output
    assert!(!debug.contains("ct_finnhub_key"));
    // The one present key should show [REDACTED]
    assert_eq!(debug.matches("[REDACTED]").count(), 1);
    // The absent key should show <not set>
    assert_eq!(debug.matches("<not set>").count(), 1);
}

#[test]
fn providers_config_debug_never_leaks_secrets() {
    let providers = ProvidersConfig {
        openai: ProviderSettings {
            api_key: Some(SecretString::from("sk-live-abc123")),
            ..Default::default()
        },
        anthropic: ProviderSettings {
            api_key: Some(SecretString::from("sk-ant-secret")),
            ..Default::default()
        },
        openrouter: ProviderSettings {
            api_key: Some(SecretString::from("or-live-secret")),
            ..Default::default()
        },
        ..Default::default()
    };
    let debug = format!("{providers:?}");

    // No secret value should appear anywhere in the debug output
    assert!(!debug.contains("sk-live-abc123"));
    assert!(!debug.contains("sk-ant-secret"));
    assert!(!debug.contains("or-live-secret"));
    // All set provider keys should show [REDACTED]
    assert!(debug.contains("[REDACTED]"));
    assert_eq!(debug.matches("[REDACTED]").count(), 3);
    // Providers without keys should show <not set>
    assert!(debug.contains("<not set>"));
}

#[test]
fn config_debug_does_not_leak_secrets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#,
    )
    .expect("write config");
    let cfg = Config::load_from(&path).expect("config should load");
    let debug = format!("{cfg:?}");
    // Even after loading, no raw env var values should appear
    assert!(!debug.contains("sk-live"));
    assert!(!debug.contains("sk-ant"));
}

// ── Timeout error formatting ───────────────────────────────────────

#[test]
fn network_timeout_displays_duration_and_message() {
    let err = TradingError::NetworkTimeout {
        elapsed: Duration::from_secs(30),
        message: "analyst fetch timed out".to_owned(),
    };
    let display = err.to_string();
    assert!(display.contains("30"));
    assert!(display.contains("analyst fetch timed out"));
}

#[test]
fn schema_violation_surfaces_context() {
    let err = TradingError::SchemaViolation {
        message: "missing field `confidence`".to_owned(),
    };
    assert!(err.to_string().contains("missing field `confidence`"));
}

// ── Retry policy edge cases ───────────────────────────────────────

#[test]
fn retry_delay_saturates_on_high_attempt() {
    let policy = RetryPolicy {
        max_retries: 10,
        base_delay: Duration::from_millis(500),
    };
    // Attempt 30 — 2^30 * 500ms would overflow u32 but saturating_pow prevents panic
    let delay = policy.delay_for_attempt(30);
    assert!(delay >= Duration::from_millis(500));
}

#[test]
fn retry_zero_base_delay_yields_zero() {
    let policy = RetryPolicy {
        max_retries: 3,
        base_delay: Duration::ZERO,
    };
    assert_eq!(policy.delay_for_attempt(0), Duration::ZERO);
    assert_eq!(policy.delay_for_attempt(2), Duration::ZERO);
}

// ── Analyst degradation boundary cases ────────────────────────────

#[test]
fn degradation_zero_total_zero_failures_is_ok() {
    assert!(check_analyst_degradation(0, &[]).is_ok());
}

#[test]
fn degradation_one_of_one_failure_aborts() {
    // 1 total, 1 failure = total failure → abort
    assert!(check_analyst_degradation(1, &["Agent".to_owned()]).is_err());
}

#[test]
fn degradation_boundary_at_two_failures() {
    assert!(check_analyst_degradation(4, &["Fundamental Analyst".to_owned()]).is_ok());
    let two = vec!["Fundamental Analyst".to_owned(), "News Analyst".to_owned()];
    assert!(check_analyst_degradation(4, &two).is_err());
}
