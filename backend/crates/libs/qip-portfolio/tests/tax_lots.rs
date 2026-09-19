//! The tax dimension of a position lot: which jurisdiction a lot sits in,
//! and which lot a close consumes once holding period is allowed to matter.
//!
//! Every test here guards one of the two failures §5.7 named. The first is
//! that the platform realises a gain and cannot say whether it was short- or
//! long-term. The second is subtler and is the one this repository has
//! shipped before, in `MaxExpectedShortfall`: a selection that reads as a tax
//! policy and orders the lots exactly as first-in-first-out, because the rule
//! it consults could never apply to the lots in front of it.

use qip_core::{Context, Currency, Decimal, Duration, ObjectId, PortfolioId, Timestamp, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::constraints::{Jurisdiction, RegulatoryConstraints};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_portfolio::lot::{
    HoldingPeriodTest, HoldingTerm, Lot, LotMethod, LotSelection, close_lots_under,
};
use qip_portfolio::portfolio::Portfolio;
use qip_portfolio::position::Position;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn days_after(days: i64) -> Timestamp {
    now().saturating_add(Duration::from_days(days))
}

/// The United States' twelve-month line, stated by this test and not by the
/// crate under test — which is the point: `qip-portfolio` holds no table of
/// thresholds and has no default one.
fn one_year() -> Duration {
    Duration::from_days(365)
}

fn us_test() -> HoldingPeriodTest {
    HoldingPeriodTest::declare(Jurisdiction::UnitedStates, one_year()).expect("a positive span")
}

/// An instrument whose regulatory record names exactly the jurisdictions
/// given, in the order given.
fn instrument(symbol: &str, jurisdictions: &[Jurisdiction]) -> FinancialObject {
    let (context, _clock) = Context::deterministic(now(), 1);
    let mut regulatory = RegulatoryConstraints::unrestricted();
    for jurisdiction in jurisdictions {
        regulatory = regulatory.with_jurisdiction(*jurisdiction);
    }
    FinancialObject::builder(
        ObjectId::from_string(format!("OBJ{symbol:0>23}")),
        symbol,
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(1_000_000), 3.0),
    )
    .name(symbol)
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .geography("US")
    .price(dec!("100"))
    .transaction_costs(TransactionCostModel::listed(3.0))
    .provenance(Provenance::synthetic("test", now()))
    .build(now())
    .map(|mut object| {
        object.regulatory = regulatory;
        let _ = &context;
        object
    })
    .expect("valid object")
}

fn portfolio() -> Portfolio {
    Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(1_000_000),
        now(),
    )
}

/// Two lots: one bought on day zero and held past the year, one bought on
/// day 400. Closed on day 500, the first is long-term and the second is not.
fn two_lots_one_long_one_short() -> Vec<Lot> {
    vec![
        Lot::new(Decimal::from_int(100), dec!("50"), now()),
        Lot::new(Decimal::from_int(100), dec!("60"), days_after(400)),
    ]
}

// --- the jurisdiction reaches the position from the instrument --------------

#[test]
fn a_fill_in_an_instrument_that_names_one_jurisdiction_opens_a_position_in_it() {
    let object = instrument("AAA", &[Jurisdiction::UnitedStates]);
    // Premise: the instrument really does name exactly one.
    assert_eq!(object.regulatory.jurisdictions.len(), 1);

    let mut book = portfolio();
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("50"),
        dec!("0"),
        now(),
        None,
    );

    let position = book
        .position(&object.object_id)
        .expect("a position was opened");
    assert_eq!(position.jurisdiction(), Some(Jurisdiction::UnitedStates));
}

#[test]
fn a_fill_in_an_instrument_that_names_two_jurisdictions_opens_a_position_in_neither() {
    // The failure this prevents: narrowing a set of two to whichever comes
    // first in `BTreeSet` order, which would have this platform assert a tax
    // position — here the United States', because `Jurisdiction` derives
    // `Ord` from declaration order and that arm is declared first — which no
    // operator took and no record supports.
    let object = instrument(
        "BBB",
        &[Jurisdiction::UnitedStates, Jurisdiction::EuropeanUnion],
    );
    // Premise: both really are on the record, and one of them really is
    // first, so an implementation that took `.iter().next()` would return a
    // `Some` rather than trivially nothing.
    assert_eq!(object.regulatory.jurisdictions.len(), 2);
    assert_eq!(
        object.regulatory.jurisdictions.iter().next().copied(),
        Some(Jurisdiction::UnitedStates)
    );

    let mut book = portfolio();
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("50"),
        dec!("0"),
        now(),
        None,
    );

    let position = book
        .position(&object.object_id)
        .expect("a position was opened");
    assert_eq!(position.jurisdiction(), None);
}

#[test]
fn a_fill_in_an_instrument_that_names_no_jurisdiction_opens_a_position_in_none() {
    let object = instrument("CCC", &[]);
    assert!(object.regulatory.jurisdictions.is_empty());

    let mut book = portfolio();
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("50"),
        dec!("0"),
        now(),
        None,
    );

    assert_eq!(
        book.position(&object.object_id)
            .expect("a position was opened")
            .jurisdiction(),
        None
    );
}

// --- the rule refuses rather than clamping ----------------------------------

#[test]
fn a_long_term_threshold_of_zero_is_refused_rather_than_taken() {
    // A threshold of zero makes every lot long-term at the instant it is
    // acquired, which distinguishes nothing while reading as a working rule.
    let refusal = HoldingPeriodTest::declare(Jurisdiction::UnitedStates, Duration::ZERO)
        .expect_err("a zero threshold must be refused");
    let message = refusal.to_string();
    assert!(
        message.contains("distinguishes nothing"),
        "the refusal must name why zero is not a rule: {message}"
    );
    assert!(
        HoldingPeriodTest::declare(Jurisdiction::UnitedStates, Duration::from_nanos(-1)).is_err(),
        "a negative threshold must be refused too"
    );
    // And it admits a good value, which is the half that distinguishes a
    // working gate from one that refuses everything.
    assert!(HoldingPeriodTest::declare(Jurisdiction::UnitedStates, one_year()).is_ok());
}

#[test]
fn a_holding_period_rule_for_another_jurisdiction_is_refused_by_the_position_it_cannot_classify() {
    // The `MaxExpectedShortfall` shape. A rule declared for Japan classifies
    // every lot of a United States position `Undetermined`, which leaves the
    // ordering identical to first-in-first-out — a tax policy that reads as
    // in force and is not. It is refused here, where the mismatch is
    // knowable, not discovered from a gain that came out the wrong term.
    let mut position = Position::new(
        ObjectId::from_string("OBJ0000000000000000000001"),
        "AAA",
        now(),
    )
    .with_jurisdiction(Some(Jurisdiction::UnitedStates));

    let japan = HoldingPeriodTest::declare(Jurisdiction::Japan, one_year()).expect("positive");
    let refusal = position
        .declare_selection(LotSelection::LongTermFirst(japan))
        .expect_err("a rule for another jurisdiction must be refused");
    let message = refusal.to_string();
    assert!(
        message.contains("japan") && message.contains("united_states"),
        "the refusal must name both jurisdictions so an operator can fix it: {message}"
    );

    // The selection did not move, so a refused rule leaves the position
    // closing the way it already did rather than half-applying.
    assert_eq!(
        position.selection(),
        LotSelection::Mechanical(LotMethod::FirstInFirstOut)
    );

    // And the matching rule is admitted.
    position
        .declare_selection(LotSelection::LongTermFirst(us_test()))
        .expect("a rule for this position's own jurisdiction is admitted");
    assert_eq!(position.selection(), LotSelection::LongTermFirst(us_test()));
}

#[test]
fn a_position_with_no_jurisdiction_refuses_every_holding_period_rule() {
    let mut position = Position::new(
        ObjectId::from_string("OBJ0000000000000000000002"),
        "BBB",
        now(),
    );
    // Premise: this position genuinely names no jurisdiction.
    assert_eq!(position.jurisdiction(), None);

    let refusal = position
        .declare_selection(LotSelection::ShortTermFirst(us_test()))
        .expect_err("a position that cannot classify a lot must refuse the rule");
    assert!(
        refusal
            .to_string()
            .contains("no single regulatory jurisdiction"),
        "the refusal must name the missing fact: {refusal}"
    );
    // A mechanical selection is still admitted, because it needs no rule.
    position
        .declare_selection(LotSelection::Mechanical(LotMethod::LastInFirstOut))
        .expect("a mechanical method needs no jurisdiction");
}

// --- the selection actually changes which lot is consumed -------------------

#[test]
fn the_long_term_first_selection_closes_the_year_old_lot_before_the_recent_one() {
    let mut lots = two_lots_one_long_one_short();
    // Premise: the two lots are in the order that makes the assertion mean
    // something. `lots[0]` is the older AND the cheaper, so a selection that
    // silently behaved as first-in-first-out would give the same answer as
    // long-term-first and prove nothing — which is why the short-term case
    // below is asserted against the same stack.
    assert!(lots[0].acquired_at < lots[1].acquired_at);

    let trades = close_lots_under(
        &mut lots,
        Decimal::from_int(100),
        dec!("70"),
        Decimal::ZERO,
        days_after(500),
        LotSelection::LongTermFirst(us_test()),
        Some(Jurisdiction::UnitedStates),
    );

    assert_eq!(trades.len(), 1);
    assert_eq!(
        trades[0].open_price,
        dec!("50"),
        "the year-old lot was taken"
    );
    assert_eq!(trades[0].term, HoldingTerm::Long);
    // The surviving lot is the recent one.
    assert_eq!(lots.len(), 1);
    assert_eq!(lots[0].price, dec!("60"));
}

#[test]
fn the_short_term_first_selection_closes_the_recent_lot_instead_of_the_oldest() {
    // This is the test that proves the term arm is not first-in-first-out
    // wearing a tax policy's name: the same stack, the same close, and the
    // *other* lot is consumed.
    let mut lots = two_lots_one_long_one_short();
    assert!(lots[0].acquired_at < lots[1].acquired_at);

    let trades = close_lots_under(
        &mut lots,
        Decimal::from_int(100),
        dec!("70"),
        Decimal::ZERO,
        days_after(500),
        LotSelection::ShortTermFirst(us_test()),
        Some(Jurisdiction::UnitedStates),
    );

    assert_eq!(trades.len(), 1);
    assert_eq!(
        trades[0].open_price,
        dec!("60"),
        "the recent lot was taken, not the oldest"
    );
    assert_eq!(trades[0].term, HoldingTerm::Short);
    assert_eq!(lots.len(), 1);
    assert_eq!(lots[0].price, dec!("50"));
}

#[test]
fn a_term_aware_close_against_the_wrong_jurisdiction_reports_every_trade_undetermined() {
    // The fallback is allowed to happen — `close_lots_under` is public and a
    // direct caller may pass a mismatched pair — but it must never be
    // silent. Every trade says the term was not determined, so a caller who
    // asked for a term-aware close is told in the output that no rule
    // applied.
    let mut lots = two_lots_one_long_one_short();

    let trades = close_lots_under(
        &mut lots,
        Decimal::from_int(200),
        dec!("70"),
        Decimal::ZERO,
        days_after(500),
        LotSelection::LongTermFirst(us_test()),
        Some(Jurisdiction::Japan),
    );

    assert_eq!(trades.len(), 2, "both lots were closed");
    assert!(
        trades
            .iter()
            .all(|trade| trade.term == HoldingTerm::Undetermined),
        "a rule that could not apply must not leave a trade claiming a term"
    );
}

// --- what the platform books on the production path -------------------------

#[test]
fn a_realised_gain_booked_through_the_portfolio_carries_the_term_the_rule_gives_it() {
    // The production seam: `Portfolio::apply_fill`, which is what
    // `AccountLedger::apply` in `qip-brokers` and the backtester's rebalance
    // in `qip-simulation-engine` call.
    let object = instrument("AAA", &[Jurisdiction::UnitedStates]);
    let mut book = portfolio();
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("50"),
        dec!("0"),
        now(),
        None,
    );
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("60"),
        dec!("0"),
        days_after(400),
        None,
    );

    // Premise: the jurisdiction arrived from the instrument, so the rule can
    // be attached at all.
    assert_eq!(
        book.position(&object.object_id)
            .expect("a position was opened")
            .jurisdiction(),
        Some(Jurisdiction::UnitedStates)
    );
    book.declare_selection(&object.object_id, LotSelection::LongTermFirst(us_test()))
        .expect("the rule matches the position's jurisdiction");

    book.apply_fill(
        &object,
        Decimal::from_int(-100),
        dec!("70"),
        dec!("0"),
        days_after(500),
        None,
    );

    let position = book.position(&object.object_id).expect("still held");
    let split = position.realised_by_term();
    // Every state is present, so an absent key cannot be read as a measured
    // zero.
    assert_eq!(split.len(), HoldingTerm::ALL.len());
    // The year-old lot at 50 was consumed, not the 400-day-old one at 60.
    assert_eq!(split[&HoldingTerm::Long], Decimal::from_int(2000));
    assert_eq!(split[&HoldingTerm::Short], Decimal::ZERO);
    assert_eq!(split[&HoldingTerm::Undetermined], Decimal::ZERO);
}

#[test]
fn a_gain_booked_with_no_rule_declared_is_reported_undetermined_and_not_short() {
    // `Undetermined` is never folded into `Short`. A desk that finds its
    // whole book undetermined is being told no holding-period rule reached
    // it, which is a different fact from a book of short-term gains and is
    // the one an operator needs.
    let object = instrument("AAA", &[Jurisdiction::UnitedStates]);
    let mut book = portfolio();
    book.apply_fill(
        &object,
        Decimal::from_int(100),
        dec!("50"),
        dec!("0"),
        now(),
        None,
    );
    book.apply_fill(
        &object,
        Decimal::from_int(-100),
        dec!("70"),
        dec!("0"),
        days_after(500),
        None,
    );

    let position = book.position(&object.object_id).expect("still held");
    let split = position.realised_by_term();
    assert_eq!(split[&HoldingTerm::Undetermined], Decimal::from_int(2000));
    assert_eq!(split[&HoldingTerm::Short], Decimal::ZERO);
    assert_eq!(split[&HoldingTerm::Long], Decimal::ZERO);
}
