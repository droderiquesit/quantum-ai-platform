//! Tests for the counterfactual digital twin.
//!
//! The tests that matter here are not the ones checking arithmetic. They are
//! the ones checking that a simulated figure cannot become a real one, that an
//! alternative cannot be evaluated against prices the decision could not have
//! seen, and that a missed opportunity is captured as a first-class outcome
//! rather than as an absence. Two of those three are also asserted at compile
//! time, in the `compile_fail` doctests on the crate root; the runtime halves
//! are here.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::signal::{Conviction, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{DecisionId, EventId, ObjectId, OpportunityId, OrderId};
use qip_core::lineage::{CausationId, CorrelationId, TraceId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_financial::quality::DataQuality;
use qip_learning_engine::attribution::{PositionAttribution, Source};
use qip_market::bar::{Bar, Interval};
use qip_simulation_engine::costs::CostModel;
use qip_twin::asof::TwinMarket;
use qip_twin::capture::{Action, Decision, OutcomeCapture, RealisedOutcome};
use qip_twin::counterfactual::{
    ActualTrade, Alternative, AlternativeMenu, CounterfactualEngine, CounterfactualSet,
    SimulatedFill, VenueOption,
};
use qip_twin::record::{
    AgentOutput, CounterfactualSummary, FillSummary, LearningRecord, MarketOutcome, MarketState,
    RiskSummary, WorldState,
};
use qip_twin::regret::RegretAnalysis;
use qip_twin::value::Simulated;
use std::collections::BTreeMap;

// --- fixtures ---------------------------------------------------------------

/// Whole days, so every timestamp in these tests survives millisecond
/// serialization and the round-trip test measures the record rather than the
/// clock.
fn day(n: i64) -> Timestamp {
    Timestamp::from_civil(2025, 1, 1).saturating_add(Duration::from_days(n))
}

/// The instant every decision in these tests is taken at.
fn decided_at() -> Timestamp {
    day(5)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

/// A daily bar. `volume` is what the impact model participates against.
fn bar(object_id: &ObjectId, index: i64, open: i64, close: i64, volume: i64) -> Bar {
    let open_price = Decimal::from_int(open);
    let close_price = Decimal::from_int(close);
    Bar {
        object_id: object_id.clone(),
        venue: "XTST".to_string(),
        interval: Interval::Day,
        open_time: day(index),
        open: open_price,
        high: open_price.max(close_price) + Decimal::ONE,
        low: open_price.min(close_price) - Decimal::ONE,
        close: close_price,
        volume: Decimal::from_int(volume),
        vwap: None,
        trade_count: 0,
        quality: DataQuality::clean(),
    }
}

/// Twenty sessions where the price rises by two a day.
fn rising(object_id: &ObjectId) -> Vec<Bar> {
    (0..20)
        .map(|i| bar(object_id, i, 100 + 2 * i, 101 + 2 * i, 60_000))
        .collect()
}

/// Twenty sessions where the price falls by two a day.
fn falling(object_id: &ObjectId) -> Vec<Bar> {
    (0..20)
        .map(|i| bar(object_id, i, 200 - 2 * i, 199 - 2 * i, 60_000))
        .collect()
}

fn market_of(bars: Vec<Bar>) -> Result<TwinMarket> {
    TwinMarket::new(bars, CostModel::default(), 10)
}

/// A buy of ten thousand, which is a sixth of the day's volume: inside the
/// participation the impact law is calibrated for, and three times that is not.
fn a_buy(object_id: &ObjectId) -> Result<ActualTrade> {
    ActualTrade::new(
        object_id.clone(),
        BookSide::Bid,
        dec!("10000"),
        VenueId::new("XTST"),
        "us",
        decided_at(),
    )
}

fn engine(seed: u64) -> Result<CounterfactualEngine> {
    CounterfactualEngine::new(
        seed,
        AlternativeMenu::standard(Duration::from_days(1)),
        Duration::from_days(5),
    )
}

fn a_decision(id: &str, object_id: &ObjectId) -> Decision {
    Decision::new(
        DecisionId::from_string(id),
        TraceId::new("trace-alpha"),
        CorrelationId::from_string("cor-alpha"),
        decided_at(),
        object_id.clone(),
        Action::Filled {
            order_id: OrderId::from_string("ord-1"),
            venue: VenueId::new("XTST"),
            quantity: dec!("10000"),
            price: dec!("108"),
        },
    )
}

/// Evaluate one decision against a market, returning the set.
fn evaluate(
    bars: Vec<Bar>,
    realised_pnl: Decimal,
    seed: u64,
    id: &str,
) -> Result<CounterfactualSet> {
    let subject = object("obj-alpha");
    let mut market = market_of(bars)?;
    let engine = engine(seed)?;
    let decision = a_decision(id, &subject);
    let actual = a_buy(&subject)?;
    let outcome = RealisedOutcome::realised(day(10), realised_pnl, dec!("4500"), dec!("10000"));
    engine.evaluate(&mut market, &decision, &actual, &outcome)
}

// --- a counterfactual can never be reported as an actual ---------------------

#[test]
fn a_simulated_figure_and_a_realised_one_are_totalled_separately() -> Result<()> {
    // The headline property. `realised_pnl` and `forgone` return different
    // types, so the two totals cannot be combined even by accident; this
    // asserts the arithmetic half, and the `compile_fail` doctest on the crate
    // root asserts that the types genuinely refuse to add.
    let subject = object("obj-alpha");
    let mut capture = OutcomeCapture::new();
    capture.record(
        a_decision("dec-1", &subject),
        RealisedOutcome::realised(day(10), dec!("1000"), dec!("50"), dec!("10000")),
    )?;
    capture.record(
        Decision::new(
            DecisionId::from_string("dec-2"),
            TraceId::new("trace-alpha"),
            CorrelationId::from_string("cor-alpha"),
            day(6),
            subject.clone(),
            Action::MissedOpportunity {
                opportunity: OpportunityId::from_string("opp-1"),
                object_id: subject.clone(),
                reason: "the conviction did not clear the bar after shrinkage".to_string(),
                would_have_earned: Simulated::of(dec!("50000")),
            },
        )
        .after(&a_decision("dec-1", &subject)),
        RealisedOutcome::nothing_happened(day(6)),
    )?;

    // The refusal contributes nothing to what the platform earned.
    assert_eq!(capture.realised_pnl(), dec!("1000"));
    // And everything to what it declined.
    // Compared as `Simulated` values, which is exact: the taint carries the
    // `Decimal` underneath, so the total does not have to go through `f64` to
    // be checked.
    assert_eq!(capture.forgone(), Simulated::of(dec!("50000")));
    Ok(())
}

#[test]
fn the_taint_on_a_simulated_figure_survives_serialization() -> Result<()> {
    // A wire format that drops the marker would let a simulated P&L be read
    // back into a `Decimal` field somewhere downstream, which is the same
    // failure as the type system's, one process later.
    let hypothetical = Simulated::of(dec!("42.5"));
    let json = serde_json::to_string(&hypothetical)?;
    assert!(json.contains("\"simulated\":true"), "{json}");
    assert!(
        serde_json::from_str::<Decimal>(&json).is_err(),
        "a simulated figure deserialized straight into an exact one"
    );

    // And a record claiming a simulated figure is real does not parse.
    let laundered = json.replace("\"simulated\":true", "\"simulated\":false");
    let error =
        serde_json::from_str::<Simulated<Decimal>>(&laundered).expect_err("simulated=false parsed");
    assert!(
        error
            .to_string()
            .contains("cannot be reported as an actual")
    );
    Ok(())
}

// --- a counterfactual is evaluated as of the decision ------------------------

#[test]
fn planning_an_alternative_against_a_later_market_is_refused_and_names_the_leak() -> Result<()> {
    // The runtime half of the leakage guard. The compile-time halves are the
    // absence of any timestamp-taking accessor on `DecisionView` and the
    // `&mut self` on `view_at`, both asserted in the crate-root doctests.
    let subject = object("obj-alpha");
    let mut market = market_of(rising(&subject))?;
    let engine = engine(7)?;
    let actual = a_buy(&subject)?;

    let view = market.view_at(day(9))?;
    let error = engine
        .plan(&actual, Alternative::Trade, &view)
        .expect_err("an alternative was planned against prices from four days later");
    assert_eq!(error.code(), "guard");
    assert!(
        error.message().contains("leak"),
        "the refusal did not name the leak: {error}"
    );
    assert!(error.message().contains("had not printed"), "{error}");
    Ok(())
}

#[test]
fn every_alternative_is_fixed_against_the_decision_instant_or_earlier() -> Result<()> {
    // The property the guard exists to preserve, checked on the output rather
    // than on the refusal: no plan in a set was allowed to see past the moment
    // the decision was taken.
    let set = evaluate(rising(&object("obj-alpha")), dec!("95000"), 11, "dec-1")?;
    assert!(!set.is_empty());
    for entry in set.entries() {
        assert!(
            entry.counterfactual_outcome.fixed_as_of() <= set.decided_at,
            "the {} alternative was fixed as of {}, after the decision at {}",
            entry.counterfactual_action.kind(),
            entry.counterfactual_outcome.fixed_as_of(),
            set.decided_at
        );
    }
    Ok(())
}

#[test]
fn a_counterfactual_dated_apart_from_its_decision_is_refused() -> Result<()> {
    // A trade and a decision that disagree about when the decision happened
    // means one of the two instants is wrong, and evaluating against either is
    // evaluating as of an instant nobody decided at.
    let subject = object("obj-alpha");
    let mut market = market_of(rising(&subject))?;
    let engine = engine(3)?;
    let mut actual = a_buy(&subject)?;
    actual.decided_at = day(7);
    let decision = a_decision("dec-1", &subject);
    let outcome = RealisedOutcome::realised(day(10), dec!("1"), Decimal::ZERO, dec!("10000"));

    let error = engine
        .evaluate(&mut market, &decision, &actual, &outcome)
        .expect_err("a counterfactual was evaluated as of the wrong instant");
    assert!(error.message().contains("wrong instant"), "{error}");
    Ok(())
}

// --- the fill model is the simulator's --------------------------------------

#[test]
fn the_counterfactual_fill_uses_the_simulators_cost_model_exactly() -> Result<()> {
    // Composition asserted numerically rather than claimed in a comment: the
    // twin's simulated cost for the plain `trade` alternative is what
    // `CostModel::cost_of` returns for the same size at the same two prices,
    // with the liquidity the point-in-time view estimated.
    let subject = object("obj-alpha");
    let set = evaluate(rising(&subject), dec!("95000"), 5, "dec-1")?;
    let traded = set.by_kind("trade").expect("the trade alternative");
    let SimulatedFill::Filled {
        entry_price,
        exit_price,
        ..
    } = traded.counterfactual_outcome.fill()
    else {
        panic!("the plain trade alternative did not fill");
    };

    let mut market = market_of(rising(&subject))?;
    let liquidity = {
        let view = market.view_at(decided_at())?;
        view.liquidity(&subject).expect("liquidity")
    };
    let costs = market.costs();
    let expected: f64 = [entry_price, exit_price]
        .into_iter()
        .map(|price| {
            costs
                .cost_of(
                    dec!("10000"),
                    *price,
                    liquidity.daily_volume_f64,
                    liquidity.daily_volatility_f64,
                )
                .map(|cost| cost.total())
                .unwrap_or(f64::NAN)
        })
        .sum();

    let actual_costs = traded
        .counterfactual_outcome
        .simulated_costs()
        .as_f64_for_statistics();
    assert!(
        (actual_costs - expected).abs() < 1e-6,
        "the twin charged {actual_costs} where the simulator's model charges {expected}"
    );
    Ok(())
}

#[test]
fn an_alternative_beyond_the_calibrated_participation_is_unfillable_not_profitable() -> Result<()> {
    // The reason "we should have traded three times the size" does not win by
    // default. The impact law is calibrated to twenty percent of a day's
    // volume; the twin refuses to price beyond it rather than returning a
    // number, exactly as the backtester does.
    let subject = object("obj-alpha");
    let mut market = market_of(rising(&subject))?;
    let menu = AlternativeMenu {
        larger: Decimal::from_int(3),
        ..AlternativeMenu::standard(Duration::from_days(1))
    };
    let engine = CounterfactualEngine::new(5, menu, Duration::from_days(5))?;
    let decision = a_decision("dec-1", &subject);
    let actual = a_buy(&subject)?;
    let outcome = RealisedOutcome::realised(day(10), dec!("95000"), dec!("4500"), dec!("10000"));

    let set = engine.evaluate(&mut market, &decision, &actual, &outcome)?;
    let larger = set.by_kind("larger_size").expect("the larger-size entry");
    match larger.counterfactual_outcome.fill() {
        SimulatedFill::Unfillable { reason } => {
            assert!(reason.contains("volume"), "{reason}");
        }
        other => panic!("three times the size was priced as {other:?}"),
    }
    assert!(
        larger.counterfactual_outcome.simulated_pnl().is_zero(),
        "an unfillable alternative earned something"
    );
    Ok(())
}

#[test]
fn a_decision_with_only_one_closed_bar_of_history_is_refused_rather_than_priced_at_zero_impact()
-> Result<()> {
    // Before the two-bar floor in `DecisionView::liquidity`, a decision this
    // early in a history got a `Liquidity` anyway: `stats::stddev` of a
    // single return is `0.0` by construction (sample variance divides by
    // `n - 1`), and `CostModel::cost_of` treats a non-positive volatility as
    // "no impact term". Every counterfactual settled from it would have paid
    // commission and spread but never impact — the alternative would have
    // looked cheapest exactly when the estimate behind it was worth least.
    // The fix makes the twin refuse rather than guess; this proves the
    // refusal reaches all the way up through `evaluate`, not just `liquidity`
    // itself.
    let subject = object("obj-alpha");
    let one_bar = vec![bar(&subject, 0, 100, 101, 60_000)];
    let mut market = market_of(one_bar)?;
    let engine = engine(1)?;
    let decision = Decision::new(
        DecisionId::from_string("dec-1"),
        TraceId::new("trace-alpha"),
        CorrelationId::from_string("cor-alpha"),
        day(1),
        subject.clone(),
        Action::Filled {
            order_id: OrderId::from_string("ord-1"),
            venue: VenueId::new("XTST"),
            quantity: dec!("10000"),
            price: dec!("101"),
        },
    );
    let actual = ActualTrade::new(
        subject.clone(),
        BookSide::Bid,
        dec!("10000"),
        VenueId::new("XTST"),
        "us",
        day(1),
    )?;
    let outcome = RealisedOutcome::realised(day(6), dec!("1"), Decimal::ZERO, dec!("10000"));

    let error = engine
        .evaluate(&mut market, &decision, &actual, &outcome)
        .expect_err(
            "a single closed bar was treated as a liquidity estimate and priced with zero impact",
        );
    assert!(
        error.message().contains("not enough closed bars"),
        "{error}"
    );
    Ok(())
}

// --- regret has the sign the world does -------------------------------------

#[test]
fn standing_aside_shows_positive_regret_on_a_loss_and_negative_on_a_win() -> Result<()> {
    // The property that makes the whole apparatus worth having: a platform that
    // lost money on a trade should be able to see that not trading was better,
    // and one that made money should see the opposite.
    let subject = object("obj-alpha");

    let lost = evaluate(falling(&subject), dec!("-104500"), 1, "dec-loss")?;
    let stood_aside = lost.by_kind("do_not_trade").expect("do_not_trade");
    assert!(
        stood_aside.difference.is_positive(),
        "standing aside on a losing trade did not read as a regret: {:?}",
        stood_aside.difference
    );
    assert!(stood_aside.favours_the_alternative());

    let won = evaluate(rising(&subject), dec!("95000"), 1, "dec-win")?;
    let stood_aside = won.by_kind("do_not_trade").expect("do_not_trade");
    assert!(
        stood_aside.difference.is_negative(),
        "standing aside on a winning trade read as a regret: {:?}",
        stood_aside.difference
    );
    assert!(!stood_aside.favours_the_alternative());
    Ok(())
}

// --- missed opportunities ---------------------------------------------------

#[test]
fn a_missed_opportunity_is_captured_with_its_value_and_is_not_a_trade() -> Result<()> {
    // The audit's specific finding. A refusal is an outcome, and the number
    // attached to it is what the platform declined — carried in a type that
    // keeps it out of the P&L it sits beside.
    let subject = object("obj-alpha");
    let mut capture = OutcomeCapture::new();
    capture.record(
        a_decision("dec-taken", &subject),
        RealisedOutcome::realised(day(10), dec!("1000"), dec!("50"), dec!("10000")),
    )?;
    capture.record(
        Decision::new(
            DecisionId::from_string("dec-missed"),
            TraceId::new("trace-beta"),
            CorrelationId::from_string("cor-beta"),
            day(6),
            subject.clone(),
            Action::MissedOpportunity {
                opportunity: OpportunityId::from_string("opp-1"),
                object_id: subject.clone(),
                reason: "the capital envelope had expired".to_string(),
                would_have_earned: Simulated::of(dec!("50000")),
            },
        ),
        RealisedOutcome::nothing_happened(day(6)),
    )?;
    capture.record(
        Decision::new(
            DecisionId::from_string("dec-stale"),
            TraceId::new("trace-gamma"),
            CorrelationId::from_string("cor-gamma"),
            day(7),
            subject.clone(),
            Action::StaleOpportunity {
                opportunity: OpportunityId::from_string("opp-2"),
                object_id: subject.clone(),
                seen_at: day(6),
                age: Duration::from_days(1),
                would_have_earned: Simulated::of(dec!("7000")),
            },
        ),
        RealisedOutcome::nothing_happened(day(7)),
    )?;

    assert_eq!(capture.missed().len(), 2);
    assert_eq!(capture.taken().len(), 1);
    for entry in capture.missed() {
        assert!(
            !entry.decision.action.is_taken(),
            "a decision the platform declined was counted as a trade"
        );
        assert!(entry.decision.action.is_refusal());
        assert!(entry.outcome.filled_quantity().is_zero());
    }
    assert_eq!(capture.forgone(), Simulated::of(dec!("57000")));
    assert_eq!(capture.realised_pnl(), dec!("1000"));

    let tally = capture.tally();
    assert_eq!(tally.get("missed_opportunity"), Some(&1));
    assert_eq!(tally.get("stale_opportunity"), Some(&1));
    assert_eq!(tally.get("filled"), Some(&1));
    Ok(())
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_same_seed_produces_identical_counterfactual_sets() -> Result<()> {
    // Bit-exact replay is the platform's whole disposition toward randomness.
    // The availability draw on a delayed entry is the only stochastic part of
    // this crate, and it is derived from the seed and the decision id.
    let subject = object("obj-alpha");
    let first = evaluate(rising(&subject), dec!("95000"), 0xABCD, "dec-1")?;
    let second = evaluate(rising(&subject), dec!("95000"), 0xABCD, "dec-1")?;
    assert_eq!(first, second);
    Ok(())
}

#[test]
fn a_set_does_not_depend_on_the_order_decisions_were_evaluated_in() -> Result<()> {
    // The random stream is forked from the decision id rather than consumed
    // sequentially, so a decision evaluated first and the same decision
    // evaluated tenth draw the same numbers. Without that, adding a decision to
    // a batch would silently change every counterfactual after it.
    let subject = object("obj-alpha");
    let alone = evaluate(rising(&subject), dec!("95000"), 0x1234, "dec-target")?;

    let mut market = market_of(rising(&subject))?;
    let engine = engine(0x1234)?;
    let actual = a_buy(&subject)?;
    let outcome = RealisedOutcome::realised(day(10), dec!("95000"), dec!("4500"), dec!("10000"));
    for id in ["dec-a", "dec-b", "dec-c"] {
        engine.evaluate(&mut market, &a_decision(id, &subject), &actual, &outcome)?;
    }
    let after_others = engine.evaluate(
        &mut market,
        &a_decision("dec-target", &subject),
        &actual,
        &outcome,
    )?;
    assert_eq!(alone, after_others);
    Ok(())
}

// --- the whole menu ---------------------------------------------------------

#[test]
fn all_nine_alternatives_are_generated_when_the_menu_names_somewhere_to_go() -> Result<()> {
    // Trade, do not trade, larger, smaller, another venue, another region, a
    // delayed entry, another hedge, and no hedge.
    let subject = object("obj-alpha");
    let proxy = object("obj-europe");
    let hedge = object("obj-hedge");
    let mut bars = rising(&subject);
    bars.extend(falling(&proxy));
    bars.extend(rising(&hedge));

    let mut market = market_of(bars)?;
    let menu = AlternativeMenu::standard(Duration::from_days(1))
        .with_venue(VenueOption {
            venue: VenueId::new("XALT"),
            costs: CostModel::small_cap(),
            firm_quotes: false,
        })
        .with_region("eu", proxy.clone())
        .with_hedge(hedge.clone(), dec!("0.5"));
    let engine = CounterfactualEngine::new(0x5EED, menu, Duration::from_days(5))?;
    let actual = a_buy(&subject)?.hedged_with(hedge.clone(), dec!("0.25"));
    let outcome = RealisedOutcome::realised(day(10), dec!("40000"), dec!("6000"), dec!("10000"));

    let set = engine.evaluate(
        &mut market,
        &a_decision("dec-1", &subject),
        &actual,
        &outcome,
    )?;
    let kinds: Vec<&str> = set
        .entries()
        .iter()
        .map(|entry| entry.counterfactual_action.kind())
        .collect();
    for expected in [
        "trade",
        "do_not_trade",
        "larger_size",
        "smaller_size",
        "different_venue",
        "different_region",
        "delayed_entry",
        "different_hedge",
        "no_hedge",
    ] {
        assert!(
            kinds.contains(&expected),
            "{expected} missing from {kinds:?}"
        );
    }
    assert_eq!(set.len(), 9);
    Ok(())
}

// --- regret across many decisions -------------------------------------------

/// `count` identical losing decisions, so the only thing varying between the
/// two sample sizes in the test below is the sample size.
fn losing_sets(count: usize) -> Result<Vec<CounterfactualSet>> {
    let subject = object("obj-alpha");
    let mut market = market_of(falling(&subject))?;
    let engine = engine(0x77)?;
    let actual = a_buy(&subject)?;
    let outcome = RealisedOutcome::realised(day(10), dec!("-104500"), dec!("4500"), dec!("10000"));
    (0..count)
        .map(|i| {
            engine.evaluate(
                &mut market,
                &a_decision(&format!("dec-{i}"), &subject),
                &actual,
                &outcome,
            )
        })
        .collect()
}

#[test]
fn regret_from_a_tiny_sample_does_not_read_as_significant() -> Result<()> {
    // Three observations where standing aside won every time is an anecdote.
    // Three hundred is a finding. The two must not read alike, and the
    // shrinkage that separates them is the same one a strategy's own conviction
    // goes through.
    let tiny = RegretAnalysis::over(&losing_sets(3)?)?;
    let ample = RegretAnalysis::over(&losing_sets(300)?)?;

    let from_three = tiny.get("do_not_trade").expect("do_not_trade");
    let from_many = ample.get("do_not_trade").expect("do_not_trade");

    // Standing aside beat the platform every single time in both samples.
    assert_eq!(from_three.better, from_three.observations);
    assert_eq!(from_many.better, from_many.observations);

    assert!(
        !from_three.is_systematic(0.6),
        "three observations read as a systematic pattern at {:.3}",
        from_three.shrunk_win_rate()
    );
    assert!(
        from_many.is_systematic(0.6),
        "three hundred observations did not read as systematic at {:.3}",
        from_many.shrunk_win_rate()
    );

    // And the magnitude is shrunk too, not only the win rate.
    let raw = from_three.mean.as_f64_for_statistics();
    let shrunk = from_three.shrunk_mean().as_f64_for_statistics();
    assert!(
        shrunk < raw * 0.2,
        "a mean from three observations was quoted at {shrunk} against a raw {raw}"
    );
    let ample_raw = from_many.mean.as_f64_for_statistics();
    let ample_shrunk = from_many.shrunk_mean().as_f64_for_statistics();
    assert!(
        ample_shrunk > ample_raw * 0.85,
        "a mean from three hundred observations was shrunk to {ample_shrunk} from {ample_raw}"
    );

    // Ranked on the shrunk figure, and standing aside is among the
    // alternatives that would have done better. It is not asserted to be *the*
    // worst: on a losing trade every alternative that avoided the position ties
    // with it, including the oversized one the participation limit refused.
    let worst = ample
        .worst_forgone()
        .expect("some alternative would have done better than losing money");
    assert!(worst.shrunk_mean().is_positive());
    assert!(
        from_many.shrunk_mean().is_positive(),
        "standing aside was not reported as a regret even at three hundred observations"
    );
    Ok(())
}

#[test]
fn a_regret_analysis_over_nothing_is_refused() -> Result<()> {
    let error = RegretAnalysis::over(&[]).expect_err("an analysis over no decisions");
    assert!(error.message().contains("nothing"), "{error}");
    Ok(())
}

// --- the trace chain --------------------------------------------------------

#[test]
fn every_captured_outcome_carries_a_trace_id_and_the_chain_reconstructs() -> Result<()> {
    // Source event to decision to execution to outcome, walkable by trace id.
    // A record that cannot do this cannot answer why a position was put on.
    let subject = object("obj-alpha");
    let trace = TraceId::new("trace-alpha");
    let event = EventId::from_string("evt-source");
    let mut capture = OutcomeCapture::new();

    let recommended = Decision::new(
        DecisionId::from_string("dec-model"),
        trace.clone(),
        CorrelationId::from_string("cor-alpha"),
        day(5),
        subject.clone(),
        Action::ModelRecommendation {
            model: "carry-v3".to_string(),
            version: "2025.01".to_string(),
            recommendation: "enter".to_string(),
            confidence: Conviction::new(0.71, 240),
        },
    )
    .from_event(event.clone())
    .because("the basis widened past two standard deviations");
    capture.record(
        recommended.clone(),
        RealisedOutcome::nothing_happened(day(5)),
    )?;

    let sized = Decision::new(
        DecisionId::from_string("dec-capital"),
        trace.clone(),
        CorrelationId::from_string("cor-alpha"),
        day(5),
        subject.clone(),
        Action::CapitalDecision {
            strategy: StrategyId::new("carry"),
            requested: dec!("2000000"),
            granted: dec!("1080000"),
            reason: "the envelope's remaining gross".to_string(),
        },
    )
    .after(&recommended);
    capture.record(sized.clone(), RealisedOutcome::nothing_happened(day(5)))?;

    let placed = Decision::new(
        DecisionId::from_string("dec-order"),
        trace.clone(),
        CorrelationId::from_string("cor-alpha"),
        day(5),
        subject.clone(),
        Action::OrderPlaced {
            order_id: OrderId::from_string("ord-1"),
            venue: VenueId::new("XTST"),
            side: BookSide::Bid,
            quantity: dec!("10000"),
            method: "participation".to_string(),
        },
    )
    .after(&sized);
    capture.record(placed.clone(), RealisedOutcome::nothing_happened(day(5)))?;

    let filled = Decision::new(
        DecisionId::from_string("dec-fill"),
        trace.clone(),
        CorrelationId::from_string("cor-alpha"),
        day(5),
        subject.clone(),
        Action::Filled {
            order_id: OrderId::from_string("ord-1"),
            venue: VenueId::new("XTST"),
            quantity: dec!("10000"),
            price: dec!("108"),
        },
    )
    .after(&placed);
    capture.record(
        filled,
        RealisedOutcome::realised(day(10), dec!("95000"), dec!("4500"), dec!("10000"))
            .with_slippage_bps(1.5)
            .with_latency(Duration::from_millis(240)),
    )?;

    for entry in capture.entries() {
        assert!(
            !entry.trace().as_str().trim().is_empty(),
            "an outcome was captured without a trace id"
        );
    }
    capture.verify()?;

    let chain = capture.reconstruct(&trace)?;
    assert!(chain.is_rooted());
    assert_eq!(chain.source_event, Some(event));
    assert_eq!(
        chain
            .links
            .iter()
            .map(|link| link.kind.as_str())
            .collect::<Vec<_>>(),
        vec![
            "model_recommendation",
            "capital_decision",
            "order_placed",
            "filled"
        ]
    );
    assert_eq!(chain.realised_pnl, dec!("95000"));
    assert!(chain.forgone.is_zero());
    Ok(())
}

#[test]
fn a_chain_with_a_parent_that_was_never_captured_is_refused() -> Result<()> {
    // Returning the links that happen to be present would read like the whole
    // story, which is worse than admitting the record is incomplete.
    let subject = object("obj-alpha");
    let trace = TraceId::new("trace-broken");
    let mut capture = OutcomeCapture::new();
    let first = Decision::new(
        DecisionId::from_string("dec-1"),
        trace.clone(),
        CorrelationId::from_string("cor-1"),
        day(5),
        subject.clone(),
        Action::RiskDecision {
            control: "gross-exposure".to_string(),
            allowed: true,
            reason: "inside the limit".to_string(),
        },
    )
    .from_event(EventId::from_string("evt-source"));
    capture.record(first, RealisedOutcome::nothing_happened(day(5)))?;

    let mut orphan = Decision::new(
        DecisionId::from_string("dec-2"),
        trace.clone(),
        CorrelationId::from_string("cor-1"),
        day(5),
        subject,
        Action::Cancelled {
            order_id: OrderId::from_string("ord-9"),
            reason: "the signal expired".to_string(),
        },
    );
    orphan.caused_by = Some(CausationId("dec-never-recorded".to_string()));
    capture.record(orphan, RealisedOutcome::nothing_happened(day(5)))?;

    let error = capture
        .reconstruct(&trace)
        .expect_err("a chain with a dangling parent reconstructed");
    assert!(error.message().contains("breaks at"), "{error}");
    assert!(error.message().contains("never captured"), "{error}");
    Ok(())
}

#[test]
fn an_outcome_capture_without_a_trace_id_is_refused() -> Result<()> {
    let subject = object("obj-alpha");
    let mut capture = OutcomeCapture::new();
    let mut untraceable = a_decision("dec-1", &subject);
    untraceable.trace = TraceId::new("   ");
    let error = capture
        .record(untraceable, RealisedOutcome::nothing_happened(day(5)))
        .expect_err("an untraceable outcome was captured");
    assert!(error.message().contains("trace id"), "{error}");
    Ok(())
}

#[test]
fn the_capture_chain_notices_an_edited_record() -> Result<()> {
    // The same discipline as the edge cell's journal, on the central side: a
    // record that can be altered without the digest moving is not a record.
    let subject = object("obj-alpha");
    let mut capture = OutcomeCapture::new();
    capture.record(
        a_decision("dec-1", &subject),
        RealisedOutcome::realised(day(10), dec!("1000"), dec!("50"), dec!("10000")),
    )?;
    capture.record(
        a_decision("dec-2", &subject).after(&a_decision("dec-1", &subject)),
        RealisedOutcome::realised(day(10), dec!("2000"), dec!("50"), dec!("10000")),
    )?;
    capture.verify()?;

    let mut json: serde_json::Value = serde_json::from_str(&serde_json::to_string(&capture)?)?;
    json["entries"][0]["outcome"]["realised_pnl"] = serde_json::Value::String("9999".to_string());
    let edited: OutcomeCapture = serde_json::from_value(json)?;
    let error = edited.verify().expect_err("an edited chain verified");
    assert!(error.message().contains("breaks its chain"), "{error}");
    Ok(())
}

#[test]
fn an_outcome_dated_before_its_decision_is_refused() -> Result<()> {
    let subject = object("obj-alpha");
    let mut capture = OutcomeCapture::new();
    let error = capture
        .record(
            a_decision("dec-1", &subject),
            RealisedOutcome::realised(day(1), dec!("1000"), Decimal::ZERO, dec!("10000")),
        )
        .expect_err("an outcome preceded its own decision");
    assert!(error.message().contains("was made at"), "{error}");
    Ok(())
}

// --- the learning record ----------------------------------------------------

fn a_record() -> LearningRecord {
    LearningRecord {
        decision_id: DecisionId::from_string("dec-1"),
        trace: TraceId::new("trace-alpha"),
        at: day(5),
        object_id: object("obj-alpha"),
        market: MarketState {
            regime: "trending".to_string(),
            volatility_f64: 0.0134,
            spread_bps_f64: 2.5,
            reference_price: dec!("108"),
            venue_status: "open".to_string(),
        },
        world: WorldState {
            factors: BTreeMap::from([
                ("policy_rate_expectation".to_string(), 0.0425),
                ("positioning_percentile".to_string(), 0.81),
            ]),
            narrative: "a hawkish repricing into a thin week".to_string(),
        },
        features: vec![
            ("basis.z".to_string(), 41),
            ("flow.imbalance".to_string(), 9),
        ],
        sources: vec!["venue-a".to_string(), "reference-data".to_string()],
        model_versions: vec![("carry".to_string(), "2025.01".to_string())],
        agents: vec![AgentOutput {
            agent: "macro".to_string(),
            stance: "for".to_string(),
            confidence: Conviction::new(0.68, 190),
        }],
        strategy: StrategyId::new("carry"),
        decision: "filled".to_string(),
        side: BookSide::Bid,
        position_size: dec!("10000"),
        execution_method: "participation".to_string(),
        cost: dec!("4500.125"),
        fill: Some(FillSummary {
            quantity: dec!("10000"),
            price: dec!("108.25"),
            venue: VenueId::new("XTST"),
            at: day(5),
        }),
        market_outcome: MarketOutcome {
            horizon: Duration::from_days(5),
            return_f64: 0.0926,
            exit_price: dec!("118"),
        },
        realised_pnl: dec!("95000.875"),
        pnl_by_source: BTreeMap::new(),
        counterfactuals: vec![CounterfactualSummary {
            alternative: "do_not_trade".to_string(),
            difference: Simulated::of(dec!("-95000.875")),
            fill: "not_traded".to_string(),
        }],
        risk: RiskSummary {
            var_f64: 41_200.0,
            exposure: dec!("1080000"),
            limit_used_f64: 0.36,
        },
        latency: Duration::from_millis(240),
        confidence: Conviction::new(0.71, 240),
    }
}

#[test]
fn a_learning_record_round_trips_through_json_exactly() -> Result<()> {
    let record = a_record();
    let restored = LearningRecord::from_json(&record.to_json()?)?;
    assert_eq!(record, restored);
    // And the counterfactual inside it is still marked as one.
    let json = record.to_json()?;
    assert!(json.contains("\"simulated\":true"), "{json}");
    Ok(())
}

#[test]
fn a_record_stamped_with_the_point_in_time_sentinel_is_refused() -> Result<()> {
    // `Timestamp::MAX` means "no upper bound" to a point-in-time read. It is
    // not an instant anything happened at, and it does not survive the
    // millisecond precision the timestamp serializes with.
    let mut record = a_record();
    record.at = Timestamp::MAX;
    let error = record.to_json().expect_err("Timestamp::MAX was written");
    assert_eq!(error.code(), "schema");
    assert!(error.message().contains("sentinel"), "{error}");

    // The failure it is standing in for, demonstrated: the sentinel does not
    // come back as itself.
    let round_tripped: Timestamp = serde_json::from_str(&serde_json::to_string(&Timestamp::MAX)?)?;
    assert_ne!(round_tripped, Timestamp::MAX);
    Ok(())
}

#[test]
fn a_record_with_sub_millisecond_precision_is_refused() -> Result<()> {
    let mut record = a_record();
    record.at = Timestamp::from_nanos(day(5).as_nanos() + 1);
    let error = record
        .to_json()
        .expect_err("a sub-millisecond instant was written");
    assert!(error.message().contains("truncate"), "{error}");
    Ok(())
}

#[test]
fn a_record_without_a_trace_id_is_refused() -> Result<()> {
    let mut record = a_record();
    record.trace = TraceId::new("");
    let error = record.to_json().expect_err("an untraceable record");
    assert!(error.message().contains("trace id"), "{error}");
    Ok(())
}

#[test]
fn a_records_pnl_decomposition_comes_from_the_learn_stages_attribution() -> Result<()> {
    // The composition point. The twin remembers the decomposition the LEARN
    // stage computed; it does not compute a second one, because two
    // decompositions of the same P&L eventually disagree and the one that gets
    // believed is whichever was read last.
    let attribution = PositionAttribution {
        object_id: "obj-alpha".to_string(),
        hypotheses: vec!["hyp-carry".to_string()],
        components: BTreeMap::from([
            (Source::MarketMove.as_str().to_string(), dec!("100000")),
            (Source::Commission.as_str().to_string(), dec!("-1000")),
            (Source::Spread.as_str().to_string(), dec!("-3000")),
            (Source::MarketImpact.as_str().to_string(), dec!("-1000")),
        ]),
        total: dec!("95000"),
        average_notional: dec!("1080000"),
    };

    let record = a_record().with_attribution(&attribution);
    assert_eq!(record.realised_pnl, dec!("95000"));
    assert_eq!(
        record.pnl_by_source.get(Source::MarketMove.as_str()),
        Some(&dec!("100000"))
    );
    let restored = LearningRecord::from_json(&record.to_json()?)?;
    assert_eq!(record, restored);
    Ok(())
}

#[test]
fn a_decomposition_that_does_not_add_up_is_refused() -> Result<()> {
    // The exactness the attribution itself insists on, carried into the corpus.
    let mut record = a_record();
    record.realised_pnl = dec!("95000");
    record.pnl_by_source = BTreeMap::from([
        (Source::MarketMove.as_str().to_string(), dec!("100000")),
        (Source::Commission.as_str().to_string(), dec!("-1000")),
    ]);
    let error = record
        .to_json()
        .expect_err("a decomposition that misses four thousand was written");
    assert!(error.message().contains("nearly adds up"), "{error}");
    Ok(())
}
