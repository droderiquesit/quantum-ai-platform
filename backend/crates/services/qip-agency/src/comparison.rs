//! AGENCY-036: candidates are compared on nine declared axes.
//!
//! A comparison that silently omits `downside` or `conduct_risk` ranks the
//! dangerous option first. Records are only buildable with all nine, so
//! nothing partial can reach the ranker: [`crate::plan::select`] takes a
//! built `Comparison` on every proposal. It ranks on `expected_causal_effect`
//! and refuses on `legally_eligible`; the other seven axes are recorded for
//! the reader and weighed by nothing yet.

use crate::required;
use qip_core::{Decimal, Error};

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ComparisonDraft {
    pub expected_causal_effect: Option<Decimal>,
    pub confidence: Option<Decimal>,
    pub cost: Option<Decimal>,
    pub capital_usage: Option<Decimal>,
    pub time_to_effect_secs: Option<u64>,
    pub reversibility: Option<Decimal>,
    pub legally_eligible: Option<bool>,
    pub conduct_risk: Option<Decimal>,
    pub downside: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Comparison {
    pub expected_causal_effect: Decimal,
    pub confidence: Decimal,
    pub cost: Decimal,
    pub capital_usage: Decimal,
    pub time_to_effect_secs: u64,
    pub reversibility: Decimal,
    pub legally_eligible: bool,
    pub conduct_risk: Decimal,
    pub downside: Decimal,
}

impl ComparisonDraft {
    pub fn build(self) -> Result<Comparison, Error> {
        let confidence = required("confidence", self.confidence)?;
        if confidence < Decimal::ZERO || confidence > Decimal::ONE {
            return Err(Error::invalid("`confidence` must lie in [0, 1]"));
        }
        Ok(Comparison {
            expected_causal_effect: required(
                "expected_causal_effect",
                self.expected_causal_effect,
            )?,
            confidence,
            cost: required("cost", self.cost)?,
            capital_usage: required("capital_usage", self.capital_usage)?,
            time_to_effect_secs: required("time_to_effect_secs", self.time_to_effect_secs)?,
            reversibility: required("reversibility", self.reversibility)?,
            legally_eligible: required("legally_eligible", self.legally_eligible)?,
            conduct_risk: required("conduct_risk", self.conduct_risk)?,
            downside: required("downside", self.downside)?,
        })
    }
}
