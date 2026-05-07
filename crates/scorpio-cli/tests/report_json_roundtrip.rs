//! End-to-end test: `scorpio report show --json` produces parseable JSON.

use std::path::PathBuf;
use std::process::Command;

use scorpio_cli::cli::report::ReportJson;
use scorpio_core::state::TradingState;
use scorpio_core::workflow::snapshot::{SnapshotPhase, SnapshotStore};

#[tokio::test]
async fn report_show_json_round_trips() {
    let tmp_home = tempfile::tempdir().expect("temp home");
    let tmp_db = tempfile::NamedTempFile::new().expect("temp file");
    let db_path: PathBuf = tmp_db.path().to_path_buf();

    let store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("open store");
    let state = TradingState::new("AAPL", "2026-01-15");
    let exec_id = state.execution_id.to_string();
    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save");

    drop(store);

    let bin = env!("CARGO_BIN_EXE_scorpio-cli");
    let output = Command::new(bin)
        .args(["report", "show", &exec_id, "--json"])
        .env("HOME", tmp_home.path())
        .env("SCORPIO__STORAGE__SNAPSHOT_DB_PATH", &db_path)
        .output()
        .expect("run CLI");

    assert!(
        output.status.success(),
        "CLI exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: ReportJson = serde_json::from_slice(&output.stdout)
        .expect("output should round-trip through ReportJson");

    assert_eq!(parsed.execution_id, exec_id);
    assert_eq!(parsed.phase_number, 5);
    assert_eq!(parsed.phases_present, 1);
    assert!(
        parsed.is_complete,
        "phase 5 save should mark is_complete true"
    );
}
