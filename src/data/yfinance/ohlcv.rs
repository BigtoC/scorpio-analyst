//! Yahoo Finance OHLCV data types, raw fetcher, and `rig` tool plumbing.
//!
//! Provides a typed async interface for fetching historical price bars from
//! Yahoo Finance via the `yfinance-rs` crate.  The crate's internal [`Candle`]
//! type uses `paft_money::Money` for prices; this module defines its own
//! [`Candle`] struct with plain `f64` fields and converts on the boundary.
//!
//! Higher-level price queries (latest close, VIX snapshot) live in the sibling
//! [`super::price`] module, which builds on top of [`YFinanceClient`].

use std::collections::HashMap;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use yfinance_rs::core::conversions::money_to_f64;
use yfinance_rs::{HistoryBuilder, Interval, YfClient, YfError};

use crate::{config::RateLimitConfig, error::TradingError, rate_limit::SharedRateLimiter};

use crate::data::symbol::validate_symbol;

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
    /// Convert from a `yfinance_rs::Candle` (which uses `paft_money::Money`
    /// for prices) into our plain-`f64` representation.
    fn from_yf(c: yfinance_rs::Candle) -> Self {
        Self {
            date: c.ts.format("%Y-%m-%d").to_string(),
            open: money_to_f64(&c.open),
            high: money_to_f64(&c.high),
            low: money_to_f64(&c.low),
            close: money_to_f64(&c.close),
            volume: c.volume,
        }
    }
}

// ─── Client ──────────────────────────────────────────────────────────────────

/// Cache key: normalized (uppercase) symbol + start date + end date.
type OhlcvCacheKey = (String, String, String);

/// Thin async wrapper around `yfinance-rs` for fetching historical OHLCV data.
///
/// Results of [`get_ohlcv`](YFinanceClient::get_ohlcv) are cached in memory by
/// `(symbol, start, end)` so that repeated calls with the same parameters —
/// whether from the LLM's tool loop or from different agents in the same session
/// — return the cached `Vec<Candle>` without hitting the Yahoo Finance API more
/// than once.
#[derive(Clone)]
pub struct YFinanceClient {
    inner: YfClient,
    limiter: SharedRateLimiter,
    /// Shared across all `Clone`s of this client; keyed by the normalized
    /// (uppercase) symbol + ISO-8601 start/end dates.
    pub(super) cache: Arc<RwLock<HashMap<OhlcvCacheKey, Arc<Vec<Candle>>>>>,
}

impl std::fmt::Debug for YFinanceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Avoid blocking inside `fmt` — use `try_read` so that holding a write
        // lock elsewhere at the same time doesn't deadlock the debug path.
        let cache_len = self.cache.try_read().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("YFinanceClient")
            .field("limiter", &self.limiter.label())
            .field("cached_entries", &cache_len)
            .finish()
    }
}

impl YFinanceClient {
    /// Create a new client using a shared provider-scoped rate limiter.
    #[must_use]
    pub fn new(limiter: SharedRateLimiter) -> Self {
        Self {
            inner: YfClient::default(),
            limiter,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new client from `RateLimitConfig`.
    ///
    /// Uses `SharedRateLimiter::yahoo_finance_from_config` so operators can tune or
    /// disable the Yahoo Finance rate limit via config without recompiling. When
    /// `cfg.yahoo_finance_rps == 0` the limiter is disabled (no blocking).
    #[must_use]
    pub fn from_config(cfg: &RateLimitConfig) -> Self {
        let limiter = SharedRateLimiter::yahoo_finance_from_config(cfg)
            .unwrap_or_else(|| SharedRateLimiter::disabled("yahoo_finance"));
        Self::new(limiter)
    }

    /// Fetch daily OHLCV bars for `symbol` between `start` and `end`
    /// (inclusive), both expressed as `"YYYY-MM-DD"` strings.
    ///
    /// # Errors
    ///
    /// - `TradingError::SchemaViolation` if either date cannot be parsed.
    /// - `TradingError::SchemaViolation` if `end` is before `start`.
    /// - `TradingError::NetworkTimeout` on transport failures.
    /// - `TradingError::SchemaViolation` on response parsing failures.
    pub async fn get_ohlcv(
        &self,
        symbol: &str,
        start: &str,
        end: &str,
    ) -> Result<Vec<Candle>, TradingError> {
        let symbol = validate_symbol(symbol)?;
        let start_date = parse_date(start)?;
        let end_date = parse_date(end)?;

        if end_date < start_date {
            return Err(TradingError::SchemaViolation {
                message: format!("invalid date range: end ({end}) is before start ({start})"),
            });
        }

        // --- In-memory cache lookup -------------------------------------------
        // Normalize symbol to uppercase so "aapl" and "AAPL" share the same entry.
        let cache_key: OhlcvCacheKey = (
            symbol.to_ascii_uppercase(),
            start.to_owned(),
            end.to_owned(),
        );
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok((**cached).clone());
            }
        }
        // ----------------------------------------------------------------------

        let start_dt = Utc
            .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or_else(|| {
                TradingError::SchemaViolation {
                    message: format!("invalid start datetime for {start}"),
                }
            })?)
            .single()
            .ok_or_else(|| TradingError::SchemaViolation {
                message: format!("invalid start datetime for {start}"),
            })?;
        let end_dt = Utc
            .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or_else(|| {
                TradingError::SchemaViolation {
                    message: format!("invalid end datetime for {end}"),
                }
            })?)
            .single()
            .ok_or_else(|| TradingError::SchemaViolation {
                message: format!("invalid end datetime for {end}"),
            })?;

        self.limiter.acquire().await;
        let candles = HistoryBuilder::new(&self.inner, symbol)
            .between(start_dt, end_dt)
            .interval(Interval::D1)
            .fetch()
            .await
            .map_err(map_yf_err)?;

        let mut result: Vec<Candle> = candles.into_iter().map(Candle::from_yf).collect();
        // Ensure chronological order (the API usually returns them sorted, but
        // the spec requires it).
        result.sort_by(|a, b| a.date.cmp(&b.date));

        // Store in session cache so subsequent calls with the same key skip the network.
        self.cache
            .write()
            .await
            .insert(cache_key, Arc::new(result.clone()));

        Ok(result)
    }
}

impl Default for YFinanceClient {
    fn default() -> Self {
        Self::from_config(&RateLimitConfig::default())
    }
}

// ─── Helpers (shared with the price module) ───────────────────────────────────

pub(super) fn parse_date(s: &str) -> Result<NaiveDate, TradingError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| TradingError::SchemaViolation {
        message: format!("cannot parse date {s:?}: {e}"),
    })
}

fn map_yf_err(err: YfError) -> TradingError {
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
    /// Create a new OHLCV tool wrapper backed by `client`.
    #[must_use]
    pub fn new(client: YFinanceClient) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: None,
            allowed_start: None,
            allowed_end: None,
            context: None,
        }
    }

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

    #[tokio::test]
    async fn end_before_start_returns_error() {
        let client = YFinanceClient::default();
        let result = client.get_ohlcv("AAPL", "2024-06-01", "2024-01-01").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TradingError::SchemaViolation { ref message } if message.contains("before start"))
        );
    }

    #[tokio::test]
    async fn same_start_and_end_is_valid() {
        // Dates are equal — should not return an "invalid range" error.
        // (The network call itself is expected to fail in CI, so we only
        // check that we do NOT get a SchemaViolation about the date range.)
        let client = YFinanceClient::default();
        let result = client.get_ohlcv("AAPL", "2024-01-15", "2024-01-15").await;
        if let Err(ref e) = result {
            assert!(
                !matches!(e, TradingError::SchemaViolation { message } if message.contains("before start")),
                "should not fail with date-range error, got: {e:?}"
            );
        }
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
        use paft_money::{Currency, IsoCurrency};
        use rust_decimal::Decimal;

        fn make_money(val: f64) -> paft_money::Money {
            let d = Decimal::from_str_exact(&format!("{val:.4}")).unwrap();
            paft_money::Money::new(d, Currency::Iso(IsoCurrency::USD)).unwrap()
        }

        let ts = DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let raw = yfinance_rs::Candle {
            ts,
            open: make_money(100.0),
            high: make_money(105.0),
            low: make_money(98.0),
            close: make_money(103.0),
            close_unadj: None,
            volume: Some(1_000_000),
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

    // ── In-memory client cache ────────────────────────────────────────────

    #[tokio::test]
    async fn get_ohlcv_returns_cached_result_on_second_call_with_same_params() {
        // Pre-populate the cache directly so we don't need a real network call.
        let client = YFinanceClient::default();
        let candles = vec![Candle {
            date: "2024-01-02".to_owned(),
            open: 180.0,
            high: 182.0,
            low: 179.0,
            close: 181.0,
            volume: Some(30_000_000),
        }];
        // Insert directly into the cache to simulate a prior successful fetch.
        client.cache.write().await.insert(
            (
                "AAPL".to_owned(),
                "2024-01-01".to_owned(),
                "2024-01-31".to_owned(),
            ),
            Arc::new(candles.clone()),
        );

        // Both calls with the same params should return the cached data.
        let first = client
            .get_ohlcv("AAPL", "2024-01-01", "2024-01-31")
            .await
            .expect("cache hit must succeed");
        let second = client
            .get_ohlcv("AAPL", "2024-01-01", "2024-01-31")
            .await
            .expect("cache hit must succeed");

        assert_eq!(first, candles);
        assert_eq!(second, candles);
        // Only one entry in the cache (no duplication).
        assert_eq!(client.cache.read().await.len(), 1);
    }

    #[tokio::test]
    async fn get_ohlcv_cache_is_case_insensitive_for_symbol() {
        let client = YFinanceClient::default();
        let candles = vec![Candle {
            date: "2024-03-01".to_owned(),
            open: 170.0,
            high: 172.0,
            low: 169.0,
            close: 171.0,
            volume: None,
        }];
        // Pre-populate with uppercase key (as would happen after a real fetch).
        client.cache.write().await.insert(
            (
                "MSFT".to_owned(),
                "2024-03-01".to_owned(),
                "2024-03-31".to_owned(),
            ),
            Arc::new(candles.clone()),
        );

        // Call with lowercase — should still hit the cache.
        let result = client
            .get_ohlcv("msft", "2024-03-01", "2024-03-31")
            .await
            .expect("case-insensitive cache hit must succeed");
        assert_eq!(result, candles);
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

    // ── from_config constructor ───────────────────────────────────────────

    #[test]
    fn from_config_with_zero_rps_creates_client_without_panic() {
        use crate::config::RateLimitConfig;
        let cfg = RateLimitConfig {
            finnhub_rps: 0,
            fred_rps: 0,
            yahoo_finance_rps: 0,
        };
        // Should construct without panicking (disabled limiter path).
        let _client = YFinanceClient::from_config(&cfg);
    }

    #[test]
    fn from_config_with_nonzero_rps_creates_client_without_panic() {
        use crate::config::RateLimitConfig;
        let cfg = RateLimitConfig {
            finnhub_rps: 0,
            fred_rps: 0,
            yahoo_finance_rps: 5,
        };
        let _client = YFinanceClient::from_config(&cfg);
    }

    #[test]
    fn default_and_from_config_default_produce_same_limiter_label() {
        use crate::config::RateLimitConfig;
        let default_client = YFinanceClient::default();
        let config_client = YFinanceClient::from_config(&RateLimitConfig::default());
        // Both should surface the "yahoo_finance" label in their debug output.
        let default_debug = format!("{default_client:?}");
        let config_debug = format!("{config_client:?}");
        assert!(
            default_debug.contains("yahoo_finance"),
            "default client debug should show yahoo_finance label: {default_debug}"
        );
        assert!(
            config_debug.contains("yahoo_finance"),
            "from_config client debug should show yahoo_finance label: {config_debug}"
        );
    }
}
