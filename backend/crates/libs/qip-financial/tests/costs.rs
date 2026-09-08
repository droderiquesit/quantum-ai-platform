//! The transaction cost model: the square-root impact law and its inverse.
//!
//! `TransactionCostModel::breakeven_participation` inverts the same law
//! `TransactionCostModel::impact_bps` prices, and the two must agree: a
//! participation rate the breakeven names as "where impact eats the alpha"
//! has to be a participation rate at which `impact_bps` actually reports that
//! much impact. Before this file existed, nothing in the crate exercised
//! `costs.rs` at all — `qip-capital`'s capacity sizing was the only caller of
//! `breakeven_participation`, and a defect here would have surfaced first as
//! a mis-sized position rather than a failing test naming the arithmetic.

// Exact float comparison is deliberate below: these assert that a refused or
// capped case yields exactly zero or exactly the cap, not merely something
// close to it.
#![allow(clippy::float_cmp)]

use qip_core::testing::approx_eq;
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};

/// A model with round numbers, so the arithmetic below is checkable by hand:
/// commission 1bp, no tax, half-spread 2.5bp, impact coefficient 40bp.
fn model() -> TransactionCostModel {
    TransactionCostModel::default()
}

#[test]
fn impact_follows_the_square_root_law_below_the_cap() {
    let m = model();
    // Premise: at 25% participation the naive square root is exactly 0.5,
    // comfortably below the 4.0 cap, so this checks the uncapped formula.
    let expected = 40.0 * 0.25_f64.sqrt();
    assert!(approx_eq(m.impact_bps(0.25), expected, 1e-9));
    assert!(approx_eq(expected, 20.0, 1e-9));
}

#[test]
fn impact_is_capped_at_four_times_participation() {
    let m = model();
    // Participation of 400% (four full days of volume in one) and anything
    // beyond it must report the same, capped, impact — not a climbing one.
    let at_cap = m.impact_bps(4.0);
    assert!(approx_eq(at_cap, 80.0, 1e-9), "impact at the cap: {at_cap}");
    assert!(approx_eq(m.impact_bps(9.0), at_cap, 1e-9));
    assert!(approx_eq(m.impact_bps(1_000.0), at_cap, 1e-9));
}

#[test]
fn zero_and_negative_participation_carry_no_impact() {
    let m = model();
    assert_eq!(m.impact_bps(0.0), 0.0);
    assert_eq!(m.impact_bps(-0.5), 0.0);
    assert_eq!(m.impact_bps(f64::NAN), 0.0);
    assert_eq!(m.impact_bps(f64::INFINITY), 0.0);
}

#[test]
fn breakeven_participation_is_where_impact_actually_reaches_the_budget() {
    let m = model();
    // Premise: this alpha budget, after subtracting the linear costs, is
    // small enough that the naive inverse-square lands below the 4.0 cap
    // (budget = 30 - 1 - 0 - 2.5 = 26.5; sqrt-domain check: 26.5 < 80).
    let alpha_bps = 30.0;
    let budget = alpha_bps - m.commission_bps - m.tax_bps - m.half_spread_bps;
    assert!(
        budget < 2.0 * m.impact_coefficient_bps,
        "fixture must stay under the cap boundary: budget {budget}"
    );

    let breakeven = m.breakeven_participation(alpha_bps);
    // The defining property: feeding the reported breakeven back into the
    // impact function must reproduce the budget it claims to exhaust.
    assert!(
        approx_eq(m.impact_bps(breakeven), budget, 1e-6),
        "impact at the reported breakeven ({}) is {}, not the budget {budget}",
        breakeven,
        m.impact_bps(breakeven)
    );
}

/// The mutation this guards: dropping the cap check from
/// `breakeven_participation` makes it return `(budget/coeff)^2` unconditionally,
/// which for a large alpha names a participation several multiples of a full
/// day's volume — a figure `impact_bps` itself never prices, because it caps
/// modelled impact at participation 4.0. This is exactly the inconsistency
/// `qip-capital`'s capacity sizing would have inherited silently.
#[test]
fn breakeven_participation_never_exceeds_the_caps_own_domain() {
    let m = model();
    // A large alpha budget: 100.0 - 1.0 - 0.0 - 2.5 = 96.5 exhausts more bps
    // than the model can ever report as impact (the cap tops out at 80bp),
    // so the true breakeven is "as fast as the model prices at all": 4.0.
    let alpha_bps = 1_000.0;
    let budget = alpha_bps - m.commission_bps - m.tax_bps - m.half_spread_bps;
    assert!(
        budget >= 2.0 * m.impact_coefficient_bps,
        "fixture must exceed the cap boundary to exercise it: budget {budget}"
    );

    let breakeven = m.breakeven_participation(alpha_bps);
    assert!(
        breakeven <= 4.0,
        "breakeven {breakeven} exceeds the participation domain impact_bps ever prices"
    );
    assert!(approx_eq(breakeven, 4.0, 1e-12));
    // And the impact at that reported breakeven must be the capped value,
    // not a number that pretends to consume a budget the model cannot reach.
    assert!(approx_eq(
        m.impact_bps(breakeven),
        2.0 * m.impact_coefficient_bps,
        1e-9
    ));
}

#[test]
fn a_budget_that_cannot_clear_linear_costs_has_no_breakeven() {
    let m = model();
    // Alpha smaller than commission + tax + half-spread: there is no
    // participation, however small, at which trading is worthwhile.
    assert_eq!(m.breakeven_participation(0.0), 0.0);
    assert_eq!(m.breakeven_participation(m.commission_bps), 0.0);
}

#[test]
fn a_zero_impact_coefficient_has_no_breakeven_regardless_of_alpha() {
    // Negotiated instruments price no square-root impact at all; dividing by
    // a zero coefficient must be refused rather than producing infinity.
    let m = TransactionCostModel::negotiated();
    assert_eq!(m.impact_coefficient_bps, 0.0);
    assert_eq!(m.breakeven_participation(1_000.0), 0.0);
}

#[test]
fn total_bps_is_the_sum_of_every_component_at_the_given_participation() {
    let m = model();
    let participation = 0.5;
    let expected = m.commission_bps + m.tax_bps + m.half_spread_bps + m.impact_bps(participation);
    assert!(approx_eq(m.total_bps(participation), expected, 1e-12));
}

#[test]
fn estimate_scales_with_notional_and_includes_the_fixed_fee() {
    let m = TransactionCostModel {
        fixed_fee: qip_core::dec!("5"),
        ..TransactionCostModel::default()
    };
    let notional = qip_core::dec!("1000000");
    let participation = 0.1;
    let cost = m.estimate(notional, participation);

    // Premise: the total-bps figure is non-zero, so the estimate below is
    // actually checking a computed cost rather than a coincidental zero.
    let total_bps = m.total_bps(participation);
    assert!(total_bps > 0.0);

    let expected_bps_cost = notional.apply_bps(total_bps - 0.0);
    // total_bps already sums commission+tax+half_spread+impact; apply_bps of
    // that on the notional plus the fixed fee is the same total the estimate
    // computes component-by-component.
    let expected = expected_bps_cost + m.fixed_fee;
    assert!(approx_eq(cost.to_f64(), expected.to_f64(), 1e-6));

    // A trade with zero notional still pays the fixed fee.
    let zero_notional_cost = m.estimate(qip_core::Decimal::ZERO, participation);
    assert_eq!(zero_notional_cost, m.fixed_fee);
}

#[test]
fn estimate_uses_the_magnitude_of_a_negative_notional() {
    // A sell is represented with a negative notional in some callers; the
    // cost of trading it is the same as the cost of the equivalent buy.
    let m = model();
    let buy = m.estimate(qip_core::dec!("100000"), 0.1);
    let sell = m.estimate(qip_core::dec!("-100000"), 0.1);
    assert_eq!(buy, sell);
}

#[test]
fn days_to_exit_scales_inversely_with_the_permitted_participation_rate() {
    // Every field is named because there is no default to fall back on: the
    // two this test turns on are the volume and the participation rate, and
    // the other four are stated so the reader can see they are not the subject.
    let liquidity = LiquidityProfile {
        average_daily_volume: qip_core::Decimal::from_int(1_000_000),
        typical_spread_bps: 4.0,
        top_of_book_depth: qip_core::Decimal::from_int(500),
        days_to_liquidate: 1.0,
        max_participation_rate: 0.1,
        is_negotiated: false,
    };
    let quantity = qip_core::Decimal::from_int(500_000);
    let days = liquidity
        .days_to_exit(quantity)
        .expect("a listed instrument reports a volume-based estimate");
    // 500,000 units at 10% of 1,000,000 ADV per day = 100,000/day = 5 days.
    assert!(approx_eq(days, 5.0, 1e-9), "days: {days}");

    // A negative (short) quantity exits over the same horizon as the long.
    let short_days = liquidity.days_to_exit(-quantity).expect("short quantity");
    assert!(approx_eq(short_days, days, 1e-9));

    // A negotiated instrument has no volume-based estimate at all.
    let negotiated = LiquidityProfile::illiquid(30.0, 900.0);
    assert!(negotiated.days_to_exit(quantity).is_none());

    // Zero participation policy (nobody may trade this) has no estimate either.
    let frozen = LiquidityProfile {
        average_daily_volume: qip_core::Decimal::from_int(1_000_000),
        typical_spread_bps: 4.0,
        top_of_book_depth: qip_core::Decimal::from_int(500),
        days_to_liquidate: 1.0,
        max_participation_rate: 0.0,
        is_negotiated: false,
    };
    assert!(frozen.days_to_exit(quantity).is_none());
}

/// A negotiated profile states the spread its caller measured. It used to
/// invent one.
///
/// `LiquidityProfile::illiquid` hardcoded `typical_spread_bps: 250.0` and took
/// only the exit time, so every negotiated holding asserted a quote nobody had
/// measured — the `MaxExpectedShortfall` shape in a cost model, a figure that
/// reads as evidence and is not.
///
/// It was not inert, which is why this is a test and not a note. The liquidity
/// ladder proves cost rises as it descends and `Rung::classify` puts a
/// negotiated holding below every listed one, so the invented 250 became the
/// ceiling on what any listed instrument above it could be quoted at: one
/// ordinary small-cap at 300bps inverted the per-rung rate, the ladder refused,
/// and — once that refusal was made to fail closed — the desk stopped trading.
/// Three percent is a spread a desk considers valid.
#[test]
fn a_negotiated_profile_carries_the_spread_its_caller_stated_and_invents_none() {
    // Two callers, two measurements, two profiles. One value alone could be
    // satisfied by a constructor that ignored its argument and happened to
    // hardcode that number.
    for stated in [12.5_f64, 900.0, 4000.0] {
        let profile = LiquidityProfile::illiquid(30.0, stated);
        assert_eq!(
            profile.typical_spread_bps, stated,
            "a negotiated profile asked for {stated}bps reported {}bps; the constructor is \
             inventing a spread nobody measured, and whatever it invents becomes the ceiling on \
             every listed quote above it",
            profile.typical_spread_bps
        );
    }

    // The rest of what the constructor vouches for is unchanged: this is a
    // holding that trades by appointment, with no volume-based exit estimate.
    let profile = LiquidityProfile::illiquid(30.0, 900.0);
    assert!(profile.is_negotiated);
    assert_eq!(profile.days_to_liquidate, 30.0);
    assert!(
        profile
            .days_to_exit(qip_core::Decimal::from_int(1_000))
            .is_none()
    );
}

// --- what the committed catalogue says about liquidity -----------------------
//
// `LiquidityProfile` had a `Default` asserting `typical_spread_bps: 10.0` and
// `days_to_liquidate: 1.0`. Until the catalogue format carried a liquidity
// block, every record in `data/datasets/universe.json` inherited exactly that,
// so `MinLiquidity` and `MaxDaysToLiquidate` — controls whose whole job is to
// veto trading — were evaluated for every deployed instrument against a figure
// that appeared in no file, in no diff a reviewer read, and under no manifest
// hash. It is `illiquid`'s hardcoded 250bps one level out: an invented number
// standing where a control reads. It is milder only because it was uniform,
// and so could not invert the ladder the way the 250 did.
//
// These tests are here rather than in `catalogue.rs`'s own suite because the
// subject is the `LiquidityProfile` a record carries, which is this file's
// subject, not the loader's field-by-field parsing.

/// The file every central root reads, and the tape the four listed names in it
/// were demonstrated on. Both relative to this crate, so the tests read the
/// committed artefacts and not a copy of them.
const COMMITTED_UNIVERSE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);
const COMMITTED_TAPE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/loop-demonstration-tape.json"
);

fn catalogue_now() -> qip_core::Timestamp {
    qip_core::Timestamp::from_civil(2026, 9, 2)
}

fn universe_text() -> String {
    std::fs::read_to_string(COMMITTED_UNIVERSE).expect("the committed catalogue is readable")
}

/// What the committed tape actually shows about one instrument.
struct TapeFigures {
    /// Total volume over the distinct dates it trades on — the only volume
    /// measurement this repository contains for these names.
    average_daily_volume: f64,
    /// The narrowest whole high-low range on any bar, in basis points of its
    /// own mid. A quoted spread wider than the tightest bar's entire range is
    /// not consistent with those bars having traded, so this is an upper bound
    /// the data supports rather than an opinion about the instrument.
    tightest_range_bps: f64,
}

fn tape_figures() -> std::collections::BTreeMap<String, TapeFigures> {
    let text = std::fs::read_to_string(COMMITTED_TAPE).expect("the committed tape is readable");
    let tape: serde_json::Value = serde_json::from_str(&text).expect("the tape is JSON");
    let observations = tape["observations"]
        .as_array()
        .expect("the tape holds an observations array");
    let number = |value: &serde_json::Value, key: &str| -> f64 {
        value[key]
            .as_str()
            .unwrap_or_else(|| panic!("an observation has a string `{key}`: {value}"))
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("`{key}` is a number: {value}"))
    };

    let mut volume: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let mut dates: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    let mut tightest: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for observation in observations {
        let id = observation["object_id"]
            .as_str()
            .expect("an observation names its instrument")
            .to_string();
        *volume.entry(id.clone()).or_insert(0.0) += number(observation, "volume");
        let at = observation["at"]
            .as_str()
            .expect("an observation is stamped")
            .to_string();
        dates.entry(id.clone()).or_default().insert(at[..10].into());
        let (high, low) = (number(observation, "high"), number(observation, "low"));
        let range_bps = (high - low) / ((high + low) / 2.0) * 10_000.0;
        let slot = tightest.entry(id).or_insert(f64::INFINITY);
        if range_bps < *slot {
            *slot = range_bps;
        }
    }

    volume
        .into_iter()
        .map(|(id, total)| {
            let days = dates[&id].len() as f64;
            (
                id.clone(),
                TapeFigures {
                    // Truncated, as the catalogue states it: the first and last
                    // dates on the tape are partial, so this understates the
                    // real daily volume, and understating capacity is the
                    // direction that trades less rather than more.
                    average_daily_volume: (total / days).trunc(),
                    tightest_range_bps: tightest[&id],
                },
            )
        })
        .collect()
}

/// A record that states no liquidity is refused by name, rather than being
/// handed a 10bp quote and a one-day exit that nobody measured.
///
/// The premise matters as much as the refusal: the record being broken must
/// carry a liquidity block that is *not* the figures the deleted constructor
/// invented, or the refusal below could be about a file that never said
/// anything different.
///
/// `LiquidityProfile::default()` no longer exists, so the profile compared
/// against is written out here rather than obtained from the library. That is
/// the point of writing it out: the test still fails if the committed
/// catalogue ever comes to carry those six figures, whether a constructor put
/// them there or an editor did.
#[test]
fn a_catalogue_record_that_states_no_liquidity_is_refused_by_name_and_inherits_no_default() {
    let text = universe_text();
    let loaded = qip_financial::catalogue::load(&text, catalogue_now())
        .expect("the committed catalogue loads as committed");
    let deleted_default = LiquidityProfile {
        average_daily_volume: qip_core::Decimal::ZERO,
        typical_spread_bps: 10.0,
        top_of_book_depth: qip_core::Decimal::ZERO,
        days_to_liquidate: 1.0,
        max_participation_rate: 0.1,
        is_negotiated: false,
    };
    // Premise: the loader installs each record's own figures, and they are not
    // the ones the constructor used to invent.
    for object in loaded.universe.iter() {
        assert_ne!(
            object.liquidity, deleted_default,
            "{} carries the six figures the deleted default asserted, so the catalogue is not \
             the source of the figure the ladder prices",
            object.object_id
        );
    }
    assert_eq!(deleted_default.typical_spread_bps, 10.0);
    assert_eq!(deleted_default.days_to_liquidate, 1.0);

    let mut value: serde_json::Value = serde_json::from_str(&text).expect("the catalogue is JSON");
    let broken = 3usize;
    let object_id = value["instruments"][broken]["object_id"]
        .as_str()
        .expect("the record names its object id")
        .to_string();
    assert!(
        value["instruments"][broken]
            .as_object_mut()
            .expect("a record is an object")
            .remove("liquidity")
            .is_some(),
        "the record had no liquidity block to remove"
    );
    let text = serde_json::to_string(&value).expect("the bent catalogue re-serialises");

    let error = match qip_financial::catalogue::load(&text, catalogue_now()) {
        Ok(loaded) => panic!(
            "a catalogue with a record stating no liquidity loaded {} instrument(s); every one \
             of them would be vetoed, or not vetoed, on a spread nobody measured",
            loaded.universe.len()
        ),
        Err(error) => error.message().to_string(),
    };
    assert!(
        error.contains(&format!("record #{}", broken + 1)),
        "the refusal does not name the record's position: {error}"
    );
    assert!(
        error.contains(&format!("`{object_id}`")),
        "the refusal does not name the record's object id: {error}"
    );
    assert!(
        error.contains("`liquidity`"),
        "the refusal does not name the missing field: {error}"
    );
}

/// A misspelled key inside a liquidity block is named as the key that should
/// not be there, not only as the one that is missing.
///
/// Without `deny_unknown_fields` on [`LiquidityProfile`] the stray key is
/// discarded in silence and serde refuses the record for a *missing*
/// `days_to_liquidate` — pointing the operator at a key they are looking
/// straight at, in a file where they have just written it. Both halves are
/// asserted, and on the backtick-delimited token rather than the bare word:
/// `days_to_liquidate` is a substring of `days_to_liquidation`, so a
/// `contains("days_to_liquidate")` here would pass whichever field serde
/// happened to name.
#[test]
fn a_misspelled_key_in_a_liquidity_block_is_refused_naming_the_key_that_does_not_belong() {
    let text = universe_text();
    // Premise: the file loads as committed, and the key about to be bent is
    // really in the record being bent.
    assert!(qip_financial::catalogue::load(&text, catalogue_now()).is_ok());
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("the catalogue is JSON");
    let block = value["instruments"][0]["liquidity"]
        .as_object_mut()
        .expect("the first record states a liquidity block");
    let days = block
        .remove("days_to_liquidate")
        .expect("the block states days_to_liquidate");
    block.insert("days_to_liquidation".into(), days);
    let text = serde_json::to_string(&value).expect("the bent catalogue re-serialises");

    let error = match qip_financial::catalogue::load(&text, catalogue_now()) {
        Ok(_) => panic!("a liquidity block with a misspelled key was accepted"),
        Err(error) => error.message().to_string(),
    };
    assert!(
        error.contains("unknown field `days_to_liquidation`"),
        "the refusal does not name the key that does not belong: {error}"
    );
    assert!(
        error.contains("record #1"),
        "the refusal does not name the record: {error}"
    );
}

/// Every committed record states its own liquidity, and for the four the
/// committed tape covers, the stated volume is the volume the tape shows and
/// the stated spread sits inside the interval that data admits.
///
/// The bounds are the point. A stated figure that no committed data contradicts
/// is a reference fact of the same standing as the `price` beside it; a stated
/// figure outside them is an opinion the file is passing off as a measurement.
#[test]
fn every_committed_record_states_its_own_liquidity_and_the_tape_backs_the_four_it_covers() {
    let text = universe_text();
    let loaded = qip_financial::catalogue::load(&text, catalogue_now())
        .expect("the committed catalogue loads");
    let tape = tape_figures();
    // Premise: there are records on both sides of the tape, so the loop below
    // exercises the measured arm and the stated arm rather than one of them.
    assert_eq!(tape.len(), 4, "the committed tape covers four instruments");
    assert!(
        loaded.universe.len() > tape.len(),
        "every catalogued record is on the tape; the stated arm is untested"
    );

    let mut checked = 0usize;
    for object in loaded.universe.iter() {
        let liquidity = &object.liquidity;
        assert!(
            !liquidity.is_negotiated,
            "{} is a listed common stock stated as trading by negotiation",
            object.object_id
        );
        // A book with depth at the touch but no volume behind it is a
        // measurement contradicting itself.
        assert_eq!(
            liquidity.top_of_book_depth.is_positive(),
            liquidity.average_daily_volume.is_positive(),
            "{} states a top-of-book depth and an average daily volume that cannot both be true",
            object.object_id
        );
        // The minimum quotable spread: one tick over the record's own price.
        // Nothing can be quoted tighter than the venue's grid.
        let tick_bps = object.tick_size.to_f64() / object.price.to_f64() * 10_000.0;
        assert!(
            liquidity.typical_spread_bps > tick_bps,
            "{} is quoted at {}bps, tighter than the {tick_bps}bps its own tick size allows",
            object.object_id,
            liquidity.typical_spread_bps
        );

        let Some(measured) = tape.get(object.object_id.as_str()) else {
            continue;
        };
        checked += 1;
        assert_eq!(
            liquidity.average_daily_volume.to_f64(),
            measured.average_daily_volume,
            "{} states an average daily volume the committed tape does not show",
            object.object_id
        );
        assert!(
            liquidity.typical_spread_bps < measured.tightest_range_bps,
            "{} is quoted at {}bps, wider than the whole {}bps range of the tightest bar the \
             tape shows it trading in",
            object.object_id,
            liquidity.typical_spread_bps,
            measured.tightest_range_bps
        );
    }
    assert_eq!(
        checked,
        tape.len(),
        "the catalogue does not carry every instrument the tape covers, so the measured arm \
         above checked fewer records than it claims"
    );
}

/// Where the catalogue states a figure it cannot measure, the figure may only
/// tighten a control, never loosen one.
///
/// Two committed records appear on no tape in this repository, so nothing backs
/// their liquidity the way the other four are backed. That is the moment a
/// reference file turns into an invented number under a different name. The
/// rule that keeps it honest is directional: an unmeasured record is quoted no
/// tighter, exits no faster, and is participated in no harder than every record
/// that *is* measured, and claims no volume at all — so the worst a stated
/// figure can do is trade less.
#[test]
fn a_committed_record_the_tape_does_not_cover_is_quoted_no_tighter_than_every_record_it_does() {
    let loaded = qip_financial::catalogue::load(&universe_text(), catalogue_now())
        .expect("the committed catalogue loads");
    let tape = tape_figures();

    let (measured, stated): (Vec<_>, Vec<_>) = loaded
        .universe
        .iter()
        .partition(|object| tape.contains_key(object.object_id.as_str()));
    // Premise: both sides are non-empty, or the comparison below is vacuous —
    // a filter over an empty list satisfies every rule ever written.
    assert!(!measured.is_empty(), "no record is backed by the tape");
    assert!(!stated.is_empty(), "no record is stated without the tape");

    let widest = measured
        .iter()
        .map(|object| object.liquidity.typical_spread_bps)
        .fold(f64::NEG_INFINITY, f64::max);
    let slowest = measured
        .iter()
        .map(|object| object.liquidity.days_to_liquidate)
        .fold(f64::NEG_INFINITY, f64::max);
    let keenest = measured
        .iter()
        .map(|object| object.liquidity.max_participation_rate)
        .fold(f64::INFINITY, f64::min);
    assert!(
        widest.is_finite() && slowest.is_finite() && keenest.is_finite(),
        "the measured records state no bound to compare against"
    );

    for object in stated {
        let liquidity = &object.liquidity;
        assert!(
            liquidity.typical_spread_bps >= widest,
            "{} is on no tape and is quoted at {}bps, tighter than the {widest}bps of the widest \
             record the tape backs",
            object.object_id,
            liquidity.typical_spread_bps
        );
        assert!(
            liquidity.days_to_liquidate >= slowest,
            "{} is on no tape and claims to exit in {} days, faster than the {slowest} days of \
             the slowest record the tape backs",
            object.object_id,
            liquidity.days_to_liquidate
        );
        assert!(
            liquidity.max_participation_rate <= keenest,
            "{} is on no tape and would be {} of a day's volume, more than the {keenest} the \
             platform permits itself in a record the tape backs",
            object.object_id,
            liquidity.max_participation_rate
        );
        assert!(
            !liquidity.average_daily_volume.is_positive(),
            "{} is on no tape and states an average daily volume of {}; nothing in this \
             repository observed it trading",
            object.object_id,
            liquidity.average_daily_volume
        );
        // And so it offers no volume-based exit estimate at all, rather than a
        // fast one: `days_to_liquidate` is the only guide it has.
        assert!(
            liquidity
                .days_to_exit(qip_core::Decimal::from_int(1_000))
                .is_none()
        );
    }
}

#[test]
fn a_cost_model_whose_spread_is_not_a_number_is_refused_before_it_reaches_the_money_path() {
    // The premise first, because the refusal below is only worth having if the
    // figure really is unusable. Every component of `estimate` crosses through
    // `Decimal::apply_bps`, which is `Decimal::from_f64(bps / 10_000.0)`
    // followed by a `checked_mul`, and for these rates that conversion has no
    // answer at all — asserted here on `from_f64` itself rather than on
    // `apply_bps`, so that this test states the fact the finding rests on and
    // not whichever way `apply_bps` currently reports its failure. It has
    // answered `Decimal::ZERO`, which prices the instrument as free to trade;
    // whatever it does instead, a cost model the arithmetic cannot apply has no
    // business being in the world model, and refusing it here is what keeps the
    // question from arising on the money path.
    for rate in [f64::NAN, f64::INFINITY, 1e35] {
        assert!(
            qip_core::Decimal::from_f64(rate / 10_000.0).is_none(),
            "{rate}bp has no Decimal factor, which is what makes it unusable"
        );
    }
    let poisoned = TransactionCostModel {
        commission_bps: 0.0,
        fixed_fee: qip_core::Decimal::ZERO,
        half_spread_bps: f64::NAN,
        impact_coefficient_bps: 0.0,
        tax_bps: 0.0,
        short_borrow_bps_annual: 0.0,
    };
    let notional = qip_core::dec!("1000000");

    // So the figure is refused before it reaches the arithmetic. A document is
    // the way it arrived: `#[derive(Deserialize)]` wrote straight to the
    // fields, exactly as it did for `ValuationInput`.
    // Each document is refused *by this validation* and not by serde's own
    // parsing, which is what the message assertion establishes: `1e400` would
    // fail as "number out of range" whatever this crate did, and a test
    // asserting only `is_err()` on it would guard nothing. Every figure below
    // is a perfectly good `f64` that decodes and then does not price a trade.
    for (described, field, document) in [
        (
            "a spread too large to represent as a factor",
            "half_spread_bps",
            r#"{"commission_bps":1.0,"fixed_fee":"0","half_spread_bps":1e35,
                "impact_coefficient_bps":40.0,"tax_bps":0.0,"short_borrow_bps_annual":50.0}"#,
        ),
        (
            "a spread at the whole value of the notional",
            "half_spread_bps",
            r#"{"commission_bps":1.0,"fixed_fee":"0","half_spread_bps":10000.0,
                "impact_coefficient_bps":40.0,"tax_bps":0.0,"short_borrow_bps_annual":50.0}"#,
        ),
        (
            "a commission the venue pays the desk",
            "commission_bps",
            r#"{"commission_bps":-5.0,"fixed_fee":"0","half_spread_bps":2.5,
                "impact_coefficient_bps":40.0,"tax_bps":0.0,"short_borrow_bps_annual":50.0}"#,
        ),
        (
            "an impact coefficient beyond the notional",
            "impact_coefficient_bps",
            r#"{"commission_bps":1.0,"fixed_fee":"0","half_spread_bps":2.5,
                "impact_coefficient_bps":1e30,"tax_bps":0.0,"short_borrow_bps_annual":50.0}"#,
        ),
        (
            "a fixed fee the venue pays the desk",
            "fixed_fee",
            r#"{"commission_bps":1.0,"fixed_fee":"-5","half_spread_bps":2.5,
                "impact_coefficient_bps":40.0,"tax_bps":0.0,"short_borrow_bps_annual":50.0}"#,
        ),
    ] {
        let error = serde_json::from_str::<TransactionCostModel>(document)
            .expect_err(described)
            .to_string();
        assert!(
            error.contains(field),
            "{described} must be refused by name, got {error}"
        );
    }

    // `checked` is the same gate reached without a document, and it names the
    // field and the value so the record can be corrected.
    let refusal = poisoned
        .clone()
        .checked()
        .expect_err("a spread that is not a number cannot price a trade");
    assert_eq!(refusal.code(), "invalid", "got {refusal}");
    assert!(
        refusal.message().contains("half_spread_bps"),
        "the refusal must name the field to correct, got {refusal}"
    );

    // The admitting half, and what makes this a validation rather than a wall:
    // an ordinary listed cost model still decodes, and the cost it prices is
    // the non-zero figure the refused one reported as free.
    let document = r#"{"commission_bps":1.0,"fixed_fee":"0","half_spread_bps":2.5,
        "impact_coefficient_bps":40.0,"tax_bps":0.0,"short_borrow_bps_annual":50.0}"#;
    let sound: TransactionCostModel =
        serde_json::from_str(document).expect("an ordinary listed cost model decodes");
    assert_eq!(sound, TransactionCostModel::default());
    assert!(
        sound.estimate(notional, 0.1).is_positive(),
        "a stated cost model prices a million-dollar trade above zero"
    );
    // And the wide end is admitted too: a hard-to-borrow name really does quote
    // past 100% a year, so the borrow ceiling is not the per-trade one.
    let hard_to_borrow = TransactionCostModel {
        short_borrow_bps_annual: 45_000.0,
        ..TransactionCostModel::default()
    };
    assert!(
        hard_to_borrow.checked().is_ok(),
        "a 450% annual borrow is a rate a lending desk writes down"
    );
}

#[test]
fn a_cost_model_a_struct_literal_poisoned_refuses_to_price_instead_of_aborting_or_charging_zero() {
    // `checked` and `FinancialObject::validate` keep an unpriceable rate off a
    // record. Neither sees a model assembled field by field, and the fields are
    // public — `qip-simulation-engine`'s `CostModel::pricing_at` builds one that
    // way — so `checked_estimate` is the last seam before the money path.
    //
    // Three ways the arithmetic gives out, and the premise is asserted first so
    // that this test cannot pass on rates that were priceable all along.
    for bps in [f64::NAN, f64::INFINITY, 1e35] {
        assert!(
            qip_core::Decimal::from_f64(bps / 10_000.0).is_none(),
            "{bps}bp has no Decimal factor, which is what makes it unusable"
        );
    }
    let notional = qip_core::dec!("1000000");
    for (field, poisoned) in [
        (
            "half_spread_bps",
            TransactionCostModel {
                half_spread_bps: f64::NAN,
                ..TransactionCostModel::default()
            },
        ),
        (
            "commission_bps + tax_bps",
            TransactionCostModel {
                commission_bps: f64::INFINITY,
                ..TransactionCostModel::default()
            },
        ),
        (
            "impact_coefficient_bps at this participation",
            TransactionCostModel {
                impact_coefficient_bps: 1e35,
                ..TransactionCostModel::default()
            },
        ),
    ] {
        // The refusal, and not a panic: `Decimal::apply_bps` panics on exactly
        // these rates since `f1b8840`, which is fail-closed but under
        // `panic = "abort"` costs the process rather than the order. That the
        // call returns at all is half of what is asserted here.
        let refusal = poisoned
            .checked_estimate(notional, 0.1)
            .expect_err("a rate the arithmetic cannot apply prices no trade");
        assert_eq!(refusal.code(), "numeric", "got {refusal}");
        assert!(
            refusal.message().contains(field),
            "the refusal must name the term to correct, got {refusal}"
        );
        // An error here is a refusal, so it names what to do instead. Without
        // this the message could shrink to the fact of the failure, which
        // sends an operator looking for a bug in the arithmetic rather than
        // for the field they have to correct.
        assert!(
            refusal.message().contains("reference record"),
            "the refusal must say where the remedy is, got {refusal}"
        );
    }

    // The admitting half, which is what makes this a validation rather than a
    // wall: an ordinary listed model still prices, and it prices the same
    // figure the panicking form quotes, so the checked path is not a
    // second-class approximation of it.
    let sound = TransactionCostModel::default();
    let priced = sound
        .checked_estimate(notional, 0.1)
        .expect("an ordinary listed cost model prices a million-dollar trade");
    assert!(
        priced.is_positive(),
        "a stated cost model prices a million-dollar trade above zero, got {priced}"
    );
    assert_eq!(priced, sound.estimate(notional, 0.1));
}
