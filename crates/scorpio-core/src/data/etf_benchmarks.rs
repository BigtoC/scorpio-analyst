//! Static ETF → benchmark-index lookup.
//!
//! Used as a fallback when upstream metadata (yfinance `FundInfo` or SEC
//! N-PORT) does not surface a stated benchmark. The values returned are
//! Yahoo Finance index tickers (e.g. `^GSPC`) so that the same OHLCV
//! fetch path used elsewhere can pull benchmark price history without a
//! new provider.
//!
//! # Coverage
//!
//! Only ETFs whose stated benchmark has a clean Yahoo Finance ticker are
//! included. ETFs tracking indices that Yahoo does not publish as a free
//! symbol (MSCI EAFE/EM, Bloomberg Aggregate, CRSP/FTSE proprietary
//! indices, S&P GICS sector sub-indices) are intentionally omitted — they
//! would require a different data source to compute tracking error against
//! their actual benchmark, and a misleading proxy mapping would silently
//! corrupt the metric.
//!
//! When an ETF maps to a *proxy* benchmark (e.g. `VTI` → `^GSPC` instead
//! of its actual CRSP US Total Market index), it is marked in the table
//! comment and acceptable because the proxy index is highly correlated
//! and the resulting tracking error is still directionally meaningful.

/// Resolve the Yahoo Finance benchmark index ticker for an ETF symbol.
///
/// Returns `None` for ETFs not in the lookup. Symbol matching is
/// case-insensitive but the canonical Yahoo benchmark ticker is returned
/// verbatim.
#[must_use]
pub fn resolve(etf_symbol: &str) -> Option<&'static str> {
    match etf_symbol.trim().to_ascii_uppercase().as_str() {
        // ── S&P 500 ──────────────────────────────────────────────────
        "SPY" | "IVV" | "VOO" | "SPLG" => Some("^GSPC"),

        // ── Nasdaq-100 / Composite ───────────────────────────────────
        "QQQ" | "QQQM" => Some("^NDX"),
        "ONEQ" => Some("^IXIC"),

        // ── Dow Jones Industrial Average ─────────────────────────────
        "DIA" => Some("^DJI"),

        // ── Russell 2000 ─────────────────────────────────────────────
        "IWM" | "VTWO" => Some("^RUT"),

        // ── S&P MidCap 400 ───────────────────────────────────────────
        "IJH" | "MDY" => Some("^MID"),

        // ── S&P SmallCap 600 ─────────────────────────────────────────
        "IJR" | "SLY" => Some("^SP600"),

        // ── Semiconductor (PHLX SOX) ─────────────────────────────────
        "SMH" | "SOXX" | "SOXL" | "SOXS" => Some("^SOX"),

        // ── Proxy mappings ───────────────────────────────────────────
        // VTI tracks CRSP US Total Market; ITOT tracks S&P 1500.
        // Neither index is published cleanly on Yahoo, so we use ^GSPC
        // as a highly-correlated proxy. Tracking error against this
        // proxy is directionally meaningful but not vs the stated index.
        "VTI" | "ITOT" => Some("^GSPC"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_sp500_family() {
        assert_eq!(resolve("SPY"), Some("^GSPC"));
        assert_eq!(resolve("IVV"), Some("^GSPC"));
        assert_eq!(resolve("VOO"), Some("^GSPC"));
        assert_eq!(resolve("SPLG"), Some("^GSPC"));
    }

    #[test]
    fn resolves_nasdaq_family() {
        assert_eq!(resolve("QQQ"), Some("^NDX"));
        assert_eq!(resolve("QQQM"), Some("^NDX"));
        assert_eq!(resolve("ONEQ"), Some("^IXIC"));
    }

    #[test]
    fn resolves_dow() {
        assert_eq!(resolve("DIA"), Some("^DJI"));
    }

    #[test]
    fn resolves_russell_2000() {
        assert_eq!(resolve("IWM"), Some("^RUT"));
        assert_eq!(resolve("VTWO"), Some("^RUT"));
    }

    #[test]
    fn resolves_sp_midcap_smallcap() {
        assert_eq!(resolve("IJH"), Some("^MID"));
        assert_eq!(resolve("MDY"), Some("^MID"));
        assert_eq!(resolve("IJR"), Some("^SP600"));
        assert_eq!(resolve("SLY"), Some("^SP600"));
    }

    #[test]
    fn resolves_semiconductor_family() {
        assert_eq!(resolve("SMH"), Some("^SOX"));
        assert_eq!(resolve("SOXX"), Some("^SOX"));
        assert_eq!(resolve("SOXL"), Some("^SOX"));
        assert_eq!(resolve("SOXS"), Some("^SOX"));
    }

    #[test]
    fn resolves_proxy_mappings() {
        assert_eq!(resolve("VTI"), Some("^GSPC"));
        assert_eq!(resolve("ITOT"), Some("^GSPC"));
    }

    #[test]
    fn case_insensitive_and_trim() {
        assert_eq!(resolve("spy"), Some("^GSPC"));
        assert_eq!(resolve("  QQQ  "), Some("^NDX"));
        assert_eq!(resolve("IwM"), Some("^RUT"));
    }

    #[test]
    fn unmapped_returns_none() {
        // International, bond, sector, and exotic ETFs are intentionally
        // omitted until their benchmark has a verified Yahoo ticker.
        assert_eq!(resolve("VEA"), None);
        assert_eq!(resolve("AGG"), None);
        assert_eq!(resolve("XLF"), None);
        assert_eq!(resolve("ARKK"), None);
    }

    #[test]
    fn blank_returns_none() {
        assert_eq!(resolve(""), None);
        assert_eq!(resolve("   "), None);
    }
}
