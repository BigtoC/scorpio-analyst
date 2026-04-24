//! Runtime registry that maps [`AnalystId`] to the metadata handles used by
//! the graph builder.
//!
//! The registry holds [`Arc<dyn Analyst>`] values that expose each analyst's
//! identity and data needs. Actual task construction still happens in
//! `workflow/pipeline/runtime.rs`, which reads the pack's `required_inputs`,
//! resolves them to [`AnalystId`] values via [`AnalystId::from_required_input`],
//! and spawns the matching `Task` — the registry's job is to be the canonical
//! answer for "does this deployment know about analyst X" and "what does X
//! consume."
use std::collections::HashMap;
use std::sync::Arc;

use super::traits::{Analyst, AnalystId};

/// Central catalog of analysts available to the pipeline.
///
/// Construct via [`AnalystRegistry::equity_baseline`] for the current live
/// equity pack, or build incrementally with [`AnalystRegistry::register`].
#[derive(Default, Clone)]
pub struct AnalystRegistry {
    inner: HashMap<AnalystId, Arc<dyn Analyst>>,
}

impl AnalystRegistry {
    /// Empty registry — callers add analysts via [`Self::register`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an analyst. Replaces any previous entry for the same id.
    pub fn register(&mut self, analyst: Arc<dyn Analyst>) {
        self.inner.insert(analyst.id(), analyst);
    }

    /// Look up a registered analyst by id.
    #[must_use]
    pub fn get(&self, id: AnalystId) -> Option<&Arc<dyn Analyst>> {
        self.inner.get(&id)
    }

    /// True if `id` has been registered.
    #[must_use]
    pub fn contains(&self, id: AnalystId) -> bool {
        self.inner.contains_key(&id)
    }

    /// Number of registered analysts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True iff the registry holds no analysts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Resolve a pack's `required_inputs` list into the ordered set of
    /// [`AnalystId`]s to spawn.
    ///
    /// Order is preserved from the input list so packs control graph
    /// ordering explicitly. Unknown input names and analysts that are not
    /// registered are silently dropped — matching the existing graceful-
    /// degradation behaviour in `workflow/tasks/analyst::input_missing`.
    #[must_use]
    pub fn for_inputs<I, S>(&self, inputs: I) -> Vec<AnalystId>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        inputs
            .into_iter()
            .filter_map(|s| AnalystId::from_required_input(s.as_ref()))
            .filter(|id| self.contains(*id))
            .collect()
    }
}

/// Minimal [`Analyst`] implementer used by the registry entries.
///
/// Holds the canonical id + data needs for each analyst so the registry is
/// self-contained and doesn't require wiring concrete analyst structs (which
/// carry LLM clients and config) into the lookup path.
pub struct AnalystMetadata {
    id: AnalystId,
    required_data: Vec<super::traits::DataNeed>,
}

impl AnalystMetadata {
    #[must_use]
    pub fn new(id: AnalystId, required_data: Vec<super::traits::DataNeed>) -> Self {
        Self { id, required_data }
    }
}

impl Analyst for AnalystMetadata {
    fn id(&self) -> AnalystId {
        self.id
    }

    fn required_data(&self) -> Vec<super::traits::DataNeed> {
        self.required_data.clone()
    }
}

impl AnalystRegistry {
    /// The equity-baseline analyst roster — the four analysts that ship
    /// today. Order matches `workflow/pipeline/runtime::build_graph` so
    /// baseline fan-out stays byte-identical.
    #[must_use]
    pub fn equity_baseline() -> Self {
        use super::traits::DataNeed;
        let mut reg = Self::new();
        reg.register(Arc::new(AnalystMetadata::new(
            AnalystId::Fundamental,
            vec![DataNeed::Fundamentals],
        )));
        reg.register(Arc::new(AnalystMetadata::new(
            AnalystId::Sentiment,
            vec![DataNeed::Sentiment],
        )));
        reg.register(Arc::new(AnalystMetadata::new(
            AnalystId::News,
            vec![DataNeed::News, DataNeed::Macro],
        )));
        reg.register(Arc::new(AnalystMetadata::new(
            AnalystId::Technical,
            vec![DataNeed::PriceHistory],
        )));
        reg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::analyst::traits::DataNeed;

    #[test]
    fn registry_returns_analyst_by_id() {
        let reg = AnalystRegistry::equity_baseline();
        let fund = reg
            .get(AnalystId::Fundamental)
            .expect("fundamental registered");
        assert_eq!(fund.id(), AnalystId::Fundamental);
        assert_eq!(fund.required_data(), vec![DataNeed::Fundamentals]);
    }

    #[test]
    fn for_inputs_maps_strings_to_analysts_preserving_order() {
        let reg = AnalystRegistry::equity_baseline();
        let ids = reg.for_inputs(["news", "technical", "fundamentals"]);
        assert_eq!(
            ids,
            vec![
                AnalystId::News,
                AnalystId::Technical,
                AnalystId::Fundamental
            ],
        );
    }

    #[test]
    fn unknown_id_returns_none() {
        let reg = AnalystRegistry::equity_baseline();
        assert!(reg.get(AnalystId::Tokenomics).is_none());
    }

    #[test]
    fn for_inputs_drops_unknown_strings() {
        let reg = AnalystRegistry::equity_baseline();
        let ids = reg.for_inputs(["fundamentals", "nonsense", "news"]);
        assert_eq!(ids, vec![AnalystId::Fundamental, AnalystId::News]);
    }

    #[test]
    fn for_inputs_skips_analysts_not_registered_even_when_mapped() {
        // Crypto input maps to an AnalystId the baseline registry doesn't hold
        // — it must be filtered out so build_graph doesn't fan out for an
        // analyst it can't actually spawn.
        let reg = AnalystRegistry::equity_baseline();
        let ids = reg.for_inputs(["fundamentals", "tokenomics"]);
        assert_eq!(ids, vec![AnalystId::Fundamental]);
    }

    #[test]
    fn equity_baseline_contains_all_four_canonical_analysts() {
        let reg = AnalystRegistry::equity_baseline();
        assert_eq!(reg.len(), 4);
        assert!(reg.contains(AnalystId::Fundamental));
        assert!(reg.contains(AnalystId::Sentiment));
        assert!(reg.contains(AnalystId::News));
        assert!(reg.contains(AnalystId::Technical));
    }
}
