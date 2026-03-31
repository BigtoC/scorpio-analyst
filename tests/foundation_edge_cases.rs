//! Focused edge-case tests for secret redaction and error/timeout behavior.

use std::time::Duration;

use scorpio_analyst::config::{ApiConfig, Config};
use scorpio_analyst::error::{check_analyst_degradation, RetryPolicy, TradingError};
use secrecy::SecretString;

// ── Secret redaction ───────────────────────────────────────────────

#[test]
fn api_config_debug_never_leaks_secrets() {
    let api = ApiConfig {
        openai_api_key: Some(SecretString::from("sk-live-abc123")),
        anthropic_api_key: Some(SecretString::from("sk-ant-secret")),
        gemini_api_key: None,
        finnhub_api_key: Some(SecretString::from("ct_finnhub_key")),
    };
    let debug = format!("{api:?}");

    // No secret value should appear anywhere in the debug output
    assert!(!debug.contains("sk-live-abc123"));
    assert!(!debug.contains("sk-ant-secret"));
    assert!(!debug.contains("ct_finnhub_key"));
    // All present keys should show [REDACTED]
    assert_eq!(debug.matches("[REDACTED]").count(), 3);
    // Absent key should show <not set>
    assert!(debug.contains("<not set>"));
}

#[test]
fn config_debug_does_not_leak_secrets() {
    let cfg = Config::load_from("config.toml");
    if let Ok(cfg) = cfg {
        let debug = format!("{cfg:?}");
        // Even after loading, no raw env var values should appear
        assert!(!debug.contains("sk-live"));
        assert!(!debug.contains("sk-ant"));
    }
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
