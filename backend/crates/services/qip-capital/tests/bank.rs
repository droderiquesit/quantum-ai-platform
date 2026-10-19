//! The Capital Bank's paper records, each against the simulator that stands
//! in for the other side: a venue holding collateral (CAPITAL-011), an FX
//! counterparty quoting a rate (CAPITAL-013) and a custodian's shelf of
//! instruments (CAPITAL-009).
//!
//! The simulators are small on purpose. Each holds only what the other side
//! of the record would hold, so a test can check the two sides agree rather
//! than checking the book against itself.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_capital::bank::{
    CurrencyBook, IdleCash, InstrumentClass, PostingBook, PostingState, ReleaseConfirmation,
    RoutingTerms, Shelf, YieldInstrument, route_idle_cash, yield_entry_id,
};
use qip_capital::collateral::{
    CollateralAsset, CollateralGraph, MarginDomain, MarginRegime, Rehypothecation,
};
use qip_capital::dated_ladder::{DatedLadder, EntryKind, LadderEntry, RedemptionNotice};
use qip_capital::reservation::ReservationLedger;
use qip_capital::survival::{CapitalSurvivalState, SurvivalReport};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use std::collections::{BTreeMap, BTreeSet};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

// --- collateral, against a simulated venue ----------------------------------

/// A venue that holds what was posted to it and confirms a release only of
/// something it holds.
struct SimulatedVenue {
    id: VenueId,
    held: BTreeMap<String, Decimal>,
}

impl SimulatedVenue {
    fn new(id: &str) -> Self {
        Self {
            id: VenueId::new(id),
            held: BTreeMap::new(),
        }
    }

    fn receive(&mut self, posting: &str, amount: Decimal) {
        self.held.insert(posting.to_owned(), amount);
    }

    fn release(&mut self, posting: &str, at: Timestamp) -> Option<ReleaseConfirmation> {
        self.held.remove(posting).map(|amount| ReleaseConfirmation {
            posting: posting.to_owned(),
            venue: self.id.clone(),
            amount,
            at,
        })
    }
}

fn holdings(pairs: &[(&str, Decimal)]) -> BTreeMap<String, Decimal> {
    pairs.iter().map(|(a, v)| ((*a).to_owned(), *v)).collect()
}

#[test]
fn posted_collateral_is_encumbered_from_the_moment_it_is_posted_and_free_again_only_when_the_venue_confirms_its_release()
-> Result<()> {
    let mut venue = SimulatedVenue::new("SIM-A");
    let mut other = SimulatedVenue::new("SIM-B");
    let mut book = PostingBook::new(holdings(&[("UST-2Y", dec!("1000"))]))?;
    // Premise: nothing is posted, so everything held is available. Without
    // this the encumbrance below could have been there all along.
    assert_eq!(book.available("UST-2Y"), dec!("1000"));
    assert_eq!(book.encumbered("UST-2Y"), Decimal::ZERO);

    book.post(
        "p1",
        "UST-2Y",
        venue.id.clone(),
        "margin:SIM-A",
        dec!("600"),
        t(0),
    )?;
    venue.receive("p1", dec!("600"));
    // Encumbered at once, with no step in between.
    assert_eq!(book.encumbered("UST-2Y"), dec!("600"));
    assert_eq!(book.available("UST-2Y"), dec!("400"));

    // The same units cannot back a second obligation.
    let refusal = book
        .post(
            "p2",
            "UST-2Y",
            other.id.clone(),
            "repo:SIM-B",
            dec!("401"),
            t(1),
        )
        .expect_err("encumbered collateral was posted a second time");
    assert!(
        refusal.message().contains("only 400 is unencumbered"),
        "{}",
        refusal.message()
    );
    assert!(book.posting("p2").is_none());

    // Asking for it back frees nothing.
    book.request_recall("p1", t(10))?;
    assert_eq!(book.encumbered("UST-2Y"), dec!("600"));
    assert_eq!(book.available("UST-2Y"), dec!("400"));

    // A release confirmed by a venue that does not hold it frees nothing.
    other.receive("p1", dec!("600"));
    let stranger = other.release("p1", t(11)).expect("the other venue answers");
    assert!(book.confirm_release(&stranger).is_err());
    assert_eq!(book.available("UST-2Y"), dec!("400"));

    // The venue's own confirmation does.
    let confirmation = venue.release("p1", t(12)).expect("the venue holds p1");
    book.confirm_release(&confirmation)?;
    assert_eq!(book.encumbered("UST-2Y"), Decimal::ZERO);
    assert_eq!(book.available("UST-2Y"), dec!("1000"));
    assert_eq!(
        book.posting("p1").map(|p| p.state),
        Some(PostingState::Released { at: t(12) })
    );
    Ok(())
}

#[test]
fn a_release_nobody_recalled_is_refused_rather_than_taken_as_freeing_the_collateral() -> Result<()>
{
    let mut venue = SimulatedVenue::new("SIM-A");
    let mut book = PostingBook::new(holdings(&[("UST-2Y", dec!("100"))]))?;
    book.post(
        "p1",
        "UST-2Y",
        venue.id.clone(),
        "margin",
        dec!("100"),
        t(0),
    )?;
    venue.receive("p1", dec!("100"));
    let unsolicited = venue.release("p1", t(5)).expect("the venue holds p1");
    let refusal = book
        .confirm_release(&unsolicited)
        .expect_err("an unrequested release freed collateral");
    assert!(
        refusal.message().contains("request the recall first"),
        "{}",
        refusal.message()
    );
    assert_eq!(book.available("UST-2Y"), Decimal::ZERO);
    Ok(())
}

#[test]
fn the_collateral_model_is_built_from_what_the_book_says_is_posted() -> Result<()> {
    let venue = VenueId::new("SIM-A");
    let mut book = PostingBook::new(holdings(&[("UST-2Y", dec!("1000"))]))?;
    // Premise: an empty book pledges nothing.
    assert!(book.pledges().is_empty());
    book.post("p1", "UST-2Y", venue.clone(), "margin-1", dec!("300"), t(0))?;
    book.post("p2", "UST-2Y", venue.clone(), "margin-2", dec!("200"), t(1))?;

    let graph = CollateralGraph::build(
        vec![CollateralAsset {
            id: "UST-2Y".into(),
            value: dec!("1000"),
            haircut: dec!("0"),
            driver: None,
        }],
        vec![MarginDomain {
            venue: venue.clone(),
            regime: MarginRegime::Isolated,
            rehypothecation: Rehypothecation::Forbidden,
            initial: dec!("400"),
            maintenance: dec!("300"),
            drivers: BTreeSet::new(),
        }],
        book.pledges(),
        Vec::new(),
    )?;
    let coverage = graph.coverage()?;
    let at_venue = coverage.get(&venue).expect("the venue has coverage");
    assert_eq!(at_venue.effective(), dec!("500"));

    // Released collateral leaves the model with the confirmation, not before.
    book.request_recall("p2", t(2))?;
    assert_eq!(book.pledges()[0].amount, dec!("500"));
    book.confirm_release(&ReleaseConfirmation {
        posting: "p2".into(),
        venue,
        amount: dec!("200"),
        at: t(3),
    })?;
    assert_eq!(book.pledges()[0].amount, dec!("300"));
    Ok(())
}

#[test]
fn across_random_postings_recalls_and_releases_collateral_only_ever_becomes_available_on_a_confirmation()
-> Result<()> {
    let mut venue = SimulatedVenue::new("SIM-A");
    let mut book = PostingBook::new(holdings(&[("BOND", dec!("5000"))]))?;
    let mut rng = Xoshiro256::seeded(0x00CA_0011);
    let (mut posted, mut refused, mut recalled, mut confirmed) = (0, 0, 0, 0);
    for step in 0..600u64 {
        let before = book.available("BOND");
        let id = format!("p{}", rng.below(60));
        let mut was_confirmation = false;
        match rng.below(3) {
            0 => {
                let amount = Decimal::from_int(1 + rng.below(900) as i64);
                match book.post(
                    &id,
                    "BOND",
                    venue.id.clone(),
                    "margin",
                    amount,
                    t(step as i64),
                ) {
                    Ok(()) => {
                        venue.receive(&id, amount);
                        posted += 1;
                    }
                    Err(_) => refused += 1,
                }
            }
            1 => recalled += usize::from(book.request_recall(&id, t(step as i64)).is_ok()),
            _ => {
                // The venue confirms only what the bank has asked back.
                let asked = matches!(
                    book.posting(&id).map(|p| p.state),
                    Some(PostingState::RecallRequested { .. })
                );
                if asked && let Some(confirmation) = venue.release(&id, t(step as i64)) {
                    book.confirm_release(&confirmation)?;
                    confirmed += 1;
                    was_confirmation = true;
                }
            }
        }
        let after = book.available("BOND");
        assert_eq!(after + book.encumbered("BOND"), dec!("5000"), "step {step}");
        assert!(!after.is_negative(), "step {step}: more posted than held");
        if after > before {
            assert!(
                was_confirmation,
                "step {step}: collateral was freed without a confirmation"
            );
        }
    }
    // Premise: the sweep posted, was refused, recalled and released.
    assert!(
        posted > 10 && refused > 10 && recalled > 10 && confirmed > 10,
        "{posted} {refused} {recalled} {confirmed}"
    );
    Ok(())
}

// --- currency conversion, against a simulated FX counterparty ---------------

/// The other side of every conversion: it quotes a rate and keeps its own
/// two balances, so the test can check both parties' books move together.
struct SimulatedFxCounterparty {
    rate: Decimal,
    balances: BTreeMap<Currency, Decimal>,
}

impl SimulatedFxCounterparty {
    fn settle(&mut self, receives: Currency, received: Decimal, pays: Currency, paid: Decimal) {
        *self.balances.entry(receives).or_insert(Decimal::ZERO) += received;
        *self.balances.entry(pays).or_insert(Decimal::ZERO) -= paid;
    }

    fn balance(&self, currency: Currency) -> Decimal {
        self.balances
            .get(&currency)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }
}

#[test]
fn a_conversion_debits_one_currency_and_credits_the_other_at_the_recorded_rate_and_both_ledgers_balance()
-> Result<()> {
    let mut counterparty = SimulatedFxCounterparty {
        rate: dec!("0.92"),
        balances: BTreeMap::from([(Currency::EUR, dec!("50000"))]),
    };
    let mut book = CurrencyBook::new();
    book.fund(Currency::USD, dec!("10000"))?;
    // Premise: the book holds dollars and no euros, and nothing is journaled.
    assert_eq!(book.balance(Currency::USD), dec!("10000"));
    assert_eq!(book.balance(Currency::EUR), Decimal::ZERO);
    assert!(book.journal().is_empty());
    let usd_in_the_world = book.balance(Currency::USD) + counterparty.balance(Currency::USD);
    let eur_in_the_world = book.balance(Currency::EUR) + counterparty.balance(Currency::EUR);

    let done = book.convert(
        "fx-1",
        "SIM-FX",
        Currency::USD,
        dec!("2500"),
        Currency::EUR,
        counterparty.rate,
        t(0),
    )?;
    counterparty.settle(done.sold, done.sold_amount, done.bought, done.bought_amount);

    // Debited and credited at the recorded rate, exactly.
    assert_eq!(done.rate, dec!("0.92"));
    assert_eq!(done.bought_amount, dec!("2300"));
    assert_eq!(book.balance(Currency::USD), dec!("7500"));
    assert_eq!(book.balance(Currency::EUR), dec!("2300"));
    // Both currencies balance across the two parties: nothing was created.
    assert_eq!(
        book.balance(Currency::USD) + counterparty.balance(Currency::USD),
        usd_in_the_world
    );
    assert_eq!(
        book.balance(Currency::EUR) + counterparty.balance(Currency::EUR),
        eur_in_the_world
    );

    // And the journal alone rebuilds the balances.
    let mut replayed = BTreeMap::from([(Currency::USD, dec!("10000"))]);
    for conversion in book.journal() {
        *replayed.entry(conversion.sold).or_insert(Decimal::ZERO) -= conversion.sold_amount;
        *replayed.entry(conversion.bought).or_insert(Decimal::ZERO) += conversion.bought_amount;
    }
    assert_eq!(replayed[&Currency::USD], book.balance(Currency::USD));
    assert_eq!(replayed[&Currency::EUR], book.balance(Currency::EUR));
    Ok(())
}

#[test]
fn a_conversion_larger_than_the_balance_is_refused_whole_and_moves_neither_currency() -> Result<()>
{
    let mut book = CurrencyBook::new();
    book.fund(Currency::USD, dec!("100"))?;
    let refusal = book
        .convert(
            "fx-1",
            "SIM-FX",
            Currency::USD,
            dec!("100.01"),
            Currency::JPY,
            dec!("150"),
            t(0),
        )
        .expect_err("a conversion beyond the balance was made");
    assert!(
        refusal.message().contains("the book holds 100"),
        "{}",
        refusal.message()
    );
    assert_eq!(book.balance(Currency::USD), dec!("100"));
    assert_eq!(book.balance(Currency::JPY), Decimal::ZERO);
    assert!(book.journal().is_empty());
    Ok(())
}

// --- idle cash, against a simulated custodian's shelf ------------------------

fn hours(n: i64) -> Duration {
    Duration::from_hours(n)
}

fn day(n: i64) -> Duration {
    Duration::from_days(n)
}

fn instrument(
    id: &str,
    class: InstrumentClass,
    realisation: Duration,
    capacity: &str,
    rate: &str,
) -> YieldInstrument {
    YieldInstrument {
        id: id.to_owned(),
        class,
        realisation,
        capacity: Decimal::parse(capacity).expect("a literal capacity"),
        yield_rate: Decimal::parse(rate).expect("a literal rate"),
    }
}

/// What a simulated custodian offers. The best yield on the shelf is in a
/// class idle cash may not go into, which is the temptation the rule exists
/// to refuse. Every routable instrument is realised within the day a
/// reservation may hold capital (ADR 0159); the longer ones the blueprint
/// names are the cost that decision accepted and
/// `a_shelf_instrument_realised_past_the_reservation_ceiling_is_refused_at_load`
/// pins.
fn shelf() -> Shelf {
    Shelf::new(vec![
        instrument(
            "overnight",
            InstrumentClass::Cash,
            hours(6),
            "2000",
            "0.030",
        ),
        instrument(
            "stable-fund",
            InstrumentClass::StableValue,
            hours(12),
            "1500",
            "0.041",
        ),
        instrument(
            "bills-1d",
            InstrumentClass::ShortDuration,
            hours(24),
            "1000",
            "0.048",
        ),
        instrument(
            "high-yield-credit",
            InstrumentClass::Other,
            day(5),
            "100000",
            "0.090",
        ),
    ])
    .expect("a valid shelf")
}

fn terms(share: &str) -> RoutingTerms {
    RoutingTerms::new(
        Decimal::parse(share).expect("a literal share"),
        vec![hours(24), day(7), day(30)],
    )
    .expect("valid terms")
}

/// A waterfall that leaves `opportunity` over: equity 10,000 less a 2,000
/// emergency reserve and whatever is held.
fn waterfall(opportunity: Decimal) -> SurvivalReport {
    let withheld = dec!("10000") - dec!("2000") - opportunity;
    CapitalSurvivalState::new(dec!("10000"))
        .unwrap()
        .with_nav_floor(dec!("2000"))
        .unwrap()
        .with_reserved_cash(withheld)
        .unwrap()
        .report()
        .unwrap()
}

#[test]
fn idle_cash_goes_only_into_permitted_classes_is_held_in_the_ledger_and_each_amount_is_dated_at_its_realisation_horizon()
-> Result<()> {
    let now = t(0);
    let mut reservations = ReservationLedger::new(dec!("10000"))?;
    let mut ladder = DatedLadder::new(now, &[hours(24), day(7), day(30)])?;
    let report = waterfall(dec!("4000"));
    // Premise: 4000 is left over, nothing is routed, and the shelf really
    // does offer a class that must be refused.
    assert_eq!(report.opportunity_capital, dec!("4000"));
    assert_eq!(reservations.reserved_total(), Decimal::ZERO);
    assert!(
        shelf()
            .instruments()
            .iter()
            .any(|i| !i.class.takes_idle_cash())
    );

    let idle = IdleCash::from_waterfall(&report, Decimal::ZERO)?;
    let outcome = route_idle_cash(
        idle,
        &shelf(),
        &terms("1"),
        &mut reservations,
        &mut ladder,
        now,
    )?;

    // Only the three named classes, best yield first, and never the fourth.
    let routed: Vec<(&str, Decimal)> = outcome
        .placements
        .iter()
        .map(|p| (p.instrument.as_str(), p.amount))
        .collect();
    assert_eq!(
        routed,
        vec![
            ("bills-1d", dec!("1000")),
            ("stable-fund", dec!("1500")),
            ("overnight", dec!("1500")),
        ]
    );
    // All the idle cash went, as holds the ledger now carries.
    assert_eq!(reservations.reserved_total(), dec!("4000"));
    assert_eq!(reservations.free(now), dec!("6000"));

    // Each amount sits on the ladder at the instant it can be had back.
    let shelf = shelf();
    for placement in &outcome.placements {
        let instrument = shelf
            .instruments()
            .iter()
            .find(|i| i.id == placement.instrument)
            .expect("a placement names an instrument on the shelf");
        let entry = ladder
            .entry(&yield_entry_id(&placement.instrument))
            .expect("every routed amount is on the ladder");
        assert_eq!(entry.amount, placement.amount);
        assert_eq!(entry.at, now.saturating_add(instrument.realisation));
        assert_eq!(placement.available_at, entry.at);
        // And the hold lapses at the same instant, so the ledger and the
        // ladder cannot disagree about when the cash is back.
        let hold = reservations
            .reservation(&yield_entry_id(&placement.instrument))
            .expect("every placement is held");
        assert_eq!(hold.expires_at, entry.at);
    }
    assert_eq!(
        ladder.available_by(now.saturating_add(hours(24))),
        dec!("4000")
    );
    assert_eq!(
        ladder.available_by(now.saturating_add(hours(6))),
        Decimal::ZERO
    );
    Ok(())
}

#[test]
fn nothing_left_unreleased_routes_nothing() -> Result<()> {
    let now = t(0);
    let mut reservations = ReservationLedger::new(dec!("10000"))?;
    let mut ladder = DatedLadder::new(now, &[hours(24)])?;
    // Premise: the cycle released everything the waterfall left.
    let report = waterfall(dec!("3000"));
    let idle = IdleCash::from_waterfall(&report, dec!("3000"))?;
    assert_eq!(idle.amount(), Decimal::ZERO);
    let outcome = route_idle_cash(
        idle,
        &shelf(),
        &terms("1"),
        &mut reservations,
        &mut ladder,
        now,
    )?;
    assert!(outcome.placements.is_empty());
    assert_eq!(reservations.reserved_total(), Decimal::ZERO);
    assert_eq!(ladder.total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn an_empty_shelf_routes_nothing_whatever_is_idle() -> Result<()> {
    let now = t(0);
    let mut reservations = ReservationLedger::new(dec!("10000"))?;
    let mut ladder = DatedLadder::new(now, &[hours(24)])?;
    let idle = IdleCash::from_waterfall(&waterfall(dec!("4000")), Decimal::ZERO)?;
    // Premise: there is idle cash to route.
    assert_eq!(idle.amount(), dec!("4000"));
    let outcome = route_idle_cash(
        idle,
        &Shelf::empty(),
        &terms("1"),
        &mut reservations,
        &mut ladder,
        now,
    )?;
    assert!(outcome.placements.is_empty());
    assert_eq!(reservations.reserved_total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn only_the_declared_share_of_the_unreleased_capital_is_routed() -> Result<()> {
    let now = t(0);
    let mut reservations = ReservationLedger::new(dec!("10000"))?;
    let mut ladder = DatedLadder::new(now, &[hours(24)])?;
    let idle = IdleCash::from_waterfall(&waterfall(dec!("4000")), Decimal::ZERO)?;
    let outcome = route_idle_cash(
        idle,
        &shelf(),
        &terms("0.25"),
        &mut reservations,
        &mut ladder,
        now,
    )?;
    // A quarter of 4000 is 1000, taken by the best yield; the rest stays free.
    let routed: Vec<(&str, Decimal)> = outcome
        .placements
        .iter()
        .map(|p| (p.instrument.as_str(), p.amount))
        .collect();
    assert_eq!(routed, vec![("bills-1d", dec!("1000"))]);
    assert_eq!(reservations.reserved_total(), dec!("1000"));
    Ok(())
}

#[test]
fn idle_cash_cannot_be_built_from_a_release_the_waterfall_did_not_leave() {
    let report = waterfall(dec!("4000"));
    assert!(IdleCash::from_waterfall(&report, dec!("4000.01")).is_err());
    assert!(IdleCash::from_waterfall(&report, dec!("-1")).is_err());
    assert_eq!(
        IdleCash::from_waterfall(&report, dec!("1500"))
            .unwrap()
            .amount(),
        dec!("2500")
    );
}

#[test]
fn a_shelf_instrument_realised_past_the_reservation_ceiling_is_refused_at_load() {
    let refused = Shelf::new(vec![instrument(
        "bills-30d",
        InstrumentClass::ShortDuration,
        day(30),
        "1000",
        "0.048",
    )]);
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("bills-30d"), "{message}");
    assert!(message.contains("24.0"), "names the ceiling: {message}");
    // Exactly the ceiling is admitted: the boundary is the day, not before it.
    assert!(
        Shelf::new(vec![instrument(
            "bills-1d",
            InstrumentClass::ShortDuration,
            hours(24),
            "1",
            "0.01"
        )])
        .is_ok()
    );
    // A class that is never routed may be listed with any realisation.
    assert!(
        Shelf::new(vec![instrument(
            "credit",
            InstrumentClass::Other,
            day(30),
            "1",
            "0.09"
        )])
        .is_ok()
    );
}

#[test]
fn a_shelf_refuses_a_duplicate_id_a_blank_id_a_zero_capacity_and_a_negative_yield() {
    let good = || instrument("a", InstrumentClass::Cash, hours(1), "10", "0.01");
    assert!(Shelf::new(vec![good(), good()]).is_err());
    let mut blank = good();
    blank.id = " ".into();
    assert!(Shelf::new(vec![blank]).is_err());
    let mut nothing = good();
    nothing.capacity = Decimal::ZERO;
    assert!(Shelf::new(vec![nothing]).is_err());
    let mut negative = good();
    negative.yield_rate = dec!("-0.01");
    assert!(Shelf::new(vec![negative]).is_err());
    let mut instant = good();
    instant.realisation = Duration::from_nanos(0);
    assert!(Shelf::new(vec![instant]).is_err());
}

#[test]
fn routing_terms_refuse_a_share_outside_zero_to_one_and_unordered_horizons() {
    assert!(RoutingTerms::new(Decimal::ZERO, vec![hours(1)]).is_err());
    assert!(RoutingTerms::new(dec!("1.01"), vec![hours(1)]).is_err());
    assert!(RoutingTerms::new(dec!("0.5"), vec![]).is_err());
    assert!(RoutingTerms::new(dec!("0.5"), vec![day(2), day(1)]).is_err());
    assert!(RoutingTerms::new(dec!("1"), vec![hours(1)]).is_ok());
}

#[test]
fn a_declared_outflow_before_an_instruments_availability_refuses_the_placement_while_a_later_one_does_not()
-> Result<()> {
    let now = t(0);
    let notice = |id: &str, hours_out: i64| {
        RedemptionNotice::new(
            id,
            dec!("500"),
            now.saturating_add(hours(hours_out)),
            "operator-1",
            now,
        )
    };
    let route = |outflow_in_hours: i64| -> Result<_> {
        let mut reservations = ReservationLedger::new(dec!("10000"))?;
        let mut ladder = DatedLadder::new(now, &[hours(24), day(7)])?;
        ladder.place(notice("n-1", outflow_in_hours)?.as_outflow())?;
        let idle = IdleCash::from_waterfall(&waterfall(dec!("4000")), Decimal::ZERO)?;
        let outcome = route_idle_cash(
            idle,
            &shelf(),
            &terms("1"),
            &mut reservations,
            &mut ladder,
            now,
        )?;
        Ok((outcome, reservations))
    };

    // Premise: with no outflow all three routable instruments are placed.
    let mut reservations = ReservationLedger::new(dec!("10000"))?;
    let mut ladder = DatedLadder::new(now, &[hours(24), day(7)])?;
    let idle = IdleCash::from_waterfall(&waterfall(dec!("4000")), Decimal::ZERO)?;
    let clear = route_idle_cash(
        idle,
        &shelf(),
        &terms("1"),
        &mut reservations,
        &mut ladder,
        now,
    )?;
    assert_eq!(clear.placements.len(), 3);

    // An outflow in 8 hours: the 6-hour instrument can be had back first; the
    // 12- and 24-hour ones cannot, and the refusal names the notice.
    let (early, held) = route(8)?;
    let placed: Vec<&str> = early
        .placements
        .iter()
        .map(|p| p.instrument.as_str())
        .collect();
    assert_eq!(placed, vec!["overnight"]);
    assert_eq!(early.refused.len(), 2);
    assert!(
        early
            .refused
            .iter()
            .all(|r| r.reason.contains("notice:n-1")),
        "{:?}",
        early.refused
    );
    assert_eq!(
        held.reserved_total(),
        dec!("2000"),
        "a refused instrument holds nothing"
    );

    // The same notice a day and a half out refuses nothing.
    let (late, _) = route(36)?;
    assert_eq!(late.placements.len(), 3);
    assert!(late.refused.is_empty());
    Ok(())
}

#[test]
fn an_instrument_still_held_from_an_earlier_pass_is_skipped_not_topped_up() -> Result<()> {
    let now = t(0);
    let mut reservations = ReservationLedger::new(dec!("10000"))?;
    let mut ladder = DatedLadder::new(now, &[hours(24)])?;
    let idle = || IdleCash::from_waterfall(&waterfall(dec!("4000")), Decimal::ZERO);
    let first = route_idle_cash(
        idle()?,
        &shelf(),
        &terms("1"),
        &mut reservations,
        &mut ladder,
        now,
    )?;
    assert_eq!(first.placements.len(), 3);
    let held = reservations.reserved_total();
    let mut again = DatedLadder::new(now, &[hours(24)])?;
    let second = route_idle_cash(
        idle()?,
        &shelf(),
        &terms("1"),
        &mut reservations,
        &mut again,
        now,
    )?;
    assert!(second.placements.is_empty());
    assert_eq!(second.refused.len(), 3);
    assert_eq!(reservations.reserved_total(), held);
    // Once the holds lapse the instruments are routable again.
    let later = now.saturating_add(hours(25));
    let mut fresh = DatedLadder::new(later, &[hours(24)])?;
    let third = route_idle_cash(
        idle()?,
        &shelf(),
        &terms("1"),
        &mut reservations,
        &mut fresh,
        later,
    )?;
    assert_eq!(third.placements.len(), 3);
    Ok(())
}

#[test]
fn the_earliest_outflow_is_the_earliest_by_instant_then_id_and_ignores_other_kinds() -> Result<()> {
    let now = t(0);
    let entry = |id: &str, kind, at: i64| LadderEntry {
        id: id.to_owned(),
        kind,
        amount: dec!("1"),
        at: now.saturating_add(hours(at)),
    };
    let mut ladder = DatedLadder::new(now, &[hours(24)])?;
    assert!(ladder.earliest_outflow().is_none());
    ladder.place(entry("balance-early", EntryKind::Balance, 1))?;
    assert!(ladder.earliest_outflow().is_none(), "a balance is not owed");
    ladder.place(entry("b", EntryKind::Outflow, 5))?;
    ladder.place(entry("a", EntryKind::Outflow, 5))?;
    ladder.place(entry("late", EntryKind::Outflow, 9))?;
    assert_eq!(ladder.earliest_outflow().map(|e| e.id.as_str()), Some("a"));
    Ok(())
}

#[test]
fn a_redemption_notice_refuses_a_back_dated_blank_or_non_positive_declaration() {
    let at = t(0);
    let ok = RedemptionNotice::new("n-1", dec!("1"), at, "operator-1", at);
    assert!(ok.is_ok());
    assert_eq!(ok.unwrap().entry_id(), "notice:n-1");
    assert!(RedemptionNotice::new("n 1", dec!("1"), at, "operator-1", at).is_err());
    assert!(RedemptionNotice::new("", dec!("1"), at, "operator-1", at).is_err());
    assert!(RedemptionNotice::new("n-1", Decimal::ZERO, at, "operator-1", at).is_err());
    assert!(RedemptionNotice::new("n-1", dec!("1"), at, " ", at).is_err());
    assert!(
        RedemptionNotice::new(
            "n-1",
            dec!("1"),
            at,
            "operator-1",
            at.saturating_add(hours(1))
        )
        .is_err()
    );
}

#[test]
fn a_ladder_rebuilt_from_the_same_facts_is_the_same_ladder() -> Result<()> {
    let now = t(0);
    let facts = || {
        vec![
            LadderEntry {
                id: "b".into(),
                kind: EntryKind::Balance,
                amount: dec!("2"),
                at: now.saturating_add(hours(3)),
            },
            LadderEntry {
                id: "a".into(),
                kind: EntryKind::Outflow,
                amount: dec!("1"),
                at: now.saturating_add(hours(5)),
            },
        ]
    };
    let one = DatedLadder::rebuilt(now, &[hours(24)], facts())?;
    let two = DatedLadder::rebuilt(now, &[hours(24)], facts().into_iter().rev())?;
    assert_eq!(one.buckets(), two.buckets());
    // A duplicate fact is refused, as `place` refuses it.
    let mut doubled = facts();
    doubled.extend(facts());
    assert!(DatedLadder::rebuilt(now, &[hours(24)], doubled).is_err());
    Ok(())
}
