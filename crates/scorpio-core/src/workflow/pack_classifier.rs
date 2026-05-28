//! Runtime pack classification.
//!
//! Decides which analysis pack a given symbol routes to, based on
//! yfinance Profile + fund metadata. Pure function: inputs in, decision out.
//!
//! The async profile/fund-info fetches happen in the per-run path; this
//! module is intentionally I/O-free so it can be unit-tested with simple
//! literals.

use yfinance_rs::profile::Profile;

use crate::analysis_packs::PackId;
use crate::data::yfinance::etf::{FundInfo, is_supported_etf_kind};

/// Outcome of runtime classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimePackSelection {
    /// Use the baseline pack as the expected, matched route (for corporate
    /// equities and other non-ETF runs). No warning is surfaced.
    BaselineMatched,
    /// Use the baseline pack as a fallback from an ETF-oriented route.
    BaselineFallback { reason: &'static str },
    /// Use the ETF baseline pack.
    EtfBaseline,
}

impl RuntimePackSelection {
    /// The pack id this selection routes to.
    #[must_use]
    pub fn pack_id(&self) -> PackId {
        match self {
            RuntimePackSelection::BaselineMatched
            | RuntimePackSelection::BaselineFallback { .. } => PackId::Baseline,
            RuntimePackSelection::EtfBaseline => PackId::EtfBaseline,
        }
    }

    /// The fallback reason, when the selection is a fallback.
    #[must_use]
    pub fn fallback_reason(&self) -> Option<&'static str> {
        match self {
            RuntimePackSelection::BaselineMatched => None,
            RuntimePackSelection::BaselineFallback { reason } => Some(reason),
            RuntimePackSelection::EtfBaseline => None,
        }
    }
}

/// Classify a runtime pack from a resolved profile + optional fund metadata.
///
/// The contract:
/// - Fund + supported ETF kind → `EtfBaseline`
/// - Fund + non-ETF kind → `BaselineFallback { reason: "unsupported_fund_shape" }`
/// - Company → `BaselineMatched`
/// - `None` profile → `BaselineFallback { reason: "profile_lookup_unavailable" }`
#[must_use]
pub fn classify_runtime_pack(
    profile: Option<&Profile>,
    fund_info: Option<&FundInfo>,
) -> RuntimePackSelection {
    match profile {
        Some(Profile::Fund(_)) => match fund_info.and_then(|info| info.fund_kind.as_deref()) {
            Some(kind) if is_supported_etf_kind(kind) => RuntimePackSelection::EtfBaseline,
            _ => RuntimePackSelection::BaselineFallback {
                reason: "unsupported_fund_shape",
            },
        },
        Some(Profile::Company(_)) => RuntimePackSelection::BaselineMatched,
        // `Profile` is `#[non_exhaustive]` in paft 0.8. An unrecognized profile
        // shape can't be routed to the ETF pack, so fall back to the baseline.
        Some(_) => RuntimePackSelection::BaselineFallback {
            reason: "unsupported_profile_shape",
        },
        None => RuntimePackSelection::BaselineFallback {
            reason: "profile_lookup_unavailable",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::yfinance::etf::FundInfo;

    fn fund_info(kind: Option<&str>) -> FundInfo {
        FundInfo {
            symbol: "SPY".into(),
            category: None,
            fund_family: None,
            expense_ratio: None,
            total_assets: None,
            leverage_factor: None,
            fund_kind: kind.map(str::to_owned),
            stated_benchmark: None,
        }
    }

    fn fund_profile() -> Profile {
        use yfinance_rs::profile::Fund;
        Profile::Fund(Fund {
            name: "SPDR S&P 500 ETF Trust".to_owned(),
            family: Some("State Street Global Advisors".to_owned()),
            kind: Default::default(),
            isin: None,
        })
    }

    fn company_profile() -> Profile {
        use yfinance_rs::profile::Company;
        Profile::Company(Company {
            name: "Apple Inc.".to_owned(),
            sector: None,
            industry: None,
            website: None,
            address: None,
            summary: None,
            isin: None,
        })
    }

    #[test]
    fn no_profile_falls_back_with_lookup_unavailable_reason() {
        let result = classify_runtime_pack(None, None);
        assert_eq!(
            result,
            RuntimePackSelection::BaselineFallback {
                reason: "profile_lookup_unavailable",
            }
        );
    }

    #[test]
    fn no_profile_with_fund_info_still_falls_back() {
        let info = fund_info(Some("etf"));
        let result = classify_runtime_pack(None, Some(&info));
        assert_eq!(
            result,
            RuntimePackSelection::BaselineFallback {
                reason: "profile_lookup_unavailable",
            }
        );
    }

    #[test]
    fn company_profile_routes_to_baseline_matched() {
        let profile = company_profile();
        let result = classify_runtime_pack(Some(&profile), None);
        assert_eq!(result, RuntimePackSelection::BaselineMatched);
        assert_eq!(result.pack_id(), PackId::Baseline);
        assert!(result.fallback_reason().is_none());
    }

    #[test]
    fn fund_with_supported_etf_kind_routes_to_etf_baseline() {
        let profile = fund_profile();
        let info = fund_info(Some("etf"));
        let result = classify_runtime_pack(Some(&profile), Some(&info));
        assert_eq!(result, RuntimePackSelection::EtfBaseline);
        assert_eq!(result.pack_id(), PackId::EtfBaseline);
        assert!(result.fallback_reason().is_none());
    }

    #[test]
    fn fund_with_unsupported_kind_falls_back_to_baseline() {
        let profile = fund_profile();
        let info = fund_info(Some("mutual_fund"));
        let result = classify_runtime_pack(Some(&profile), Some(&info));
        assert_eq!(
            result,
            RuntimePackSelection::BaselineFallback {
                reason: "unsupported_fund_shape",
            }
        );
        assert_eq!(result.pack_id(), PackId::Baseline);
        assert_eq!(result.fallback_reason(), Some("unsupported_fund_shape"));
    }

    #[test]
    fn fund_with_no_fund_info_falls_back_to_baseline() {
        let profile = fund_profile();
        let result = classify_runtime_pack(Some(&profile), None);
        assert_eq!(
            result,
            RuntimePackSelection::BaselineFallback {
                reason: "unsupported_fund_shape",
            }
        );
    }

    #[test]
    fn fund_with_missing_kind_falls_back_to_baseline() {
        let profile = fund_profile();
        let info = fund_info(None);
        let result = classify_runtime_pack(Some(&profile), Some(&info));
        assert_eq!(
            result,
            RuntimePackSelection::BaselineFallback {
                reason: "unsupported_fund_shape",
            }
        );
    }
}
