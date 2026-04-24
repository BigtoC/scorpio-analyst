//! Resolves [`ValuatorId`] to the concrete [`Valuator`] the pipeline
//! should use. Composition (DCF + multiples + …) stays hidden behind a
//! single manifest-selected strategy id per [`AssetShape`], per the plan's
//! Decision point on Phase 5 selection.
use std::collections::HashMap;
use std::sync::Arc;

use super::{EquityDefaultValuator, Valuator, ValuatorId};

/// Central catalog of valuation strategies the pipeline knows about.
#[derive(Clone, Default)]
pub struct ValuatorRegistry {
    inner: HashMap<ValuatorId, Arc<dyn Valuator>>,
}

impl ValuatorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a strategy. Replaces any previous entry with the same id.
    pub fn register(&mut self, valuator: Arc<dyn Valuator>) {
        self.inner.insert(valuator.id(), valuator);
    }

    /// Look up a strategy by id.
    #[must_use]
    pub fn get(&self, id: ValuatorId) -> Option<&Arc<dyn Valuator>> {
        self.inner.get(&id)
    }

    /// Equity-baseline registry — the one strategy we ship today plus
    /// placeholders so packs that select a crypto id won't fail the
    /// lookup before the crypto implementation lands.
    #[must_use]
    pub fn equity_baseline() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(EquityDefaultValuator));
        reg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equity_baseline_registers_equity_default() {
        let reg = ValuatorRegistry::equity_baseline();
        let v = reg.get(ValuatorId::EquityDefault).expect("registered");
        assert_eq!(v.id(), ValuatorId::EquityDefault);
    }

    #[test]
    fn unknown_id_returns_none() {
        let reg = ValuatorRegistry::equity_baseline();
        assert!(reg.get(ValuatorId::CryptoTokenomics).is_none());
    }
}
