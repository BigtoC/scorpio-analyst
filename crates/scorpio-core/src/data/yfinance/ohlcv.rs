//! Yahoo Finance OHLCV data types, raw fetcher, and `rig` tool plumbing.
//!
//! Provides a typed async interface for fetching historical price bars from
//! Yahoo Finance via the `yfinance-rs` crate.  The crate's internal [`Candle`]
//! type uses `paft_money::Money` for prices; this module defines its own
//! [`Candle`] struct with plain `f64` fields and converts on the boundary.
//!
//! Higher-level price queries (latest close, VIX snapshot) live in the sibling
//! [`super::price`] module, which builds on top of [`YFinanceClient`].

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use yfinance_rs::YfError;
use yfinance_rs::core::conversions::money_to_f64;

use super::client::{YFinanceClient, YFinanceData};
use crate::error::TradingError;

// ─── Our Candle type ─────────────────────────────────────────────────────────

/// A single daily OHLCV bar with plain `f64` prices and an ISO-8601 date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    /// ISO-8601 date string (UTC), e.g. `"2024-01-15"`.
    pub date: String,
    /// Opening price.
    pub open: f64,
    /// Intraday high.
    pub high: f64,
    /// Intraday low.
    pub low: f64,
    /// Closing (or adjusted-close) price.
    pub close: f64,
    /// Volume; `None` when the provider does not supply it.
    pub volume: Option<u64>,
}

impl Candle {
    /// Convert from a `yfinance_rs::Candle` (whose OHLC fields are
    /// `paft_money::Price` as of paft 0.8) into our plain-`f64` representation.
    pub(super) fn from_yf(c: yfinance_rs::Candle) -> Self {
        Self {
            date: c.ts.format("%Y-%m-%d").to_string(),
            // `money_to_f64` accepts `&impl CurrencyValue`; `Price` implements
            // it, so these call sites are unchanged despite the Money→Price move.
            open: money_to_f64(&c.open),
            high: money_to_f64(&c.high),
            low: money_to_f64(&c.low),
            close: money_to_f64(&c.close),
            volume: c.volume,
        }
    }
}

// ─── Helpers (shared with the price module) ───────────────────────────────────

pub(super) fn parse_date(s: &str) -> Result<NaiveDate, TradingError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| TradingError::SchemaViolation {
        message: format!("cannot parse date {s:?}: {e}"),
    })
}

pub(super) fn map_yf_err(err: YfError) -> TradingError {
    match &err {
        YfError::Http(e) if e.is_timeout() => TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "Yahoo Finance request timed out".to_owned(),
        },
        YfError::Http(_) => TradingError::NetworkTimeout {
            elapsed: Duration::ZERO,
            message: "Yahoo Finance HTTP request failed".to_owned(),
        },
        YfError::Json(_) | YfError::MissingData(_) | YfError::Api(_) => {
            TradingError::SchemaViolation {
                message: "Yahoo Finance response could not be parsed".to_owned(),
            }
        }
        YfError::RateLimited { .. } => TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "Yahoo Finance throttled the request".to_owned(),
        },
        YfError::ServerError { .. } => TradingError::NetworkTimeout {
            elapsed: Duration::ZERO,
            message: "Yahoo Finance server error".to_owned(),
        },
        _ => TradingError::NetworkTimeout {
            elapsed: Duration::ZERO,
            message: "Yahoo Finance request failed".to_owned(),
        },
    }
}

// ─── rig::tool::Tool wrapper ──────────────────────────────────────────────────

/// Args for the `get_ohlcv` tool call.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct OhlcvArgs {
    /// The stock ticker symbol, e.g. `"AAPL"`.
    pub symbol: String,
    /// Start date in `YYYY-MM-DD` format (inclusive).
    pub start: String,
    /// End date in `YYYY-MM-DD` format (inclusive).
    pub end: String,
}

/// Shared analysis-scoped OHLCV cache populated by the retrieval tool and consumed by indicator tools.
///
/// The inner value is wrapped in a second `Arc` so that each call to [`load`](OhlcvToolContext::load)
/// returns a cheap pointer-bump clone rather than a full `Vec<Candle>` copy (~220 KB per analyst
/// run).  The outer `Arc<RwLock<…>>` provides shared ownership of the slot across tool instances;
/// the inner `Arc<Vec<Candle>>` avoids allocating a new `Vec` every time an indicator tool reads it.
#[derive(Debug, Clone, Default)]
pub struct OhlcvToolContext {
    candles: Arc<RwLock<Option<Arc<Vec<Candle>>>>>,
}

impl OhlcvToolContext {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Store OHLCV candles in the context.
    ///
    /// Returns an error if candles have already been stored (write-once
    /// semantics). This prevents a manipulated LLM from overwriting the
    /// trusted first fetch with adversarial data by calling `get_ohlcv`
    /// a second time.
    pub async fn store(&self, candles: Vec<Candle>) -> Result<(), TradingError> {
        let mut lock = self.candles.write().await;
        if lock.is_some() {
            return Err(TradingError::SchemaViolation {
                message: "OHLCV data has already been fetched for this analysis; \
                          get_ohlcv may only be called once per analysis cycle"
                    .to_owned(),
            });
        }
        *lock = Some(Arc::new(candles));
        Ok(())
    }

    /// Load the pre-fetched OHLCV candles.
    ///
    /// Returns an `Arc`-wrapped reference to avoid copying the full `Vec` on every
    /// indicator tool call. Callers can dereference via `&*candles` or `candles.as_slice()`
    /// to obtain a `&[Candle]` slice.
    pub async fn load(&self) -> Result<Arc<Vec<Candle>>, TradingError> {
        self.candles
            .read()
            .await
            .clone()
            .ok_or_else(|| TradingError::SchemaViolation {
                message: "OHLCV context is empty; call get_ohlcv before indicator tools".to_owned(),
            })
    }
}

/// `rig` tool: fetch historical daily OHLCV bars for a symbol and date range.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetOhlcv {
    /// The underlying client used to satisfy tool calls.
    #[serde(skip)]
    client: Option<YFinanceClient>,
    #[serde(skip)]
    allowed_symbol: Option<String>,
    #[serde(skip)]
    allowed_start: Option<String>,
    #[serde(skip)]
    allowed_end: Option<String>,
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl GetOhlcv {
    #[must_use]
    pub fn scoped(
        client: YFinanceClient,
        symbol: impl Into<String>,
        start: impl Into<String>,
        end: impl Into<String>,
        context: OhlcvToolContext,
    ) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: Some(symbol.into()),
            allowed_start: Some(start.into()),
            allowed_end: Some(end.into()),
            context: Some(context),
        }
    }

    fn validate_scope(&self, args: &OhlcvArgs) -> Result<(), TradingError> {
        // Symbol comparison is case-insensitive so "aapl" and "AAPL" are equivalent.
        if let Some(symbol) = &self.allowed_symbol
            && !args.symbol.eq_ignore_ascii_case(symbol)
        {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "get_ohlcv tool is scoped to symbol {symbol}, got {}",
                    args.symbol
                ),
            });
        }
        if let Some(start) = &self.allowed_start
            && args.start != *start
        {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "get_ohlcv tool is scoped to start {start}, got {}",
                    args.start
                ),
            });
        }
        if let Some(end) = &self.allowed_end
            && args.end != *end
        {
            return Err(TradingError::SchemaViolation {
                message: format!("get_ohlcv tool is scoped to end {end}, got {}", args.end),
            });
        }

        Ok(())
    }
}

impl Tool for GetOhlcv {
    const NAME: &'static str = "get_ohlcv";

    type Error = TradingError;
    type Args = OhlcvArgs;
    type Output = Vec<Candle>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let mut desc = "Fetch historical daily OHLCV (open/high/low/close/volume) bars for a \
                        stock symbol between start and end dates (YYYY-MM-DD, inclusive) from \
                        Yahoo Finance."
            .to_owned();

        // Surface the scoped constraints so the LLM uses the exact values.
        if let Some(sym) = &self.allowed_symbol {
            desc.push_str(&format!(" The symbol MUST be exactly \"{sym}\"."));
        }
        if let Some(start) = &self.allowed_start {
            desc.push_str(&format!(" The start date MUST be exactly \"{start}\"."));
        }
        if let Some(end) = &self.allowed_end {
            desc.push_str(&format!(" The end date MUST be exactly \"{end}\"."));
        }

        // Build per-property schema, pinning enum when a scope value is set.
        let symbol_schema = match &self.allowed_symbol {
            Some(s) => {
                json!({ "type": "string", "description": format!("Ticker symbol — must be \"{s}\""), "enum": [s] })
            }
            None => {
                json!({ "type": "string", "description": "The stock ticker symbol, e.g. \"AAPL\"" })
            }
        };
        let start_schema = match &self.allowed_start {
            Some(s) => {
                json!({ "type": "string", "description": format!("Start date — must be \"{s}\""), "enum": [s] })
            }
            None => {
                json!({ "type": "string", "description": "Start date in YYYY-MM-DD format (inclusive)" })
            }
        };
        let end_schema = match &self.allowed_end {
            Some(e) => {
                json!({ "type": "string", "description": format!("End date — must be \"{e}\""), "enum": [e] })
            }
            None => {
                json!({ "type": "string", "description": "End date in YYYY-MM-DD format (inclusive)" })
            }
        };

        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: desc,
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": symbol_schema,
                    "start": start_schema,
                    "end": end_schema
                },
                "required": ["symbol", "start", "end"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.validate_scope(&args)?;

        // If the context already contains candles from a previous call, return
        // them directly without hitting the network or calling store again.
        // This makes duplicate `get_ohlcv` calls within the same analysis scope
        // idempotent while preserving write-once semantics on the context itself.
        if let Some(context) = &self.context
            && let Ok(cached) = context.load().await
        {
            return Ok((*cached).clone());
        }

        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("YFinanceClient not set on GetOhlcv tool"))
        })?;
        let candles = client
            .get_ohlcv(&args.symbol, &args.start, &args.end)
            .await?;
        if let Some(context) = &self.context {
            context.store(candles.clone()).await?;
        }
        Ok(candles)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // ── Date validation ───────────────────────────────────────────────────

    #[test]
    fn parse_valid_date_succeeds() {
        assert!(parse_date("2024-01-15").is_ok());
    }

    #[test]
    fn parse_invalid_date_returns_schema_violation() {
        let err = parse_date("not-a-date").unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    // ── Error mapping ─────────────────────────────────────────────────────

    #[test]
    fn rate_limited_maps_to_rate_limit_exceeded() {
        let err = YfError::RateLimited {
            url: "https://finance.yahoo.com".to_owned(),
        };
        let mapped = map_yf_err(err);
        assert!(matches!(mapped, TradingError::NetworkTimeout { .. }));
    }

    #[test]
    fn api_error_maps_to_schema_violation() {
        let err = YfError::Api("bad response".to_owned());
        let mapped = map_yf_err(err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn missing_data_maps_to_schema_violation() {
        let err = YfError::MissingData("no candles".to_owned());
        let mapped = map_yf_err(err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
    }

    // ── Candle conversion ─────────────────────────────────────────────────

    #[test]
    fn candle_from_yf_converts_fields() {
        use chrono::DateTime;
        use paft_money::{Currency, IsoCurrency, Price};
        use rust_decimal::Decimal;

        // paft 0.8 candle OHLC fields are `Price` (full-precision per-unit value).
        fn make_price(val: f64) -> Price {
            let d = Decimal::from_str_exact(&format!("{val:.4}")).unwrap();
            Price::new(d, Currency::Iso(IsoCurrency::USD))
        }

        let ts = DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let raw = yfinance_rs::Candle {
            ts,
            open: make_price(100.0),
            high: make_price(105.0),
            low: make_price(98.0),
            close: make_price(103.0),
            close_unadj: None,
            volume: Some(1_000_000),
            provider: (),
        };

        let candle = Candle::from_yf(raw);
        assert_eq!(candle.date, "2024-01-15");
        assert!((candle.open - 100.0).abs() < 0.01);
        assert!((candle.high - 105.0).abs() < 0.01);
        assert!((candle.low - 98.0).abs() < 0.01);
        assert!((candle.close - 103.0).abs() < 0.01);
        assert_eq!(candle.volume, Some(1_000_000));
    }

    // ── Tool ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_ohlcv_tool_name() {
        let tool = GetOhlcv {
            client: None,
            allowed_symbol: None,
            allowed_start: None,
            allowed_end: None,
            context: None,
        };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_ohlcv");
    }

    #[tokio::test]
    async fn tool_call_without_client_returns_config_error() {
        // Use a fully scoped tool so scope validation passes and we reach the client check.
        let tool = GetOhlcv {
            client: None,
            allowed_symbol: Some("AAPL".to_owned()),
            allowed_start: Some("2024-01-01".to_owned()),
            allowed_end: Some("2024-01-31".to_owned()),
            context: None,
        };
        let result = tool
            .call(OhlcvArgs {
                symbol: "AAPL".to_owned(),
                start: "2024-01-01".to_owned(),
                end: "2024-01-31".to_owned(),
            })
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
    }

    #[tokio::test]
    async fn ohlcv_context_store_write_once_rejects_second_write() {
        let ctx = OhlcvToolContext::new();
        let candles = vec![Candle {
            date: "2024-01-01".to_owned(),
            open: 100.0,
            high: 105.0,
            low: 98.0,
            close: 103.0,
            volume: None,
        }];

        // First store succeeds.
        ctx.store(candles.clone())
            .await
            .expect("first store must succeed");
        // Second store must fail — write-once semantics.
        let result = ctx.store(candles).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[tokio::test]
    async fn symbol_case_insensitive_scope_match() {
        // Scoped to "AAPL" (uppercase) but called with "aapl" (lowercase) should pass.
        let tool = GetOhlcv {
            client: None,
            allowed_symbol: Some("AAPL".to_owned()),
            allowed_start: Some("2024-01-01".to_owned()),
            allowed_end: Some("2024-01-31".to_owned()),
            context: None,
        };
        let result = tool
            .call(OhlcvArgs {
                symbol: "aapl".to_owned(),
                start: "2024-01-01".to_owned(),
                end: "2024-01-31".to_owned(),
            })
            .await;
        // Should reach the "client not set" error, not a scope error.
        assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
    }

    // ── Idempotent reuse ─────────────────────────────────────────────────

    fn sample_candle() -> Candle {
        Candle {
            date: "2024-01-02".to_owned(),
            open: 185.0,
            high: 186.5,
            low: 184.0,
            close: 185.9,
            volume: Some(50_000_000),
        }
    }

    #[tokio::test]
    async fn get_ohlcv_returns_cached_candles_when_context_is_already_populated() {
        let ctx = OhlcvToolContext::new();
        ctx.store(vec![sample_candle()])
            .await
            .expect("pre-populate must succeed");

        let tool = GetOhlcv {
            client: None,
            allowed_symbol: Some("AAPL".to_owned()),
            allowed_start: Some("2024-01-01".to_owned()),
            allowed_end: Some("2024-01-31".to_owned()),
            context: Some(ctx),
        };

        let result = tool
            .call(OhlcvArgs {
                symbol: "AAPL".to_owned(),
                start: "2024-01-01".to_owned(),
                end: "2024-01-31".to_owned(),
            })
            .await;

        assert!(
            result.is_ok(),
            "expected Ok, got: {:?}",
            result.unwrap_err()
        );
        assert_eq!(result.unwrap(), vec![sample_candle()]);
    }

    #[tokio::test]
    async fn get_ohlcv_still_rejects_mismatched_scoped_args_after_context_is_populated() {
        let ctx = OhlcvToolContext::new();
        ctx.store(vec![sample_candle()])
            .await
            .expect("pre-populate must succeed");

        let tool = GetOhlcv {
            client: None,
            allowed_symbol: Some("AAPL".to_owned()),
            allowed_start: Some("2024-01-01".to_owned()),
            allowed_end: Some("2024-01-31".to_owned()),
            context: Some(ctx),
        };

        let result = tool
            .call(OhlcvArgs {
                symbol: "AAPL".to_owned(),
                start: "2024-01-01".to_owned(),
                end: "2024-02-01".to_owned(), // mismatched end date
            })
            .await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), TradingError::SchemaViolation { ref message } if message.contains("2024-01-31")),
            "expected SchemaViolation mentioning the scoped end date"
        );
    }

    // ── Chronological ordering ────────────────────────────────────────────

    #[test]
    fn candles_sort_chronologically() {
        let mut candles = [
            Candle {
                date: "2024-01-03".to_owned(),
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: None,
            },
            Candle {
                date: "2024-01-01".to_owned(),
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: None,
            },
            Candle {
                date: "2024-01-02".to_owned(),
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: None,
            },
        ];
        candles.sort_by(|a, b| a.date.cmp(&b.date));
        assert_eq!(candles[0].date, "2024-01-01");
        assert_eq!(candles[1].date, "2024-01-02");
        assert_eq!(candles[2].date, "2024-01-03");
    }
}
