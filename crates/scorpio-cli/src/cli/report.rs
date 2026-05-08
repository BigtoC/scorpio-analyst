//! `scorpio report` subcommand handler.

use anyhow::Context;
use serde::{Deserialize, Serialize};

use scorpio_core::state::{AgentTokenUsage, TradingState};
use scorpio_core::workflow::{
    ExecutionListing, LoadedReport, SnapshotPhase, SnapshotStore, THESIS_MEMORY_SCHEMA_VERSION,
};
use scorpio_reporters::terminal::{render_execution_list, render_final_report};

use super::{ReportArgs, ReportSubcommand};

/// JSON payload emitted by `report show --json`.
///
/// Round-trippable: callers can deserialize back into this struct to drive
/// audit/replay tooling.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReportJson {
    pub execution_id: String,
    pub state: TradingState,
    pub token_usage: Option<Vec<AgentTokenUsage>>,
    /// Phase number of `state` (the highest visible phase for this execution).
    pub phase_number: i64,
    /// Total phases visible for this execution under the active schema.
    pub phases_present: usize,
    /// Whether this execution reached the final phase (FundManager).
    /// Canonical completion check — JSON consumers should branch on this rather
    /// than inspect `state` to decide whether the run is final.
    pub is_complete: bool,
    /// Schema version this payload was produced against.
    pub schema_version: i64,
    /// Phase numbers skipped because the persisted row was unreadable.
    pub skipped_phases: Vec<i64>,
}

/// Dispatch `scorpio report` subcommands.
pub async fn run(args: &ReportArgs) -> anyhow::Result<()> {
    match &args.subcommand {
        ReportSubcommand::List { json } => run_list(*json).await,
        ReportSubcommand::Show { execution_id, json } => run_show(execution_id, *json).await,
    }
}

/// List all past analysis executions.
async fn run_list(json: bool) -> anyhow::Result<()> {
    let store = SnapshotStore::from_runtime_storage().await?;
    let listing = store.list_executions().await?;

    println!("{}", render_list_output(&listing, json)?);

    if !json {
        if listing.stale_count > 0 {
            eprintln!(
                "Note: {} run(s) are not displayed because they were created with an older schema. \
                 Re-run the analysis to produce a new execution under schema version {}.",
                listing.stale_count, THESIS_MEMORY_SCHEMA_VERSION,
            );
        }
        if listing.invalid_timestamp_count > 0 {
            eprintln!(
                "Warning: {} run(s) were omitted because their persisted timestamps were unreadable.",
                listing.invalid_timestamp_count,
            );
        }
    }

    Ok(())
}

/// Show the full report for a specific execution.
async fn run_show(execution_id: &str, json: bool) -> anyhow::Result<()> {
    let store = SnapshotStore::from_runtime_storage().await?;
    let report = store.load_full_report(execution_id).await?;

    if report.snapshots.is_empty() {
        let exists_any_schema = if report.skipped_phases.is_empty() {
            store.execution_exists(execution_id).await?
        } else {
            true
        };
        return Err(report_lookup_error(
            execution_id,
            &report,
            exists_any_schema,
        ));
    }

    println!("{}", render_show_output(execution_id, &report, json)?);

    if !json && !report.skipped_phases.is_empty() {
        eprintln!(
            "Warning: {} phase(s) were unreadable (corrupt rows): {:?}",
            report.skipped_phases.len(),
            report.skipped_phases,
        );
    }

    Ok(())
}

fn render_list_output(listing: &ExecutionListing, json: bool) -> anyhow::Result<String> {
    if json {
        serde_json::to_string_pretty(listing).context("failed to serialize execution listing")
    } else if listing.summaries.is_empty() {
        Ok("No executions found.".to_owned())
    } else {
        Ok(render_execution_list(&listing.summaries))
    }
}

fn render_show_output(
    execution_id: &str,
    report: &LoadedReport,
    json: bool,
) -> anyhow::Result<String> {
    let phases_present = report.snapshots.len();
    let Some(selected) = report.snapshots.last() else {
        return Err(anyhow::anyhow!("cannot render empty report"));
    };
    let is_complete = selected.phase_number == i64::from(SnapshotPhase::FundManager.number());

    if json {
        let payload = ReportJson {
            execution_id: execution_id.to_string(),
            state: selected.state.clone(),
            token_usage: selected.token_usage.clone(),
            phase_number: selected.phase_number,
            phases_present,
            is_complete,
            schema_version: THESIS_MEMORY_SCHEMA_VERSION,
            skipped_phases: report.skipped_phases.clone(),
        };
        serde_json::to_string_pretty(&payload).context("failed to serialize ReportJson")
    } else {
        let mut rendered = String::new();
        if !is_complete {
            rendered.push_str(&format!(
                "(incomplete run - {phases_present} of {} phases present)\n",
                SnapshotPhase::FundManager.number()
            ));
        }
        rendered.push_str(&render_final_report(&selected.state));
        Ok(rendered)
    }
}

fn report_lookup_error(
    execution_id: &str,
    report: &LoadedReport,
    exists_any_schema: bool,
) -> anyhow::Error {
    if !report.skipped_phases.is_empty() {
        anyhow::anyhow!(
            "Report exists but all visible phases were unreadable (corrupt rows): {:?}",
            report.skipped_phases
        )
    } else if exists_any_schema {
        anyhow::anyhow!(
            "Report exists but is incompatible with the current binary (schema version mismatch)."
        )
    } else {
        anyhow::anyhow!("No report found for execution ID: {execution_id}")
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use std::fs;

    use super::*;
    use scorpio_core::workflow::{ExecutionSummary, LoadedReportSnapshot};

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct EnvGuard {
        key: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self {
                key,
                value: previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.value {
                unsafe {
                    std::env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn sample_summary(
        execution_id: &str,
        symbol: &str,
        created_at: chrono::DateTime<Utc>,
    ) -> ExecutionSummary {
        ExecutionSummary {
            execution_id: execution_id.to_owned(),
            symbol: Some(symbol.to_owned()),
            created_at,
        }
    }

    #[test]
    fn render_list_output_json_round_trips() {
        let listing = ExecutionListing {
            summaries: vec![sample_summary(
                "exec-1",
                "AAPL",
                Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 0).unwrap(),
            )],
            stale_count: 2,
            invalid_timestamp_count: 1,
        };

        let rendered = render_list_output(&listing, true).expect("json list output");
        let parsed: Value = serde_json::from_str(&rendered).expect("valid json");

        assert_eq!(parsed["stale_count"], 2);
        assert_eq!(parsed["invalid_timestamp_count"], 1);
        assert_eq!(parsed["summaries"][0]["execution_id"], "exec-1");
    }

    #[test]
    fn render_list_output_human_empty_message() {
        let listing = ExecutionListing {
            summaries: Vec::new(),
            stale_count: 0,
            invalid_timestamp_count: 0,
        };

        let rendered = render_list_output(&listing, false).expect("human empty output");

        assert_eq!(rendered, "No executions found.");
    }

    #[test]
    fn render_list_output_human_renders_table() {
        let listing = ExecutionListing {
            summaries: vec![sample_summary(
                "exec-1",
                "AAPL",
                Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 0).unwrap(),
            )],
            stale_count: 0,
            invalid_timestamp_count: 0,
        };

        let rendered = render_list_output(&listing, false).expect("human table output");

        assert!(rendered.contains("Execution ID"));
        assert!(rendered.contains("exec-1"));
        assert!(rendered.contains("AAPL"));
    }

    #[test]
    fn report_lookup_error_distinguishes_not_found_from_schema_mismatch() {
        let empty = LoadedReport {
            snapshots: Vec::new(),
            skipped_phases: Vec::new(),
        };

        assert!(
            report_lookup_error("missing", &empty, false)
                .to_string()
                .contains("No report found")
        );
        assert!(
            report_lookup_error("stale", &empty, true)
                .to_string()
                .contains("incompatible with the current binary")
        );
    }

    #[test]
    fn report_lookup_error_surfaces_corrupt_only_reports() {
        let corrupt_only = LoadedReport {
            snapshots: Vec::new(),
            skipped_phases: vec![2, 4],
        };

        assert!(
            report_lookup_error("corrupt", &corrupt_only, true)
                .to_string()
                .contains("all visible phases were unreadable")
        );
    }

    #[test]
    fn render_show_output_json_includes_skipped_phases() {
        let state = TradingState::new("AAPL", "2026-01-15");
        let report = LoadedReport {
            snapshots: vec![LoadedReportSnapshot {
                state: state.clone(),
                token_usage: None,
                phase_number: 5,
            }],
            skipped_phases: vec![2, 3],
        };

        let rendered = render_show_output("exec-1", &report, true).expect("json output");
        let parsed: ReportJson = serde_json::from_str(&rendered).expect("report json");

        assert_eq!(parsed.execution_id, "exec-1");
        assert_eq!(parsed.phase_number, 5);
        assert_eq!(parsed.skipped_phases, vec![2, 3]);
        assert_eq!(parsed.state.asset_symbol, state.asset_symbol);
    }

    #[test]
    fn render_show_output_human_includes_incomplete_banner() {
        let state = TradingState::new("AAPL", "2026-01-15");
        let report = LoadedReport {
            snapshots: vec![LoadedReportSnapshot {
                state,
                token_usage: None,
                phase_number: 2,
            }],
            skipped_phases: Vec::new(),
        };

        let rendered = render_show_output("exec-1", &report, false).expect("human output");

        assert!(rendered.starts_with("(incomplete run - 1 of 5 phases present)\n"));
        assert!(rendered.contains("AAPL"));
    }

    #[test]
    fn render_show_output_rejects_empty_reports() {
        let report = LoadedReport {
            snapshots: Vec::new(),
            skipped_phases: Vec::new(),
        };

        let err = render_show_output("exec-1", &report, false).expect_err("empty report");

        assert!(err.to_string().contains("cannot render empty report"));
    }

    #[tokio::test]
    async fn run_list_store_loading_errors_on_malformed_user_config() {
        let _lock = ENV_LOCK.lock().await;
        let home = tempfile::tempdir().expect("temp home");
        let config_dir = home.path().join(".scorpio-analyst");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(config_dir.join("config.toml"), "not = [valid toml").expect("write malformed");

        let _home = EnvGuard::set("HOME", home.path());
        let _path = EnvGuard::remove("SCORPIO__STORAGE__SNAPSHOT_DB_PATH");

        let err = run_list(false)
            .await
            .expect_err("malformed config should fail");

        assert!(err.to_string().contains("failed to parse config file"));
    }
}
