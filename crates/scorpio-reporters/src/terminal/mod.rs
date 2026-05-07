mod coverage;
mod final_report;
mod provenance;
mod valuation;

use std::sync::Arc;

use async_trait::async_trait;
use comfy_table::{Cell, Table};
use scorpio_core::state::TradingState;
use scorpio_core::workflow::snapshot::ExecutionSummary;

use crate::{ReportContext, Reporter};

pub struct TerminalReporter;

pub fn render_final_report(state: &TradingState) -> String {
    final_report::format_final_report(state)
}

/// Render a list of execution summaries as a terminal table.
///
/// Always returns a comfy-table dump (header + zero or more rows). Empty-state
/// messaging is the CLI's responsibility — callers can branch on the input
/// slice before invoking this function.
pub fn render_execution_list(summaries: &[ExecutionSummary]) -> String {
    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("Execution ID"),
        Cell::new("Symbol"),
        Cell::new("Date"),
    ]);

    for summary in summaries {
        table.add_row(vec![
            Cell::new(&summary.execution_id),
            Cell::new(summary.symbol.as_deref().unwrap_or("—")),
            Cell::new(
                summary
                    .created_at
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string(),
            ),
        ]);
    }

    table.to_string()
}

#[async_trait]
impl Reporter for TerminalReporter {
    fn name(&self) -> &'static str {
        "terminal"
    }

    async fn emit(&self, state: Arc<TradingState>, _ctx: Arc<ReportContext>) -> anyhow::Result<()> {
        println!("{}", render_final_report(&state));
        Ok(())
    }
}
