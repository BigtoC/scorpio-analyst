use serde::{Deserialize, Serialize};

/// Built-in analysis pack identifier.
///
/// First-slice: only built-in packs selected by config/env string.
/// Serde support enables lightweight persistence in snapshot metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PackId {
    /// Balanced institutional strategy — the default equity pack.
    Baseline,
    /// ETF-native pack — premium/discount band + composition + tracking.
    ///
    /// Not user-selectable via [`PackId::from_str`]: ETF routing is
    /// determined automatically at runtime by the pack classifier based on
    /// Profile + fund metadata. The CLI must not let users force it.
    EtfBaseline,
    /// Digital-asset (crypto) pack. Stub manifest in this slice — registered
    /// in the pack registry so crypto-side wiring can validate, but
    /// deliberately excluded from [`PackId::from_str`] until the crypto
    /// implementation lands. Don't select it via CLI / config.
    CryptoDigitalAsset,
}

impl PackId {
    /// Canonical string representation for config/env selection.
    pub fn as_str(self) -> &'static str {
        match self {
            PackId::Baseline => "baseline",
            PackId::EtfBaseline => "etf_baseline",
            PackId::CryptoDigitalAsset => "crypto_digital_asset",
        }
    }
}

impl std::fmt::Display for PackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PackId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Only baseline is user-selectable in this slice. `CryptoDigitalAsset`
        // is intentionally missing from this match so config / env strings
        // cannot pick it up until the crypto pack is wired through end-to-end.
        // `EtfBaseline` is also intentionally missing: ETF routing is
        // automatic — chosen by the runtime classifier based on Profile +
        // fund metadata — so the CLI must not let users force it.
        match s.trim().to_ascii_lowercase().as_str() {
            "baseline" => Ok(PackId::Baseline),
            unknown => Err(format!(
                "unknown analysis pack: \"{unknown}\" (available: baseline)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etf_baseline_has_canonical_str() {
        assert_eq!(PackId::EtfBaseline.as_str(), "etf_baseline");
    }

    #[test]
    fn etf_baseline_not_selectable_via_from_str() {
        // ETF routing is automatic — the CLI must not let users force it.
        let err = "etf_baseline"
            .parse::<PackId>()
            .expect_err("must not parse");
        assert!(err.contains("unknown analysis pack"));
    }

    #[test]
    fn baseline_still_parses() {
        assert_eq!("baseline".parse::<PackId>().unwrap(), PackId::Baseline);
    }
}
