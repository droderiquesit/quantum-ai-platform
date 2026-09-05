//! The liquidity ladder: rung ordering, its refusals, and what a request for
//! cash would actually cost.
//!
//! The ladder's whole value is that serving from the top downward is serving
//! from the cheapest downward. Most of what follows tests the refusals that
//! keep that true, because a ladder whose rungs are assigned wrongly is worse
//! than no ladder: it reports a cheap plan while naming the expensive source.

use qip_core::{Decimal, dec};
use qip_financial::asset_class::AssetClass;
use qip_financial::costs::LiquidityProfile;
use qip_financial::ladder::{LadderEntry, LiquidationHorizon, LiquidityLadder, Rung};

/// A book with something on five rungs, costs rising as it descends.
fn book() -> LiquidityLadder {
    LiquidityLadder::new(vec![
        // Cash: immediate, zero cost.
        LadderEntry::new("obj-usd", Rung::CashAtVenue, dec!("100000"), Decimal::ZERO),
        // Liquid spot: 5bp.
        LadderEntry::new(
            "obj-btc",
            Rung::LiquidSpotAndPerpetual,
            dec!("200000"),
            dec!("100"),
        ),
        // Listed equity: 20bp.
        LadderEntry::new(
            "obj-aapl",
            Rung::ListedEquityAndFutures,
            dec!("400000"),
            dec!("800"),
        ),
        // Corporate bond: 100bp.
        LadderEntry::new(
            "obj-corp",
            Rung::BondsAndLessLiquidListed,
            dec!("300000"),
            dec!("3000"),
        ),
        // Private credit: 1000bp.
        LadderEntry::new(
            "obj-pc",
            Rung::PrivateCreditAndRealAssets,
            dec!("500000"),
            dec!("50000"),
        ),
    ])
    .unwrap()
}

// --- the ladder's shape ------------------------------------------------------

#[test]
fn the_rungs_are_declared_top_first_so_their_ordering_is_the_ladders_ordering() {
    // Premise: there are seven rungs, which is what blueprint 25.4 names.
    assert_eq!(Rung::ALL.len(), 7);

    // The derived Ord must agree with the declared depth, because a BTreeMap
    // keyed on a rung iterates by Ord and that ordering reaches every plan.
    for pair in Rung::ALL.windows(2) {
        let (higher, lower) = (pair[0], pair[1]);
        assert!(
            higher < lower,
            "{} must sort above {}",
            higher.as_str(),
            lower.as_str()
        );
        assert!(
            higher.depth() < lower.depth(),
            "{} must be shallower than {}",
            higher.as_str(),
            lower.as_str()
        );
        assert!(
            higher.horizon() <= lower.horizon(),
            "the horizon must not shorten as the ladder descends"
        );
    }

    assert_eq!(Rung::CashAtVenue.horizon(), LiquidationHorizon::Immediate);
    assert_eq!(
        Rung::PrivateEquityCommitments.horizon(),
        LiquidationHorizon::Years
    );
}

#[test]
fn entries_come_back_in_ladder_order_whatever_order_they_were_supplied_in() {
    // Supplied bottom-up on purpose: a replay that reorders is not a replay,
    // and the ordering must be the type's property rather than the caller's.
    let ladder = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-pe",
            Rung::PrivateEquityCommitments,
            dec!("1000"),
            dec!("200"),
        ),
        LadderEntry::new(
            "obj-aapl",
            Rung::ListedEquityAndFutures,
            dec!("1000"),
            dec!("2"),
        ),
        LadderEntry::new("obj-usd", Rung::CashAtVenue, dec!("1000"), Decimal::ZERO),
    ])
    .unwrap();

    let order: Vec<&str> = ladder.entries().map(|e| e.object_id.as_str()).collect();
    // Premise: three entries went in, so there is an order to check.
    assert_eq!(order.len(), 3);
    assert_eq!(order, vec!["obj-usd", "obj-aapl", "obj-pe"]);
}

#[test]
fn the_reachable_value_within_a_horizon_counts_only_the_rungs_that_clear_it() {
    let ladder = book();

    // Premise: the book totals 1.5m across five rungs.
    assert_eq!(ladder.total_value(), dec!("1500000"));

    assert_eq!(
        ladder.reachable_within(LiquidationHorizon::Immediate),
        dec!("100000"),
        "only cash is immediate"
    );
    assert_eq!(
        ladder.reachable_within(LiquidationHorizon::Seconds),
        dec!("300000"),
        "cash plus liquid spot"
    );
    assert_eq!(
        ladder.reachable_within(LiquidationHorizon::SameDay),
        dec!("700000"),
        "plus listed equity"
    );
    assert_eq!(
        ladder.reachable_within(LiquidationHorizon::Days),
        dec!("1000000"),
        "plus the corporate bond"
    );
    assert_eq!(
        ladder.reachable_within(LiquidationHorizon::Years),
        dec!("1500000"),
        "everything, eventually"
    );
}

// --- construction refusals ---------------------------------------------------

#[test]
fn a_ladder_whose_lower_rung_is_cheaper_than_a_higher_one_is_refused_by_rung_name() {
    // The listed-equity rung priced at 500bp while the private-credit rung is
    // priced at 10bp: the rungs have been assigned wrongly, and every plan
    // built on this ladder would sell the expensive thing first while
    // reporting that it served from the top.
    let result = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-aapl",
            Rung::ListedEquityAndFutures,
            dec!("100000"),
            dec!("5000"),
        ),
        LadderEntry::new(
            "obj-pc",
            Rung::PrivateCreditAndRealAssets,
            dec!("100000"),
            dec!("100"),
        ),
    ]);

    let err = result.expect_err("a non-monotonic ladder must be refused");
    let message = err.message();
    assert!(
        message.contains("more expensive as it descends"),
        "the refusal must name the invariant, got: {message}"
    );
    // Delimited token, not a substring: "listed_equity_and_futures" must be the
    // rung named, and it is not a substring of any other rung's name.
    assert!(
        message.contains("listed_equity_and_futures"),
        "the refusal must name the higher rung, got: {message}"
    );
    assert!(
        message.contains("private_credit_and_real_assets"),
        "the refusal must name the lower rung, got: {message}"
    );
}

#[test]
fn a_ladder_whose_costs_rise_as_it_descends_is_admitted() {
    // The other half of the gate. A monotonicity check that refuses everything
    // is not a working gate, and the ladder above is the same shape as the one
    // refused, differing only in which rung is dearer.
    let ladder = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-aapl",
            Rung::ListedEquityAndFutures,
            dec!("100000"),
            dec!("100"),
        ),
        LadderEntry::new(
            "obj-pc",
            Rung::PrivateCreditAndRealAssets,
            dec!("100000"),
            dec!("5000"),
        ),
    ])
    .expect("a ladder that gets dearer as it descends must be admitted");
    assert_eq!(ladder.total_value(), dec!("200000"));
}

#[test]
fn equal_cost_rates_on_adjacent_rungs_are_admitted_because_the_ladder_only_forbids_a_reversal() {
    // A boundary worth pinning: the invariant is "does not get cheaper", not
    // "gets strictly dearer". Two rungs at the same rate is a flat step, not a
    // reversal, and refusing it would refuse an all-cash book.
    let ladder = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-a",
            Rung::ListedEquityAndFutures,
            dec!("100000"),
            dec!("500"),
        ),
        LadderEntry::new(
            "obj-b",
            Rung::BondsAndLessLiquidListed,
            dec!("200000"),
            dec!("1000"),
        ),
    ]);
    assert!(
        ladder.is_ok(),
        "identical cost rates on two rungs must be admitted"
    );
}

#[test]
fn the_same_holding_placed_on_two_rungs_is_refused_because_it_double_counts_the_book() {
    let result = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-aapl",
            Rung::ListedEquityAndFutures,
            dec!("100000"),
            dec!("100"),
        ),
        LadderEntry::new(
            "obj-aapl",
            Rung::BondsAndLessLiquidListed,
            dec!("100000"),
            dec!("500"),
        ),
    ]);

    let err = result.expect_err("a repeated holding must be refused");
    assert!(
        err.message().contains("appears twice"),
        "got: {}",
        err.message()
    );
    assert!(
        err.message().contains("obj-aapl"),
        "the refusal must name the holding, got: {}",
        err.message()
    );
}

#[test]
fn a_holding_that_costs_more_to_exit_than_it_is_marked_at_is_refused() {
    // Serving a request from such a holding would report raising cash while
    // destroying more than it raised.
    let result = LiquidityLadder::new(vec![LadderEntry::new(
        "obj-junk",
        Rung::PrivateCreditAndRealAssets,
        dec!("1000"),
        dec!("1500"),
    )]);

    let err = result.expect_err("a cost above the mark must be refused");
    assert!(
        err.message().contains("correct the mark or the cost model"),
        "the refusal must say what to do instead, got: {}",
        err.message()
    );
}

#[test]
fn a_non_positive_value_or_a_negative_cost_is_refused() {
    let zero_value = LiquidityLadder::new(vec![LadderEntry::new(
        "obj-dead",
        Rung::CashAtVenue,
        Decimal::ZERO,
        Decimal::ZERO,
    )]);
    assert!(
        zero_value
            .expect_err("a worthless holding has no rung")
            .message()
            .contains("not positive")
    );

    let negative_cost = LiquidityLadder::new(vec![LadderEntry::new(
        "obj-usd",
        Rung::CashAtVenue,
        dec!("1000"),
        dec!("-5"),
    )]);
    assert!(
        negative_cost
            .expect_err("a negative cost to liquidate must be refused")
            .message()
            .contains("negative cost to liquidate")
    );
}

// --- planning ----------------------------------------------------------------

#[test]
fn a_request_smaller_than_the_top_rung_is_served_from_cash_alone_at_no_cost() {
    let ladder = book();

    // Premise: there is 100,000 of cash, so a 50,000 request need go no deeper.
    assert_eq!(
        ladder.reachable_within(LiquidationHorizon::Immediate),
        dec!("100000")
    );

    let plan = ladder.plan(dec!("50000")).unwrap();
    assert_eq!(plan.legs.len(), 1, "one leg: cash");
    assert_eq!(plan.legs[0].object_id, "obj-usd");
    assert_eq!(plan.raised, dec!("50000"));
    assert_eq!(plan.cost, Decimal::ZERO, "cash costs nothing to spend");
    assert_eq!(plan.deepest_rung, Rung::CashAtVenue);
}

#[test]
fn a_request_larger_than_the_top_rungs_descends_and_reports_the_deepest_rung_reached() {
    let ladder = book();

    // 800,000: 100k cash + 200k spot + 400k equity + 100k of the bond.
    let plan = ladder.plan(dec!("800000")).unwrap();

    let drawn: Vec<(&str, Decimal)> = plan
        .legs
        .iter()
        .map(|l| (l.object_id.as_str(), l.amount))
        .collect();
    assert_eq!(
        drawn,
        vec![
            ("obj-usd", dec!("100000")),
            ("obj-btc", dec!("200000")),
            ("obj-aapl", dec!("400000")),
            ("obj-corp", dec!("100000")),
        ],
        "the plan must be served from the top downward"
    );
    assert_eq!(plan.raised, dec!("800000"));
    assert_eq!(
        plan.deepest_rung,
        Rung::BondsAndLessLiquidListed,
        "the operator's headline: this request reached the bond rung"
    );
    // It never touched private credit, which is the point of ordering.
    assert!(plan.legs.iter().all(|l| l.object_id != "obj-pc"));

    // 0 + 100 + 800 + a pro-rata third of 3000 = 1900.
    assert_eq!(plan.cost, dec!("1900"), "cost is charged leg by leg");
}

#[test]
fn a_partial_draw_on_a_holding_is_charged_pro_rata() {
    let ladder = LiquidityLadder::new(vec![LadderEntry::new(
        "obj-corp",
        Rung::BondsAndLessLiquidListed,
        dec!("300000"),
        dec!("3000"),
    )])
    .unwrap();

    // Premise: the whole holding costs 3000 to exit.
    assert_eq!(ladder.plan(dec!("300000")).unwrap().cost, dec!("3000"));

    // A quarter of it costs a quarter.
    let plan = ladder.plan(dec!("75000")).unwrap();
    assert_eq!(plan.legs.len(), 1);
    assert_eq!(plan.legs[0].amount, dec!("75000"));
    assert_eq!(plan.cost, dec!("750"));
}

#[test]
fn a_request_deeper_than_the_book_is_refused_and_names_the_shortfall() {
    let ladder = book();

    // Premise: the book holds exactly 1.5m, and a request for all of it works.
    assert_eq!(ladder.total_value(), dec!("1500000"));
    assert!(
        ladder.plan(dec!("1500000")).is_ok(),
        "a request for the whole book must be served"
    );

    // One more unit than exists. Serving this as far as it goes would read
    // downstream as a plan that succeeded.
    let err = ladder
        .plan(dec!("1500001"))
        .expect_err("a request exceeding the book must be refused");
    let message = err.message();
    assert!(
        message.contains("short"),
        "the refusal must name the shortfall, got: {message}"
    );
    assert!(
        message.contains("1500001") && message.contains("1500000"),
        "the refusal must name both the request and the book, got: {message}"
    );
}

#[test]
fn a_non_positive_request_is_refused_rather_than_answered_with_an_empty_plan() {
    let ladder = book();
    assert!(ladder.plan(Decimal::ZERO).is_err());
    assert!(
        ladder
            .plan(dec!("-1000"))
            .expect_err("a negative request must be refused")
            .message()
            .contains("positive amount of cash")
    );
}

#[test]
fn a_plan_carries_no_field_any_execution_surface_could_act_on() {
    // The ladder describes depth; it never places against it. If a venue, a
    // side or a time in force ever appears on a leg, this assertion is where
    // the reviewer is meant to stop and ask why. The paper-trading boundary
    // does not depend on this test, but the ladder's claim to be a valuation
    // instrument does.
    let plan = book().plan(dec!("150000")).unwrap();
    assert!(!plan.legs.is_empty(), "premise: there is a leg to inspect");

    let encoded = serde_json::to_string(&plan).unwrap();
    let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let leg = &decoded["legs"][0];
    let fields: Vec<&str> = leg
        .as_object()
        .expect("a leg is an object")
        .keys()
        .map(String::as_str)
        .collect();
    // `serde_json::Value` holds an object in a sorted map, so this is the
    // field set in name order rather than declaration order.
    assert_eq!(
        fields,
        vec!["amount", "cost", "object_id", "rung"],
        "a plan leg carries identity, rung, amount and cost — and nothing executable"
    );
}

// --- classification ----------------------------------------------------------

#[test]
fn an_instrument_lands_on_the_rung_its_class_and_its_liquidity_together_imply() {
    let listed = LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0);

    assert_eq!(Rung::classify(AssetClass::Cash, &listed), Rung::CashAtVenue);
    assert_eq!(
        Rung::classify(AssetClass::DigitalAsset, &listed),
        Rung::LiquidSpotAndPerpetual
    );
    assert_eq!(
        Rung::classify(AssetClass::Equity, &listed),
        Rung::ListedEquityAndFutures
    );
    assert_eq!(
        Rung::classify(AssetClass::FixedIncome, &listed),
        Rung::BondsAndLessLiquidListed
    );
    assert_eq!(
        Rung::classify(AssetClass::PrivateMarket, &listed),
        Rung::PrivateEquityCommitments
    );
}

#[test]
fn a_negotiated_instrument_cannot_sit_on_a_listed_rung_however_its_class_is_labelled() {
    // A private placement in an equity is still an equity by class. It does
    // not settle same-day, and a ladder that says it does understates how long
    // the book takes to become cash.
    let negotiated = LiquidityProfile::illiquid(90.0);

    // Premise: the same class with a listed profile is a listed rung.
    assert_eq!(
        Rung::classify(
            AssetClass::Equity,
            &LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
        ),
        Rung::ListedEquityAndFutures
    );

    assert_eq!(
        Rung::classify(AssetClass::Equity, &negotiated),
        Rung::PrivateCreditAndRealAssets,
        "a negotiated instrument is pushed down the ladder, never up"
    );
}

#[test]
fn an_instrument_taking_more_than_a_day_to_exit_falls_to_the_bond_rung() {
    let slow = LiquidityProfile {
        days_to_liquidate: 5.0,
        ..LiquidityProfile::listed(Decimal::from_int(1000), 3.0)
    };

    // Premise: at one day it is a listed rung.
    let fast = LiquidityProfile {
        days_to_liquidate: 1.0,
        ..LiquidityProfile::listed(Decimal::from_int(1000), 3.0)
    };
    assert_eq!(
        Rung::classify(AssetClass::Equity, &fast),
        Rung::ListedEquityAndFutures
    );

    assert_eq!(
        Rung::classify(AssetClass::Equity, &slow),
        Rung::BondsAndLessLiquidListed
    );
}

#[test]
fn classification_never_returns_the_resting_rung_because_no_instrument_property_reveals_it() {
    // A position rests because a strategy is anchoring it. That is a fact about
    // the strategy, not the instrument, and a classifier that guessed it would
    // put positions on a rung nobody chose.
    let profiles = [
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        LiquidityProfile::illiquid(30.0),
        LiquidityProfile::default(),
    ];
    // Premise: this really does cover every class.
    assert_eq!(AssetClass::ALL.len(), 13);

    for class in AssetClass::ALL {
        for profile in &profiles {
            assert_ne!(
                Rung::classify(class, profile),
                Rung::RestingAndAnchored,
                "{} must not be classified as resting",
                class.as_str()
            );
        }
    }
}

// --- horizons in days --------------------------------------------------------
//
// The risk domain states its liquidity controls in days — `MinLiquidity` and
// `MaxDaysToLiquidate` both do — and the ladder's horizons are categorical.
// One rule translates, in both directions, so a book sitting exactly on a
// horizon cannot be inside it by one function and outside it by the other.

#[test]
fn every_horizons_floor_in_days_rises_with_the_horizon() {
    // The property that makes a floor safe to read into a ceiling limit: it
    // never says an exit is faster than the horizon admits. A pair that went
    // the other way would let a deeper rung report a shorter exit, and
    // `MaxDaysToLiquidate` would pass the holding it exists to catch.
    let horizons = [
        LiquidationHorizon::Immediate,
        LiquidationHorizon::Seconds,
        LiquidationHorizon::SameDay,
        LiquidationHorizon::Days,
        LiquidationHorizon::Months,
        LiquidationHorizon::Years,
    ];
    // The premise: the list is the whole enum, in ladder order.
    assert_eq!(horizons.len(), 6);
    for window in horizons.windows(2) {
        assert!(
            window[0].least_days() <= window[1].least_days(),
            "{} floors at {} days but the deeper {} floors at {}",
            window[0].as_str(),
            window[0].least_days(),
            window[1].as_str(),
            window[1].least_days()
        );
    }
    // And it is not constant, which a monotonicity assertion alone permits.
    assert!(
        LiquidationHorizon::Years.least_days() > LiquidationHorizon::Immediate.least_days(),
        "a floor that never moves would let every rung answer every limit"
    );
}

#[test]
fn the_deepest_horizon_within_a_day_count_is_the_one_whose_floor_still_clears_it() {
    // The mapping the kernel's liquidity floor turns a limit's `days` into.
    // Named cases rather than a loop, because the boundaries are the whole
    // question: a limit at five days must reach the `Days` rungs and stop
    // short of `Months`.
    assert_eq!(
        LiquidationHorizon::deepest_within(5.0),
        Some(LiquidationHorizon::Days),
        "a five-day floor reaches everything that exits in days"
    );
    assert_eq!(
        LiquidationHorizon::deepest_within(1.0),
        Some(LiquidationHorizon::SameDay),
        "one day reaches the same-day rung and no further"
    );
    assert_eq!(
        LiquidationHorizon::deepest_within(0.0),
        Some(LiquidationHorizon::Seconds),
        "zero days is still seconds; `Seconds` and `Immediate` are the same floor and \
         `reachable_within(Seconds)` already counts the immediate rungs"
    );
    assert_eq!(
        LiquidationHorizon::deepest_within(29.0),
        Some(LiquidationHorizon::Days),
        "twenty-nine days does not reach a rung that floors at a month"
    );
    assert_eq!(
        LiquidationHorizon::deepest_within(30.0),
        Some(LiquidationHorizon::Months),
        "thirty days is exactly the month floor, and the boundary is inclusive"
    );
    assert_eq!(
        LiquidationHorizon::deepest_within(365.0),
        Some(LiquidationHorizon::Years),
        "a year reaches everything"
    );
}

#[test]
fn a_horizon_that_is_not_a_number_of_days_is_refused_rather_than_floored() {
    // A limit configured with a nonsense horizon must read as unevaluated, not
    // as one every book passed. Flooring at `Immediate` would have been the
    // clamping the platform forbids, and it would have produced a real
    // fraction from a horizon nobody stated.
    //
    // Infinity is refused with the rest. It would map to `Years` and read as
    // a horizon that reaches the whole book, which is the one answer a limit
    // set by a broken calculation must not silently get.
    for days in [-1.0, f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
        assert_eq!(
            LiquidationHorizon::deepest_within(days),
            None,
            "a horizon of {days} days was given a rung"
        );
    }
    // The premise the four cases above rest on: a real horizon does get one,
    // so `None` is a judgement about the input and not the function's only
    // answer.
    assert_eq!(
        LiquidationHorizon::deepest_within(5.0),
        Some(LiquidationHorizon::Days)
    );
}

#[test]
fn the_two_horizon_functions_agree_on_every_rungs_own_floor() {
    // The property that keeps one rule rather than two: taking a rung's floor
    // and asking which horizon that many days reaches must not land shallower
    // than the rung itself, or a holding would be excluded from the very
    // horizon its rung defines.
    for rung in Rung::ALL {
        let horizon = rung.horizon();
        let round_trip = LiquidationHorizon::deepest_within(horizon.least_days())
            .unwrap_or_else(|| panic!("{} floors at a real number of days", rung.as_str()));
        assert!(
            horizon <= round_trip,
            "{} sits on horizon {} but its own floor of {} days reaches only {}",
            rung.as_str(),
            horizon.as_str(),
            horizon.least_days(),
            round_trip.as_str()
        );
    }
}
