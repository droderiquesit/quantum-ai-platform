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
fn the_reachable_value_within_a_number_of_days_counts_only_the_holdings_that_clear_it() {
    let ladder = book();

    // Premise: the book totals 1.5m across five rungs.
    assert_eq!(
        ladder.total_value().expect("the book adds up"),
        dec!("1500000")
    );

    // Stated in days, because every limit that reads this is. None of these
    // holdings carries a measurement of its own, so each answers with its
    // rung's floor — the one case where the bucket boundary is the best
    // available lower bound rather than a replacement for a better one.
    //
    // The boundaries are named rather than looped, because they are the whole
    // question. Five days must reach the rungs that exit in days and stop
    // short of the one that floors at a month; that case used to be a test of
    // `LiquidationHorizon::deepest_within`, the mapping that turned a limit's
    // days into a bucket, and it is asserted here on the function that now
    // answers directly so that removing the mapping did not remove the
    // boundary.
    for (days, expected, why) in [
        (
            0.0,
            dec!("300000"),
            "cash and liquid spot are both inside a day",
        ),
        (1.0, dec!("700000"), "one day reaches listed equity"),
        (2.0, dec!("1000000"), "two days reaches the corporate bond"),
        (
            5.0,
            dec!("1000000"),
            "five days reaches everything that exits in days and no more",
        ),
        (
            29.0,
            dec!("1000000"),
            "and not the private-credit rung, which floors at a month",
        ),
        (
            30.0,
            dec!("1500000"),
            "thirty days is exactly the private-credit floor, and the boundary is inclusive",
        ),
    ] {
        assert_eq!(
            ladder.reachable_within(days).expect("the book adds up"),
            expected,
            "at {days} days: {why}"
        );
    }
}

#[test]
fn a_holdings_own_stated_exit_time_beats_the_boundary_of_its_rungs_bucket() {
    // The failure this prevents, reproduced against the tree before the fix:
    // `Rung::classify` puts every non-negotiated holding stated at more than
    // one day on `BondsAndLessLiquidListed`, whose horizon floors at two days.
    // A holding the record stated at forty-five days therefore reported two,
    // counted toward the fraction exitable within a week, and passed a
    // `MaxDaysToLiquidate { limit: 10.0 }` ceiling. The record held the right
    // number and the bucket boundary discarded it.
    let ladder = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-slog",
            Rung::BondsAndLessLiquidListed,
            dec!("1000"),
            dec!("25"),
        )
        .exiting_over_days(45.0),
        LadderEntry::new("obj-usd", Rung::CashAtVenue, dec!("1000"), Decimal::ZERO),
    ])
    .expect("the ladder assembles");

    // The premise: the holding is on the rung whose floor is two days, so a
    // reading that answered 2.0 would be reading the rung.
    let slog = ladder
        .entries()
        .find(|entry| entry.object_id == "obj-slog")
        .expect("the holding is on the ladder");
    assert_eq!(slog.rung, Rung::BondsAndLessLiquidListed);
    assert!(
        (slog.rung.horizon().least_days() - 2.0).abs() < f64::EPSILON,
        "the premise moved: the rung's own floor is no longer two days"
    );

    assert!(
        (slog.days_to_exit() - 45.0).abs() < f64::EPSILON,
        "the record says forty-five days and the ladder answered {}",
        slog.days_to_exit()
    );
    assert_eq!(
        ladder.reachable_within(5.0).expect("the book adds up"),
        dec!("1000"),
        "only the cash is exitable within a week; the forty-five-day holding is not"
    );
    // And the other half: a horizon that does reach it counts it, so the
    // filter is a judgement about the holding rather than a refusal of it.
    assert_eq!(
        ladder.reachable_within(45.0).expect("the book adds up"),
        dec!("2000"),
        "at forty-five days the whole book is reachable"
    );
}

#[test]
fn a_holding_measured_faster_than_its_rung_is_still_held_to_the_rungs_floor() {
    // The other direction, and the reason `days_to_exit` takes a maximum
    // rather than preferring the measurement. A private-credit holding whose
    // record claims a same-day exit is claiming something its rung says
    // cannot happen; the ladder answers the rung's floor, so a measurement
    // cannot be used to talk a holding up the ladder.
    let ladder = LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-pc",
            Rung::PrivateCreditAndRealAssets,
            dec!("1000"),
            dec!("100"),
        )
        .exiting_over_days(0.5),
    ])
    .expect("the ladder assembles");
    let entry = ladder.entries().next().expect("one holding");
    assert!(
        (entry.days_to_exit() - 30.0).abs() < f64::EPSILON,
        "a private-credit holding answered {} days",
        entry.days_to_exit()
    );
    assert_eq!(
        ladder.reachable_within(5.0).expect("the book adds up"),
        Decimal::ZERO,
        "nothing on the private-credit rung is exitable within a week"
    );
}

#[test]
fn an_exit_time_that_is_not_a_number_of_days_is_refused_rather_than_carried() {
    // A `NaN` measurement would make `days_to_exit` fall back to the rung's
    // floor, which is the defect the measurement exists to close, wearing the
    // measurement's own clothes. The premise first: the same entry with a
    // readable figure assembles.
    assert!(
        LiquidityLadder::new(vec![
            LadderEntry::new(
                "obj-a",
                Rung::ListedEquityAndFutures,
                dec!("100"),
                dec!("1")
            )
            .exiting_over_days(3.0)
        ])
        .is_ok(),
        "the premise failed: a readable exit time was refused too"
    );
    for days in [f64::NAN, f64::INFINITY, -1.0] {
        let refusal = LiquidityLadder::new(vec![
            LadderEntry::new(
                "obj-a",
                Rung::ListedEquityAndFutures,
                dec!("100"),
                dec!("1"),
            )
            .exiting_over_days(days),
        ])
        .expect_err("an unreadable exit time is refused");
        assert!(
            refusal.message().contains("obj-a") && refusal.message().contains("days to exit"),
            "the refusal names neither the holding nor the figure: {}",
            refusal.message()
        );
    }
}

#[test]
fn a_horizon_that_is_not_a_number_of_days_is_refused_rather_than_floored() {
    // A limit configured with a nonsense horizon must read as unevaluated,
    // not as one every book passed. Flooring at zero would be the clamping
    // the platform forbids, and it would produce a real fraction from a
    // horizon nobody stated. This property used to sit on
    // `LiquidationHorizon::deepest_within`, which mapped a limit's days onto
    // a bucket; the mapping is gone, and the refusal moved to the function
    // that now takes the days.
    let ladder = book();
    // The premise and the admitting half: a real horizon is answered.
    assert_eq!(
        ladder.reachable_within(5.0).expect("the book adds up"),
        dec!("1000000")
    );
    for days in [-1.0, f64::NAN, f64::NEG_INFINITY, f64::INFINITY] {
        let refusal = ladder
            .reachable_within(days)
            .expect_err("a horizon that is not a number of days is refused");
        assert!(
            refusal.message().contains("not a horizon an exit can have"),
            "the refusal for {days} does not say what is wrong with it: {}",
            refusal.message()
        );
    }
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
    assert_eq!(
        ladder.total_value().expect("the book adds up"),
        dec!("200000")
    );
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

    // Premise: there is 100,000 of cash, so a 50,000 request need go no
    // deeper. Read off the rung rather than off a horizon: two rungs are
    // inside a day, so a day count cannot say what is on the cash rung alone.
    assert_eq!(
        ladder
            .value_by_rung()
            .expect("the book adds up")
            .get(&Rung::CashAtVenue)
            .copied(),
        Some(dec!("100000"))
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
    assert_eq!(
        ladder.total_value().expect("the book adds up"),
        dec!("1500000")
    );
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

    assert_eq!(
        Rung::classify(AssetClass::Cash, &listed).expect("a stated number of days places a rung"),
        Rung::CashAtVenue
    );
    assert_eq!(
        Rung::classify(AssetClass::DigitalAsset, &listed)
            .expect("a stated number of days places a rung"),
        Rung::LiquidSpotAndPerpetual
    );
    assert_eq!(
        Rung::classify(AssetClass::Equity, &listed).expect("a stated number of days places a rung"),
        Rung::ListedEquityAndFutures
    );
    assert_eq!(
        Rung::classify(AssetClass::FixedIncome, &listed)
            .expect("a stated number of days places a rung"),
        Rung::BondsAndLessLiquidListed
    );
    assert_eq!(
        Rung::classify(AssetClass::PrivateMarket, &listed)
            .expect("a stated number of days places a rung"),
        Rung::PrivateEquityCommitments
    );
}

#[test]
fn a_negotiated_instrument_cannot_sit_on_a_listed_rung_however_its_class_is_labelled() {
    // A private placement in an equity is still an equity by class. It does
    // not settle same-day, and a ladder that says it does understates how long
    // the book takes to become cash.
    let negotiated = LiquidityProfile::illiquid(90.0, 900.0);

    // Premise: the same class with a listed profile is a listed rung.
    assert_eq!(
        Rung::classify(
            AssetClass::Equity,
            &LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
        )
        .expect("a stated number of days places a rung"),
        Rung::ListedEquityAndFutures
    );

    assert_eq!(
        Rung::classify(AssetClass::Equity, &negotiated)
            .expect("a stated number of days places a rung"),
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
        Rung::classify(AssetClass::Equity, &fast).expect("a stated number of days places a rung"),
        Rung::ListedEquityAndFutures
    );

    assert_eq!(
        Rung::classify(AssetClass::Equity, &slow).expect("a stated number of days places a rung"),
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
        LiquidityProfile::illiquid(30.0, 900.0),
        LiquidityProfile::default(),
    ];
    // Premise: this really does cover every class.
    assert_eq!(AssetClass::ALL.len(), 13);

    for class in AssetClass::ALL {
        for profile in &profiles {
            assert_ne!(
                Rung::classify(class, profile).expect("a stated number of days places a rung"),
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
// `MaxDaysToLiquidate` both do — and the ladder's rungs are categorical.
// There is now **one** comparison rather than a mapping at each end: a limit's
// days go to `reachable_within` as days, and each holding answers with its own
// `days_to_exit`. There used to be a `LiquidationHorizon::deepest_within` that
// turned a limit's days into a bucket, and the two mappings agreed with each
// other while both disagreed with the record: a holding stated at forty-five
// days answered two.

#[test]
fn every_horizons_floor_in_days_rises_with_the_horizon() {
    // Why this still matters with the measurement carried: `days_to_exit`
    // falls back to the rung's floor where nobody measured, so a pair that
    // went the other way would let a deeper rung report a shorter exit for
    // exactly the holdings nothing is known about.
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
fn every_rungs_own_floor_reaches_that_rung() {
    // One rule rather than two: asking the ladder for a rung's own floor in
    // days must include that rung, or a holding would be excluded from the
    // very horizon its rung defines. It held across two functions before, and
    // it has to keep holding now that there is one.
    for rung in Rung::ALL {
        let ladder = LiquidityLadder::new(vec![LadderEntry::new(
            "obj-one",
            rung,
            dec!("100"),
            Decimal::ZERO,
        )])
        .expect("a single-rung ladder is monotonic");
        assert_eq!(
            ladder
                .reachable_within(rung.horizon().least_days())
                .expect("the book adds up"),
            dec!("100"),
            "{} sits on horizon {} and its own floor of {} days did not reach it",
            rung.as_str(),
            rung.horizon().as_str(),
            rung.horizon().least_days()
        );
    }
}

// --- refusals the arithmetic used to swallow ---------------------------------

#[test]
fn a_liquidity_record_that_is_not_a_number_of_days_is_refused_rather_than_classified() {
    // The failure this prevents, confirmed against the tree before it was
    // fixed. `days_to_liquidate` is an `f64` and the comparison that pushes a
    // holding below its asset class is `> 1.0`, which `f64` makes `false` for
    // `NaN` and for a negative and `true` for infinity. So an equity whose
    // exit time nobody had measured classified `ListedEquityAndFutures` —
    // same day — and one stated to take *forever* classified
    // `BondsAndLessLiquidListed`, two days. Both figures reach
    // `RiskState::liquidatable_within` and therefore `LimitKind::MinLiquidity`,
    // which vetoes new risk, so an unreadable measurement was a liquidity
    // floor computed over a book nobody had measured.
    //
    // The premise, first and deliberately: a real number of days does push the
    // holding down, so what is refused below is the figure and not the field.
    let measured = LiquidityProfile {
        days_to_liquidate: 30.0,
        ..LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0)
    };
    assert_eq!(
        Rung::classify(AssetClass::Equity, &measured).expect("thirty days is a number of days"),
        Rung::BondsAndLessLiquidListed,
        "the premise failed: the classifier is not reading days_to_liquidate at all"
    );

    // Refused — not merely landed on a different rung. A rung assertion would
    // pass on a classifier that had quietly picked the bottom of the ladder,
    // which is still a number nobody computed.
    for (label, days) in [
        ("not a number", f64::NAN),
        ("infinite", f64::INFINITY),
        ("negatively infinite", f64::NEG_INFINITY),
        ("negative", -7.0),
    ] {
        let unreadable = LiquidityProfile {
            days_to_liquidate: days,
            ..LiquidityProfile::listed(Decimal::from_int(1_000_000), 5.0)
        };
        for class in AssetClass::ALL {
            let refusal = match Rung::classify(class, &unreadable) {
                Ok(rung) => panic!(
                    "a {label} days-to-liquidate on a {} was classified {rung:?} instead of \
                     refused",
                    class.as_str()
                ),
                Err(error) => error,
            };
            let message = refusal.message();
            assert!(
                message.contains("days to liquidate"),
                "the refusal does not name the figure it could not read: {message}"
            );
            assert!(
                message.contains("correct the record"),
                "the refusal does not say what to do instead: {message}"
            );
        }
    }
}

#[test]
fn a_book_whose_value_leaves_the_decimal_range_is_refused_rather_than_under_reported() {
    // The failure this prevents, measured on the tree before it was fixed: a
    // ladder of one unit of cash beside 1.7e29 of liquid spot reported a
    // `total_value` of **1** and a `reachable_within(Seconds)` of **1**. The
    // fold kept its accumulator on overflow — `checked_add(...).unwrap_or(acc)`
    // — so the entry that did not fit was silently dropped. That total is the
    // denominator `RiskState::liquidatable_within` divides by and the
    // numerator it files under the horizon `LimitKind::MinLiquidity` reads, so
    // the clamp sat inside a control that vetoes trading.
    //
    // Premise: the same two-rung shape with representable marks is admitted
    // and totals exactly their sum, so the refusal below is about the range
    // and not about the shape.
    let representable = LiquidityLadder::new(vec![
        LadderEntry::new("obj-usd", Rung::CashAtVenue, Decimal::ONE, Decimal::ZERO),
        LadderEntry::new(
            "obj-btc",
            Rung::LiquidSpotAndPerpetual,
            dec!("1000"),
            Decimal::ZERO,
        ),
    ])
    .expect("the premise failed: a two-rung book of ordinary size must be admitted");
    assert_eq!(
        representable
            .total_value()
            .expect("the premise failed: an ordinary book must add up"),
        dec!("1001")
    );

    // The same book with the lower rung marked at the top of the range. Its
    // per-rung totals each fit; their sum does not.
    let refusal = LiquidityLadder::new(vec![
        LadderEntry::new("obj-usd", Rung::CashAtVenue, Decimal::ONE, Decimal::ZERO),
        LadderEntry::new(
            "obj-btc",
            Rung::LiquidSpotAndPerpetual,
            Decimal::MAX,
            Decimal::ZERO,
        ),
    ])
    .expect_err("a book whose value does not add up was admitted");
    let message = refusal.message();
    assert!(
        message.contains("total value leaves the decimal range"),
        "the refusal does not name what could not be computed: {message}"
    );
    assert!(
        message.contains("obj-btc"),
        "the refusal does not name the holding the sum failed at: {message}"
    );
}

#[test]
fn a_rung_total_that_leaves_the_decimal_range_is_refused_rather_than_saturated() {
    // The other half of the same clamp, and the worse half: the per-rung
    // totals saturated at `Decimal::MAX` instead of dropping an entry, and
    // `prove_monotonic` then compared that fabricated total against a real one
    // and pronounced the ladder sound. A ladder is admitted on the strength of
    // that proof.
    //
    // Premise: two holdings of ordinary size on one rung are admitted and the
    // rung reports their sum.
    let representable = LiquidityLadder::new(vec![
        LadderEntry::new("obj-usd", Rung::CashAtVenue, dec!("400"), Decimal::ZERO),
        LadderEntry::new("obj-eur", Rung::CashAtVenue, dec!("600"), Decimal::ZERO),
    ])
    .expect("the premise failed: two ordinary holdings on one rung must be admitted");
    assert_eq!(
        representable
            .value_by_rung()
            .expect("the premise failed: an ordinary rung must add up")
            .get(&Rung::CashAtVenue)
            .copied(),
        Some(dec!("1000")),
        "the premise failed: the rung total is not the sum of its holdings"
    );

    let refusal = LiquidityLadder::new(vec![
        LadderEntry::new("obj-usd", Rung::CashAtVenue, Decimal::ONE, Decimal::ZERO),
        LadderEntry::new("obj-eur", Rung::CashAtVenue, Decimal::MAX, Decimal::ZERO),
    ])
    .expect_err("a rung whose value does not add up was admitted");
    let message = refusal.message();
    assert!(
        message.contains("value on rung cash_at_venue"),
        "the refusal does not name the rung whose total could not be computed: {message}"
    );
    assert!(
        message.contains("monotonicity proof"),
        "the refusal does not say why a saturated rung total matters: {message}"
    );
}

/// A catalogue whose quotes widen as the ladder descends can be held together.
///
/// The admitting half, and it is the half that matters most for this gate:
/// `prove_quotes_can_coexist` refuses `Platform::new`, so a version of it that
/// refused every catalogue would be an outage rather than a control, and a
/// suite that only proved the refusal could not tell the two apart.
///
/// Deliberately includes two records on one rung quoted differently, and a
/// pair quoted *identically* across adjacent rungs: monotonicity is
/// `cost does not fall`, so equal rates coexist, and a gate written with `>=`
/// would refuse an ordinary catalogue that prices two rungs the same.
#[test]
fn reference_records_whose_quotes_widen_as_the_ladder_descends_can_coexist() {
    qip_financial::ladder::prove_quotes_can_coexist([
        ("obj-usd", Rung::CashAtVenue, 0.0),
        ("obj-btc", Rung::LiquidSpotAndPerpetual, 5.0),
        ("obj-aapl", Rung::ListedEquityAndFutures, 20.0),
        ("obj-smallcap", Rung::ListedEquityAndFutures, 300.0),
        ("obj-corp", Rung::BondsAndLessLiquidListed, 300.0),
        ("obj-credit", Rung::PrivateCreditAndRealAssets, 900.0),
    ])
    .expect("a catalogue whose quotes widen as the ladder descends was refused");
}

/// A record quoted wider than one on a rung beneath it is refused, and the
/// refusal names both.
///
/// The failure this prevents was live and it inverted a control. A listed name
/// quoted at 300bps beside a negotiated holding whose profile asserted 250
/// assembled in silence, and then `LiquidityLadder::new` refused the whole book
/// every cycle — and because that refusal was made to fail closed, one ordinary
/// small-cap stopped the desk trading anything at all, telling the operator
/// that a ladder was not monotonic. The property is relational: neither record
/// is wrong alone, which is why both are named.
#[test]
fn a_record_quoted_wider_than_one_on_a_rung_beneath_it_is_refused_naming_both() {
    // The premise, against this exact pair: each record is admissible on its
    // own, so the refusal below is about the pair and not about either figure.
    for lone in [
        ("obj-smallcap", Rung::ListedEquityAndFutures, 300.0),
        ("obj-credit", Rung::PrivateCreditAndRealAssets, 250.0),
    ] {
        qip_financial::ladder::prove_quotes_can_coexist([lone])
            .expect("the premise failed: a single record was refused on its own");
    }

    let refusal = qip_financial::ladder::prove_quotes_can_coexist([
        ("obj-smallcap", Rung::ListedEquityAndFutures, 300.0),
        ("obj-credit", Rung::PrivateCreditAndRealAssets, 250.0),
    ])
    .expect_err("two records whose exit costs invert the ladder were admitted");
    let message = refusal.message();
    assert!(
        message.contains("obj-smallcap") && message.contains("obj-credit"),
        "the refusal must name both records; neither can be corrected without the other: \
         {message}"
    );
    assert!(
        message.contains("rung listed_equity_and_futures quoted"),
        "the refusal does not name the rung the wider record sits on: {message}"
    );
    assert!(
        message.contains("lower rung private_credit_and_real_assets at"),
        "the refusal does not name the rung beneath: {message}"
    );
    // Delimited on both sides. `250bps` is a substring of `1250bps`, and a
    // test in this repository has already passed a mutation that deleted the
    // exact value it was written to protect for that reason.
    assert!(
        message.contains("at 300bps,") && message.contains("at 250bps;"),
        "the refusal does not state both rates: {message}"
    );

    // And the pair really would have taken a book down, which is the reason
    // this gate exists rather than a claim about it. The same two rates on the
    // same two rungs, as one holding each.
    LiquidityLadder::new(vec![
        LadderEntry::new(
            "obj-smallcap",
            Rung::ListedEquityAndFutures,
            dec!("10000"),
            dec!("300"),
        ),
        LadderEntry::new(
            "obj-credit",
            Rung::PrivateCreditAndRealAssets,
            dec!("10000"),
            dec!("250"),
        ),
    ])
    .expect_err(
        "the premise failed: this pair does not actually break the ladder, so the assembly \
         refusal above is refusing something harmless",
    );
}

/// A rung is judged by its widest and its tightest quote, not by whichever
/// record happened to arrive first.
///
/// A rung holding several records has a value-weighted rate, so the book that
/// breaks the ladder is the one holding the widest above and the tightest
/// below. A check that compared, say, the first record of each rung would
/// admit a catalogue that a book drawable from it refuses — which is the exact
/// shape of the defect this gate closes, one level down.
#[test]
fn a_rung_is_judged_by_its_widest_and_its_tightest_quote_and_not_by_one_record() {
    // The tight record on the upper rung is listed first; only `obj-wide`
    // inverts, and only against `obj-tight-below`, which is listed last.
    let quotes = [
        ("obj-narrow", Rung::ListedEquityAndFutures, 5.0),
        ("obj-wide", Rung::ListedEquityAndFutures, 300.0),
        ("obj-loose-below", Rung::PrivateCreditAndRealAssets, 900.0),
        ("obj-tight-below", Rung::PrivateCreditAndRealAssets, 250.0),
    ];
    let refusal = qip_financial::ladder::prove_quotes_can_coexist(quotes)
        .expect_err("a rung was judged by one of its records rather than by its widest");
    assert!(
        refusal.message().contains("obj-wide") && refusal.message().contains("obj-tight-below"),
        "the refusal names the wrong pair of records: {}",
        refusal.message()
    );

    // The same four records in the reverse order name the same pair. A refusal
    // a replay could attribute to a different record is not a replay.
    let mut reversed = quotes;
    reversed.reverse();
    let again = qip_financial::ladder::prove_quotes_can_coexist(reversed)
        .expect_err("the reversed catalogue was admitted");
    assert_eq!(
        again.message(),
        refusal.message(),
        "the same catalogue in a different order named a different pair"
    );
}
