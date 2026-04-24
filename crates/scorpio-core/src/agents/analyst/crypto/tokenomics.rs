// TODO: implement in crypto-pack change.

use crate::agents::analyst::traits::{Analyst, AnalystId, DataNeed};

/// Placeholder for the crypto tokenomics analyst (supply, unlocks, treasury).
pub struct TokenomicsAnalyst;

impl Analyst for TokenomicsAnalyst {
    fn id(&self) -> AnalystId {
        AnalystId::Tokenomics
    }

    fn required_data(&self) -> Vec<DataNeed> {
        vec![DataNeed::Tokenomics]
    }
}
