// TODO: implement in crypto-pack change.

use crate::agents::analyst::traits::{Analyst, AnalystId, DataNeed};

/// Placeholder for the crypto social-signals analyst.
pub struct SocialAnalyst;

impl Analyst for SocialAnalyst {
    fn id(&self) -> AnalystId {
        AnalystId::Social
    }

    fn required_data(&self) -> Vec<DataNeed> {
        vec![DataNeed::Social]
    }
}
