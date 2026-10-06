//! AGENCY-048: an effect attribution keeps the causal chain, not just a KPI.
//!
//! A bare "KPI moved +3%" cannot be audited: it names no mechanism, no
//! confounder it was adjusted for and no rival explanation. Each of those is
//! required here, and an explicitly empty confounder or rival list is a
//! declaration ("none considered") that a reviewer can challenge, which an
//! omitted one is not.

use crate::affordance::Method;
use crate::{non_empty, required, text};
use qip_core::{Decimal, Error};

/// How the effect was told apart from what would have happened anyway.
///
/// AGENCY-056 reads this: predictive accuracy and correlation are both
/// `NotIdentified`, however good the fit, and nothing widens on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identification {
    /// A randomised or staged experiment assigned the treatment.
    Experiment,
    /// An observational estimate with a named identifying strategy.
    Observational { strategy: String },
    /// A correlation, a forecast that came true, or anything else with no
    /// identifying strategy.
    NotIdentified,
}

impl Identification {
    pub fn is_identified(&self) -> bool {
        !matches!(self, Self::NotIdentified)
    }
}

#[derive(Debug, Clone, Default)]
pub struct EffectAttributionDraft {
    /// The class of action whose effect this is.
    pub action_class: Option<Method>,
    pub identification: Option<Identification>,
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
    pub action_class: Method,
    pub identification: Identification,
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
        let action_class = required("action_class", self.action_class)?;
        let identification = required("identification", self.identification)?;
        if let Identification::Observational { strategy } = &identification {
            text("identification strategy", Some(strategy.clone()))?;
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
            action_class,
            identification,
            kpi_change,
            causal_chain,
            confounders,
            competing_explanations: competing,
            confidence,
        })
    }
}
