use std::{fs::OpenOptions, path::PathBuf, sync::Arc};

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
        let ts = ctx.finished_at.format("%Y%m%dT%H%M%S%3fZ");
        ctx.output_dir
            .as_ref()
            .expect("JsonReporter requires ReportContext.output_dir to be set")
            .join(format!("{}-{}.json", ctx.symbol, ts))
    }
}

#[async_trait]
impl Reporter for JsonReporter {
    fn name(&self) -> &'static str {
        "json"
    }

    async fn emit(&self, state: Arc<TradingState>, ctx: Arc<ReportContext>) -> anyhow::Result<()> {
        let output_dir = ctx
            .output_dir
            .as_ref()
            .context("json reporter requires an output directory")?;
        let path = Self::filename(&ctx);
        let report = JsonReport {
            schema_version: 1,
            generated_at: ctx.finished_at,
            trading_state: (*state).clone(),
        };
        let body = serde_json::to_string_pretty(&report).context("serialising JsonReport")?;
        tokio::fs::create_dir_all(output_dir)
            .await
            .with_context(|| format!("creating {}", output_dir.display()))?;

        let path_for_write = path.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path_for_write)
                .with_context(|| format!("writing {}", path_for_write.display()))?;
            std::io::Write::write_all(&mut file, body.as_bytes())
                .with_context(|| format!("writing {}", path_for_write.display()))?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("json writer task failed: {e}"))??;

        Ok(())
    }
}
