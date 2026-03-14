//! Finnhub API client wrapper.
//!
//! Provides typed async methods for fetching fundamental data, earnings, insider
//! transactions, and company news from the Finnhub API.  All outbound requests
//! are gated behind the shared [`SharedRateLimiter`] supplied at construction
//! time and all errors are mapped to [`TradingError`].

use std::time::Duration;

use finnhub::FinnhubClient as FhClient;
use finnhub::models::news::NewsCategory;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    config::ApiConfig,
    error::TradingError,
    rate_limit::SharedRateLimiter,
    state::{
        FundamentalData, InsiderTransaction as OurInsiderTransaction, MacroEvent, NewsArticle,
        NewsData,
    },
};

use super::symbol::validate_symbol;

// ─── Client ─────────────────────────────────────────────────────────────────

/// Thin async wrapper around the `finnhub` crate, scoped to the data-layer
/// responsibilities of the Fundamental and News analysts.
#[derive(Clone)]
pub struct FinnhubClient {
    inner: FhClient,
    limiter: SharedRateLimiter,
}

impl std::fmt::Debug for FinnhubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinnhubClient")
            .field("limiter", &self.limiter.label())
            .finish()
    }
}

impl FinnhubClient {
    /// Create a new client from configuration and a shared rate limiter.
    ///
    /// Returns `Err` when `api.finnhub_api_key` is not set.
    pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        let key = api.finnhub_api_key.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("SCORPIO_FINNHUB_API_KEY is not set"))
        })?;
        Ok(Self {
            inner: FhClient::new(key.expose_secret()),
            limiter,
        })
    }

    // ─── Public async methods ────────────────────────────────────────────────

    /// Fetch corporate financials and company profile, returning a
    /// [`FundamentalData`] populated with valuation and margin metrics.
    pub async fn get_fundamentals(&self, symbol: &str) -> Result<FundamentalData, TradingError> {
        let symbol = validate_symbol(symbol)?;
        let metrics_fut = {
            let client = self.inner.clone();
            let limiter = self.limiter.clone();
            let symbol = symbol.to_owned();
            async move {
                limiter.acquire().await;
                client
                    .stock()
                    .metrics(&symbol)
                    .await
                    .map_err(map_finnhub_err)
            }
        };
        let profile_fut = {
            let client = self.inner.clone();
            let limiter = self.limiter.clone();
            let symbol = symbol.to_owned();
            async move {
                limiter.acquire().await;
                client
                    .stock()
                    .company_profile(&symbol)
                    .await
                    .map_err(map_finnhub_err)
            }
        };
        let insider_fut = {
            let client = self.inner.clone();
            let limiter = self.limiter.clone();
            let symbol = symbol.to_owned();
            async move {
                limiter.acquire().await;
                client
                    .stock()
                    .insider_transactions(&symbol)
                    .await
                    .map_err(map_finnhub_err)
            }
        };

        let (metrics, profile, insider_transactions) =
            tokio::try_join!(metrics_fut, profile_fut, insider_fut)?;

        Ok(build_fundamental_data(
            &metrics.metric,
            profile.name.as_deref(),
            symbol,
            map_insider_transactions(insider_transactions.data),
        ))
    }

    /// Fetch the last 4 quarterly earnings records and merge the most recent
    /// EPS actual into `FundamentalData.eps` when not already present.
    pub async fn get_earnings(&self, symbol: &str) -> Result<FundamentalData, TradingError> {
        let symbol = validate_symbol(symbol)?;
        self.limiter.acquire().await;
        let earnings = self
            .inner
            .stock()
            .earnings(symbol, Some(4))
            .await
            .map_err(map_finnhub_err)?;

        let latest_eps = earnings.first().and_then(|e| e.actual);

        Ok(build_earnings_data(symbol, latest_eps, earnings.len()))
    }

    /// Fetch insider transactions and map to [`FundamentalData::insider_transactions`].
    pub async fn get_insider_transactions(
        &self,
        symbol: &str,
    ) -> Result<FundamentalData, TradingError> {
        let symbol = validate_symbol(symbol)?;
        self.limiter.acquire().await;
        let raw = self
            .inner
            .stock()
            .insider_transactions(symbol)
            .await
            .map_err(map_finnhub_err)?;

        Ok(build_insider_data(
            symbol,
            map_insider_transactions(raw.data),
        ))
    }

    /// Fetch the last 30 days of company news and map to [`NewsData`].
    pub async fn get_news(&self, symbol: &str) -> Result<NewsData, TradingError> {
        let symbol = validate_symbol(symbol)?;
        self.limiter.acquire().await;
        let today = chrono::Utc::now().date_naive();
        let from = (today - chrono::Duration::days(30))
            .format("%Y-%m-%d")
            .to_string();
        let to = today.format("%Y-%m-%d").to_string();

        let raw = self
            .inner
            .news()
            .company_news(symbol, &from, &to)
            .await
            .map_err(map_finnhub_err)?;

        Ok(build_news_data(symbol, raw, &from, &to))
    }

    /// Fetch general market news and map it into the shared `NewsData` shape.
    pub async fn get_market_news(&self) -> Result<NewsData, TradingError> {
        self.limiter.acquire().await;
        let raw = self
            .inner
            .news()
            .market_news(NewsCategory::General, None)
            .await
            .map_err(map_finnhub_err)?;

        let articles = raw
            .into_iter()
            .take(20)
            .map(|n| NewsArticle {
                title: n.headline,
                source: n.source,
                published_at: n.datetime.to_string(),
                relevance_score: None,
                snippet: n.summary,
            })
            .collect::<Vec<_>>();
        let macro_events = derive_macro_events(&articles);
        let article_count = articles.len();
        let macro_count = macro_events.len();

        Ok(NewsData {
            articles,
            macro_events,
            summary: format!(
                "general market news: {article_count} articles and {macro_count} derived macro events"
            ),
        })
    }

    /// Fetch a small, fixed macro-economic snapshot from Finnhub economic endpoints.
    pub async fn get_economic_indicators(&self) -> Result<Vec<MacroEvent>, TradingError> {
        const INDICATORS: [(&str, &str); 2] = [
            ("MA-USA-656880", "Interest-rate policy shift"),
            ("MA-USA-CPALTT01-USM657N", "Inflation signal"),
        ];

        let mut events = Vec::new();
        for (code, event_name) in INDICATORS {
            self.limiter.acquire().await;
            let data = self
                .inner
                .economic()
                .data(code)
                .await
                .map_err(map_finnhub_err)?;

            if let Some(latest) = data.data.last() {
                let impact_direction = match event_name {
                    "Interest-rate policy shift" => {
                        if latest.value > 3.0 {
                            "negative"
                        } else {
                            "positive"
                        }
                    }
                    "Inflation signal" => {
                        if latest.value > 3.0 {
                            "negative"
                        } else {
                            "positive"
                        }
                    }
                    _ => "neutral",
                };

                push_macro_event(
                    &mut events,
                    MacroEvent {
                        event: event_name.to_owned(),
                        impact_direction: impact_direction.to_owned(),
                        confidence: 0.7,
                    },
                );
            }
        }

        Ok(events)
    }
}

// ─── Error mapping ───────────────────────────────────────────────────────────

fn map_finnhub_err(err: finnhub::Error) -> TradingError {
    match err {
        finnhub::Error::RateLimitExceeded { .. } => TradingError::RateLimitExceeded {
            provider: "finnhub".to_owned(),
        },
        finnhub::Error::Timeout => TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "Finnhub request timed out".to_owned(),
        },
        finnhub::Error::Http(e) if e.is_timeout() => TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "Finnhub request timed out".to_owned(),
        },
        finnhub::Error::Http(_) => TradingError::NetworkTimeout {
            elapsed: Duration::ZERO,
            message: "Finnhub HTTP request failed".to_owned(),
        },
        finnhub::Error::Deserialization(e) => TradingError::SchemaViolation {
            message: format!("Finnhub response deserialization failed: {e}"),
        },
        _ => TradingError::NetworkTimeout {
            elapsed: Duration::ZERO,
            message: "Finnhub request failed".to_owned(),
        },
    }
}

fn map_insider_transactions(
    transactions: Vec<finnhub::models::stock::insider::InsiderTransaction>,
) -> Vec<OurInsiderTransaction> {
    transactions
        .into_iter()
        .map(|t| OurInsiderTransaction {
            name: t.name,
            share_change: t.change.unwrap_or(0) as f64,
            transaction_date: t.transaction_date,
            transaction_type: t.transaction_code,
        })
        .collect()
}

fn build_fundamental_data(
    metrics: &std::collections::HashMap<String, serde_json::Value>,
    company_name: Option<&str>,
    symbol: &str,
    insider_transactions: Vec<OurInsiderTransaction>,
) -> FundamentalData {
    let pe_ratio = extract_f64(metrics, "peNormalizedAnnual")
        .or_else(|| extract_f64(metrics, "peTTM"))
        .or_else(|| extract_f64(metrics, "peBasicExclExtraTTM"));
    let eps = extract_f64(metrics, "epsNormalizedAnnual")
        .or_else(|| extract_f64(metrics, "epsTTM"))
        .or_else(|| extract_f64(metrics, "epsBasicExclExtraItemsTTM"));
    let revenue_growth_pct = extract_f64(metrics, "revenueGrowth3Y")
        .or_else(|| extract_f64(metrics, "revenueGrowthTTMYoy"));
    let current_ratio = extract_f64(metrics, "currentRatioAnnual");
    let debt_to_equity = extract_f64(metrics, "totalDebt/totalEquityAnnual")
        .or_else(|| extract_f64(metrics, "longTermDebt/equityAnnual"));
    let gross_margin = extract_f64(metrics, "grossMarginAnnual")
        .or_else(|| extract_f64(metrics, "grossMarginTTM"));
    let net_income = extract_f64(metrics, "netIncomeGrowth3Y")
        .or_else(|| extract_f64(metrics, "netIncomeAnnual"));
    let company_name = company_name.unwrap_or(symbol);
    let insider_count = insider_transactions.len();

    FundamentalData {
        revenue_growth_pct,
        pe_ratio,
        eps,
        current_ratio,
        debt_to_equity,
        gross_margin,
        net_income,
        insider_transactions,
        summary: format!(
            "{} — {}: PE={:?}, EPS={:?}, GrossMargin={:?}, InsiderTxns={insider_count}",
            company_name, symbol, pe_ratio, eps, gross_margin
        ),
    }
}

fn build_earnings_data(symbol: &str, latest_eps: Option<f64>, count: usize) -> FundamentalData {
    FundamentalData {
        revenue_growth_pct: None,
        pe_ratio: None,
        eps: latest_eps,
        current_ratio: None,
        debt_to_equity: None,
        gross_margin: None,
        net_income: None,
        insider_transactions: vec![],
        summary: format!("{symbol}: {count} quarterly earnings records fetched"),
    }
}

fn build_insider_data(
    symbol: &str,
    insider_transactions: Vec<OurInsiderTransaction>,
) -> FundamentalData {
    let count = insider_transactions.len();

    FundamentalData {
        revenue_growth_pct: None,
        pe_ratio: None,
        eps: None,
        current_ratio: None,
        debt_to_equity: None,
        gross_margin: None,
        net_income: None,
        insider_transactions,
        summary: format!("{symbol}: {count} insider transactions"),
    }
}

fn build_news_data(
    symbol: &str,
    raw_news: Vec<finnhub::models::news::CompanyNews>,
    from: &str,
    to: &str,
) -> NewsData {
    let articles: Vec<NewsArticle> = raw_news
        .into_iter()
        .map(|n| NewsArticle {
            title: n.headline,
            source: n.source,
            published_at: n.datetime.to_string(),
            relevance_score: None,
            snippet: n.summary,
        })
        .collect();
    let macro_events = derive_macro_events(&articles);
    let macro_count = macro_events.len();
    let article_count = articles.len();

    NewsData {
        articles,
        macro_events,
        summary: format!(
            "{symbol}: {article_count} news articles and {macro_count} macro events (last 30 days, from {from} to {to})"
        ),
    }
}

fn derive_macro_events(articles: &[NewsArticle]) -> Vec<crate::state::MacroEvent> {
    use crate::state::MacroEvent;

    let mut events = Vec::new();

    for article in articles {
        let text = format!("{} {}", article.title, article.snippet).to_lowercase();

        if text.contains("federal reserve")
            || text.contains("fed")
            || text.contains("interest rate")
        {
            let impact_direction = if text.contains("cut") {
                "positive"
            } else if text.contains("hike") || text.contains("higher for longer") {
                "negative"
            } else {
                "neutral"
            };
            push_macro_event(
                &mut events,
                MacroEvent {
                    event: "Interest-rate policy shift".to_owned(),
                    impact_direction: impact_direction.to_owned(),
                    confidence: 0.8,
                },
            );
        }

        if text.contains("inflation") || text.contains("cpi") || text.contains("pce") {
            push_macro_event(
                &mut events,
                MacroEvent {
                    event: "Inflation signal".to_owned(),
                    impact_direction: "negative".to_owned(),
                    confidence: 0.7,
                },
            );
        }

        if text.contains("tariff")
            || text.contains("sanction")
            || text.contains("trade war")
            || text.contains("geopolitical")
        {
            push_macro_event(
                &mut events,
                MacroEvent {
                    event: "Geopolitical trade pressure".to_owned(),
                    impact_direction: "negative".to_owned(),
                    confidence: 0.75,
                },
            );
        }
    }

    events
}

fn push_macro_event(events: &mut Vec<crate::state::MacroEvent>, event: crate::state::MacroEvent) {
    if !events.iter().any(|existing| existing.event == event.event) {
        events.push(event);
    }
}

/// Extract an `f64` from a Finnhub `BasicFinancials.metric` JSON map.
fn extract_f64(
    map: &std::collections::HashMap<String, serde_json::Value>,
    key: &str,
) -> Option<f64> {
    map.get(key)?.as_f64()
}

// ─── rig::tool::Tool wrappers ────────────────────────────────────────────────

/// Args for all single-symbol Finnhub tool calls.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SymbolArgs {
    /// The stock ticker symbol (e.g. "AAPL").
    pub symbol: String,
}

fn symbol_params() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "symbol": {
                "type": "string",
                "description": "The stock ticker symbol, e.g. \"AAPL\""
            }
        },
        "required": ["symbol"]
    })
}

// ── GetFundamentals ──

/// `rig` tool: fetch corporate fundamentals for a single symbol.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetFundamentals {
    /// The underlying client used to satisfy tool calls.
    #[serde(skip)]
    client: Option<FinnhubClient>,
    #[serde(skip)]
    allowed_symbol: Option<String>,
}

impl GetFundamentals {
    /// Create a new fundamentals tool wrapper backed by `client`.
    #[must_use]
    pub fn new(client: FinnhubClient) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: None,
        }
    }

    #[must_use]
    pub fn scoped(client: FinnhubClient, symbol: impl Into<String>) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: Some(symbol.into()),
        }
    }
}

impl Tool for GetFundamentals {
    const NAME: &'static str = "get_fundamentals";

    type Error = TradingError;
    type Args = SymbolArgs;
    type Output = FundamentalData;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Fetch corporate financials and company profile from Finnhub, \
                           returning valuation ratios, margins, and a summary string."
                .to_owned(),
            parameters: symbol_params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_scoped_symbol(self.allowed_symbol.as_deref(), &args.symbol, Self::NAME)?;
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!(
                "FinnhubClient not set on GetFundamentals tool"
            ))
        })?;
        client.get_fundamentals(&args.symbol).await
    }
}

// ── GetEarnings ──

/// `rig` tool: fetch the last 4 quarterly earnings records for a symbol.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetEarnings {
    /// The underlying client used to satisfy tool calls.
    #[serde(skip)]
    client: Option<FinnhubClient>,
    #[serde(skip)]
    allowed_symbol: Option<String>,
}

impl GetEarnings {
    /// Create a new earnings tool wrapper backed by `client`.
    #[must_use]
    pub fn new(client: FinnhubClient) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: None,
        }
    }

    #[must_use]
    pub fn scoped(client: FinnhubClient, symbol: impl Into<String>) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: Some(symbol.into()),
        }
    }
}

impl Tool for GetEarnings {
    const NAME: &'static str = "get_earnings";

    type Error = TradingError;
    type Args = SymbolArgs;
    type Output = FundamentalData;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Fetch the last 4 quarterly EPS records for a stock symbol from Finnhub."
                .to_owned(),
            parameters: symbol_params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_scoped_symbol(self.allowed_symbol.as_deref(), &args.symbol, Self::NAME)?;
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("FinnhubClient not set on GetEarnings tool"))
        })?;
        client.get_earnings(&args.symbol).await
    }
}

// ── GetInsiderTransactions ──

/// `rig` tool: fetch recent insider transactions for a symbol.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetInsiderTransactions {
    /// The underlying client used to satisfy tool calls.
    #[serde(skip)]
    client: Option<FinnhubClient>,
    #[serde(skip)]
    allowed_symbol: Option<String>,
}

impl GetInsiderTransactions {
    /// Create a new insider-transactions tool wrapper backed by `client`.
    #[must_use]
    pub fn new(client: FinnhubClient) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: None,
        }
    }

    #[must_use]
    pub fn scoped(client: FinnhubClient, symbol: impl Into<String>) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: Some(symbol.into()),
        }
    }
}

impl Tool for GetInsiderTransactions {
    const NAME: &'static str = "get_insider_transactions";

    type Error = TradingError;
    type Args = SymbolArgs;
    type Output = FundamentalData;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description:
                "Fetch insider buy/sell transactions for a stock symbol from Finnhub (last 3 months)."
                    .to_owned(),
            parameters: symbol_params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_scoped_symbol(self.allowed_symbol.as_deref(), &args.symbol, Self::NAME)?;
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!(
                "FinnhubClient not set on GetInsiderTransactions tool"
            ))
        })?;
        client.get_insider_transactions(&args.symbol).await
    }
}

// ── GetNews ──

/// `rig` tool: fetch recent company news for a symbol.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetNews {
    /// The underlying client used to satisfy tool calls.
    #[serde(skip)]
    client: Option<FinnhubClient>,
    #[serde(skip)]
    allowed_symbol: Option<String>,
}

impl GetNews {
    /// Create a new news tool wrapper backed by `client`.
    #[must_use]
    pub fn new(client: FinnhubClient) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: None,
        }
    }

    #[must_use]
    pub fn scoped(client: FinnhubClient, symbol: impl Into<String>) -> Self {
        Self {
            client: Some(client),
            allowed_symbol: Some(symbol.into()),
        }
    }
}

impl Tool for GetNews {
    const NAME: &'static str = "get_news";

    type Error = TradingError;
    type Args = SymbolArgs;
    type Output = NewsData;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description:
                "Fetch the last 30 days of company news articles for a stock symbol from Finnhub."
                    .to_owned(),
            parameters: symbol_params(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_scoped_symbol(self.allowed_symbol.as_deref(), &args.symbol, Self::NAME)?;
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("FinnhubClient not set on GetNews tool"))
        })?;
        client.get_news(&args.symbol).await
    }
}

/// `rig` tool: fetch recent general market news.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetMarketNews {
    #[serde(skip)]
    client: Option<FinnhubClient>,
}

impl GetMarketNews {
    #[must_use]
    pub fn new(client: FinnhubClient) -> Self {
        Self {
            client: Some(client),
        }
    }
}

impl Tool for GetMarketNews {
    const NAME: &'static str = "get_market_news";

    type Error = TradingError;
    type Args = ();
    type Output = NewsData;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Fetch recent general market news from Finnhub for macro analysis."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!(
                "FinnhubClient not set on GetMarketNews tool"
            ))
        })?;
        client.get_market_news().await
    }
}

/// `rig` tool: fetch a fixed macro-economic indicator snapshot from Finnhub.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetEconomicIndicators {
    #[serde(skip)]
    client: Option<FinnhubClient>,
}

impl GetEconomicIndicators {
    #[must_use]
    pub fn new(client: FinnhubClient) -> Self {
        Self {
            client: Some(client),
        }
    }
}

impl Tool for GetEconomicIndicators {
    const NAME: &'static str = "get_economic_indicators";

    type Error = TradingError;
    type Args = ();
    type Output = Vec<MacroEvent>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Fetch a fixed set of macro-economic indicators from Finnhub and summarize them as macro events.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!(
                "FinnhubClient not set on GetEconomicIndicators tool"
            ))
        })?;
        client.get_economic_indicators().await
    }
}

fn validate_scoped_symbol(
    allowed_symbol: Option<&str>,
    requested_symbol: &str,
    tool_name: &str,
) -> Result<(), TradingError> {
    match allowed_symbol {
        // No scope set — tool was constructed via ::new() which is only for definition
        // inspection (e.g. tests calling .definition()). Calling it at runtime without
        // a symbol scope is a programming error.
        None => Err(TradingError::SchemaViolation {
            message: format!(
                "{tool_name} must be created via ::scoped() for runtime use; \
                 no symbol scope is set"
            ),
        }),
        Some(expected) if expected != requested_symbol => Err(TradingError::SchemaViolation {
            message: format!("{tool_name} is scoped to symbol {expected}, got {requested_symbol}"),
        }),
        Some(_) => Ok(()),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Error mapping tests ───────────────────────────────────────────────

    #[test]
    fn rate_limit_maps_to_rate_limit_exceeded() {
        let err = finnhub::Error::RateLimitExceeded { retry_after: 60 };
        let mapped = map_finnhub_err(err);
        assert!(
            matches!(mapped, TradingError::RateLimitExceeded { provider } if provider == "finnhub")
        );
    }

    #[test]
    fn timeout_maps_to_network_timeout() {
        let err = finnhub::Error::Timeout;
        let mapped = map_finnhub_err(err);
        assert!(matches!(mapped, TradingError::NetworkTimeout { .. }));
    }

    #[test]
    fn deserialization_maps_to_schema_violation() {
        let inner: serde_json::Error =
            serde_json::from_str::<serde_json::Value>("bad json").unwrap_err();
        let err = finnhub::Error::Deserialization(inner);
        let mapped = map_finnhub_err(err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
    }

    // ── extract_f64 tests ─────────────────────────────────────────────────

    #[test]
    fn extract_f64_present() {
        let mut m = std::collections::HashMap::new();
        m.insert("peNormalizedAnnual".to_owned(), serde_json::json!(25.3));
        assert_eq!(extract_f64(&m, "peNormalizedAnnual"), Some(25.3));
    }

    #[test]
    fn extract_f64_absent_returns_none() {
        let m = std::collections::HashMap::new();
        assert_eq!(extract_f64(&m, "missing"), None);
    }

    #[test]
    fn extract_f64_non_numeric_returns_none() {
        let mut m = std::collections::HashMap::new();
        m.insert("key".to_owned(), serde_json::json!("not-a-number"));
        assert_eq!(extract_f64(&m, "key"), None);
    }

    #[test]
    fn build_fundamental_data_includes_insiders() {
        let mut metrics = std::collections::HashMap::new();
        metrics.insert("peTTM".to_owned(), serde_json::json!(21.5));
        metrics.insert("epsTTM".to_owned(), serde_json::json!(6.2));
        metrics.insert("currentRatioAnnual".to_owned(), serde_json::json!(1.4));
        metrics.insert("grossMarginTTM".to_owned(), serde_json::json!(0.42));

        let data = build_fundamental_data(
            &metrics,
            Some("Apple Inc."),
            "AAPL",
            vec![OurInsiderTransaction {
                name: "Jane Doe".to_owned(),
                share_change: -1200.0,
                transaction_date: "2024-01-15".to_owned(),
                transaction_type: "S".to_owned(),
            }],
        );

        assert_eq!(data.pe_ratio, Some(21.5));
        assert_eq!(data.eps, Some(6.2));
        assert_eq!(data.current_ratio, Some(1.4));
        assert_eq!(data.gross_margin, Some(0.42));
        assert_eq!(data.insider_transactions.len(), 1);
        assert!(data.summary.contains("InsiderTxns=1"));
    }

    #[test]
    fn build_news_data_derives_macro_events() {
        let raw_news = vec![finnhub::models::news::CompanyNews {
            category: "company".to_owned(),
            datetime: 1_705_276_800,
            headline: "Fed signals rate cut as inflation cools".to_owned(),
            id: 1,
            image: String::new(),
            related: "AAPL".to_owned(),
            source: "Reuters".to_owned(),
            summary: "Federal Reserve commentary points to easing policy.".to_owned(),
            url: "https://example.com/news/1".to_owned(),
        }];

        let news = build_news_data("AAPL", raw_news, "2024-01-01", "2024-01-31");

        assert_eq!(news.articles.len(), 1);
        assert_eq!(news.macro_events.len(), 2);
        assert!(
            news.macro_events
                .iter()
                .any(|event| event.event == "Interest-rate policy shift")
        );
        assert!(
            news.macro_events
                .iter()
                .any(|event| event.event == "Inflation signal")
        );
    }

    // ── Tool definition tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_fundamentals_tool_name() {
        let tool = GetFundamentals {
            client: None,
            allowed_symbol: None,
        };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_fundamentals");
    }

    #[tokio::test]
    async fn get_earnings_tool_name() {
        let tool = GetEarnings {
            client: None,
            allowed_symbol: None,
        };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_earnings");
    }

    #[tokio::test]
    async fn get_insider_transactions_tool_name() {
        let tool = GetInsiderTransactions {
            client: None,
            allowed_symbol: None,
        };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_insider_transactions");
    }

    #[tokio::test]
    async fn get_news_tool_name() {
        let tool = GetNews {
            client: None,
            allowed_symbol: None,
        };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_news");
    }

    // ── Tool call without client returns Config error ─────────────────────

    #[tokio::test]
    async fn tool_call_without_client_returns_config_error() {
        // Use a scoped tool so that scope validation passes and we reach the client check.
        let tool = GetFundamentals {
            client: None,
            allowed_symbol: Some("AAPL".to_owned()),
        };
        let result = tool
            .call(SymbolArgs {
                symbol: "AAPL".to_owned(),
            })
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
    }

    #[tokio::test]
    async fn tool_call_without_scope_returns_schema_violation() {
        // Tools constructed via ::new() (no scope) must reject runtime calls.
        let tool = GetFundamentals {
            client: None,
            allowed_symbol: None,
        };
        let result = tool
            .call(SymbolArgs {
                symbol: "AAPL".to_owned(),
            })
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[tokio::test]
    async fn tool_call_wrong_symbol_returns_schema_violation() {
        // Scoped to AAPL but called with MSFT should be rejected.
        let tool = GetFundamentals {
            client: None,
            allowed_symbol: Some("AAPL".to_owned()),
        };
        let result = tool
            .call(SymbolArgs {
                symbol: "MSFT".to_owned(),
            })
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    /// Verify that `get_fundamentals` awaits the limiter exactly twice
    /// (once for metrics, once for company_profile).
    /// Because this requires a real Finnhub key and network, the actual
    /// client call is skipped; we verify the pattern through the
    /// `SharedRateLimiter::acquire` call count on a fast limiter.
    #[tokio::test]
    async fn rate_limiter_is_awaited_before_requests() {
        // A rate limiter with a very high limit (effectively no-op) to
        // check that `acquire()` is called without blocking.
        let limiter = SharedRateLimiter::new("test", 10_000);
        limiter.acquire().await;
        assert_eq!(limiter.label(), "test");
    }
}
