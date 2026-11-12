//! The platform's tick/microstructure model, updated in LEARN from the
//! residuals the cells conclude (blueprint v11.6 §5, R12-MODEL-1198).
//!
//! Before this module a concluded [`MicrostructureDelta`] was held as the
//! latest per venue and cell and read by one thing: the release of a
//! replay-divergence hold, which it could keep. Nothing accumulated what the
//! deltas said. A cell could report that a venue's slippage ran a hundred
//! basis points past its model campaign after campaign, and the platform
//! knew only the latest campaign — and only while a strategy at that venue
//! happened to be held. The learning brain the blueprint names "updates the
//! tick/microstructure models"; nothing here did.
//!
//! [`MicrostructureModel`] is that model's state at the centre: per venue, a
//! fill-probability, a slippage and an impact estimate, each the signed
//! error between what the cells' models said and what the venue did. LEARN
//! folds every newly concluded delta into it, once, and journals the fold
//! **before** the state moves, carrying the residuals it folded — so the
//! model is the deterministic left fold of its own log records, and
//! [`resume`] rebuilds it from the log alone.
//!
//! **The reader, and its limits.** [`MicrostructureModel::misfit`] says a
//! venue's slippage or impact estimate rests on at least
//! [`REPLAY_DIVERGENCE_MIN_SAMPLE`] samples and sits
//! [`TWIN_MISFIT_BIAS_BPS`] or more from zero. The platform reads it beside
//! the twin's own misfit test to charge the venue to the misfit-venue cap
//! (`twin-misfit-venue-gross`): a venue whose execution model the cells
//! measure as wrong by more than the cost of trading there is narrowed the
//! same way a venue the twin misprices is. It only ever narrows: no estimate
//! widens a cap, releases a hold, withdraws a venue or sizes a trade up. Both
//! figures are already in the tree and are reused rather than restated; the
//! fill-probability estimate is folded and measured but decides nothing,
//! because no figure in the tree states how wrong a fill probability may be
//! before a venue is narrowed, and inventing one is not this module's call.

use crate::platform::TWIN_MISFIT_BIAS_BPS;
use crate::replay_divergence::{MAX_DELTA_VENUES, REPLAY_DIVERGENCE_MIN_SAMPLE};
use qip_contracts::microstructure_delta::{MicrostructureDelta, Residual, ResidualKind};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The producer on every fold record, told apart from the promotion, grant,
/// hold and versions records that share [`Topic::ComplianceEvaluated`].
pub const MICROSTRUCTURE_MODEL_ORIGIN: &str = "kernel/microstructure-model";

/// One residual family's running estimate at one venue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    /// Observed minus modelled, in the family's own unit (basis points for
    /// slippage and impact, probability in basis points for fills): an
    /// exponentially weighted average over deltas, each delta contributing
    /// its sample-weighted mean across instruments. Signed: the sign is the
    /// finding.
    pub error: Decimal,
    /// Observations the estimate rests on, summed over every delta folded.
    pub samples: u64,
    /// Deltas folded into it.
    pub deltas: u64,
}

/// One venue's microstructure state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueMicrostructure {
    pub fill_probability: Option<Estimate>,
    pub slippage: Option<Estimate>,
    pub impact: Option<Estimate>,
}

impl VenueMicrostructure {
    fn slot(&mut self, kind: ResidualKind) -> Option<&mut Option<Estimate>> {
        match kind {
            ResidualKind::FillProbability => Some(&mut self.fill_probability),
            ResidualKind::Slippage => Some(&mut self.slippage),
            ResidualKind::Impact => Some(&mut self.impact),
            // Liquidity, lead/lag and venue behaviour are not part of the
            // per-venue execution model this state holds; they stay on the
            // delta, where the divergence-hold release already reads it.
            ResidualKind::Liquidity | ResidualKind::LeadLag | ResidualKind::VenueBehaviour => None,
        }
    }
}

/// The model: per-venue estimates, and per (venue, cell) the window end of
/// the last delta folded, so a delta is folded once however often it is
/// re-admitted and an older one read late is never folded behind a newer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicrostructureModel {
    venues: BTreeMap<String, VenueMicrostructure>,
    folded: BTreeMap<(String, String), Timestamp>,
}

/// One fold as the log carries it: the delta's identity and every residual
/// folded, so the fold is re-derivable from the record alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrostructureFold {
    pub venue: String,
    pub cell: String,
    pub delta_id: String,
    pub model_version: String,
    pub window_end: Timestamp,
    pub residuals: Vec<Residual>,
    pub at: Timestamp,
}

impl EventBody for MicrostructureFold {
    /// A fact that moves which venues the misfit cap charges, so in the
    /// group the log never evicts, beside the grant and hold records: a
    /// fold evicted from the log is a model [`resume`] cannot rebuild.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "microstructure-model:{}:{}:{}",
            self.venue, self.cell, self.delta_id
        ))
    }
}

impl MicrostructureFold {
    pub fn of(delta: &MicrostructureDelta, at: Timestamp) -> Self {
        Self {
            venue: delta.venue.as_str().to_string(),
            cell: delta.cell.clone(),
            delta_id: delta.delta_id.clone(),
            model_version: delta.model_version.clone(),
            window_end: delta.window_end,
            residuals: delta.residuals.clone(),
            at,
        }
    }
}

/// The weight a delta's error moves an estimate by, reusing the twin's own
/// fill-calibration step (`qip_twin::calibration::DEFAULT_LEARNING_RATE`)
/// rather than stating a second: one campaign cannot reprice a venue, and a
/// persistent residual is absorbed within a few.
fn learning_rate() -> Result<Decimal> {
    Decimal::from_f64(qip_twin::calibration::DEFAULT_LEARNING_RATE).ok_or_else(|| {
        Error::numeric("the microstructure model's learning rate has no decimal form")
    })
}

impl MicrostructureModel {
    /// The estimates held for `venue`, if any delta has been folded there.
    pub fn venue(&self, venue: &str) -> Option<&VenueMicrostructure> {
        self.venues.get(venue)
    }

    /// Every venue's estimates, in venue order.
    pub fn venues(&self) -> &BTreeMap<String, VenueMicrostructure> {
        &self.venues
    }

    /// Whether a delta from `cell` at `venue` ending at `window_end` is
    /// newer than the last one folded for that pair.
    pub fn is_unfolded(&self, venue: &str, cell: &str, window_end: Timestamp) -> bool {
        self.folded
            .get(&(venue.to_string(), cell.to_string()))
            .is_none_or(|last| window_end > *last)
    }

    /// The model with `fold` applied, or a refusal; `self` is untouched, so
    /// a caller can journal the fold between computing and committing it.
    ///
    /// Refuses a fold no newer than its pair's last, a new pair past
    /// [`MAX_DELTA_VENUES`], a residual over zero samples, and arithmetic
    /// that overflows — each of which would leave a model the log could not
    /// reproduce or one that grew without bound.
    pub fn folded(&self, fold: &MicrostructureFold) -> Result<Self> {
        let key = (fold.venue.clone(), fold.cell.clone());
        match self.folded.get(&key) {
            Some(last) if *last >= fold.window_end => {
                return Err(Error::invalid(format!(
                    "delta {} for {} from {} ends at {}, not after the last fold for the pair at \
                     {}; folding it would count a window twice",
                    fold.delta_id,
                    fold.venue,
                    fold.cell,
                    fold.window_end.to_rfc3339(),
                    last.to_rfc3339()
                )));
            }
            None if self.folded.len() >= MAX_DELTA_VENUES => {
                return Err(Error::denied(format!(
                    "the microstructure model holds {MAX_DELTA_VENUES} venue-cell pairs; a fold \
                     for {} from {} is refused rather than growing the set",
                    fold.venue, fold.cell
                )));
            }
            _ => {}
        }
        let rate = learning_rate()?;
        let overflow = || {
            Error::numeric(format!(
                "folding delta {} for {} overflowed the decimal range",
                fold.delta_id, fold.venue
            ))
        };
        // Per family, the sample-weighted mean error across instruments.
        let mut families: BTreeMap<ResidualKind, (Decimal, u64)> = BTreeMap::new();
        for residual in &fold.residuals {
            if residual.samples == 0 {
                return Err(Error::invalid(format!(
                    "delta {} carries a {:?} residual over zero samples",
                    fold.delta_id, residual.kind
                )));
            }
            let weight = i64::try_from(residual.samples)
                .map(Decimal::from_int)
                .map_err(|_| overflow())?;
            let entry = families.entry(residual.kind).or_insert((Decimal::ZERO, 0));
            entry.0 = residual
                .error()
                .checked_mul(weight)
                .and_then(|weighted| entry.0.checked_add(weighted))
                .ok_or_else(overflow)?;
            entry.1 = entry.1.saturating_add(residual.samples);
        }
        let mut next = self.clone();
        let venue = next.venues.entry(fold.venue.clone()).or_default();
        for (kind, (weighted, samples)) in families {
            let Some(slot) = venue.slot(kind) else {
                continue;
            };
            let total = i64::try_from(samples)
                .map(Decimal::from_int)
                .map_err(|_| overflow())?;
            let mean = weighted.checked_div(total).ok_or_else(overflow)?;
            *slot = Some(match slot.take() {
                None => Estimate {
                    error: mean,
                    samples,
                    deltas: 1,
                },
                Some(held) => Estimate {
                    error: mean
                        .checked_sub(held.error)
                        .and_then(|step| step.checked_mul(rate))
                        .and_then(|step| held.error.checked_add(step))
                        .ok_or_else(overflow)?,
                    samples: held.samples.saturating_add(samples),
                    deltas: held.deltas.saturating_add(1),
                },
            });
        }
        next.folded.insert(key, fold.window_end);
        Ok(next)
    }

    /// Why the model says `venue`'s execution model is wrong by more than
    /// the cost of trading there, or `None` (see the module doc). Narrowing
    /// only: the platform charges the venue to the misfit-venue cap.
    pub fn misfit(&self, venue: &str) -> Option<String> {
        let state = self.venues.get(venue)?;
        let threshold = Decimal::from_f64(TWIN_MISFIT_BIAS_BPS)?;
        [("slippage", &state.slippage), ("impact", &state.impact)]
            .into_iter()
            .find_map(|(family, estimate)| {
                estimate
                    .as_ref()
                    .filter(|estimate| {
                        estimate.samples >= REPLAY_DIVERGENCE_MIN_SAMPLE as u64
                            && estimate.error.abs() >= threshold
                    })
                    .map(|estimate| {
                        format!(
                            "{venue}'s {family} estimate is {} bps from its model over {} samples \
                             in {} delta(s), at or past {TWIN_MISFIT_BIAS_BPS} bps",
                            estimate.error.round_dp(2),
                            estimate.samples,
                            estimate.deltas
                        )
                    })
            })
    }
}

/// The model rebuilt from the log: every fold record, in log order, folded
/// again. A record the fold refuses stops the resume rather than being
/// skipped — a model rebuilt around a record it could not read is not the
/// model the platform acted on.
pub fn resume(log: &EventLog) -> Result<MicrostructureModel> {
    let mut model = MicrostructureModel::default();
    for event in log.events() {
        if event.topic != MicrostructureFold::TOPIC
            || event.lineage.producer != MICROSTRUCTURE_MODEL_ORIGIN
        {
            continue;
        }
        let fold = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<MicrostructureFold>())?
            .body;
        model = model.folded(&fold)?;
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn fold(
        cell: &str,
        id: &str,
        ends: i64,
        kind: ResidualKind,
        observed: i64,
    ) -> MicrostructureFold {
        MicrostructureFold {
            venue: "XV".to_string(),
            cell: cell.to_string(),
            delta_id: id.to_string(),
            model_version: "m1".to_string(),
            window_end: Timestamp::from_secs(ends),
            residuals: vec![Residual {
                kind,
                instrument: "AAA".to_string(),
                modelled: dec!("5"),
                observed: Decimal::from_int(observed),
                samples: 10,
            }],
            at: Timestamp::from_secs(ends),
        }
    }

    /// Mutation: drop the learning-rate step (assign `mean`) — fails at the
    /// second fold's value.
    #[test]
    fn a_second_delta_moves_the_estimate_a_fifth_of_the_way_toward_it() {
        let model = MicrostructureModel::default()
            .folded(&fold("c", "d1", 10, ResidualKind::Slippage, 105))
            .expect("the fold is admitted");
        let first = model.venue("XV").and_then(|v| v.slippage.clone());
        assert_eq!(first.as_ref().map(|e| e.error), Some(dec!("100")));
        let model = model
            .folded(&fold("c", "d2", 20, ResidualKind::Slippage, 5))
            .expect("the fold is admitted");
        let second = model.venue("XV").and_then(|v| v.slippage.clone());
        assert_eq!(
            second,
            Some(Estimate {
                error: dec!("80"),
                samples: 20,
                deltas: 2
            })
        );
    }

    /// Mutation: `>=` to `>` in the watermark guard — fails at "folded twice".
    #[test]
    fn a_delta_no_newer_than_its_pairs_last_fold_is_refused() {
        let model = MicrostructureModel::default()
            .folded(&fold("c", "d1", 10, ResidualKind::Impact, 105))
            .expect("the fold is admitted");
        assert!(
            model
                .folded(&fold("c", "d1", 10, ResidualKind::Impact, 105))
                .is_err(),
            "the same window folded twice"
        );
        assert!(
            model
                .folded(&fold("c", "d0", 5, ResidualKind::Impact, 5))
                .is_err()
        );
        // Another cell's campaign at the same venue is its own pair.
        assert!(
            model
                .folded(&fold("c2", "d1", 10, ResidualKind::Impact, 5))
                .is_ok()
        );
    }

    /// Mutation: drop the sample floor from `misfit` — fails at "few samples".
    #[test]
    fn misfit_needs_a_full_sample_and_an_error_at_the_twins_threshold() {
        let mut thin = fold("c", "d1", 10, ResidualKind::Slippage, 205);
        thin.residuals[0].samples = 9;
        let model = MicrostructureModel::default()
            .folded(&thin)
            .expect("the fold is admitted");
        assert!(
            model.misfit("XV").is_none(),
            "few samples called a venue wrong"
        );
        let model = MicrostructureModel::default()
            .folded(&fold("c", "d1", 10, ResidualKind::Slippage, 54))
            .expect("the fold is admitted");
        assert!(model.misfit("XV").is_none(), "49 bps called a venue wrong");
        let model = MicrostructureModel::default()
            .folded(&fold("c", "d1", 10, ResidualKind::Impact, 55))
            .expect("the fold is admitted");
        assert!(
            model.misfit("XV").is_some(),
            "50 bps on impact was not misfit"
        );
        // Fill probability decides nothing, however far off.
        let model = MicrostructureModel::default()
            .folded(&fold("c", "d1", 10, ResidualKind::FillProbability, 9_000))
            .expect("the fold is admitted");
        assert!(model.misfit("XV").is_none());
    }
}
