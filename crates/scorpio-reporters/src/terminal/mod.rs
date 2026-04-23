mod coverage;
mod final_report;
mod provenance;
mod valuation;

use std::sync::Arc;

use async_trait::async_trait;
use scorpio_core::state::TradingState;

use crate::{ReportContext, Reporter};

pub struct TerminalReporter;

pub fn render_final_report(state: &TradingState) -> String {
    final_report::format_final_report(state)
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
