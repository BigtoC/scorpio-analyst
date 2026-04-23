use std::sync::Arc;

use chrono::Utc;
use scorpio_core::state::TradingState;
use scorpio_reporters::json::{JsonReport, JsonReporter};
use scorpio_reporters::{ReportContext, Reporter};
use tempfile::tempdir;

fn test_state(symbol: &str) -> Arc<TradingState> {
    Arc::new(TradingState::new(symbol, "2026-04-23"))
}

fn test_ctx(symbol: &str, output_dir: std::path::PathBuf) -> Arc<ReportContext> {
    Arc::new(ReportContext {
        symbol: symbol.to_owned(),
        finished_at: Utc::now(),
        output_dir: Some(output_dir),
    })
}

fn test_ctx_at(
    symbol: &str,
    output_dir: std::path::PathBuf,
    finished_at: chrono::DateTime<Utc>,
) -> Arc<ReportContext> {
    Arc::new(ReportContext {
        symbol: symbol.to_owned(),
        finished_at,
        output_dir: Some(output_dir),
    })
}

#[tokio::test]
async fn json_reporter_writes_valid_file_with_correct_schema_version() {
    let dir = tempdir().unwrap();
    let state = test_state("AAPL");
    let ctx = test_ctx("AAPL", dir.path().to_path_buf());

    JsonReporter
        .emit(Arc::clone(&state), Arc::clone(&ctx))
        .await
        .expect("emit should succeed");

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(entries.len(), 1, "exactly one file should be written");

    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let report: JsonReport = serde_json::from_str(&content).expect("file must deserialize");
    assert_eq!(report.schema_version, 1);
    assert_eq!(report.trading_state.asset_symbol, "AAPL");
}

#[tokio::test]
async fn json_reporter_creates_missing_output_dir() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("nested/deep/reports");
    assert!(!nested.exists(), "pre-condition: dir must not exist yet");

    let state = test_state("NVDA");
    let ctx = test_ctx("NVDA", nested.clone());

    JsonReporter
        .emit(Arc::clone(&state), Arc::clone(&ctx))
        .await
        .expect("emit should succeed even when output_dir is missing");

    assert!(nested.exists(), "output_dir must be created on demand");
    let entries: Vec<_> = std::fs::read_dir(&nested).unwrap().collect();
    assert_eq!(entries.len(), 1);
}

#[tokio::test]
async fn json_reporter_filename_contains_symbol_and_timestamp() {
    let dir = tempdir().unwrap();
    let state = test_state("TSLA");
    let ctx = test_ctx("TSLA", dir.path().to_path_buf());

    JsonReporter
        .emit(Arc::clone(&state), Arc::clone(&ctx))
        .await
        .unwrap();

    let entry = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let name = entry.file_name().into_string().unwrap();
    assert!(
        name.starts_with("TSLA-"),
        "filename must start with symbol; got: {name}"
    );
    assert!(
        name.ends_with(".json"),
        "filename must end with .json; got: {name}"
    );
}

#[tokio::test]
async fn json_reporter_does_not_overwrite_existing_artifact() {
    let dir = tempdir().unwrap();
    let state = test_state("AAPL");
    let finished_at = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 4, 23, 12, 0, 0)
        .single()
        .unwrap();
    let ctx = test_ctx_at("AAPL", dir.path().to_path_buf(), finished_at);
    let path = dir.path().join("AAPL-20260423T120000000Z.json");
    std::fs::write(&path, "original").unwrap();

    let err = JsonReporter
        .emit(Arc::clone(&state), Arc::clone(&ctx))
        .await
        .expect_err("existing artifact should not be overwritten silently");

    assert!(err.to_string().contains("writing"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
}
