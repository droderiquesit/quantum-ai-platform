//! The crowding comparison: does an edge survive company? (ADR 0053, §15.3)
//!
//! The failure this suite guards is subtler than a wrong number. A comparison
//! between two runs is only evidence if the two runs actually differ in the way
//! the comparison claims — and a panel that places no order produces a
//! difference of exactly zero, which reads as a robust strategy and is a panel
//! that sat the round out. Most of what is asserted here is that the experiment
//! is an experiment.

// See the other suites in this crate: in a test the assertion is the
// deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, ObjectId, dec};
use qip_financial::costs::LiquidityProfile;
use qip_financial::quality::DataQuality;
use qip_market::bar::{Bar, Interval};
use qip_simulation_engine::backtest::BacktestStrategy;
use qip_simulation_engine::clock::PointInTimeView;
use qip_simulation_engine::crowding::{book_shape, measure_crowding, standard_panel};
use std::collections::BTreeMap;

const SUBJECT: &str = "obj-AAA";
const VENUE: &str = "XSIM";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn subject() -> ObjectId {
    ObjectId::from_string(SUBJECT.to_string())
}

/// A liquid listed profile, stated rather than inherited — `LiquidityProfile`
/// has no `Default` for the reason `architecture.rs` enforces.
fn profile() -> LiquidityProfile {
    LiquidityProfile::listed(Decimal::from_int(500_000), 4.0)
}

/// A tape that moves, so a momentum or competitor agent has something to react
/// to. A flat tape would leave every reactive agent below its threshold and the
/// panel would place nothing — which is precisely the state one test below
/// asserts is distinguishable.
fn bars(count: i64) -> Vec<Bar> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let swing = ((i as f64) * 0.4).sin() * 0.01;
            let open = price;
            price *= 1.0 + swing;
            Bar {
                object_id: subject(),
                venue: VENUE.to_string(),
                interval: Interval::Day,
                open_time: start().saturating_add(Duration::from_days(i)),
                open: Decimal::from_f64(open).expect("finite"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("finite"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("finite"),
                close: Decimal::from_f64(price).expect("finite"),
                volume: dec!("50000"),
                trade_count: 500,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }
        })
        .collect()
}

/// A strategy that holds a fixed long weight throughout.
///
/// Deliberately trivial. The property under test is the *comparison*, and a
/// strategy with its own logic would make a difference between the two runs
/// ambiguous between the counterparties and the strategy reacting to them.
/// A constant weight reacts to nothing, so every difference is the panel's.
#[derive(Debug)]
struct AlwaysLong {
    weight: f64,
}

impl BacktestStrategy for AlwaysLong {
    fn name(&self) -> &str {
        "always-long"
    }

    fn target_weights(&mut self, _view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
        BTreeMap::from([(SUBJECT.to_string(), self.weight)])
    }
}

#[test]
fn a_panel_cannot_be_sized_off_a_volume_the_tape_did_not_record() {
    // Refuse rather than substitute. A panel sized off a default volume has no
    // relation to the market being simulated and would look exactly like one
    // that had — the difference is invisible in every number downstream.
    for volume in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let refused = standard_panel(volume);
        assert!(
            refused.is_err(),
            "a daily volume of {volume} produced a panel"
        );
    }
    // The premise: a real volume does produce one, so the assertion above is
    // about the bad values and not about the function refusing everything.
    let panel = standard_panel(500_000.0).expect("a real volume sizes a panel");
    assert_eq!(
        panel.len(),
        5,
        "the panel must carry all five behaviours the blueprint names"
    );
}

#[test]
fn a_book_shape_is_derived_from_the_observed_profile_and_never_from_a_default() {
    let tape = bars(40);
    let shape = book_shape(SUBJECT, &profile(), &tape).expect("a populated tape shapes a book");
    // Each figure traces to something observed. A shape that silently used a
    // constructor's defaults would price fills for an instrument nobody
    // measured, and would validate just as happily.
    assert!(
        (shape.daily_volume - profile().average_daily_volume.to_f64()).abs() < 1e-9,
        "the volume {} is not the profile's {}",
        shape.daily_volume,
        profile().average_daily_volume.to_f64()
    );
    assert_eq!(
        shape.level_size,
        profile().top_of_book_depth,
        "the resting size is not the profile's depth"
    );
    assert!(
        (shape.half_spread_bps - profile().typical_spread_bps / 2.0).abs() < 1e-12,
        "the half-spread {} is not half the quoted spread {}",
        shape.half_spread_bps,
        profile().typical_spread_bps
    );
    assert_eq!(
        shape.initial_price, tape[0].close,
        "the price is not the tape's"
    );
    // And the volatility is the tape's own, not a constructor's guess. The
    // premise first: this tape moves, so a zero here would be wrong rather
    // than merely uninformative.
    assert!(
        shape.step_volatility > 0.0,
        "a moving tape produced a zero step volatility"
    );

    // An empty tape is refused, not defaulted.
    assert!(
        book_shape(SUBJECT, &profile(), &[]).is_err(),
        "a book shape was produced for an instrument with no bars"
    );
}

#[test]
fn the_two_runs_differ_only_by_the_panel_and_the_panel_actually_trades() -> Result<()> {
    // The experiment has to be an experiment. If the counterparties place
    // nothing, both runs are the same run, the difference is zero, and a
    // reader takes that for a strategy that survives company.
    let tape = bars(60);
    let shape = book_shape(SUBJECT, &profile(), &tape)?;
    let outcome = measure_crowding(tape, shape, VENUE, 7, Decimal::from_int(100_000), || {
        Ok(AlwaysLong { weight: 0.5 })
    })?;

    assert!(
        outcome.panel_participated(),
        "the panel placed no order, so the comparison is between two identical \
         runs and says nothing about crowding: {}",
        outcome.summarise()
    );
    assert!(
        outcome.counterparty_orders > 0,
        "the flow is empty: {}",
        outcome.summarise()
    );
    // Company changes the outcome. Not a claim about the direction — the panel
    // is uncalibrated and its sign is not a finding — only that the two runs
    // are distinguishable, which is what makes the measurement a measurement.
    assert_ne!(
        outcome.bare,
        outcome.crowded,
        "the crowded run produced exactly the bare run's P&L, so either the \
         panel is not reaching the book or the strategy never traded: {}",
        outcome.summarise()
    );
    Ok(())
}

#[test]
fn every_report_opens_with_the_statement_that_the_panel_is_not_calibrated() -> Result<()> {
    // The one thing a reader must not miss. The number invites over-reading,
    // and the sentence that stops it is carried by the run rather than
    // remembered by whoever writes the summary.
    let tape = bars(60);
    let shape = book_shape(SUBJECT, &profile(), &tape)?;
    let outcome = measure_crowding(tape, shape, VENUE, 7, Decimal::from_int(100_000), || {
        Ok(AlwaysLong { weight: 0.5 })
    })?;
    let summary = outcome.summarise();
    let statement = qip_simulation_engine::agents::NOT_CALIBRATED_STATEMENT;
    // Premise: the statement is not empty, or `starts_with` below holds for
    // any string at all.
    assert!(
        !statement.is_empty(),
        "the calibration statement is empty, so the assertion below is vacuous"
    );
    assert!(
        summary.starts_with(statement),
        "the report does not open with the calibration statement: {summary}"
    );
    Ok(())
}

#[test]
fn the_same_tape_and_seed_produce_the_same_comparison_twice() -> Result<()> {
    // A comparison that is not reproducible is not evidence: two rounds over
    // one candidate would disagree, and neither reading could be checked
    // against the record. The seed is what makes the panel the same panel.
    let run = || -> Result<_> {
        let tape = bars(60);
        let shape = book_shape(SUBJECT, &profile(), &tape)?;
        measure_crowding(tape, shape, VENUE, 11, Decimal::from_int(100_000), || {
            Ok(AlwaysLong { weight: 0.5 })
        })
    };
    let first = run()?;
    let second = run()?;
    // Premise: the run traded, so equality below is not two empty results.
    assert!(
        first.panel_participated(),
        "the panel sat this run out, so reproducing it proves nothing"
    );
    assert_eq!(first, second, "two identical runs disagreed");
    Ok(())
}

#[test]
fn the_panel_is_sized_off_the_tape_and_not_off_the_profiles_claim_about_volume() -> Result<()> {
    // Two claims about one fact, and the panel is the louder one.
    // `MarketSimulator::replay` fills the book from the bars' own recorded
    // volume; a panel sized off a `LiquidityProfile`'s `average_daily_volume`
    // trades against a book built from something else entirely.
    //
    // This is not hypothetical. The profile below claims ten times the volume
    // the bars record — the same disagreement this fixture had while the panel
    // was sized off the profile — and the crowded run then lost roughly half
    // the capital against a bare run making a fraction of a percent. That
    // reads as a devastating crowding finding and was an arithmetic error
    // about which number to trust.
    let tape = bars(60);
    let recorded: f64 = tape.iter().map(|bar| bar.volume.to_f64()).sum::<f64>() / tape.len() as f64;
    let overclaiming =
        LiquidityProfile::listed(Decimal::from_f64(recorded * 10.0).expect("finite"), 4.0);
    // Premise: the profile and the tape really do disagree, and by the factor
    // this test is about. Without it the two sizings would coincide and the
    // assertion below would hold for either implementation.
    let shape = book_shape(SUBJECT, &overclaiming, &tape)?;
    assert!(
        shape.daily_volume > recorded * 5.0,
        "the profile does not overclaim, so this test cannot tell the two sizings apart: \
         profile {} against tape {recorded}",
        shape.daily_volume
    );

    let outcome = measure_crowding(tape, shape, VENUE, 7, Decimal::from_int(100_000), || {
        Ok(AlwaysLong { weight: 0.5 })
    })?;
    assert!(
        outcome.panel_participated(),
        "the panel placed nothing, so its size is untested: {}",
        outcome.summarise()
    );
    // A panel sized off ten times the tape's volume takes a fifth of every bar
    // per aggressor and moves the book far enough to swamp any strategy. The
    // bound is loose on purpose — it is not a claim about the right magnitude,
    // only that the panel is not sized off the inflated figure.
    assert!(
        outcome.cost().abs() < Decimal::from_int(50_000),
        "the crowding cost of {} against 100,000 of capital is the profile's \
         inflated volume, not the tape's: {}",
        outcome.cost(),
        outcome.summarise()
    );
    Ok(())
}
