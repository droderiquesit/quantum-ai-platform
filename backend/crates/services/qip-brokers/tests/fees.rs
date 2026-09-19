//! What the simulated venue charges, and that the ladder it charges on can
//! actually be climbed.
//!
//! Blueprint §18.4's fee-tier row — "volume thresholds that reduce fees.
//! Reaching one can be worth more than the trades that reach it" — was for a
//! long time a sentence about something the platform could not do.
//! `qip_routing::FeeSchedule` had a tiered constructor, a maker/taker split
//! and a volume lookup, and every caller of `FeeSchedule::tiered` and of
//! `VenueProfile::listed` lived under a `tests/` directory. A ladder nothing
//! climbs is the `MaxExpectedShortfall` shape: it reads as a control and
//! cannot fire.
//!
//! So these tests are not about arithmetic. Each one drives the venue through
//! the event and asserts that the *cash charged* moved, because a rung that
//! changes a reported rate and not a booked cost has changed nothing. Each
//! asserts its own premise first: that the two notionals being compared are
//! equal, or that the threshold really was uncrossed before and crossed
//! after. A test that compared two different notionals would pass on the
//! difference in size and prove nothing about the rung.

#![allow(clippy::panic_in_result_fn)]

use qip_brokers::adapter::VenueAdapter;
use qip_brokers::credential::{RequirementKind, requirements_of_kind, standard_requirements};
use qip_brokers::exchange::{ExchangeSettings, SimulatedExchange};
use qip_brokers::{AdapterClass, VenueCredential};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_core::{Decimal, dec};
use qip_execution_engine::broker::Broker;
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_routing::venue::{FeeSchedule, FeeTier, Liquidity};

const VENUE: &str = "XSIM";
const ACCOUNT: &str = "book-under-test";

/// Where the cheaper rung begins. Chosen so that one order against the seeded
/// book lands short of it and two land past it, which is what lets a single
/// test show the same venue charging two different prices.
const THRESHOLD: &str = "5000";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000000AAA")
}

/// A liquid listed name, stated rather than inherited: `LiquidityProfile` has
/// no `Default` on purpose, because the controls that veto trading read
/// exactly these two figures.
fn instrument() -> FinancialObject {
    FinancialObject::builder(
        object(),
        "AAA",
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .name("Instrument A")
    .venue(VENUE)
    .price(dec!("100"))
    .lot_size(Decimal::ONE)
    .tick_size(dec!("0.01"))
    .provenance(Provenance::synthetic("qip-brokers fee test", start()))
    .build(start())
    .expect("a structurally valid instrument")
}

fn credential() -> VenueCredential {
    let enforced = requirements_of_kind(
        &standard_requirements(&venue()),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    VenueCredential::satisfying(VENUE, ACCOUNT, &enforced).expect("a named venue and account")
}

/// A two-rung ladder: dear below [`THRESHOLD`], cheap at or above it.
///
/// The maker rungs are below the taker rungs on both, which is the split the
/// old flat `commission_rate` could not express at all.
fn ladder() -> FeeSchedule {
    FeeSchedule::tiered(vec![
        FeeTier::new(Decimal::ZERO, 2.0, 10.0),
        FeeTier::new(dec!("5000"), 1.0, 4.0),
    ])
    .expect("a ladder starting at zero volume")
}

fn settings(fees: FeeSchedule) -> ExchangeSettings {
    ExchangeSettings {
        fees,
        ..ExchangeSettings::orderly()
    }
}

/// A venue listed with one deep ask, logged on and heartbeating.
///
/// Deeper than the other fixtures in this crate because these tests have to
/// trade past a volume threshold, and a book that runs out would cancel the
/// residual and leave the test asserting about a fill that never happened.
/// One level, so every fill here is 100.00 exactly and a difference in cost
/// between two fills can only be the rung.
///
/// **Seeded on the ask only, and that is load-bearing rather than
/// minimalism.** The maker test rests a client bid and then sends venue flow
/// to hit it. A seeded venue bid at a better price would take that flow
/// first, the client order would never trade, and the test would be asserting
/// about a fill that did not happen — which is exactly what it did on the
/// first run.
fn live_venue(fees: FeeSchedule) -> Result<SimulatedExchange> {
    let mut exchange = SimulatedExchange::new(venue(), settings(fees), 11, start());
    exchange.list(instrument());
    exchange.seed_liquidity(
        &object(),
        Side::Sell,
        dec!("100.00"),
        Decimal::from_int(400),
        start(),
    )?;
    exchange.bring_up(&credential(), start())?;
    Ok(exchange)
}

fn order_at(label: &str, side: Side, quantity: i64) -> Order {
    Order::new(
        OrderId::from_string(label),
        object(),
        side,
        Decimal::from_int(quantity),
        OrderType::Market,
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    )
}

/// Submit a market order and return the one fill it produced.
fn fill_once(exchange: &mut SimulatedExchange, label: &str, quantity: i64) -> Result<Decimal> {
    let ticket = exchange.ready(start())?;
    let ack = exchange.submit_order(&ticket, &order_at(label, Side::Buy, quantity), start())?;
    assert_eq!(
        ack.fills.len(),
        1,
        "the fixture's book is one level, so {label} should fill in exactly one trade"
    );
    Ok(ack.fills[0].costs)
}

// --- the ladder can be climbed ----------------------------------------------

#[test]
fn a_venue_the_account_has_traded_past_the_threshold_at_charges_the_next_fill_the_cheaper_rung()
-> Result<()> {
    let mut exchange = live_venue(ladder())?;
    let threshold = Decimal::parse(THRESHOLD).expect("a parsable threshold");

    // Premise: nothing has traded here yet, so the account is on the dear
    // rung. Without this the test would pass on a venue that started cheap.
    assert_eq!(
        exchange.traded_notional(),
        Decimal::ZERO,
        "a fresh venue has recorded no volume"
    );
    assert!(
        exchange.rate_bps_f64(Liquidity::Taker) > 4.0,
        "the account must start on the dear rung for this test to say anything"
    );

    // 30 at 100.00 = 3000, which is short of the threshold.
    let dear = fill_once(&mut exchange, "order-below", 30)?;
    assert!(
        exchange.traded_notional() < threshold,
        "premise: after the first fill the account is still below the threshold, but it has \
         traded {}",
        exchange.traded_notional()
    );

    // The same size again, which takes the running total past the threshold.
    let crossing = fill_once(&mut exchange, "order-crossing", 30)?;
    assert!(
        exchange.traded_notional() >= threshold,
        "premise: the second fill takes the account over the threshold, but it has traded {}",
        exchange.traded_notional()
    );

    // The third fill is the first one charged at the cheaper rung.
    let cheap = fill_once(&mut exchange, "order-above", 30)?;

    // Identical notionals, so any difference in cost is the rung and nothing
    // else. 30 * 100.00 at 10bp is 3.00; at 4bp it is 1.20.
    assert_eq!(
        dear,
        dec!("3"),
        "3000 notional at the dear taker rung of 10bp"
    );
    assert_eq!(
        crossing,
        dec!("3"),
        "the crossing fill is still charged the dear rung, because the rung is read from the \
         volume before the fill"
    );
    assert_eq!(
        cheap,
        dec!("1.2"),
        "3000 notional at the cheap taker rung of 4bp"
    );
    assert!(
        cheap < dear,
        "reaching the threshold must actually reduce what the account pays, or §18.4's row is a \
         number nobody charged: dear {dear}, cheap {cheap}"
    );
    Ok(())
}

#[test]
fn a_fill_is_charged_at_the_rung_the_volume_before_it_reached_and_never_the_rung_it_creates()
-> Result<()> {
    let mut exchange = live_venue(ladder())?;
    let threshold = Decimal::parse(THRESHOLD).expect("a parsable threshold");

    // One order that alone takes the account from nothing to well past the
    // threshold. Charging against the volume the fill itself creates would
    // let this order discount itself into a rung it had not reached when it
    // was sent — a rebate the venue never offered, and a cost the router
    // could not have reproduced from what it knew at routing time.
    assert_eq!(
        exchange.traded_notional(),
        Decimal::ZERO,
        "premise: nothing traded yet"
    );
    let costs = fill_once(&mut exchange, "one-big-order", 100)?;
    assert!(
        exchange.traded_notional() > threshold,
        "premise: this single order must cross the threshold on its own, but it traded {}",
        exchange.traded_notional()
    );

    // 100 * 100.00 = 10000 at the dear taker rung of 10bp is 10.00. At the
    // cheap rung it would have been 4.00, and that is the wrong answer this
    // test exists to refuse.
    assert_eq!(
        costs,
        dec!("10"),
        "the order that crossed the threshold is charged at the rung in force before it, not the \
         one it brought into force"
    );
    Ok(())
}

#[test]
fn the_resting_side_of_a_trade_is_charged_the_maker_rung_and_the_crossing_side_the_taker_rung()
-> Result<()> {
    let mut exchange = live_venue(ladder())?;

    // A client order that rests, then somebody else's flow takes it. The
    // client provided the liquidity, so it is the maker.
    let ticket = exchange.ready(start())?;
    let resting = Order::new(
        OrderId::from_string("client-resting"),
        object(),
        Side::Buy,
        Decimal::from_int(30),
        OrderType::Limit {
            price: dec!("99.00"),
        },
        dec!("99.00"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    );
    let ack = exchange.submit_order(&ticket, &resting, start())?;
    assert!(
        ack.fills.is_empty(),
        "premise: a buy at 99.00 against an ask of 100.00 must rest rather than trade"
    );

    exchange.seed_aggressor(
        &object(),
        Side::Sell,
        dec!("99.00"),
        Decimal::from_int(30),
        start(),
    )?;
    let maker_fills = exchange.drain_bookable_fills();
    assert_eq!(
        maker_fills.len(),
        1,
        "the venue's flow should hit the resting order once"
    );
    let maker = &maker_fills[0].fill;

    // 30 at 99.00 = 2970. Premise for the comparison below: the maker's
    // notional and the taker's must be the same, or the difference in cost is
    // size rather than side.
    let maker_notional = maker.quantity * maker.price;
    assert_eq!(
        maker_notional,
        dec!("2970"),
        "premise: the maker traded 2970 of notional"
    );

    // 2970 at the maker rung of 2bp is 0.594; at the taker rung of 10bp it
    // would be 2.97. One flat rate could not tell these apart at all, which
    // is what made a paper desk unable to see that resting is cheaper.
    assert_eq!(
        maker.costs,
        dec!("0.594"),
        "the resting side pays the maker rung"
    );

    // Now a taker fill of the same notional at the same rung, for the
    // comparison. The account has traded 2970 so far, still below the
    // threshold, so both sides are read off the same tier.
    assert!(
        exchange.traded_notional() < Decimal::parse(THRESHOLD).expect("a parsable threshold"),
        "premise: both sides must be compared on the same tier, but the account has traded {}",
        exchange.traded_notional()
    );
    let ticket = exchange.ready(start())?;
    let taker = Order::new(
        OrderId::from_string("client-crossing"),
        object(),
        Side::Buy,
        Decimal::from_int(30),
        OrderType::Limit {
            price: dec!("100.00"),
        },
        dec!("100.00"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    );
    let ack = exchange.submit_order(&ticket, &taker, start())?;
    assert_eq!(
        ack.fills.len(),
        1,
        "a buy at 100.00 crosses the resting ask"
    );
    let taker_fill = &ack.fills[0];
    assert_eq!(
        taker_fill.quantity * taker_fill.price,
        dec!("3000"),
        "premise: the taker traded 3000 of notional"
    );

    // Per unit of notional, the taker pays five times the maker: 10bp against
    // 2bp. Asserted as a ratio rather than as two constants so the property
    // survives a change of rungs — what must hold is that the two sides are
    // charged differently and that resting is the cheaper one.
    let maker_per_unit = maker.costs / maker_notional;
    let taker_per_unit = taker_fill.costs / dec!("3000");
    assert!(
        taker_per_unit > maker_per_unit,
        "crossing must cost more per unit of notional than resting: taker {taker_per_unit}, \
         maker {maker_per_unit}"
    );
    Ok(())
}

#[test]
fn a_fill_whose_rate_cannot_be_applied_is_refused_rather_than_booked_at_no_cost() -> Result<()> {
    // `FeeSchedule::flat` takes the rate it is handed and a schedule can also
    // arrive by deserialisation, so a non-finite rate reaches the charging
    // seam. This used to be `unwrap_or(Decimal::ZERO)`: a fee of exactly zero
    // is the one wrong answer that makes a venue look free, so it would not
    // merely have mis-booked this fill — it would have elected this venue for
    // everything routed afterwards.
    let mut exchange = live_venue(FeeSchedule::flat(f64::NAN, f64::NAN))?;

    let ticket = exchange.ready(start())?;
    let refusal = exchange
        .submit_order(&ticket, &order_at("unpriceable", Side::Buy, 30), start())
        .expect_err("a fill that cannot be priced must not be booked");

    assert!(
        refusal.message().contains("taker"),
        "the refusal should name the side whose rate is unusable: {}",
        refusal.message()
    );
    assert!(
        exchange.drain_bookable_fills().is_empty(),
        "nothing may be booked against a rate that could not be applied"
    );
    // And the volume the ladder is read against must not have moved either,
    // or a refused fill would silently advance the account towards a rung.
    assert_eq!(
        exchange.traded_notional(),
        Decimal::ZERO,
        "a fill that was refused contributes no volume to the ladder"
    );
    Ok(())
}

#[test]
fn the_venue_reports_the_taker_rate_as_its_headline_commission_so_the_figure_is_never_flattering()
-> Result<()> {
    // `VenueCapabilities` has one commission field where the venue has two
    // prices, so the single number has to be chosen rather than averaged. It
    // is the taker rate: the rate an order that crosses pays, and the only
    // one of the two that is never a rebate. A maker rate reported here would
    // tell an order management system that trading costs less than any order
    // it can actually send.
    let exchange = live_venue(ladder())?;

    // Premise: the two rungs really do differ, or "it reports the taker rate"
    // is true of reporting either one.
    assert!(
        exchange.rate_bps_f64(Liquidity::Taker) > exchange.rate_bps_f64(Liquidity::Maker),
        "premise: the fixture's maker and taker rungs must differ"
    );

    let reported = exchange.capabilities().commission_rate;
    let taker_fraction = exchange.rate_bps_f64(Liquidity::Taker) / 10_000.0;
    let maker_fraction = exchange.rate_bps_f64(Liquidity::Maker) / 10_000.0;
    assert!(
        (reported - taker_fraction).abs() < 1e-12,
        "the headline commission should be the taker rate {taker_fraction}, and it is {reported}"
    );
    assert!(
        (reported - maker_fraction).abs() > 1e-12,
        "reporting the maker rate would understate what any sendable order costs"
    );
    Ok(())
}

#[test]
fn the_venue_that_charges_the_ladder_is_still_a_simulated_one_and_has_no_live_class() -> Result<()>
{
    // The fee ladder touches the charging seam of the only venue this
    // platform executes against, so it is worth stating alongside it that
    // nothing here reached for a live one. `AdapterClass` has two variants
    // and no third, and a fill from this venue is stamped simulated by the
    // adapter rather than by the message.
    let exchange = live_venue(ladder())?;
    assert!(exchange.is_simulated(), "this venue is a simulator");
    assert_eq!(exchange.class(), AdapterClass::Simulated);
    assert!(
        serde_json::from_str::<AdapterClass>("\"live\"").is_err(),
        "there is no live adapter class to deserialise into"
    );
    Ok(())
}
