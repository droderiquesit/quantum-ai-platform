//! What each model rung earned per unit of money and per unit of time
//! (R12-FINOPS-10139).
//!
//! The ledger knew what every rung cost and how long it took, and the
//! reputation book knew how often each rung was right; nothing joined them,
//! so the router could rank a rung by accuracy or by price and never by the
//! one figure a budget owner asks for — what a correct call costs. A
//! [`SpendBook`] accumulates the ledgers a process charged, and
//! [`SpendBook::report`] joins it to a [`ReputationBook`] on the rung.
//!
//! **The join key is the rung, and that is a stated limit, not an accident.**
//! The ledger bills a charge to an [`IntelligenceTier`]; the book credits a
//! judgement to a `name@version` reference whose name is that tier's
//! `as_str`. Spend is therefore not split by version, and the report says
//! which versions' records it summed in [`ModelBenefit::references`] rather
//! than dividing one version's cost among them by a ratio nobody measured.
//!
//! **No figure is invented for a rung with nothing to divide by.** A rung
//! with no correct call has no cost per correct call — `None`, not zero and
//! not infinity — and a rung with records but no spend in the books read is
//! reported with zero spend rather than left out, because "right twelve times
//! and billed for none of it" is a finding about the books.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::Decimal;
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};

use crate::ledger::ComputeLedger;
use crate::reputation::ReputationBook;

/// One rung's accumulated charges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spend {
    pub charges: u64,
    pub cost: Decimal,
    pub latency: Duration,
}

/// The cost and latency every rung was billed, across the ledgers absorbed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendBook {
    rungs: BTreeMap<String, Spend>,
    ledgers: u64,
}

/// One rung's benefit per dollar and per second.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelBenefit {
    /// The rung, as [`crate::IntelligenceTier::as_str`] spells it.
    pub model: String,
    pub spend: Spend,
    /// Correct and scored calls, summed over every version and every context
    /// the book holds for the rung.
    pub correct: u64,
    pub observations: u64,
    /// The `name@version` records summed.
    pub references: BTreeSet<String>,
    /// Hit rate in basis points, floored; `None` with no observation.
    pub hit_rate_bp: Option<u32>,
    /// Billed cost over correct calls; `None` with no correct call.
    pub cost_per_correct: Option<Decimal>,
    /// Billed latency over correct calls, floored to the nanosecond; `None`
    /// with no correct call.
    pub latency_per_correct: Option<Duration>,
}

impl SpendBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add every charge `ledger` holds, under the rung it was billed to.
    /// Deterministic code is billed under no model and is left out: there is
    /// no reputation to divide it by.
    pub fn absorb(&mut self, ledger: &ComputeLedger) {
        self.ledgers += 1;
        for charge in ledger.charges() {
            let Some(model) = charge.tier.model_tier() else {
                continue;
            };
            let spend = self.rungs.entry(model.as_str().to_string()).or_default();
            spend.charges += 1;
            spend.cost += charge.cost;
            spend.latency = Duration::from_nanos(
                spend
                    .latency
                    .as_nanos()
                    .saturating_add(charge.latency.as_nanos()),
            );
        }
    }

    pub fn ledgers(&self) -> u64 {
        self.ledgers
    }

    pub fn spend(&self, model: &str) -> Option<&Spend> {
        self.rungs.get(model)
    }

    /// Join the spend to `book` per rung, in rung order.
    pub fn report(&self, book: &ReputationBook) -> Vec<ModelBenefit> {
        let mut joined: BTreeMap<String, ModelBenefit> = self
            .rungs
            .iter()
            .map(|(model, spend)| (model.clone(), ModelBenefit::empty(model, *spend)))
            .collect();
        for reputation in book.models() {
            let reference = reputation.model();
            let rung = reference
                .rsplit_once('@')
                .map_or(reference, |(name, _)| name);
            let entry = joined
                .entry(rung.to_string())
                .or_insert_with(|| ModelBenefit::empty(rung, Spend::default()));
            entry.references.insert(reference.to_string());
            for (_, record) in reputation.contexts() {
                entry.correct += u64::from(record.correct);
                entry.observations += u64::from(record.observations);
            }
        }
        joined
            .into_values()
            .map(ModelBenefit::with_ratios)
            .collect()
    }
}

impl ModelBenefit {
    fn empty(model: &str, spend: Spend) -> Self {
        Self {
            model: model.to_string(),
            spend,
            correct: 0,
            observations: 0,
            references: BTreeSet::new(),
            hit_rate_bp: None,
            cost_per_correct: None,
            latency_per_correct: None,
        }
    }

    fn with_ratios(mut self) -> Self {
        self.hit_rate_bp = (self.observations > 0).then(|| {
            u32::try_from(self.correct.saturating_mul(10_000) / self.observations)
                .unwrap_or(u32::MAX)
        });
        let correct = i64::try_from(self.correct).ok().filter(|c| *c > 0);
        self.cost_per_correct =
            correct.and_then(|c| self.spend.cost.checked_div(Decimal::from_int(c)));
        self.latency_per_correct =
            correct.map(|c| Duration::from_nanos(self.spend.latency.as_nanos() / c));
        self
    }
}
