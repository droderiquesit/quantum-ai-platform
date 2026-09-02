//! What a model is good at, and where.
//!
//! A model does not have an accuracy. It has an accuracy *in a context*, and
//! the contexts are not interchangeable: a rates model that has been right for
//! two years in a quiet tape has said nothing about what it will do in a
//! crisis, and a single global hit rate reports exactly that as competence. The
//! reputation here is therefore keyed on the five things that actually change
//! the answer — asset class, region, market regime, volatility regime and
//! horizon — and a model's record in one cell of that space is not evidence
//! about any other cell.
//!
//! Two things are composed rather than rebuilt.
//!
//! * Governance is [`qip_ai::ModelCard::decision_eligibility`]. A model that
//!   has been retired, has drifted past its threshold or has never been
//!   evaluated is not ranked here at all, however good its record looks. This
//!   crate does not get a second opinion on that question.
//! * Shrinkage is [`qip_contracts::signal::Conviction`], the same arithmetic
//!   the rest of the platform sizes on. A record of two correct calls out of
//!   two is a hit rate of one and a conviction of barely more than a coin flip,
//!   and an empty record reads as exactly a coin flip. That is the property
//!   that matters most here: a model with no observations in a regime must not
//!   read as good in it, and the safest-looking way to get that wrong is to
//!   default an unseen cell to the model's global average.

use crate::context::Conditions;
use qip_ai::registry::{ModelCard, ModelRegistry};
use qip_contracts::signal::Conviction;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A model's record in one cell of the context space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Decisions the model got right here.
    pub correct: u32,
    /// Decisions it made here at all.
    pub observations: u32,
}

impl Record {
    /// The raw hit rate, before shrinkage. A statistic, and on its own a
    /// misleading one — see [`Record::competence`].
    pub fn hit_rate_f64(&self) -> f64 {
        if self.observations == 0 {
            return 0.0;
        }
        f64::from(self.correct) / f64::from(self.observations)
    }

    /// The hit rate with the sample size attached, so a caller cannot read one
    /// without the other.
    pub fn competence(&self) -> Conviction {
        Conviction::new(self.hit_rate_f64(), self.observations)
    }
}

/// One model's record, by context.
///
/// Deliberately not serialisable, like [`qip_ai::ModelRegistry`] itself. It is
/// keyed on a struct, and the formats this platform serialises to want string
/// keys — a derive here would compile and then fail at the first attempt to
/// write it out. Reputation is rebuilt from the outcome record, which is the
/// durable thing.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelReputation {
    model: String,
    records: BTreeMap<Conditions, Record>,
}

impl ModelReputation {
    /// `model` is a [`qip_ai::ModelCard::reference`] — `name@version`. Keyed on
    /// the version rather than the name because a retrained model is a
    /// different model, and inheriting the old version's record is how a
    /// regression ships with a reputation it did not earn.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            records: BTreeMap::new(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Record one outcome in one context.
    pub fn observe(&mut self, conditions: Conditions, correct: bool) {
        let record = self.records.entry(conditions).or_default();
        record.observations = record.observations.saturating_add(1);
        if correct {
            record.correct = record.correct.saturating_add(1);
        }
    }

    pub fn record(&self, conditions: &Conditions) -> Record {
        self.records.get(conditions).copied().unwrap_or_default()
    }

    /// What this model has earned the right to be believed about here.
    ///
    /// An unseen context returns a coin flip, not the model's average
    /// elsewhere. Borrowing a record across contexts is the failure this whole
    /// module exists to avoid.
    pub fn competence(&self, conditions: &Conditions) -> Conviction {
        self.record(conditions).competence()
    }

    /// Contexts this model has ever been tried in, in a stable order.
    pub fn contexts(&self) -> impl Iterator<Item = (&Conditions, &Record)> {
        self.records.iter()
    }
}

/// A model, and how much it has earned the right to be believed here.
#[derive(Clone, Debug, PartialEq)]
pub struct Rated<'a> {
    pub card: &'a ModelCard,
    pub competence: Conviction,
}

impl Rated<'_> {
    /// The shrunk figure, which is the only one a caller should compare.
    pub fn shrunk(&self) -> f64 {
        self.competence.shrunk()
    }
}

/// Every model's contextual record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReputationBook {
    models: BTreeMap<String, ModelReputation>,
}

impl ReputationBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Record one outcome for one model in one context.
    pub fn observe(&mut self, model: &str, conditions: Conditions, correct: bool) {
        self.models
            .entry(model.to_string())
            .or_insert_with(|| ModelReputation::new(model))
            .observe(conditions, correct);
    }

    pub fn reputation(&self, model: &str) -> Option<&ModelReputation> {
        self.models.get(model)
    }

    /// What a model has earned here. A model the book has never heard of reads
    /// as a coin flip, which is the same answer as a model with no observations
    /// here — correctly, because they are the same claim.
    pub fn competence(&self, model: &str, conditions: &Conditions) -> Conviction {
        match self.models.get(model) {
            Some(reputation) => reputation.competence(conditions),
            None => Conviction::new(0.0, 0),
        }
    }

    /// Every model that may drive a decision at `now`, best here first.
    ///
    /// Eligibility is [`qip_ai::ModelCard::decision_eligibility`] and nothing
    /// else: a retired or drifted model is absent from this list however strong
    /// its record, because the registry has already decided that question and a
    /// second opinion here would be a way around it.
    ///
    /// The order is total and deterministic — shrunk competence descending,
    /// then the model reference ascending. Two models with identical records
    /// must rank the same way on every run, or the routing decision stops being
    /// reproducible for a reason that has nothing to do with the decision.
    pub fn rank<'a>(
        &self,
        registry: &'a ModelRegistry,
        conditions: &Conditions,
        now: Timestamp,
    ) -> Vec<Rated<'a>> {
        let mut rated: Vec<Rated<'a>> = registry
            .iter()
            .filter(|card| card.decision_eligibility(now).is_ok())
            .map(|card| Rated {
                card,
                competence: self.competence(&card.reference(), conditions),
            })
            .collect();
        rated.sort_by(|left, right| {
            right
                .shrunk()
                .total_cmp(&left.shrunk())
                .then_with(|| left.card.reference().cmp(&right.card.reference()))
        });
        rated
    }

    /// The model to use here, or the reason there is none.
    ///
    /// `bar` is compared against the *shrunk* figure, so a model cannot clear
    /// it on a handful of lucky calls. A refusal names the best candidate and
    /// what it actually has, because "no model is good enough here" and "no
    /// model has been tried here" are different problems with different fixes
    /// and they look identical from the outside.
    pub fn select<'a>(
        &self,
        registry: &'a ModelRegistry,
        conditions: &Conditions,
        bar: f64,
        now: Timestamp,
    ) -> Result<Rated<'a>> {
        let ranked = self.rank(registry, conditions, now);
        let Some(best) = ranked.into_iter().next() else {
            return Err(Error::not_found(format!(
                "no model is eligible to decide under {}",
                conditions.label()
            )));
        };
        if !best.competence.clears(bar) {
            return Err(Error::denied(format!(
                "the best model under {} is {}, which is {} correct over {} observations there and reads as {} after shrinkage, below the {bar} bar",
                conditions.label(),
                best.card.reference(),
                best.competence.probability(),
                best.competence.observations(),
                best.shrunk()
            )));
        }
        Ok(best)
    }
}
