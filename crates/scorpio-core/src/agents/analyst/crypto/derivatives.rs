// TODO: implement in crypto-pack change.

use crate::agents::analyst::traits::{Analyst, AnalystId, DataNeed};

/// Placeholder for the crypto derivatives analyst (funding, OI, basis).
pub struct DerivativesAnalyst;

impl Analyst for DerivativesAnalyst {
    fn id(&self) -> AnalystId {
        AnalystId::Derivatives
    }

    fn required_data(&self) -> Vec<DataNeed> {
        vec![DataNeed::Derivatives]
    }
}
