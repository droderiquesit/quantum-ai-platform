//! Scoring each catalogued connector by what it adds and what it earned
//! (R12-DATA-2022, R12-DATA-2025, R12-DATA-1105).
//!
//! `qip_data_finder::source_alpha` computes a source's incremental
//! information gain over what the platform already holds and its realised
//! contribution to the outcome that followed, and keeps a bounded history of
//! both. It was tested and called by nothing outside its crate: no binary
//! ever supplied the three series it needs, so every source this node polled
//! was priced on declared coverage alone and never on evidence.
//!
//! This desk supplies them from what the evolution engine already senses.
//! For each subject a connector arm prices, at each sense where both the arm
//! and the engine's own stream printed it:
//!
//! - **signal** — the arm's change in price since the previous sample;
//! - **incumbent** — the own stream's change over the same interval, which is
//!   what the platform held without the arm;
//! - **outcome** — the own stream's change over the *next* interval, which is
//!   what the arm's signal would have had to anticipate.
//!
//! Prices are `Decimal` on the record and cross to `f64` here, at the point
//! they become a statistic's input; nothing scored is money and nothing here
//! settles or sizes anything.
//!
//! Bounded throughout: [`WINDOW`] samples per source and subject, the
//! history's own bound per source. A scoring failure — too few aligned
//! samples, a source whose price never moved — is reported against the
//! source and never stops the node: an unscored source is a finding, and the
//! loop is owed its cycle.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::source_alpha::{MIN_OBSERVATIONS, SourceAlpha, SourceAlphaHistory};
use qip_market_ingestion::adapter::SensedRecord;
use std::collections::{BTreeMap, VecDeque};

/// The variable naming how often, in research cycles, sources are scored.
/// Unset or `0` disables the desk.
pub const SOURCE_ALPHA_EVERY_VARIABLE: &str = "QIP_DEEPBRAIN_SOURCE_ALPHA_EVERY";

/// Aligned samples kept per source and subject.
pub const WINDOW: usize = 128;

/// Samples a score needs: two changes are lost to differencing and to the
/// one-interval lead of the outcome.
pub const MIN_SAMPLES: usize = MIN_OBSERVATIONS + 2;

/// The scoring cadence in cycles; `0` when unset. Refuses anything that is
/// not a cycle count.
pub fn source_alpha_every(lookup: &dyn Fn(&str) -> Option<String>) -> Result<u64> {
    match lookup(SOURCE_ALPHA_EVERY_VARIABLE) {
        None => Ok(0),
        Some(raw) => raw.trim().parse().map_err(|_| {
            qip_core::error::Error::invalid(format!(
                "{SOURCE_ALPHA_EVERY_VARIABLE} is {raw:?}, not a cycle count; 0 disables source \
                 scoring"
            ))
        }),
    }
}

/// The price a record states for its subject, where it states one.
fn priced(record: &SensedRecord) -> Option<(String, f64)> {
    match record {
        SensedRecord::Tick(tick) => {
            Some((tick.object_id.as_str().to_string(), tick.price.to_f64()))
        }
        SensedRecord::Trade(trade) => {
            Some((trade.object_id.as_str().to_string(), trade.price.to_f64()))
        }
        SensedRecord::Bar(bar) => Some((bar.object_id.as_str().to_string(), bar.close.to_f64())),
        _ => None,
    }
    .filter(|(_, price)| price.is_finite())
}

/// One source scored on one subject, or why it could not be.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceScore {
    pub source: String,
    pub subject: String,
    pub samples: usize,
    /// The score, or the refusal's message.
    pub outcome: std::result::Result<SourceAlpha, String>,
}

/// What one scoring pass produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceAlphaRound {
    pub scores: Vec<SourceScore>,
}

impl SourceAlphaRound {
    pub fn describe(&self) -> String {
        let scored = self.scores.iter().filter(|s| s.outcome.is_ok()).count();
        format!(
            "source alpha: {scored} source-subject pair(s) scored, {} unscored",
            self.scores.len() - scored
        )
    }
}

/// The desk. See the module documentation.
#[derive(Clone, Debug, Default)]
pub struct SourceAlphaDesk {
    every_cycles: u64,
    own: BTreeMap<String, f64>,
    arms: BTreeMap<(String, String), f64>,
    samples: BTreeMap<(String, String), VecDeque<(f64, f64)>>,
    history: SourceAlphaHistory,
}

impl SourceAlphaDesk {
    pub fn new(every_cycles: u64) -> Self {
        Self {
            every_cycles,
            ..Self::default()
        }
    }

    pub const fn every_cycles(&self) -> u64 {
        self.every_cycles
    }

    /// The own stream's records this sense.
    pub fn observe_own(&mut self, records: &[SensedRecord]) {
        for (subject, price) in records.iter().filter_map(priced) {
            self.own.insert(subject, price);
        }
    }

    /// One arm's records this sense.
    pub fn observe_source(&mut self, source: &str, records: &[SensedRecord]) {
        for (subject, price) in records.iter().filter_map(priced) {
            self.arms.insert((source.to_string(), subject), price);
        }
    }

    /// Close the sense: every pair the arm and the own stream both priced
    /// becomes one aligned sample. The arm's prices are cleared, so a source
    /// that went quiet contributes no stale sample; the own stream's last
    /// price stands until it prints again.
    pub fn end_sense(&mut self) {
        for ((source, subject), price) in std::mem::take(&mut self.arms) {
            let Some(own) = self.own.get(&subject).copied() else {
                continue;
            };
            let window = self.samples.entry((source, subject)).or_default();
            if window.len() >= WINDOW {
                window.pop_front();
            }
            window.push_back((price, own));
        }
    }

    /// Score every source and subject with enough aligned samples, if this
    /// cycle is on the cadence.
    pub fn maybe_score(&mut self, cycle: u64, now: Timestamp) -> Option<SourceAlphaRound> {
        if self.every_cycles == 0 || !cycle.is_multiple_of(self.every_cycles) {
            return None;
        }
        Some(self.score(now))
    }

    fn score(&mut self, now: Timestamp) -> SourceAlphaRound {
        let mut round = SourceAlphaRound::default();
        for ((source, subject), window) in &self.samples {
            let samples = window.len();
            let outcome = if samples < MIN_SAMPLES {
                Err(format!(
                    "{samples} aligned sample(s); a score needs {MIN_SAMPLES}"
                ))
            } else {
                let points: Vec<(f64, f64)> = window.iter().copied().collect();
                let changes: Vec<(f64, f64)> = points
                    .windows(2)
                    .map(|pair| (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1))
                    .collect();
                let signal: Vec<f64> = changes[..changes.len() - 1]
                    .iter()
                    .map(|change| change.0)
                    .collect();
                let incumbent: Vec<f64> = changes[..changes.len() - 1]
                    .iter()
                    .map(|change| change.1)
                    .collect();
                let outcome: Vec<f64> = changes[1..].iter().map(|change| change.1).collect();
                self.history
                    .score(source, now, &signal, &incumbent, &outcome)
                    .map_err(|error| error.message().to_string())
            };
            round.scores.push(SourceScore {
                source: source.clone(),
                subject: subject.clone(),
                samples,
                outcome,
            });
        }
        round
    }

    /// Every score kept for `source`, oldest first.
    pub fn history(&self, source: &str) -> impl Iterator<Item = &SourceAlpha> {
        self.history.history(source)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_core::{Decimal, ObjectId};
    use qip_market::quote::Tick;

    fn tick(subject: &str, price: f64) -> SensedRecord {
        let at = Timestamp::from_secs(1_760_000_000);
        SensedRecord::Tick(Tick {
            object_id: ObjectId::from_string(subject),
            venue: "TEST".to_string(),
            at,
            capture_time: at,
            sequence: None,
            price: Decimal::from_f64(price).unwrap_or_default(),
            volume: Decimal::ZERO,
            quality: Default::default(),
        })
    }

    /// A deterministic walk, so no test depends on a random generator.
    fn walk(n: usize, seed: u64) -> Vec<f64> {
        let mut state = seed;
        let mut level = 100.0;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                level += ((state >> 33) % 200) as f64 / 100.0 - 1.0;
                level
            })
            .collect()
    }

    fn fed(own: &[f64], sources: &[(&str, Vec<f64>)]) -> SourceAlphaDesk {
        let mut desk = SourceAlphaDesk::new(1);
        for (index, price) in own.iter().enumerate() {
            desk.observe_own(&[tick("S", *price)]);
            for (source, prices) in sources {
                desk.observe_source(source, &[tick("S", prices[index])]);
            }
            desk.end_sense();
        }
        desk
    }

    #[test]
    fn a_duplicate_source_earns_no_gain_and_an_independent_one_earns_some() -> Result<()> {
        let own = walk(40, 1);
        let duplicate: Vec<f64> = own.iter().map(|p| 2.0 * p + 3.0).collect();
        let independent = walk(40, 99);
        let mut desk = fed(
            &own,
            &[("duplicate", duplicate), ("independent", independent)],
        );
        let round = desk
            .maybe_score(1, Timestamp::from_secs(1))
            .ok_or_else(|| qip_core::error::Error::not_found("no round on the cadence"))?;
        assert_eq!(round.scores.len(), 2, "{round:?}");
        let gain = |source: &str| -> Result<f64> {
            round
                .scores
                .iter()
                .find(|score| score.source == source)
                .and_then(|score| score.outcome.as_ref().ok())
                .map(|alpha| alpha.incremental_gain)
                .ok_or_else(|| qip_core::error::Error::not_found(format!("{source} unscored")))
        };
        assert!(gain("duplicate")? < 1e-9, "{round:?}");
        assert!(gain("independent")? > 0.5, "{round:?}");
        assert_eq!(desk.history("duplicate").count(), 1);
        Ok(())
    }

    #[test]
    fn a_source_that_anticipates_the_next_move_earns_a_positive_contribution() -> Result<()> {
        // The source prints, now, the own stream's next price: its change
        // leads the outcome by exactly one interval.
        let own = walk(41, 7);
        let leading: Vec<f64> = own[1..].to_vec();
        let mut desk = fed(&own[..40], &[("leading", leading)]);
        let round = desk
            .maybe_score(1, Timestamp::from_secs(1))
            .ok_or_else(|| qip_core::error::Error::not_found("no round on the cadence"))?;
        let alpha = round.scores[0]
            .outcome
            .clone()
            .map_err(qip_core::error::Error::invalid)?;
        assert!(alpha.realised_contribution > 0.99, "{alpha:?}");
        Ok(())
    }

    #[test]
    fn too_few_samples_are_reported_unscored_and_off_cadence_scores_nothing() {
        let own = walk(5, 3);
        let mut desk = fed(&own, &[("thin", walk(5, 4))]);
        assert!(
            desk.maybe_score(1, Timestamp::from_secs(1))
                .is_some_and(|round| {
                    round.scores.len() == 1 && round.scores[0].outcome.is_err()
                })
        );
        let mut off = SourceAlphaDesk::new(0);
        assert!(off.maybe_score(1, Timestamp::from_secs(1)).is_none());
        let mut every_three = SourceAlphaDesk::new(3);
        assert!(
            every_three
                .maybe_score(2, Timestamp::from_secs(1))
                .is_none()
        );
    }

    #[test]
    fn the_cadence_variable_is_zero_unless_set_and_refuses_a_non_count() {
        assert!(matches!(source_alpha_every(&|_| None), Ok(0)));
        assert!(matches!(
            source_alpha_every(&|_| Some("4".to_string())),
            Ok(4)
        ));
        assert!(source_alpha_every(&|_| Some("often".to_string())).is_err());
    }
}
