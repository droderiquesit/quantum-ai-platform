//! The per-user ledger past the desk, and the fabric journal in the loop.
//!
//! Three seams, each proven by driving the thing that should reach it. A
//! settled fill is booked across the users whose capital the strategy was
//! trading, exactly, with the rounding unit named; with no user enrolled it
//! is booked to the desk whole and the log says so; and every wallet,
//! corridor and destination decision the kernel makes is a record the
//! platform's own event log replays to the live fabric state.
//!
//! Every test asserts its premise before the property: a split of nothing
//! sums to nothing, and a replay of an empty log rebuilds an empty state,
//! so each proves the fill, the enrolment or the record exists first.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::ledger::{
    Jurisdiction, Mandate, MandateId, MandateTerms, PermittedFamilies, UserId, UserShare,
};
use qip_capital_fabric::corridor::{CorridorCaps, CorridorId, PermittedHours};
use qip_capital_fabric::custody::{CorridorKind, CustodyClass};
use qip_capital_fabric::destination::{Approver, Asset as DestinationAsset, DestinationKey};
use qip_capital_fabric::journal::{CorridorAction, DestinationAction, FabricCommand, Outcome};
use qip_capital_fabric::{CapitalLocation, Region};
use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::{FillRecord, FillShare};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ObjectId, dec};
use qip_events::{EventFilter, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::central::CellReport;
use qip_kernel::config::{PlatformConfig, UserMandate};
use qip_kernel::cycle::Stage;
use qip_kernel::platform::{BookingBasis, LedgerEntry, Platform};
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

const CELL: &str = "cell-lon-1";
const INSTRUMENT: &str = "obj-AAA";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(INSTRUMENT),
            "AAA",
            InstrumentType::CommonStock,
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn limits() -> LimitSet {
    LimitSet::new("ledger-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

/// A user mandate under the desk's: a thousand under management, every
/// family, no floor, in the desk's currency.
fn mandate(capital: Decimal) -> Result<Mandate> {
    Mandate::new(MandateTerms {
        capital,
        currency: Currency::USD,
        risk_tolerance: Decimal::ONE,
        permitted_families: PermittedFamilies::Any,
        liquidity_floor: Decimal::ZERO,
        exploration_share: Decimal::ZERO,
        jurisdiction: Jurisdiction::new("GB")?,
    })
}

fn enrolment(user: &str, capital: Decimal) -> Result<UserMandate> {
    Ok(UserMandate {
        user: UserId::new(user)?,
        id: MandateId::new(format!("mandate-{user}"))?,
        mandate: mandate(capital)?,
    })
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe()?, limits())
}

/// One order sent and filled whole for `alpha`, as a cell reports it — the
/// only road into a user's book is a report the centre accepted.
fn report(order_id: &str, side: BookSide, quantity: Decimal, price: Decimal) -> CellReport {
    let strategy = StrategyId::new("alpha");
    let order = DeltaOrder {
        order_id: order_id.to_string(),
        strategy: strategy.clone(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        side,
        quantity,
        price,
        simulated: true,
        contributors: vec![Contributor {
            strategy: strategy.clone(),
            signed_size: quantity,
            inputs: vec![("alpha-feature".to_string(), 1)],
        }],
    };
    let fill = FillRecord {
        order_id: order_id.to_string(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        side,
        quantity,
        price,
        simulated: true,
        at: start(),
        shares: vec![FillShare { strategy, quantity }],
    };
    CellReport::new(CELL, start())
        .with_orders(vec![order])
        .with_fills(vec![fill])
}

/// A buy at 50 and a sell at 60 on a hundred: a realised thousand for
/// `alpha`, which the centre's attribution states and the ledger books.
fn realise_a_thousand(platform: &mut Platform) -> Result<()> {
    let bought = platform.ingest_cell_report(
        report("ord-1", BookSide::Ask, dec!("100"), dec!("50")),
        start(),
    )?;
    assert_eq!(bought.settlement.fills_settled, 1, "the buy settled");
    let sold = platform.ingest_cell_report(
        report("ord-2", BookSide::Bid, dec!("100"), dec!("60")),
        start(),
    )?;
    assert_eq!(sold.settlement.fills_settled, 1, "the sell settled");
    let attributed = sold
        .settlement
        .by_strategy()
        .get("alpha")
        .copied()
        .expect("the sell is attributed to alpha");
    assert_eq!(
        attributed,
        dec!("1000"),
        "the premise is a realised thousand"
    );
    Ok(())
}

/// Every ledger entry the journal holds, oldest first.
fn ledger_entries(platform: &Platform) -> Result<Vec<LedgerEntry>> {
    platform
        .replay_journal(&EventFilter::new().topic(Topic::AttributionCompleted))?
        .iter()
        .map(|event| event.decode::<LedgerEntry>().map(|envelope| envelope.body))
        .collect()
}

fn settled(platform: &Platform, user: &UserId, strategy: &StrategyId) -> Option<Decimal> {
    platform
        .user_ledger()
        .balance(user, strategy, Currency::USD)
        .map(qip_capital::ledger::CashBalance::settled)
}

// --- the split ----------------------------------------------------------------

#[test]
fn a_fill_across_two_entitled_users_books_two_shares_that_sum_to_the_fill_with_the_remainder_recorded()
-> Result<()> {
    // The failure this closes: every settled fill was booked to the desk
    // whole, so two users whose capital a strategy was trading were
    // attributed nothing, and the one book that moved was the one book
    // nobody's mandate described. A thousand split one to two across a
    // hundred and two hundred at work does not divide in nine decimals —
    // the unit that truncation leaves has to go somewhere named, or the
    // shares sum to less than the fill and a residual reappears one link
    // below the attribution that closed to zero.
    let alice = UserId::new("alice")?;
    let bob = UserId::new("bob")?;
    let alpha = StrategyId::new("alpha");
    let config = PlatformConfig::default().with_user_mandates(vec![
        enrolment("alice", dec!("1000"))?,
        enrolment("bob", dec!("1000"))?,
    ]);
    let mut platform = platform(config)?;

    // Premise: both users hold a mandate under the desk's, both have
    // capital at work at alpha in a one-to-two ratio, the desk has no book
    // there, and nothing has been booked yet.
    assert_eq!(
        platform.user_ledger().mandates().len(),
        3,
        "the desk and two users"
    );
    platform.fund_user(&alice, &alpha, dec!("100"), start())?;
    platform.fund_user(&bob, &alpha, dec!("200"), start())?;
    assert_eq!(settled(&platform, &alice, &alpha), Some(dec!("100")));
    assert_eq!(settled(&platform, &bob, &alpha), Some(dec!("200")));
    assert!(
        platform
            .user_ledger()
            .book(platform.user_ledger().desk(), &alpha)
            .is_none()
    );
    assert_eq!(platform.user_ledger().fills_journalled(), 0);
    let funded = ledger_entries(&platform)?;
    assert_eq!(
        funded.len(),
        2,
        "each funding is a journal entry: {funded:?}"
    );

    realise_a_thousand(&mut platform)?;

    // The shares: a third and two thirds, truncated, and the ninth-decimal
    // unit the truncation left assigned to the larger holder and named.
    let entries = ledger_entries(&platform)?;
    let booked: Vec<&LedgerEntry> = entries
        .iter()
        .filter(|entry| matches!(entry, LedgerEntry::Booked { .. }))
        .collect();
    assert_eq!(
        booked.len(),
        2,
        "the buy and the sell were each booked: {entries:?}"
    );
    let LedgerEntry::Booked {
        strategy,
        amount,
        basis,
        ..
    } = booked[1]
    else {
        panic!("the sell's booking is a Booked entry: {:?}", booked[1]);
    };
    assert_eq!(strategy, &alpha);
    assert_eq!(*amount, dec!("1000"));
    let BookingBasis::ProRata {
        shares,
        entitlement_total,
        remainder,
        remainder_to,
    } = basis
    else {
        panic!("a fill with two entitled users was not split pro rata: {basis:?}");
    };
    assert_eq!(
        shares,
        &vec![
            UserShare {
                user: alice.clone(),
                amount: dec!("333.333333333"),
            },
            UserShare {
                user: bob.clone(),
                amount: dec!("666.666666667"),
            },
        ],
        "two shares in user order, the remainder folded into the larger"
    );
    let summed: Decimal = shares.iter().map(|share| share.amount).sum();
    assert_eq!(summed, *amount, "the shares sum to the fill exactly");
    assert_eq!(*entitlement_total, dec!("300"));
    assert_eq!(
        *remainder,
        dec!("0.000000001"),
        "the truncated unit is on the record"
    );
    assert_eq!(
        remainder_to, &bob,
        "the unit went to the larger entitlement"
    );

    // And the books moved by exactly those shares — neither to the desk.
    assert_eq!(
        settled(&platform, &alice, &alpha),
        Some(dec!("433.333333333"))
    );
    assert_eq!(
        settled(&platform, &bob, &alpha),
        Some(dec!("866.666666667"))
    );
    assert!(
        platform
            .user_ledger()
            .book(platform.user_ledger().desk(), &alpha)
            .is_none(),
        "the desk was booked a share of a fill two users were entitled to"
    );
    assert_eq!(platform.user_ledger().fills_journalled(), 2);
    Ok(())
}

#[test]
fn an_empty_registry_books_the_desk_whole_and_the_journal_says_so() -> Result<()> {
    // The failure this guards: the desk-whole booking surviving as the
    // silent default it used to be, so a reader of a desk balance could not
    // tell "no user was enrolled" from "the split was skipped". With no
    // user mandate registered the desk is the only holder and takes the
    // fill whole — and the record names that as the basis.
    let alpha = StrategyId::new("alpha");
    let mut platform = platform(PlatformConfig::default())?;
    let desk = platform.user_ledger().desk().clone();

    // Premise: the desk alone holds a mandate and no book.
    assert_eq!(platform.user_ledger().mandates().len(), 1, "the desk alone");
    assert!(platform.user_ledger().book(&desk, &alpha).is_none());
    assert!(ledger_entries(&platform)?.is_empty(), "nothing booked yet");

    realise_a_thousand(&mut platform)?;

    assert_eq!(settled(&platform, &desk, &alpha), Some(dec!("1000")));
    let entries = ledger_entries(&platform)?;
    assert_eq!(
        entries.len(),
        2,
        "the buy and the sell were each journalled"
    );
    for entry in &entries {
        let LedgerEntry::Booked { basis, .. } = entry else {
            panic!("a booking is a Booked entry: {entry:?}");
        };
        let BookingBasis::DeskWhole { user, reason } = basis else {
            panic!("with no user enrolled the basis is the desk whole: {basis:?}");
        };
        assert_eq!(user, &desk);
        assert_eq!(
            reason, "no user mandate is registered; the desk is the only holder",
            "the record says why the desk took it"
        );
    }
    Ok(())
}

#[test]
fn a_user_mandate_the_desk_cannot_cover_stops_assembly_rather_than_opening_a_book() -> Result<()> {
    // Refuse, never invent: a configuration naming more capital under a
    // user than the desk holds is a promise the desk cannot keep, and a
    // platform that assembled anyway would book fills to it.
    let desk_capital = PlatformConfig::default().initial_equity;
    let over = desk_capital + Decimal::ONE;
    // Premise: the same enrolment at the desk's capital assembles.
    assert!(
        platform(
            PlatformConfig::default().with_user_mandates(vec![enrolment("carol", desk_capital)?])
        )
        .is_ok(),
        "a mandate exactly at the ceiling is admitted"
    );
    let refused =
        platform(PlatformConfig::default().with_user_mandates(vec![enrolment("carol", over)?]));
    let error = match refused {
        Ok(_) => panic!("a mandate above the desk's capital assembled a platform"),
        Err(error) => error,
    };
    assert!(
        error
            .message()
            .contains("no user mandate exceeds the desk's capital"),
        "the refusal does not name the term: {}",
        error.message()
    );
    Ok(())
}

// --- the fabric journal ----------------------------------------------------------

#[test]
fn the_fabric_journal_replays_from_the_platforms_event_log_to_the_live_state_after_a_cycle()
-> Result<()> {
    // The failure this closes: the fabric's controls decided in memory and
    // nothing in this process wrote the decision anywhere, so `/wallet`
    // reported that no wallet existed and a corridor could not have been
    // proposed at all. Every decision now goes through the journal and into
    // the platform's own event log, and the log alone rebuilds the state
    // the cycle acted on — including the refusals.
    let mut platform = platform(PlatformConfig::default())?;
    let desk_venue = VenueId::new("simulated-venue");
    let initial_equity = platform.config().initial_equity;

    // Premise: no fabric record and no wallet before anything is decided,
    // and the statement handed in is the desk's own cash to the unit, so
    // the reconciliation below can be asserted reconciled rather than
    // whatever it happened to be.
    assert_eq!(platform.fabric_records(), 0);
    assert!(platform.fabric_state().wallet().is_none());
    assert!(platform.holdings_observed().is_empty());
    platform.observe_statement(
        desk_venue.clone(),
        "USD",
        initial_equity,
        dec!("1"),
        start(),
    )?;
    assert_eq!(platform.holdings_observed().len(), 1);

    // A destination proposed and a corridor proposed against it, through
    // the same seam; the corridor is refused a second time, and the refusal
    // is a record too.
    let destination = DestinationKey::new(DestinationAsset::new("USD")?, "treasury-account")?;
    let by = Approver::new("treasury-desk")?;
    let proposed = platform.decide_fabric(
        FabricCommand::Destination(DestinationAction::Propose {
            key: destination.clone(),
            by: by.clone(),
            at: start(),
        }),
        start(),
    )?;
    assert!(!proposed.outcome.is_refused(), "{proposed:?}");
    let corridor = CorridorAction::Propose {
        id: CorridorId::new("treasury-sweep")?,
        source: CapitalLocation::new(Region::new("home"), Currency::USD, desk_venue.clone()),
        source_class: CustodyClass::FiatAtInstitutionOfRecord,
        kind: CorridorKind::InstitutionApprovalFlow,
        destination,
        caps: CorridorCaps::new(
            dec!("1000"),
            dec!("1000"),
            dec!("5000"),
            dec!("10000"),
            Duration::from_hours(1),
            PermittedHours::ALL_DAY,
        )?,
        purpose: "sweep realised cash to the treasury account".to_string(),
        by,
        at: start(),
    };
    let first = platform.decide_fabric(FabricCommand::Corridor(corridor.clone()), start())?;
    assert!(!first.outcome.is_refused(), "{first:?}");
    let second = platform.decide_fabric(FabricCommand::Corridor(corridor), start())?;
    assert!(
        second.outcome.is_refused(),
        "a corridor proposed twice under one name was admitted: {second:?}"
    );
    assert_eq!(platform.fabric_state().corridors().len(), 1);
    assert_eq!(platform.fabric_state().destinations().len(), 1);

    // The cycle assembles and reconciles the wallet in LEARN.
    let cycle_at = start().saturating_add(Duration::from_secs(60));
    let report = platform.run_cycle(cycle_at);
    assert!(
        report.stage(Stage::Learn).is_some(),
        "the premise is a cycle whose LEARN ran: {report:?}"
    );
    let state = platform.fabric_state();
    let wallet = state.wallet().expect("the cycle assembled a wallet");
    assert_eq!(wallet.as_of(), cycle_at);
    let key = qip_capital_fabric::wallet::VenueAsset {
        venue: desk_venue.clone(),
        asset: qip_capital_fabric::wallet::Asset::new("USD")?,
    };
    let view = wallet
        .ledger_view(&key)
        .expect("the desk's cash is the ledger's view of the venue");
    assert_eq!(
        view.ledger_balance, initial_equity,
        "the ledger view is the tracked cash"
    );
    let outcome = state
        .reconciliations()
        .get(&key)
        .expect("the venue-asset was reconciled");
    assert!(
        !outcome.is_halt(),
        "a statement equal to the book to the unit halted: {outcome:?}"
    );
    assert_eq!(
        platform.fabric_records(),
        5,
        "destination, corridor, corridor refused, assemble, reconcile"
    );

    // The property: the platform's own log, replayed from genesis by the
    // fabric's replay, is the live state — every applied record and the
    // refusal — and the kernel's other records are passed over, not lost.
    let replayed = qip_capital_fabric::replay::replay(platform.event_log().records())?;
    assert_eq!(replayed.applied, platform.fabric_records());
    assert!(
        replayed.passed_over > 0,
        "the log holds the kernel's own records beside the fabric's"
    );
    assert_eq!(&replayed.state, platform.fabric_state());
    let refusals = replayed.state.corridors().len();
    assert_eq!(
        refusals, 1,
        "the refused second proposal rebuilt no second corridor"
    );
    // And the refusal itself is in the log as a refusal, which is what a
    // reader asking "why was that corridor not proposed twice" is owed.
    let refused_records = platform
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.event.topic == Topic::ComplianceEvaluated
                && record
                    .event
                    .decode::<qip_capital_fabric::journal::FabricRecord>()
                    .is_ok_and(|envelope| {
                        matches!(
                            envelope.body.outcome,
                            qip_capital_fabric::journal::FabricOutcome::Corridor(Outcome::Refused(
                                _
                            ))
                        )
                    })
        })
        .count();
    assert_eq!(refused_records, 1);
    Ok(())
}
