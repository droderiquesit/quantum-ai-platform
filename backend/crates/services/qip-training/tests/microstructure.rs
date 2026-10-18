//! Microstructure labels, models and benchmarks from replayed ticks
//! (R12-TICK-2065, 2068, 2069; consuming R12-TICK-2061's flags).

#![allow(clippy::panic_in_result_fn, clippy::float_cmp)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_training::microstructure::{
    BookObservation, FillResidualObservation, LabelPlan, MicroExample, TradeObservation,
    benchmark_fill_model, build_examples, join_fill_residuals, split_in_time, train_fill_model,
};
use std::collections::BTreeSet;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

/// A book whose next move is decided by its touch imbalance: a thin bid
/// (imbalance 0.2) is followed by a print through the bid half a second
/// later and a one-tick fall; a thick bid (0.8) by a print at the ask and a
/// one-tick rise. So whether a resting bid fills is predictable from the
/// imbalance, and from nothing else.
fn session(
    venue: &str,
    instrument: &str,
    steps: usize,
) -> (Vec<BookObservation>, Vec<TradeObservation>) {
    let mut observations = Vec::new();
    let mut trades = Vec::new();
    let mut mid = 1_000.0;
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    for i in 0..steps {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let thin = state.is_multiple_of(2);
        let imbalance = if thin { 0.2 } else { 0.8 };
        let t = i as i64 * 1_000;
        let (bid, ask) = (mid - 0.5, mid + 0.5);
        observations.push(BookObservation {
            at: at(t),
            venue: venue.into(),
            sequence: i as u64,
            entitlement: "vendor-depth".into(),
            instrument: instrument.into(),
            best_bid: bid,
            best_ask: ask,
            bid_size: imbalance * 10.0,
            ask_size: (1.0 - imbalance) * 10.0,
            unreliable: false,
        });
        trades.push(TradeObservation {
            at: at(t + 500),
            instrument: instrument.into(),
            price: if thin { bid - 1.0 } else { ask },
        });
        mid += if thin { -1.0 } else { 1.0 };
    }
    (observations, trades)
}

fn plan(horizons: Vec<Duration>) -> LabelPlan {
    LabelPlan {
        horizons,
        sample_every: 1,
        trailing: 5,
        volatile_above_bps: 50.0,
    }
}

/// Mutation: in `build_examples`, label from `fill_outcome` over
/// `[t - h, t)` or with the sign of `future_return` flipped — the
/// hand-computed values differ.
#[test]
fn every_label_comes_from_the_label_builder_and_matches_a_hand_computation() -> Result<()> {
    let (observations, trades) = session("XLON", "obj-ACME", 40);
    let set = build_examples(&observations, &trades, &plan(vec![Duration::from_secs(1)]))?;
    assert!(set.examples.len() > 20, "premise: examples were built");
    let example = &set.examples[0];
    let index = observations
        .iter()
        .position(|o| o.at == example.at)
        .expect("its observation");
    let now = (observations[index].best_bid + observations[index].best_ask) / 2.0;
    let next = (observations[index + 1].best_bid + observations[index + 1].best_ask) / 2.0;
    assert!((example.future_return - (next / now - 1.0)).abs() < 1e-12);
    let thin = observations[index].bid_size < observations[index].ask_size;
    assert_eq!(
        example.fill,
        if thin { 1.0 } else { 0.0 },
        "a print through the bid fills it"
    );
    assert!(
        (example.market_impact - (next - now) / now).abs() < 1e-12,
        "a buy's impact from the mid"
    );
    assert!(
        (example.adverse_selection
            - -((next - observations[index].best_bid) / observations[index].best_bid))
            .abs()
            < 1e-12
    );
    Ok(())
}

/// Mutation: in `build_examples`, read the imbalance from the observation
/// after `t` — changing a later observation changes an earlier example.
#[test]
fn a_feature_reads_only_what_was_knowable_at_its_decision_instant() -> Result<()> {
    let (observations, trades) = session("XLON", "obj-ACME", 30);
    let one = build_examples(&observations, &trades, &plan(vec![Duration::from_secs(1)]))?;
    let target = one.examples[3].clone();
    let mut altered = observations.clone();
    for o in altered.iter_mut().filter(|o| o.at > target.at) {
        o.bid_size = 9.9;
        o.ask_size = 0.1;
    }
    let two = build_examples(&altered, &trades, &plan(vec![Duration::from_secs(1)]))?;
    let same = two
        .examples
        .iter()
        .find(|e| e.at == target.at)
        .expect("still built");
    assert_eq!(
        same.features, target.features,
        "features do not move when only the future changes"
    );
    Ok(())
}

/// Mutation: drop the `flagged` check in `build_examples` — examples are
/// built over the flagged instant.
#[test]
fn no_example_is_built_across_a_span_the_replay_flagged_unreliable() -> Result<()> {
    let (mut observations, trades) = session("XLON", "obj-ACME", 40);
    let flagged = observations[20].at;
    observations[20].unreliable = true;
    let horizon = Duration::from_secs(1);
    let set = build_examples(&observations, &trades, &plan(vec![horizon]))?;
    assert!(
        set.refused_unreliable > 0,
        "premise: the flag refused some instants"
    );
    assert!(!set.examples.is_empty());
    for example in &set.examples {
        let window_start = observations
            .iter()
            .position(|o| o.at == example.at)
            .map(|i| observations[i + 1 - 5].at)
            .expect("its observation");
        assert!(
            !(flagged >= window_start && flagged <= example.at.saturating_add(horizon)),
            "an example at {} reads the flagged instant",
            example.at
        );
    }
    Ok(())
}

/// Mutation: make `train_fill_model` return the base rate as its
/// prediction (zero the weights) — its holdout Brier equals the baseline's
/// and `beats_baseline` is false.
#[test]
fn a_fill_model_trained_on_labelled_ticks_beats_its_base_rate_on_a_holdout_in_time() -> Result<()> {
    let (observations, trades) = session("XLON", "obj-ACME", 300);
    let set = build_examples(&observations, &trades, &plan(vec![Duration::from_secs(1)]))?;
    let model = train_fill_model(&set.examples, 0.3)?;
    assert!(model.holdout_examples > 50, "premise: a real holdout");
    let (train, holdout) = split_in_time(&set.examples, 0.3)?;
    let last_train = train.iter().map(|e| e.at).max().expect("training examples");
    assert!(
        holdout.iter().all(|e| e.at > last_train),
        "the holdout is the later slice in time"
    );
    assert!(
        model.beats_baseline(),
        "Brier {} against the base rate's {}",
        model.holdout_brier,
        model.baseline_brier
    );
    assert!(model.holdout_brier < 0.5 * model.baseline_brier);
    Ok(())
}

/// Mutation: key `benchmark_fill_model`'s cells by venue alone — the row
/// count collapses and the per-row counts no longer match.
#[test]
fn the_benchmark_has_one_row_per_venue_asset_regime_and_horizon() -> Result<()> {
    let (mut observations, mut trades) = session("XLON", "obj-ACME", 200);
    let (o2, t2) = session("XPAR", "obj-BETA", 200);
    observations.extend(o2);
    trades.extend(t2);
    let horizons = vec![Duration::from_secs(1), Duration::from_secs(2)];
    let set = build_examples(&observations, &trades, &plan(horizons))?;
    let model = train_fill_model(&set.examples, 0.3)?;
    let report = benchmark_fill_model(&model, &set.examples);
    let cells: BTreeSet<(String, String, String, i64)> = set
        .examples
        .iter()
        .map(|e: &MicroExample| {
            (
                e.venue.clone(),
                e.instrument.clone(),
                e.regime.clone(),
                e.horizon.as_nanos(),
            )
        })
        .collect();
    assert!(
        cells.len() >= 4,
        "premise: two venues and two horizons at least: {cells:?}"
    );
    assert_eq!(
        report.rows.len(),
        cells.len(),
        "one row per cell, never an average across them"
    );
    for (key, row) in &report.rows {
        let n = set
            .examples
            .iter()
            .filter(|e| {
                e.venue == key.venue
                    && e.instrument == key.asset
                    && e.regime == key.regime
                    && e.horizon.as_nanos() == key.horizon_nanos
            })
            .count();
        assert_eq!(row.examples, n, "{key:?}");
    }
    Ok(())
}

/// R12-TICK-1051: an observation that names no entitlement — or no venue —
/// is refused, not labelled. A row nobody can say the platform may use is
/// not a training row.
///
/// Mutation: drop the lineage refusal at the top of `build_examples` and the
/// blank observations are labelled into rows with no licence.
#[test]
fn an_observation_with_no_entitlement_or_venue_is_refused_rather_than_labelled() -> Result<()> {
    let (observations, trades) = session("XLON", "obj-ACME", 30);
    let plan = plan(vec![Duration::from_secs(1)]);
    assert!(
        !build_examples(&observations, &trades, &plan)?
            .examples
            .is_empty(),
        "premise: the session labels when its lineage is whole"
    );
    let mut unlicensed = observations.clone();
    unlicensed[3].entitlement = " ".into();
    assert!(build_examples(&unlicensed, &trades, &plan).is_err());
    let mut nameless = observations;
    nameless[3].venue = String::new();
    assert!(build_examples(&nameless, &trades, &plan).is_err());
    Ok(())
}

fn example(venue: &str, instrument: &str, ms: i64, horizon_s: i64) -> MicroExample {
    MicroExample {
        venue: venue.to_string(),
        sequence: 1,
        entitlement: "vendor-depth".to_string(),
        instrument: instrument.to_string(),
        at: at(ms),
        horizon: Duration::from_secs(horizon_s),
        regime: "calm".to_string(),
        features: vec![0.0],
        fill: 0.0,
        future_return: 0.0,
        realized_volatility: 0.0,
        adverse_selection: 0.0,
        market_impact: 0.0,
    }
}

fn residual(venue: &str, ms: i64, error_bps: f64) -> FillResidualObservation {
    FillResidualObservation {
        venue: venue.to_string(),
        instrument: "ACME".to_string(),
        filled_at: at(ms),
        error_bps,
    }
}

/// R12-TICK-6014: a residual joins the example drawn at the last instant at
/// or before its fill, on its own venue, and never one drawn after it.
///
/// Mutation: change `e.at <= residual.filled_at` to `e.at >= residual.filled_at`
/// in `join_fill_residuals` and the row's instant moves past the fill.
#[test]
fn a_fill_residual_joins_the_last_example_at_or_before_it_on_its_own_venue() -> Result<()> {
    let examples = vec![
        example("XLON", "ACME", 1_000, 1),
        example("XLON", "ACME", 1_000, 10),
        example("XLON", "ACME", 2_000, 1),
        example("XLON", "ACME", 3_000, 1),
        example("XNYS", "ACME", 2_500, 1),
    ];
    let joined = join_fill_residuals(
        &examples,
        &[
            residual("XLON", 2_400, 4.0),
            residual("XLON", 500, 2.0),
            residual("XNYS", 2_400, 9.0),
        ],
    )?;
    // Premise: the examples span the fill, so "at or before" has something
    // after it to wrongly choose.
    assert!(examples.iter().any(|e| e.at > at(2_400)));
    assert_eq!(joined.rows.len(), 1, "{joined:?}");
    assert_eq!(joined.unmatched, 2);
    assert_eq!(joined.rows[0].example.at, at(2_000));
    assert_eq!(joined.rows[0].example.venue, "XLON");
    assert_eq!(joined.rows[0].error_bps, 4.0);
    // Two horizons at one instant still make one row, the shortest.
    let tie = join_fill_residuals(&examples, &[residual("XLON", 1_200, 1.5)])?;
    assert_eq!(tie.rows.len(), 1);
    assert_eq!(tie.rows[0].example.horizon, Duration::from_secs(1));
    Ok(())
}

#[test]
fn a_fill_residual_that_is_not_a_number_is_refused_not_dropped() {
    let examples = vec![example("XLON", "ACME", 1_000, 1)];
    assert!(join_fill_residuals(&examples, &[residual("XLON", 1_100, 1.0)]).is_ok());
    for bad in [f64::NAN, f64::INFINITY] {
        let refused = join_fill_residuals(&examples, &[residual("XLON", 1_100, bad)])
            .expect_err("a non-finite residual was joined");
        assert!(refused.message().contains("not a finite number"));
    }
}
