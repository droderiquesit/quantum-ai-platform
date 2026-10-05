//! AGENCY-048: an effect attribution keeps the causal chain, not just a KPI.
//!
//! A bare "KPI moved +3%" cannot be audited: it names no mechanism, no
//! confounder it was adjusted for and no rival explanation. Each of those is
//! required here, and an explicitly empty confounder or rival list is a
//! declaration ("none considered") that a reviewer can challenge, which an
//! omitted one is not.

use crate::{non_empty, required, text};
use qip_core::{Decimal, Error};

#[derive(Debug, Clone, Default)]
pub struct EffectAttributionDraft {
    pub kpi_change: Option<Decimal>,
    pub causal_chain: Option<Vec<String>>,
    pub confounders: Option<Vec<String>>,
    pub competing_explanations: Option<Vec<String>>,
    /// Confidence in the estimate, in `[0, 1]`.
    pub confidence: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct EffectAttribution {
    pub kpi_change: Decimal,
    pub causal_chain: Vec<String>,
    pub confounders: Vec<String>,
    pub competing_explanations: Vec<String>,
    pub confidence: Decimal,
}

impl EffectAttributionDraft {
    pub fn build(self) -> Result<EffectAttribution, Error> {
        let confidence = required("confidence", self.confidence)?;
        if confidence < Decimal::ZERO || confidence > Decimal::ONE {
            return Err(Error::invalid("`confidence` must lie in [0, 1]"));
        }
        let kpi_change = required("kpi_change", self.kpi_change)?;
        let causal_chain = non_empty("causal_chain", self.causal_chain)?;
        let confounders = required("confounders", self.confounders)?;
        let competing = required("competing_explanations", self.competing_explanations)?;
        // `text` kept for entries a caller could blank out.
        for entry in confounders.iter().chain(competing.iter()) {
            text("confounder or competing explanation", Some(entry.clone()))?;
        }
        Ok(EffectAttribution {
            kpi_change,
            causal_chain,
            confounders,
            competing_explanations: competing,
            confidence,
        })
    }
}
