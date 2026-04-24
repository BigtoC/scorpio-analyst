// TODO: implement in crypto-pack change.

use crate::agents::analyst::traits::{Analyst, AnalystId, DataNeed};

/// Placeholder for the crypto on-chain analyst (flows, holder concentration).
pub struct OnChainAnalyst;

impl Analyst for OnChainAnalyst {
    fn id(&self) -> AnalystId {
        AnalystId::OnChain
    }

    fn required_data(&self) -> Vec<DataNeed> {
        vec![DataNeed::OnChain]
    }
}
