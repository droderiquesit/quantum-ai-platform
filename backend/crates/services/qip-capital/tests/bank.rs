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
    CurrencyBook, InstrumentClass, PostingBook, PostingState, ReleaseConfirmation, YieldInstrument,
    route_idle_cash, yield_entry_id,
};
use qip_capital::collateral::{
    CollateralAsset, CollateralGraph, MarginDomain, MarginRegime, Rehypothecation,
};
use qip_capital::dated_ladder::DatedLadder;
use qip_capital::treasury::{CapitalBook, Use};
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

fn day(n: i64) -> Duration {
    Duration::from_days(n)
}

/// What a simulated custodian offers. The best yield on the shelf is in a
/// class idle cash may not go into, which is the temptation the rule exists
/// to refuse.
fn shelf() -> Vec<YieldInstrument> {
    let offer = |id: &str, class, days, capacity: &str, rate: &str| YieldInstrument {
        id: id.to_owned(),
        class,
        realisation: day(days),
        capacity: Decimal::parse(capacity).expect("a literal capacity"),
        yield_rate: Decimal::parse(rate).expect("a literal rate"),
    };
    vec![
        offer("overnight", InstrumentClass::Cash, 1, "2000", "0.030"),
        offer(
            "stable-fund",
            InstrumentClass::StableValue,
            2,
            "1500",
            "0.041",
        ),
        offer(
            "bills-30d",
            InstrumentClass::ShortDuration,
            30,
            "1000",
            "0.048",
        ),
        offer(
            "high-yield-credit",
            InstrumentClass::Other,
            5,
            "100000",
            "0.090",
        ),
    ]
}

#[test]
fn idle_cash_above_the_floor_goes_only_into_permitted_classes_and_each_amount_is_dated_at_its_realisation_horizon()
-> Result<()> {
    let now = t(0);
    let mut book = CapitalBook::new(dec!("10000"), dec!("2000"))?;
    book.commit("grant-1", Use::Grant, dec!("4000"))?;
    let mut ladder = DatedLadder::new(now, &[day(1), day(7), day(30), day(90)])?;
    // Premise: 4000 is idle above the floor, nothing is routed, and the shelf
    // really does offer a class that must be refused.
    assert_eq!(book.uncommitted() - book.floor(), dec!("4000"));
    assert_eq!(book.assigned_to(Use::Yield), Decimal::ZERO);
    assert!(shelf().iter().any(|i| !i.class.takes_idle_cash()));

    let placements = route_idle_cash(&mut book, &mut ladder, &shelf(), now)?;

    // Only the three named classes, best yield first, and never the fourth.
    let routed: Vec<(&str, Decimal)> = placements
        .iter()
        .map(|p| (p.instrument.as_str(), p.amount))
        .collect();
    assert_eq!(
        routed,
        vec![
            ("bills-30d", dec!("1000")),
            ("stable-fund", dec!("1500")),
            ("overnight", dec!("1500")),
        ]
    );
    // All the idle cash went, and the floor did not.
    assert_eq!(book.assigned_to(Use::Yield), dec!("4000"));
    assert_eq!(book.uncommitted(), book.floor());

    // Each amount sits on the ladder at the instant it can be had back.
    let instruments = shelf();
    for placement in &placements {
        let instrument = instruments
            .iter()
            .find(|i| i.id == placement.instrument)
            .expect("a placement names an instrument on the shelf");
        let entry = ladder
            .entry(&yield_entry_id(&placement.instrument))
            .expect("every routed amount is on the ladder");
        assert_eq!(entry.amount, placement.amount);
        assert_eq!(entry.at, now.saturating_add(instrument.realisation));
        assert_eq!(placement.available_at, entry.at);
    }
    // So the thirty-day bills are not liquidity a week from now.
    assert_eq!(
        ladder.available_by(now.saturating_add(day(7))),
        dec!("3000")
    );
    assert_eq!(
        ladder.available_by(now.saturating_add(day(30))),
        dec!("4000")
    );
    Ok(())
}

#[test]
fn a_book_at_its_floor_routes_nothing() -> Result<()> {
    let now = t(0);
    let mut book = CapitalBook::new(dec!("1000"), dec!("1000"))?;
    let mut ladder = DatedLadder::new(now, &[day(1)])?;
    assert!(route_idle_cash(&mut book, &mut ladder, &shelf(), now)?.is_empty());
    assert_eq!(book.uncommitted(), dec!("1000"));
    assert_eq!(ladder.total(), Decimal::ZERO);
    Ok(())
}
