use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scorpio_core::state::TradingState;
use serde::{Deserialize, Serialize};

use crate::{ReportContext, Reporter};

/// Versioned envelope written to the JSON artifact file.
///
/// `schema_version` starts at `1` and is bumped on backward-incompatible
/// changes, matching the `THESIS_MEMORY_SCHEMA_VERSION` convention.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonReport {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub trading_state: TradingState,
}

/// Writes a pretty-printed JSON artifact to `ctx.output_dir`.
///
/// Filename: `<SYMBOL>-<ISO8601-UTC>.json` (e.g. `AAPL-20260423T142301Z.json`).
/// The output directory is created on demand.
pub struct JsonReporter;

impl JsonReporter {
    fn filename(ctx: &ReportContext) -> PathBuf {
        let ts = ctx.finished_at.format("%Y%m%dT%H%M%SZ");
        ctx.output_dir.join(format!("{}-{}.json", ctx.symbol, ts))
    }
}

#[async_trait]
impl Reporter for JsonReporter {
    fn name(&self) -> &'static str {
        "json"
    }

    async fn emit(&self, state: Arc<TradingState>, ctx: Arc<ReportContext>) -> anyhow::Result<()> {
        let path = Self::filename(&ctx);
        let report = JsonReport {
            schema_version: 1,
            generated_at: ctx.finished_at,
            trading_state: (*state).clone(),
        };
        let body = serde_json::to_string_pretty(&report).context("serialising JsonReport")?;
        tokio::fs::create_dir_all(&ctx.output_dir)
            .await
            .with_context(|| format!("creating {}", ctx.output_dir.display()))?;
        tokio::fs::write(&path, body)
            .await
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}
