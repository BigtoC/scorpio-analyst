//! `scorpio report` subcommand handler.

use anyhow::Context;
use serde::{Deserialize, Serialize};

use scorpio_core::config::Config;
use scorpio_core::state::{AgentTokenUsage, TradingState};
use scorpio_core::workflow::snapshot::{SnapshotStore, THESIS_MEMORY_SCHEMA_VERSION};
use scorpio_reporters::terminal::{render_execution_list, render_final_report};

use super::{ReportArgs, ReportSubcommand};

const CONFIG_LOAD_MSG: &str =
    "✗ Failed to load configuration. Run `scorpio setup` if this is a fresh install.";

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
}

/// Dispatch `scorpio report` subcommands.
pub async fn run(args: &ReportArgs) -> anyhow::Result<()> {
    match &args.subcommand {
        ReportSubcommand::List => run_list().await,
        ReportSubcommand::Show { execution_id, json } => run_show(execution_id, *json).await,
    }
}

/// Load only the snapshot DB path from config — report commands don't need API keys.
async fn open_store() -> anyhow::Result<SnapshotStore> {
    let cfg = Config::load().context(CONFIG_LOAD_MSG)?;
    SnapshotStore::from_config(&cfg)
        .await
        .map_err(anyhow::Error::from)
}

/// List all past analysis executions.
async fn run_list() -> anyhow::Result<()> {
    let store = open_store().await?;
    let listing = store.list_executions().await?;

    if listing.summaries.is_empty() {
        println!("No executions found.");
    } else {
        println!("{}", render_execution_list(&listing.summaries));
    }

    if listing.stale_count > 0 {
        eprintln!(
            "Note: {} run(s) are not displayed because they were created with an older schema. \
             Re-run the analysis to produce a new execution under schema version {}.",
            listing.stale_count, THESIS_MEMORY_SCHEMA_VERSION,
        );
    }

    Ok(())
}

/// Show the full report for a specific execution.
async fn run_show(execution_id: &str, json: bool) -> anyhow::Result<()> {
    let store = open_store().await?;
    let report = store.load_full_report(execution_id).await?;

    if report.snapshots.is_empty() {
        println!("No report found for execution ID: {execution_id}");
        if !report.skipped_phases.is_empty() {
            eprintln!(
                "Warning: {} phase(s) were unreadable (corrupt rows): {:?}",
                report.skipped_phases.len(),
                report.skipped_phases,
            );
        }
        return Ok(());
    }

    let phases_present = report.snapshots.len();
    let selected = report.snapshots.last().expect("non-empty vec has a last");
    let is_complete = selected.phase_number == 5;

    if json {
        let payload = ReportJson {
            execution_id: execution_id.to_string(),
            state: selected.state.clone(),
            token_usage: selected.token_usage.clone(),
            phase_number: selected.phase_number,
            phases_present,
            is_complete,
            schema_version: THESIS_MEMORY_SCHEMA_VERSION,
        };
        let out =
            serde_json::to_string_pretty(&payload).context("failed to serialize ReportJson")?;
        println!("{out}");
    } else {
        if !is_complete {
            println!("(incomplete run — {phases_present} of 5 phases present)");
        }
        let rendered = render_final_report(&selected.state);
        println!("{rendered}");
    }

    if !report.skipped_phases.is_empty() {
        eprintln!(
            "Warning: {} phase(s) were unreadable (corrupt rows): {:?}",
            report.skipped_phases.len(),
            report.skipped_phases,
        );
    }

    Ok(())
}
