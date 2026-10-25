//! Blueprint §28 (R12-RES-7104): when the twin's replay of fills diverges
//! from what a venue actually did, the strategies that trade at that venue
//! are granted less capital until they are promoted again.
//!
//! # The failure this closes
//!
//! The twin re-prices every fill the desk took against the same tape and
//! cost model it prices every counterfactual and every replayed session
//! against, and the difference — [`FillScore::trade_error_bps`] — was charted
//! on `qip_venue_fill_error_bps` and folded into the venue's calibrated
//! spread (R12-TICK-2080). Nothing that sizes capital read it. A twin a
//! thousand basis points wrong about a venue was the same twin whose replays
//! had argued every strategy at that venue up the promotion ladder, and those
//! strategies kept being granted the size the ladder had admitted, on
//! evidence the live fills had just shown was priced by a model of some
//! other market.
//!
//! # What this does, and only this
//!
//! * **Measures** the twin's divergence per venue as the mean absolute fill
//!   error over the scored fills at that venue, and calls it a divergence
//!   only past [`REPLAY_DIVERGENCE_THRESHOLD_BPS`] on at least
//!   [`REPLAY_DIVERGENCE_MIN_SAMPLE`] fills. Absolute, because a twin wrong
//!   in the generous direction flatters every replay and one wrong in the
//!   punishing direction understates every one, and neither is evidence a
//!   gate should have been reading.
//! * **Holds** every strategy registered at that venue to
//!   [`REPLAY_DIVERGENCE_MULTIPLIER`] of the grant the allocator sizes it
//!   at. Read at the capital grant, the one seam that puts a size behind a
//!   strategy, as a contraction beside the confidence governor's — so the
//!   signed envelope, the approval chain and the cell all carry the narrowed
//!   figure. The multiplier is below one and there is no branch that
//!   produces another: this can shrink a grant and cannot grow one.
//! * **Releases** a hold only on a re-promotion: an escalation on the
//!   strategy's own lifecycle ledger recorded *after* the hold was raised.
//!   That is the gate re-reading the strategy's evidence with a fresh dual
//!   approval where the rung needs one — the "renewed promotion" the
//!   requirement names. Nothing on this path can lift the hold by itself:
//!   not a quiet cycle, not the twin's error falling, not a model output.
//!   A divergence that has stopped is not evidence the strategy's case still
//!   stands, and lifting the size automatically when the error fell would
//!   let a twin that had merely been recalibrated onto the venue re-admit
//!   capital on the replays it produced before it was.
//! * **Restarts the evidence window** at the re-promotion: the next
//!   divergence on that strategy is measured over fills scored after it, so
//!   a twin still diverging re-holds the strategy on new evidence, and a
//!   recalibrated one is not charged for fills the re-promotion already
//!   answered.
//!
//! Recalibration is not scheduled here because it already runs: every fill
//! the twin scores moves that venue's calibrated spread before the next
//! pricing (`Platform::score_filled`), and the hold record carries the
//! calibration's observation count at the instant it was raised, so a
//! reader can see the twin was being moved by live telemetry while it
//! diverged. The benchmark rerun is recorded as **required** on the hold and
//! is what the re-promotion's gate is asked to read; this module cannot see
//! whether the evidence a re-promotion passed on was regenerated after the
//! hold, and says so rather than claiming it.
//!
//! # What this deliberately does not do
//!
//! **It withdraws no venue.** ADR 0062: a venue is withdrawn on feasibility
//! evidence and on nothing else, and
//! `qip-kernel/tests/learning.rs::a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue`
//! still holds. A twin wrong about a venue says the *evidence priced on that
//! twin* is suspect, which is a question about the strategies the evidence
//! promoted — not about whether the venue fills.
//!
//! **It demotes nothing.** A strategy keeps its rung; it is granted less on
//! it. Demotion is the lifecycle monitor's, on the strategy's own returns.
//!
//! **It does not shrink an envelope already in force.** A grant signed
//! before the hold runs to its expiry at its signed size; the hold narrows
//! every grant signed after it, including the renewal. Revoking a signed
//! envelope is a different act with its own approval chain.

use crate::platform::FillScore;
use qip_contracts::signal::StrategyId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_lifecycle::ledger::LedgerEntry;
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The mean absolute fill error, in basis points, past which the twin is
/// said to diverge from a venue.
///
/// One percent. The twin's liquid-equity cost model charges a half-spread
/// of a few basis points; a twin that misses where a venue filled by more
/// than a hundred on average across a sample is not modelling that venue's
/// execution, and a replay priced on it is a replay of a different market.
/// Strict: a mean exactly at the threshold is not past it.
pub const REPLAY_DIVERGENCE_THRESHOLD_BPS: f64 = 100.0;

/// The fewest scored fills at a venue before its divergence is judged.
///
/// ADR 0055's sample by value, for the reason `sizing_review` gives: the
/// platform has one answer to how much evidence makes a pattern a finding,
/// and one fill a thousand basis points out is an outlier, not a twin.
pub const REPLAY_DIVERGENCE_MIN_SAMPLE: usize = 10;

// A sample of zero would judge a venue on no fills and hold every strategy
// on a mean of nothing; refused when the crate is built, not when it runs.
const _: () = assert!(REPLAY_DIVERGENCE_MIN_SAMPLE >= 1);

/// What a held strategy's grant is narrowed to: one half.
///
/// The same discount ADR 0055 and ADR 0063 apply to the evidence they read,
/// and below one by construction — `Decimal::from_raw` at nine decimal
/// places. A hold that could be configured to one would be a control that
/// reads as protection and cannot narrow anything.
pub const REPLAY_DIVERGENCE_MULTIPLIER: Decimal = Decimal::from_raw(500_000_000);

/// The producer on every hold and release record, told apart from the
/// promotion, grant and registration records that share
/// [`Topic::ComplianceEvaluated`].
pub const REPLAY_DIVERGENCE_ORIGIN: &str = "kernel/replay-divergence";

/// A hold was raised: the strategy's grants are narrowed from here on.
pub const REDUCED: &str = "reduced";
/// A hold was released by a re-promotion recorded after it.
pub const RESTORED: &str = "restored";

/// What the benchmark rerun is on a hold: owed before the size returns.
pub const BENCHMARK_RERUN_REQUIRED: &str = "required before re-promotion";
/// What it is on a release: the re-promotion's gate read the evidence.
pub const BENCHMARK_RERUN_READ_BY_GATE: &str = "read by the re-promotion's gate";

/// The twin's divergence from one venue, over the fills a window admits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divergence {
    pub sample: usize,
    pub mean_abs_error_bps: f64,
}

impl Divergence {
    /// Whether this is past the threshold. Strict, so a mean exactly at
    /// [`REPLAY_DIVERGENCE_THRESHOLD_BPS`] holds nothing.
    pub fn exceeds_threshold(&self) -> bool {
        self.mean_abs_error_bps > REPLAY_DIVERGENCE_THRESHOLD_BPS
    }
}

/// The twin's divergence from `venue` over the fills scored after `since`
/// (all of them where `since` is `None`), or `None` below the sample.
///
/// A fill whose twin did not enter carries no error and is not counted: it
/// is absence of a comparison, not a comparison of zero. An error that is
/// not finite is refused rather than skipped — a NaN averaged in would make
/// every mean false against the threshold and hold nothing, and skipping it
/// would hide that the twin produced one.
pub fn measure(
    scores: &[FillScore],
    venue: &str,
    since: Option<Timestamp>,
) -> Result<Option<Divergence>> {
    let mut sample = 0usize;
    let mut total = 0.0_f64;
    for score in scores {
        if score.venue != venue || since.is_some_and(|since| score.scored_at <= since) {
            continue;
        }
        let Some(error) = score.trade_error_bps else {
            continue;
        };
        if !error.is_finite() {
            return Err(Error::numeric(format!(
                "the twin's fill error on {} at {venue} is {error}, which is not a measurement; \
                 the divergence at {venue} cannot be judged until the twin prices it",
                score.order_id
            )));
        }
        sample += 1;
        total += error.abs();
    }
    if sample < REPLAY_DIVERGENCE_MIN_SAMPLE {
        return Ok(None);
    }
    Ok(Some(Divergence {
        sample,
        mean_abs_error_bps: total / sample as f64,
    }))
}

/// The first escalation on a strategy's ledger recorded strictly after
/// `raised_at` — the re-promotion that releases a hold — if any.
///
/// Strictly after: a promotion at the very instant a hold was raised cannot
/// be shown to have been made on knowledge of it, so it does not count.
/// Demotions and retirements are not escalations and never release a hold.
pub fn repromotion_after(history: &[LedgerEntry], raised_at: Timestamp) -> Option<&LedgerEntry> {
    history
        .iter()
        .find(|entry| entry.is_escalation() && entry.promotion.at > raised_at)
}

/// A strategy whose grants are narrowed because the twin diverged at its
/// venue, and the measurement that put it there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DivergenceHold {
    pub strategy: StrategyId,
    pub venue: String,
    pub raised_at: Timestamp,
    pub sample: usize,
    pub mean_abs_error_bps: f64,
    /// [`REPLAY_DIVERGENCE_MULTIPLIER`] at the instant it was raised, kept
    /// on the hold so a replay applies the figure that was in force.
    pub multiplier: Decimal,
    /// How many live fills the twin's calibration at this venue had been
    /// moved by when the hold was raised.
    pub calibration_observations: u64,
}

/// One hold raised or released, as the log carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayDivergenceEntry {
    pub strategy: String,
    pub venue: String,
    /// [`REDUCED`] or [`RESTORED`].
    pub outcome: String,
    pub sample: usize,
    pub mean_abs_error_bps: f64,
    pub threshold_bps: f64,
    pub multiplier: Decimal,
    pub calibration_observations: u64,
    /// [`BENCHMARK_RERUN_REQUIRED`] or [`BENCHMARK_RERUN_READ_BY_GATE`].
    pub benchmark_rerun: String,
    pub raised_at: Timestamp,
    /// On a release: the rung the re-promotion entered and when.
    pub repromoted_to: Option<String>,
    pub repromoted_at: Option<Timestamp>,
    pub at: Timestamp,
}

impl EventBody for ReplayDivergenceEntry {
    /// A decision about how much capital a strategy may be granted, in the
    /// group the log never evicts, beside the grant and promotion records.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "replay-divergence:{}:{}:{}",
            self.strategy,
            self.outcome,
            self.raised_at.to_rfc3339()
        ))
    }
}

impl ReplayDivergenceEntry {
    pub fn reduced(hold: &DivergenceHold) -> Self {
        Self {
            strategy: hold.strategy.as_str().to_string(),
            venue: hold.venue.clone(),
            outcome: REDUCED.to_string(),
            sample: hold.sample,
            mean_abs_error_bps: hold.mean_abs_error_bps,
            threshold_bps: REPLAY_DIVERGENCE_THRESHOLD_BPS,
            multiplier: hold.multiplier,
            calibration_observations: hold.calibration_observations,
            benchmark_rerun: BENCHMARK_RERUN_REQUIRED.to_string(),
            raised_at: hold.raised_at,
            repromoted_to: None,
            repromoted_at: None,
            at: hold.raised_at,
        }
    }

    pub fn restored(hold: &DivergenceHold, repromotion: &LedgerEntry, at: Timestamp) -> Self {
        Self {
            outcome: RESTORED.to_string(),
            benchmark_rerun: BENCHMARK_RERUN_READ_BY_GATE.to_string(),
            repromoted_to: Some(repromotion.promotion.to.as_str().to_string()),
            repromoted_at: Some(repromotion.promotion.at),
            at,
            ..Self::reduced(hold)
        }
    }

    fn hold(&self) -> DivergenceHold {
        DivergenceHold {
            strategy: StrategyId::new(self.strategy.clone()),
            venue: self.venue.clone(),
            raised_at: self.raised_at,
            sample: self.sample,
            mean_abs_error_bps: self.mean_abs_error_bps,
            multiplier: self.multiplier,
            calibration_observations: self.calibration_observations,
        }
    }
}

/// The holds standing and each released strategy's window start, as the log
/// left them: every record walked in log order, so a strategy held, released
/// and held again resumes held. Selected by producer, so the other records on
/// the topic are passed over.
pub type Resumed = (
    BTreeMap<StrategyId, DivergenceHold>,
    BTreeMap<StrategyId, Timestamp>,
);

pub fn resume(log: &EventLog) -> Result<Resumed> {
    let mut holds = BTreeMap::new();
    let mut windows = BTreeMap::new();
    for event in log.events() {
        if event.topic != ReplayDivergenceEntry::TOPIC
            || event.lineage.producer != REPLAY_DIVERGENCE_ORIGIN
        {
            continue;
        }
        let record = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<ReplayDivergenceEntry>())?
            .body;
        let hold = record.hold();
        if record.outcome == REDUCED {
            holds.insert(hold.strategy.clone(), hold);
        } else if record.outcome == RESTORED {
            holds.remove(&hold.strategy);
            let start = record.repromoted_at.ok_or_else(|| {
                Error::invalid(format!(
                    "the release of {}'s replay-divergence hold names no re-promotion instant; \
                     a release the log cannot date cannot restart the evidence window",
                    record.strategy
                ))
            })?;
            windows.insert(hold.strategy, start);
        } else {
            return Err(Error::invalid(format!(
                "a replay-divergence record for {} carries the outcome `{}`, which is neither \
                 `{REDUCED}` nor `{RESTORED}`; the log cannot be resumed past it",
                record.strategy, record.outcome
            )));
        }
    }
    Ok((holds, windows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::ObjectId;
    use qip_core::ids::OrderId;

    fn score(venue: &str, error: Option<f64>, scored_at: i64) -> FillScore {
        FillScore {
            order_id: OrderId::from_string(format!("ord-{scored_at}")),
            object_id: ObjectId::from_string("obj-AAA"),
            venue: venue.to_string(),
            filled_at: Timestamp::from_secs(0),
            scored_at: Timestamp::from_secs(scored_at),
            smaller_favoured: false,
            larger_favoured: false,
            trade_error_bps: error,
            unfilled: Decimal::ZERO,
        }
    }

    #[test]
    fn the_declared_bounds_can_only_narrow_and_can_actually_fire() {
        assert!(REPLAY_DIVERGENCE_MULTIPLIER.is_positive());
        assert!(REPLAY_DIVERGENCE_MULTIPLIER < Decimal::ONE);
        assert!(
            REPLAY_DIVERGENCE_THRESHOLD_BPS.is_finite() && REPLAY_DIVERGENCE_THRESHOLD_BPS > 0.0
        );
    }

    #[test]
    fn divergence_is_measured_on_the_absolute_error_at_one_venue_after_the_window_opens() {
        let mut scores: Vec<FillScore> = (0..REPLAY_DIVERGENCE_MIN_SAMPLE as i64)
            .map(|n| score("v", Some(if n % 2 == 0 { -150.0 } else { 150.0 }), n + 10))
            .collect();
        scores.push(score("other", Some(5_000.0), 30));
        scores.push(score("v", None, 31));
        let found = measure(&scores, "v", None).unwrap().unwrap();
        assert_eq!(found.sample, REPLAY_DIVERGENCE_MIN_SAMPLE);
        assert!((found.mean_abs_error_bps - 150.0).abs() < 1e-9);
        assert!(found.exceeds_threshold());
        assert_eq!(
            measure(&scores, "v", Some(Timestamp::from_secs(10))).unwrap(),
            None,
            "a fill scored at the window's start is not after it"
        );
        let nan = vec![score("v", Some(f64::NAN), 1)];
        assert!(measure(&nan, "v", None).is_err());
    }
}
