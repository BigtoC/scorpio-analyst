use chrono::Utc;

use super::*;
use crate::state::TradingState;

mod core_errors;
mod core_roundtrip;
mod path;
mod thesis_compat;
mod thesis_lookup;

/// Open an in-memory SQLite snapshot store for tests.
async fn in_memory_store() -> SnapshotStore {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.keep().join("test.db");
    SnapshotStore::new(Some(&path))
        .await
        .expect("in-memory store")
}

fn sample_state() -> TradingState {
    TradingState::new("AAPL", "2026-01-15")
}

fn sample_thesis() -> crate::state::ThesisMemory {
    crate::state::ThesisMemory {
        symbol: "AAPL".to_owned(),
        action: "Buy".to_owned(),
        decision: "Approved".to_owned(),
        rationale: "Strong fundamentals.".to_owned(),
        summary: None,
        execution_id: "exec-thesis-001".to_owned(),
        target_date: "2026-04-07".to_owned(),
        captured_at: Utc::now(),
    }
}

#[derive(Debug)]
struct FailingSerialize;

impl serde::Serialize for FailingSerialize {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom(
            "intentional serialization failure",
        ))
    }
}
