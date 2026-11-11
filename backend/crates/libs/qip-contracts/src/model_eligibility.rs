//! Whether a model may still drive a decision, as a record.
//!
//! The model registry decides this (`qip_ai`'s `ModelRegistry`): a retired,
//! drifted or overdue model must stop influencing capital. The lifecycle's
//! demotion monitor needs the answer and must not hold the registry, because
//! the registry lives in a crate that holds models and the lifecycle is an
//! asset-brain crate (ADR 0127 decision 5, ADR 0386). So the composition
//! asks the registry and hands the monitor this.
//!
//! It is a record of verdicts taken at one instant. It decides nothing: it
//! cannot be asked about a model nobody recorded and answer "eligible", and
//! it carries the instant so a reader can refuse one taken at another time.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the record holds about one model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelVerdict<'a> {
    /// The registry cleared it for a decision at the record's instant.
    Eligible,
    /// The registry refused it, in the registry's words.
    Refused(&'a str),
    /// Nobody asked the registry about it. Not a clearance.
    Unrecorded,
}

/// The registry's verdicts on the models a set of strategies name, taken at
/// one instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEligibility {
    taken_at: Timestamp,
    /// `None` is eligible; `Some` is the registry's refusal.
    verdicts: BTreeMap<String, Option<String>>,
}

impl ModelEligibility {
    /// An empty record for the instant the registry was asked.
    pub fn taken_at(taken_at: Timestamp) -> Self {
        Self {
            taken_at,
            verdicts: BTreeMap::new(),
        }
    }

    /// Record that the registry cleared `reference`.
    pub fn record_eligible(&mut self, reference: impl Into<String>) -> Result<()> {
        self.record(reference.into(), None)
    }

    /// Record that the registry refused `reference`, and why.
    pub fn record_refused(
        &mut self,
        reference: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<()> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(Error::invalid(
                "a refused model needs the registry's reason; a refusal with no reason cannot \
                 be acted on by whoever is paged for it",
            ));
        }
        self.record(reference.into(), Some(reason))
    }

    fn record(&mut self, reference: String, verdict: Option<String>) -> Result<()> {
        if reference.trim().is_empty() {
            return Err(Error::invalid(
                "a model verdict needs the model's reference",
            ));
        }
        if self.verdicts.contains_key(&reference) {
            return Err(Error::invalid(format!(
                "{reference} already has a verdict in this record; two verdicts on one model \
                 at one instant are two claims, so take a new record instead"
            )));
        }
        self.verdicts.insert(reference, verdict);
        Ok(())
    }

    /// The instant the registry was asked.
    pub fn instant(&self) -> Timestamp {
        self.taken_at
    }

    /// What the record holds about `reference`.
    pub fn verdict(&self, reference: &str) -> ModelVerdict<'_> {
        match self.verdicts.get(reference) {
            Some(None) => ModelVerdict::Eligible,
            Some(Some(reason)) => ModelVerdict::Refused(reason),
            None => ModelVerdict::Unrecorded,
        }
    }
}
