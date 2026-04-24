//! Default equity valuator — thin wrapper over
//! [`crate::state::derive_valuation`].
use crate::{
    state::{AssetShape, derive_valuation},
    valuation::{ValuationInputs, ValuationReport, Valuator, ValuatorId},
};

/// The equity-default valuator composes DCF, EV/EBITDA, forward P/E, and
/// PEG via the existing [`derive_valuation`] implementation. This wrapper
/// exists so `AnalysisPackManifest::valuator_selection` can map
/// `AssetShape::CorporateEquity → ValuatorId::EquityDefault` without
/// refactoring the concrete math.
pub struct EquityDefaultValuator;

impl Valuator for EquityDefaultValuator {
    fn id(&self) -> ValuatorId {
        ValuatorId::EquityDefault
    }

    fn assess(&self, inputs: ValuationInputs<'_>, _shape: &AssetShape) -> ValuationReport {
        // Forward verbatim — the shim exists precisely to preserve
        // byte-identical behaviour for the 16 existing `derive_valuation`
        // tests and every downstream assertion against `DerivedValuation`.
        derive_valuation(
            inputs.profile,
            inputs.cashflow,
            inputs.balance,
            inputs.income,
            inputs.shares,
            inputs.earnings_trend,
            inputs.current_price,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::derive_valuation;

    #[test]
    fn assess_with_no_inputs_matches_derive_valuation_not_assessed_path() {
        // Empty inputs → both should return `NotAssessed` with the same
        // `asset_shape` (`Unknown`) and reason. This is the cheapest
        // byte-identity guard: if the shim ever drifts from the underlying
        // function the assertion fires immediately.
        let inputs = ValuationInputs {
            profile: None,
            cashflow: None,
            balance: None,
            income: None,
            shares: None,
            earnings_trend: None,
            current_price: None,
        };
        let via_trait = EquityDefaultValuator.assess(inputs, &AssetShape::Unknown);
        let via_fn = derive_valuation(None, None, None, None, None, None, None);
        assert_eq!(via_trait, via_fn);
    }

    #[test]
    fn valuator_id_reports_equity_default() {
        assert_eq!(EquityDefaultValuator.id(), ValuatorId::EquityDefault);
    }
}
