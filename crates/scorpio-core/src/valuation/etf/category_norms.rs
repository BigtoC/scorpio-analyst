//! Premium-band category norms.
//!
//! Phase 1 ships a hardcoded lookup table. Future revisions may load
//! from disk.

use crate::state::PremiumBand;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CategoryBand {
    pub elevated_pct: f64,
    pub extreme_pct: f64,
}

const DEFAULT_BAND: CategoryBand = CategoryBand {
    elevated_pct: 0.10,
    extreme_pct: 0.50,
};

pub(crate) fn band_for_category(category: Option<&str>) -> CategoryBand {
    let Some(category) = category else {
        return DEFAULT_BAND;
    };
    match category.trim().to_ascii_lowercase().as_str() {
        "large blend" | "large growth" | "large value" => CategoryBand {
            elevated_pct: 0.05,
            extreme_pct: 0.20,
        },
        "small blend" | "small growth" | "small value" | "mid-cap blend" => CategoryBand {
            elevated_pct: 0.15,
            extreme_pct: 0.50,
        },
        "diversified emerging mkts" | "foreign large blend" => CategoryBand {
            elevated_pct: 0.25,
            extreme_pct: 1.00,
        },
        "long government" | "intermediate-term bond" | "high yield bond" => CategoryBand {
            elevated_pct: 0.20,
            extreme_pct: 1.00,
        },
        _ => DEFAULT_BAND,
    }
}

pub(crate) fn classify_band(premium_pct: Option<f64>, band: CategoryBand) -> PremiumBand {
    let Some(p) = premium_pct else {
        return PremiumBand::Unknown;
    };
    let mag = p.abs();
    if mag >= band.extreme_pct {
        PremiumBand::Extreme
    } else if mag >= band.elevated_pct {
        PremiumBand::Elevated
    } else {
        PremiumBand::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_lookup_defaults_when_category_unknown() {
        let b = band_for_category(Some("Thematic Bobsled ETF"));
        assert_eq!(b.elevated_pct, DEFAULT_BAND.elevated_pct);
    }

    #[test]
    fn band_lookup_handles_large_blend() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(b.extreme_pct, 0.20);
    }

    #[test]
    fn classify_band_returns_unknown_when_premium_missing() {
        assert_eq!(classify_band(None, DEFAULT_BAND), PremiumBand::Unknown);
    }

    #[test]
    fn classify_band_returns_normal_inside_elevated_threshold() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(0.02), b), PremiumBand::Normal);
    }

    #[test]
    fn classify_band_returns_elevated_above_elevated_threshold() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(0.08), b), PremiumBand::Elevated);
    }

    #[test]
    fn classify_band_returns_extreme_above_extreme_threshold() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(0.25), b), PremiumBand::Extreme);
    }

    #[test]
    fn classify_band_handles_negative_premium_symmetrically() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(-0.25), b), PremiumBand::Extreme);
    }
}
