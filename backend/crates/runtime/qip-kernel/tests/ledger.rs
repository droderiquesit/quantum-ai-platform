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
    DecidedBy, Eligibility, EligibilityDecision, EligibilityTerms, Jurisdiction, Mandate,
    MandateId, MandateTerms, PermittedFamilies, ProductEligibility, UserId, UserShare,
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
use qip_data_finder::registration::RegistrationRecord;
use qip_events::{EventFilter, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::central::{CellReport, StrategyCandidate};
use qip_kernel::config::{PlatformConfig, UserEligibility, UserMandate};
use qip_kernel::cycle::Stage;
use qip_kernel::platform::{
    BookingBasis, EligibilityEntry, EligibilitySource, LedgerEntry, Platform,
};
use qip_lifecycle::trials::StrategyFamily;
use qip_market_ingestion::connector::manifest::SecretRef;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_observability::metrics::labels;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use std::collections::BTreeSet;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

const CELL: &str = "cell-lon-1";
const INSTRUMENT: &str = "obj-AAA";
/// A source the shipped requirement table says needs an account, so a
/// registration for it is one the platform actually adopts.
const ACCOUNT_SOURCE: &str = "alpaca-daily-bars";
const TERMS: &str = "https://alpaca.markets/terms-and-conditions";
const SLOT: &str = "QIP_ALPACA_API_SECRET_KEY";

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
            fixture_liquidity(),
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

/// The operator every committed eligibility in this suite is attributed to.
fn operator() -> DecidedBy {
    DecidedBy::operator("ops-carol", "oidc").expect("a named operator is valid")
}

/// A committed eligibility: verified at `start()` in GB — the fixture
/// mandate's jurisdiction — cleared to invest until `expires_at`.
fn cleared(user: &str, expires_at: Timestamp) -> Result<UserEligibility> {
    Ok(UserEligibility {
        user: UserId::new(user)?,
        eligibility: Eligibility::new(EligibilityTerms {
            verified_at: start(),
            can_invest: true,
            jurisdiction: Jurisdiction::new("GB")?,
            expires_at,
        })?,
        decided_by: operator(),
    })
}

/// An eligibility that outlives every instant this suite reasons about.
fn cleared_for_a_year(user: &str) -> Result<UserEligibility> {
    cleared(user, start().saturating_add(Duration::from_days(365)))
}

/// The producer the kernel writes eligibility records under. A literal
/// here so the test reads the log the way an auditor would — by what the
/// record says about itself — rather than through the kernel's constant.
const ELIGIBILITY_PRODUCER: &str = "kernel/eligibility";

/// Every eligibility decision the event log holds, oldest first, decoded
/// strictly: a record under the eligibility producer that does not decode
/// is a failure, not a record to pass over.
fn eligibility_entries(platform: &Platform) -> Result<Vec<EligibilityEntry>> {
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.event.topic == Topic::ComplianceEvaluated
                && record.event.lineage.producer == ELIGIBILITY_PRODUCER
        })
        .map(|record| {
            Ok(
                qip_streaming::envelope::StreamEnvelope::from_frame(&record.event)?
                    .decode::<EligibilityEntry>()?
                    .body,
            )
        })
        .collect()
}

/// The funding refusals the ledger journal holds, as `(user, gate)`.
fn funding_refusals(platform: &Platform) -> Result<Vec<(UserId, String)>> {
    Ok(ledger_entries(platform)?
        .into_iter()
        .filter_map(|entry| match entry {
            LedgerEntry::FundingRefused { user, gate, .. } => Some((user, gate)),
            LedgerEntry::Funded { .. } | LedgerEntry::Booked { .. } => None,
        })
        .collect())
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
    let config = PlatformConfig::default()
        .with_user_mandates(vec![
            enrolment("alice", dec!("1000"))?,
            enrolment("bob", dec!("1000"))?,
        ])
        .with_user_eligibilities(vec![
            cleared_for_a_year("alice")?,
            cleared_for_a_year("bob")?,
        ]);
    let mut platform = platform(config)?;

    // Premise: both users hold a mandate under the desk's and are cleared
    // to invest, both have capital at work at alpha in a one-to-two ratio,
    // the desk has no book there, and nothing has been booked yet.
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

// --- eligibility ------------------------------------------------------------------

#[test]
fn an_unverified_user_cannot_be_funded_and_the_refusal_names_the_reason() -> Result<()> {
    // The failure this closes: `fund_user` funded on the mandate alone,
    // because the process held no eligibility registry and `admit` would
    // have refused everyone — so a user nobody had verified had capital at
    // work. Premise: a user the configuration cleared, under the same desk
    // and mandate terms, funds; the refusal below is therefore the
    // registry's and not the mandate's.
    let alice = UserId::new("alice")?;
    let bob = UserId::new("bob")?;
    let alpha = StrategyId::new("alpha");
    let mut platform = platform(
        PlatformConfig::default()
            .with_user_mandates(vec![
                enrolment("alice", dec!("1000"))?,
                enrolment("bob", dec!("1000"))?,
            ])
            .with_user_eligibilities(vec![cleared_for_a_year("bob")?]),
    )?;
    platform.fund_user(&bob, &alpha, dec!("100"), start())?;
    assert_eq!(
        settled(&platform, &bob, &alpha),
        Some(dec!("100")),
        "the premise: a cleared user funds"
    );
    assert!(
        platform
            .user_ledger()
            .eligibility()
            .record(&alice)
            .is_none(),
        "the premise: no operator decided anything about alice"
    );

    let refused = platform
        .fund_user(&alice, &alpha, dec!("100"), start())
        .expect_err("a user no operator verified is refused");
    assert!(
        refused.message().contains("(unknown_user)"),
        "the refusal names the registry's reason: {}",
        refused.message()
    );
    assert!(
        platform.user_ledger().book(&alice, &alpha).is_none(),
        "a refused funding opened a book"
    );
    assert_eq!(
        funding_refusals(&platform)?,
        vec![(alice.clone(), "unknown_user".to_string())],
        "the refusal is on the record with its gate named"
    );

    // And the one committed decision is in the log, attributed to the
    // operator who committed it, from the configuration and not from
    // anywhere else.
    let entries = eligibility_entries(&platform)?;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].record.user, bob);
    assert_eq!(entries[0].source, EligibilitySource::Configuration);
    assert_eq!(entries[0].record.by.subject(), "ops-carol");
    Ok(())
}

#[test]
fn an_expired_eligibility_refuses_funding_from_the_instant_it_expires() -> Result<()> {
    // The failure: a verification taken once at deployment and honoured
    // for the life of the process. Premise: the same user funds inside the
    // window, so the refusal at the expiry is the expiry's.
    let alice = UserId::new("alice")?;
    let alpha = StrategyId::new("alpha");
    let expires_at = start().saturating_add(Duration::from_hours(1));
    let mut platform = platform(
        PlatformConfig::default()
            .with_user_mandates(vec![enrolment("alice", dec!("1000"))?])
            .with_user_eligibilities(vec![cleared("alice", expires_at)?]),
    )?;
    platform.fund_user(&alice, &alpha, dec!("100"), start())?;
    assert_eq!(settled(&platform, &alice, &alpha), Some(dec!("100")));

    let refused = platform
        .fund_user(&alice, &alpha, dec!("100"), expires_at)
        .expect_err("refused on the instant the eligibility expires");
    assert!(
        refused.message().contains("(expired)"),
        "the refusal names the expiry: {}",
        refused.message()
    );
    assert_eq!(
        settled(&platform, &alice, &alpha),
        Some(dec!("100")),
        "the refused funding moved nothing"
    );
    assert_eq!(
        funding_refusals(&platform)?,
        vec![(alice, "expired".to_string())]
    );
    Ok(())
}

#[test]
fn an_operators_eligibility_decision_is_journaled_and_replays_to_the_live_registry() -> Result<()> {
    // The failure: a decision that lived only in the registry's memory,
    // which is a second source of truth for who may be funded. Premise:
    // the user is refused before the decision, so the funding after it is
    // the decision's doing, and the sequence includes a revocation so a
    // replay that kept only the first decision per user would differ.
    let alice = UserId::new("alice")?;
    let alpha = StrategyId::new("alpha");
    let mut platform = platform(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;
    assert!(
        platform
            .fund_user(&alice, &alpha, dec!("100"), start())
            .is_err(),
        "the premise: nobody has verified alice"
    );
    assert!(eligibility_entries(&platform)?.is_empty());

    let operator = OperatorIdentity::verified("ops-dana", "hardware-token", start());
    let granted = platform.decide_eligibility(
        &alice,
        EligibilityDecision::Granted {
            eligibility: Eligibility::new(EligibilityTerms {
                verified_at: start(),
                can_invest: true,
                jurisdiction: Jurisdiction::new("GB")?,
                expires_at: start().saturating_add(Duration::from_days(365)),
            })?,
        },
        &operator,
        "identity verified against the passport on file",
        start(),
    )?;
    assert_eq!(granted.by.subject(), "ops-dana");
    assert_eq!(granted.by.method(), "hardware-token");
    platform.fund_user(&alice, &alpha, dec!("100"), start())?;
    assert_eq!(settled(&platform, &alice, &alpha), Some(dec!("100")));

    let later = start().saturating_add(Duration::from_secs(60));
    let operator = OperatorIdentity::verified("ops-dana", "hardware-token", later);
    platform.decide_eligibility(
        &alice,
        EligibilityDecision::Revoked {
            reason: "verification withdrawn on review".to_string(),
        },
        &operator,
        "the review found the document lapsed",
        later,
    )?;
    let refused = platform
        .fund_user(&alice, &alpha, dec!("100"), later)
        .expect_err("a revoked user is refused");
    assert!(
        refused.message().contains("(revoked)"),
        "{}",
        refused.message()
    );

    // Two decisions on the record, each from an operator, in order.
    let entries = eligibility_entries(&platform)?;
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(
        entries
            .iter()
            .all(|entry| entry.source == EligibilitySource::Operator)
    );
    assert!(matches!(
        entries[0].record.decision,
        EligibilityDecision::Granted { .. }
    ));
    assert!(matches!(
        entries[1].record.decision,
        EligibilityDecision::Revoked { .. }
    ));
    assert_eq!(entries[1].reason, "the review found the document lapsed");

    // The property: the log alone rebuilds the registry the platform
    // acted on.
    let replayed = platform.replay_eligibility()?;
    assert_eq!(&replayed, platform.user_ledger().eligibility());
    assert_eq!(replayed.records().len(), 1);
    assert!(matches!(
        replayed
            .record(&alice)
            .expect("alice's standing decision")
            .decision,
        EligibilityDecision::Revoked { .. }
    ));
    Ok(())
}

#[test]
fn an_eligibility_cannot_be_granted_from_a_config_value_alone_without_an_operator_identity()
-> Result<()> {
    // ADR 0021 names "a configuration value" among the things that must
    // never decide. A committed eligibility is a decision only because it
    // carries the operator who took it: strip the operator and the
    // configuration does not read; blank the operator and it does not
    // assemble; hand the runtime path a stale credential and it refuses
    // and journals nothing. Premise: the same configuration with the
    // operator named assembles and funds.
    let alice = UserId::new("alice")?;
    let alpha = StrategyId::new("alpha");
    let config = PlatformConfig::default()
        .with_user_mandates(vec![enrolment("alice", dec!("1000"))?])
        .with_user_eligibilities(vec![cleared_for_a_year("alice")?]);
    let mut named = platform(config.clone())?;
    named.fund_user(&alice, &alpha, dec!("100"), start())?;
    assert_eq!(settled(&named, &alice, &alpha), Some(dec!("100")));

    let stored = serde_json::to_value(&config).expect("a configuration serialises");
    let entry = |mut stored: serde_json::Value, edit: &dyn Fn(&mut serde_json::Value)| {
        edit(&mut stored["user_eligibilities"][0]);
        stored
    };
    assert!(
        stored["user_eligibilities"][0]["decided_by"]["subject"] == "ops-carol",
        "the premise: the stored form carries the operator to be removed"
    );

    // No operator at all: the configuration is not one.
    let unsigned = entry(stored.clone(), &|entry| {
        entry
            .as_object_mut()
            .expect("an entry is an object")
            .remove("decided_by");
    });
    assert!(
        serde_json::from_value::<PlatformConfig>(unsigned).is_err(),
        "a committed eligibility with no operator is not a configuration"
    );

    // An operator field naming nobody: refused with the reason named.
    let blank = entry(stored, &|entry| {
        entry["decided_by"]["subject"] = serde_json::Value::String(String::new());
    });
    let error = serde_json::from_value::<PlatformConfig>(blank)
        .expect_err("a blank operator is refused on the way in");
    assert!(
        error.to_string().contains("names no operator"),
        "the refusal says what is missing: {error}"
    );

    // A committed decision about a user with no mandate stops assembly.
    let nobody = platform(
        PlatformConfig::default().with_user_eligibilities(vec![cleared_for_a_year("alice")?]),
    );
    let error = match nobody {
        Ok(_) => panic!("an eligibility for a user with no mandate assembled a platform"),
        Err(error) => error,
    };
    assert!(
        error.message().contains("holds no mandate"),
        "the refusal names the missing mandate: {}",
        error.message()
    );

    // The runtime path: a credential older than the freshness bound is
    // refused, as an autonomy change would be, and nothing is journaled.
    let mut unsigned = platform(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;
    let stale = OperatorIdentity::verified(
        "ops-dana",
        "oidc",
        start().saturating_sub(Duration::from_mins(16)),
    );
    let grant = EligibilityDecision::Granted {
        eligibility: cleared_for_a_year("alice")?.eligibility,
    };
    let refused = unsigned
        .decide_eligibility(
            &alice,
            grant.clone(),
            &stale,
            "identity verified against the passport on file",
            start(),
        )
        .expect_err("a stale credential is refused");
    assert!(
        refused.message().contains("re-authenticate"),
        "{}",
        refused.message()
    );
    let terse = unsigned
        .decide_eligibility(
            &alice,
            grant,
            &OperatorIdentity::verified("ops-dana", "oidc", start()),
            "ok",
            start(),
        )
        .expect_err("a decision without a stated reason is refused");
    assert!(
        terse.message().contains("stated reason"),
        "{}",
        terse.message()
    );
    assert!(
        eligibility_entries(&unsigned)?.is_empty(),
        "a refused decision left a record"
    );
    assert!(
        unsigned
            .fund_user(&alice, &alpha, dec!("100"), start())
            .is_err(),
        "and alice is still not fundable"
    );
    Ok(())
}

// --- the product gate -------------------------------------------------------------

/// A user mandate that permits exactly one family, otherwise the suite's
/// terms: a thousand under management, no floor, GB, the desk's currency.
fn mandate_only(capital: Decimal, family: &str) -> Result<Mandate> {
    Mandate::new(MandateTerms {
        capital,
        currency: Currency::USD,
        risk_tolerance: Decimal::ONE,
        permitted_families: PermittedFamilies::Only(BTreeSet::from([family.to_string()])),
        liquidity_floor: Decimal::ZERO,
        exploration_share: Decimal::ZERO,
        jurisdiction: Jurisdiction::new("GB")?,
    })
}

fn enrolment_only(user: &str, capital: Decimal, family: &str) -> Result<UserMandate> {
    Ok(UserMandate {
        user: UserId::new(user)?,
        id: MandateId::new(format!("mandate-{user}"))?,
        mandate: mandate_only(capital, family)?,
    })
}

/// A compiled strategy the factory accepts, so the strategy has a family and
/// the product gate has something to look an offering up by. An unregistered
/// strategy has no family at all, which is a different case with its own
/// test below.
fn register_family(platform: &mut Platform, strategy: &str, family: &str) -> Result<()> {
    let subject = ObjectId::from_string(INSTRUMENT);
    let pressure =
        qip_contracts::feature::FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(
        StrategyId::new(strategy),
        subject,
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "enter",
        qip_contracts::signal::SignalKind::Enter,
        Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
        Expr::Exact(Decimal::from_int(100)),
        Expr::Statistic(0.62),
        500,
    ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    let candidate = StrategyCandidate::new(
        compiled,
        compiler.into_program(),
        StrategyFamily::new(family)?,
        CELL,
        VenueId::new("XNYS"),
        start(),
    )?;
    platform.central_mut().factory_mut().register(candidate)
}

/// The operator who takes this suite's product determinations, authenticated
/// at the instant the tests reason about.
fn compliance_officer() -> OperatorIdentity {
    OperatorIdentity::verified("ops-erin", "hardware-token", start())
}

#[test]
fn a_user_whose_entitlement_refuses_the_strategys_family_cannot_be_funded_and_the_refusal_names_the_family_and_jurisdiction()
-> Result<()> {
    // The failure this closes: funding asked the eligibility registry
    // whether an operator had verified *this user*, and nothing asked
    // whether the *family* their capital was going into may be offered
    // where they are. `Entitlement::evaluate` has always wanted a product
    // to evaluate against and the kernel had none to give, so it evaluated
    // no entitlement at all and a user verified in GB could be funded into
    // a family compliance had cleared nowhere.
    //
    // Premise, four parts, because each of them is a gate that could
    // produce this refusal instead: alice is eligible, her mandate permits
    // the family, the strategy has a family registered to look up, and the
    // catalogue holds no determination about that family.
    let alice = UserId::new("alice")?;
    let alpha = StrategyId::new("alpha");
    let mut platform = platform(
        PlatformConfig::default()
            .with_user_mandates(vec![enrolment_only("alice", dec!("1000"), "carry")?])
            .with_user_eligibilities(vec![cleared_for_a_year("alice")?]),
    )?;
    register_family(&mut platform, "alpha", "carry")?;
    assert!(
        platform
            .user_ledger()
            .eligibility_of(&alice, start())
            .is_ok(),
        "the premise: an operator verified alice and cleared her to invest"
    );
    assert!(
        platform
            .user_ledger()
            .mandate(&alice)
            .expect("alice is enrolled")
            .permitted_families()
            .permits("carry"),
        "the premise: her mandate permits the family"
    );
    assert_eq!(
        platform
            .central()
            .factory()
            .candidate(&alpha)
            .map(|candidate| candidate.family().to_string()),
        Some("carry".to_string()),
        "the premise: the strategy has a family the gate can look up"
    );
    assert!(
        platform.products().cleared("carry").is_none(),
        "the premise: nobody has cleared the family anywhere"
    );

    let refused = platform
        .fund_user(&alice, &alpha, dec!("100"), start())
        .expect_err("a family cleared in no jurisdiction cannot be funded");
    let message = refused.message();
    // The delimited tokens, not a substring: "carry" is a substring of
    // "carrying" and "GB" of "GBP", and a refusal that named neither would
    // still contain both by accident in a long enough sentence.
    assert!(
        message.split_whitespace().any(|word| word == "carry"),
        "the refusal names the family: {message}"
    );
    assert!(
        message
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "GB"),
        "the refusal names the jurisdiction it was refused in: {message}"
    );
    assert!(
        message.contains("does not grant investing"),
        "the refusal says what the entitlement withheld: {message}"
    );

    assert!(
        platform.user_ledger().book(&alice, &alpha).is_none(),
        "a refused funding opened a book"
    );
    assert_eq!(
        funding_refusals(&platform)?,
        vec![(alice, "entitlement".to_string())],
        "the refusal is journalled under the gate that refused it"
    );
    Ok(())
}

#[test]
fn a_user_whose_entitlement_grants_the_strategys_family_in_their_jurisdiction_can_be_funded()
-> Result<()> {
    // The other half, and the half that tells a working gate from one that
    // refuses everything: the same user, the same strategy, the same
    // mandate, funded once compliance has cleared the family where she is.
    // Premise: the funding is refused before the determination, so what
    // follows is the determination's doing.
    let alice = UserId::new("alice")?;
    let alpha = StrategyId::new("alpha");
    let gb = Jurisdiction::new("GB")?;
    let mut platform = platform(
        PlatformConfig::default()
            .with_user_mandates(vec![enrolment_only("alice", dec!("1000"), "carry")?])
            .with_user_eligibilities(vec![cleared_for_a_year("alice")?]),
    )?;
    register_family(&mut platform, "alpha", "carry")?;
    assert!(
        platform
            .fund_user(&alice, &alpha, dec!("100"), start())
            .is_err(),
        "the premise: the family is cleared nowhere and the funding is refused"
    );
    assert_eq!(settled(&platform, &alice, &alpha), None);

    platform.offer_product(
        ProductEligibility::new("carry").eligible_in(gb),
        &compliance_officer(),
        "cleared for retail distribution in GB by the compliance committee",
        start(),
    )?;
    assert!(
        platform.products().clears("carry", gb),
        "the determination stands"
    );

    platform.fund_user(&alice, &alpha, dec!("100"), start())?;
    assert_eq!(
        settled(&platform, &alice, &alpha),
        Some(dec!("100")),
        "a cleared family in the user's own jurisdiction funds"
    );

    // A jurisdiction the determination does not name is still refused, so
    // the clearance admits what it names and nothing wider.
    let elsewhere = UserId::new("bob")?;
    let mut abroad = platform;
    abroad.offer_product(
        ProductEligibility::new("momentum").eligible_in(Jurisdiction::new("US")?),
        &compliance_officer(),
        "cleared for distribution in the United States only",
        start(),
    )?;
    assert!(
        !abroad.products().clears("momentum", gb),
        "a family cleared in the US is not cleared in GB"
    );
    assert!(abroad.user_ledger().mandate(&elsewhere).is_none());

    // And the log alone rebuilds the catalogue the platform funded on: a
    // clearance held only in memory would be a second source of truth for
    // what a user's capital was allowed into.
    let replayed = abroad.replay_products()?;
    assert_eq!(&replayed, abroad.products());
    assert_eq!(replayed.len(), 2, "both determinations replay");
    Ok(())
}

#[test]
fn a_mandate_that_permits_only_some_families_is_refused_a_strategy_no_family_is_registered_for()
-> Result<()> {
    // The fail-closed half. With no family registered for the strategy
    // there is no offering to look up, so the product arm cannot be
    // evaluated at all; a mandate that permits only some families cannot be
    // shown that this strategy is one of them, and the restrictive answer
    // is the one taken. The gap this leaves is stated on
    // `Platform::product_refusal`: `PermittedFamilies::Any` — the desk's
    // arm — has consented to every family and so has no family gate left to
    // fail, which is why bob funds below.
    //
    // Premise: bob, under an `Any` mandate, funds the very same
    // unregistered strategy, so alice's refusal is her mandate's and not
    // the strategy's.
    let alice = UserId::new("alice")?;
    let bob = UserId::new("bob")?;
    let beta = StrategyId::new("beta");
    let mut platform = platform(
        PlatformConfig::default()
            .with_user_mandates(vec![
                enrolment_only("alice", dec!("1000"), "carry")?,
                enrolment("bob", dec!("1000"))?,
            ])
            .with_user_eligibilities(vec![
                cleared_for_a_year("alice")?,
                cleared_for_a_year("bob")?,
            ]),
    )?;
    assert!(
        platform.central().factory().candidate(&beta).is_none(),
        "the premise: no family is registered for the strategy"
    );
    platform.fund_user(&bob, &beta, dec!("100"), start())?;
    assert_eq!(
        settled(&platform, &bob, &beta),
        Some(dec!("100")),
        "the premise: a mandate permitting every family funds it"
    );

    let refused = platform
        .fund_user(&alice, &beta, dec!("100"), start())
        .expect_err("a restricted mandate is refused a strategy with no family");
    assert!(
        refused.message().contains("registered no family"),
        "the refusal names what is missing: {}",
        refused.message()
    );
    assert_eq!(settled(&platform, &alice, &beta), None);
    assert_eq!(
        funding_refusals(&platform)?,
        vec![(alice, "unregistered_family".to_string())],
        "journalled under its own gate, not the entitlement's"
    );
    Ok(())
}

// --- what the two decision series say ---------------------------------------------

/// The wire names, as a dashboard query spells them. Literals rather than
/// the `names` constants, so renaming a constant without renaming the series
/// fails here rather than silently retiring a chart.
const ELIGIBILITY_SERIES: &str = "qip_central_eligibility_decisions_total";
const REGISTRATION_SERIES: &str = "qip_central_registrations_total";

fn counter(platform: &Platform, name: &str, label: (&str, &str)) -> Option<u64> {
    platform
        .telemetry()
        .metrics
        .snapshot()
        .get(name, &labels([label]))
        .and_then(|value| match value {
            qip_observability::metrics::MetricValue::Counter(count) => Some(*count),
            _ => None,
        })
}

#[test]
fn every_eligibility_decision_and_every_refused_funding_moves_the_decision_series() -> Result<()> {
    // The failure this closes: who the platform admitted to having capital
    // put to work, and whom it turned away, reached the event log and no
    // series at all — so the one question an operator asks of a compliance
    // control ("is it refusing anybody, and how often") could only be
    // answered by replaying the log. Premise: none of the three series
    // exists before the platform is driven, so each assertion below is
    // about something that moved rather than something that was already
    // there.
    let alice = UserId::new("alice")?;
    let bob = UserId::new("bob")?;
    let alpha = StrategyId::new("alpha");
    let mut platform = platform(PlatformConfig::default().with_user_mandates(vec![
        enrolment("alice", dec!("1000"))?,
        enrolment("bob", dec!("1000"))?,
    ]))?;
    for decision in ["granted", "revoked", "refused_funding"] {
        assert_eq!(
            counter(&platform, ELIGIBILITY_SERIES, ("decision", decision)),
            None,
            "the premise: nothing has been recorded under {decision}"
        );
    }

    let operator = OperatorIdentity::verified("ops-dana", "hardware-token", start());
    platform.decide_eligibility(
        &alice,
        EligibilityDecision::Granted {
            eligibility: cleared_for_a_year("alice")?.eligibility,
        },
        &operator,
        "identity verified against the passport on file",
        start(),
    )?;
    assert_eq!(
        counter(&platform, ELIGIBILITY_SERIES, ("decision", "granted")),
        Some(1),
        "a grant the registry took is counted"
    );

    // A funding a gate refused before any book moved. Bob holds a mandate
    // and nobody has decided anything about him.
    assert!(
        platform.user_ledger().eligibility().record(&bob).is_none(),
        "the premise: nobody has decided anything about bob"
    );
    platform
        .fund_user(&bob, &alpha, dec!("100"), start())
        .expect_err("an unverified user is refused");
    assert_eq!(
        counter(
            &platform,
            ELIGIBILITY_SERIES,
            ("decision", "refused_funding")
        ),
        Some(1),
        "a refused funding is counted where the refusal is journalled"
    );

    let later = start().saturating_add(Duration::from_secs(60));
    platform.decide_eligibility(
        &alice,
        EligibilityDecision::Revoked {
            reason: "verification withdrawn on review".to_string(),
        },
        &OperatorIdentity::verified("ops-dana", "hardware-token", later),
        "the review found the document lapsed",
        later,
    )?;
    assert_eq!(
        counter(&platform, ELIGIBILITY_SERIES, ("decision", "revoked")),
        Some(1),
        "a revocation is a decision too, and a different one"
    );

    // Three events, three series, no fourth: a label outside the enum would
    // show up here as a total that does not match.
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(ELIGIBILITY_SERIES),
        3,
        "the label set is the three the enum and the refusal path name"
    );
    Ok(())
}

#[test]
fn a_venue_registration_is_counted_under_the_source_that_brought_it() -> Result<()> {
    // The failure this closes: a registration committed by the deployment
    // and one approved at runtime differ in who is accountable, and both
    // ended in the same registry with nothing afterwards to tell them
    // apart. Premise: a platform that has adopted no registration has no
    // series at all, so what is asserted below is what the adoption wrote.
    let mut runtime = platform(PlatformConfig::default())?;
    assert_eq!(
        runtime
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(REGISTRATION_SERIES),
        0,
        "the premise: nothing has been registered"
    );

    let later = start().saturating_add(Duration::from_secs(60));
    runtime.approve_registration(
        ACCOUNT_SOURCE,
        &OperatorIdentity::verified("ops-dana", "hardware-token", later),
        TERMS,
        SecretRef::new(SLOT)?,
        later,
    )?;
    assert_eq!(
        counter(&runtime, REGISTRATION_SERIES, ("source", "operator")),
        Some(1),
        "an operator's approval is counted as the operator's"
    );
    assert_eq!(
        counter(&runtime, REGISTRATION_SERIES, ("source", "configuration")),
        None,
        "and not as the configuration's"
    );

    // The other source, on a platform that committed the same record: the
    // two labels are the two arms of `RegistrationSource` and there is no
    // third.
    let committed = RegistrationRecord::new(
        ACCOUNT_SOURCE,
        "desk-owner",
        start(),
        TERMS,
        SecretRef::new(SLOT)?,
    )?;
    let assembled = platform(PlatformConfig::default().with_venue_registrations(vec![committed]))?;
    assert_eq!(
        counter(&assembled, REGISTRATION_SERIES, ("source", "configuration")),
        Some(1),
        "a committed registration is counted at assembly"
    );
    assert_eq!(
        counter(&assembled, REGISTRATION_SERIES, ("source", "operator")),
        None,
        "and not as an operator's"
    );
    assert_eq!(
        assembled
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(REGISTRATION_SERIES),
        1
    );
    Ok(())
}
