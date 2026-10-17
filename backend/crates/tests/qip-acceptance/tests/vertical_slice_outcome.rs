//! The vertical slice, desk half to ledger: a desk paper fill becomes the same
//! P1 `OutcomeRecord` a cell's fill becomes, so `qip-ledgerd` is the one
//! ledger consumer.
//!
//! ```text
//! Platform::submit_order -> OrderManager::submit -> simulated broker -> Fill
//!   -> capture_submission: DeskLedgerFeed::admit (PaperFill::try_from + post, dry)
//!        -> FillBooked (event log) -> Platform::capital      [only if admitted]
//!   -> Platform::take_desk_outcomes -> OutcomeRecord (cell = "desk")
//!   -> PaperFill::try_from -> ledger::post -> LedgerEvent
//! ```
//!
//! This closed FINDING G3: a desk `Fill` used to reach `Platform::capital` and
//! the outcome capture and nothing else, so the double-entry ledger booked
//! every edge-cell fill and none of the desk's. The last hop into the real
//! ledger store is `vertical_slice_ledgerd.rs`.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::ledger::{Account, Direction, LedgerEvent, Settlement};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::Decision as Recorded;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::desk_ledger::{DESK_CELL, outcomes_from};
use qip_kernel::platform::FillBooked;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_portfolio::ledger::{PaperFill, post};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_twin::capture::Action;
use std::collections::BTreeMap;
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                acme(),
                "ACME",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("vertical-slice", start()))
            .build(start())
            .expect("valid instrument"),
        )
        .expect("insertable");
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("vertical-slice")
        .with(Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        ))
        .with(Limit::new(
            "max-leverage",
            LimitKind::MaxLeverage { limit: 2.0 },
        ))
}

fn bar(at: Timestamp) -> Bar {
    Bar {
        object_id: acme(),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: dec!("100"),
        high: dec!("100.2"),
        low: dec!("99.8"),
        close: dec!("100"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Some(dec!("100")),
        quality: DataQuality::default(),
    }
}

/// A platform with sixty days of history and one tick absorbed, cycled once.
fn warm_platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    let mut tape: Vec<SensedRecord> = (1..=60)
        .rev()
        .map(|back| {
            SensedRecord::Bar(Box::new(bar(
                start().saturating_sub(Duration::from_days(back))
            )))
        })
        .collect();
    let printed = start().saturating_add(Duration::from_secs(30));
    tape.push(SensedRecord::Tick(Tick {
        object_id: acme(),
        venue: "XNYS".to_string(),
        at: printed,
        capture_time: printed.saturating_add(Duration::from_millis(40)),
        sequence: Some(1),
        price: dec!("100"),
        volume: dec!("100"),
        quality: DataQuality::default(),
    }));
    platform.observe(tape);
    platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    Ok(platform)
}

fn later() -> Timestamp {
    start().saturating_add(Duration::from_mins(3))
}

fn buy(
    platform: &mut Platform,
    quantity: Decimal,
    tag: &str,
) -> qip_execution_engine::order::Order {
    platform.order_from(
        acme(),
        Side::Buy,
        quantity,
        dec!("100"),
        &format!("prop-{tag}"),
        vec![format!("hyp-{tag}")],
        later(),
    )
}

fn imbalance(event: &LedgerEvent) -> BTreeMap<String, Decimal> {
    let mut by_unit: BTreeMap<String, Decimal> = BTreeMap::new();
    for posting in event.postings() {
        let entry = by_unit.entry(posting.unit.clone()).or_insert(Decimal::ZERO);
        match posting.direction {
            Direction::Debit => *entry += posting.amount,
            Direction::Credit => *entry -= posting.amount,
        }
    }
    by_unit
}

fn fills_booked(platform: &Platform) -> Vec<FillBooked> {
    use qip_events::EventBody;
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| record.event.topic == FillBooked::TOPIC)
        .map(|record| {
            qip_streaming::StreamEnvelope::from_frame(&record.event)
                .expect("a sealed frame")
                .decode::<FillBooked>()
                .expect("a FillBooked body")
                .body
        })
        .collect()
}

// --- the happy path -----------------------------------------------------------

#[test]
fn a_desk_fill_becomes_the_outcome_record_the_ledger_books_and_agrees_with_the_book() -> Result<()>
{
    let mut platform = warm_platform()?;
    assert!(
        platform.pending_desk_outcomes().is_empty(),
        "premise: nothing was filled before the order"
    );
    let order = buy(&mut platform, dec!("100"), "once");
    let order_id = order.order_id.clone();
    platform.submit_order(order, later())?;

    let fills = platform.orders().fills().len();
    assert_eq!(fills, 1, "premise: the paper broker filled once");
    let fill = platform.orders().fills()[0].clone();
    let held = platform.positions_at_cost()["obj-ACME"].0;
    assert_eq!(held, fill.quantity, "premise: the book holds the fill");

    let outcomes = platform.take_desk_outcomes();
    assert_eq!(outcomes.len(), 1, "one fill, one record");
    assert!(
        platform.take_desk_outcomes().is_empty(),
        "a record is handed over once"
    );
    let outcome = &outcomes[0];
    let record = &outcome.record;
    assert_eq!(record.cell, DESK_CELL);
    assert_eq!(record.entry.sequence, 0);
    let Recorded::Filled {
        order_id: recorded_order,
        object,
        venue,
        simulated,
        side,
        ..
    } = &record.entry.decision
    else {
        panic!(
            "the desk's record is not a fill: {:?}",
            record.entry.decision
        );
    };
    assert_eq!(recorded_order, order_id.as_str(), "the right order");
    assert_eq!(object, "obj-ACME", "the right instrument");
    assert_eq!(venue, &fill.venue);
    assert!(simulated, "the paper fence's flag travels on the record");
    assert_eq!(*side, Some(BookSide::Ask), "a buy is booked on the ask");

    // The correlation id is the one the cycle's decisions were captured under.
    let filled_row = platform
        .outcomes()
        .entries()
        .iter()
        .find(|row| matches!(&row.decision.action, Action::Filled { order_id: id, .. } if *id == order_id))
        .expect("the outcome capture holds the fill");
    assert_eq!(
        outcome.correlation_id,
        filled_row.decision.correlation.as_str(),
        "the record carries the decision's correlation"
    );

    // The ledger's two steps accept it and agree with the book.
    let paper = PaperFill::try_from(record)?;
    let event = post(&paper)?;
    assert_eq!(event.settlement(), Settlement::Simulated);
    assert_eq!(event.source().order_id, order_id.as_str());
    assert!(imbalance(&event).values().all(|net| net.is_zero()));
    let trading = Account::Trading {
        cell: DESK_CELL.to_string(),
        strategy: "central-desk".to_string(),
    };
    let signed = |unit_is_object: bool| -> Decimal {
        event
            .postings()
            .iter()
            .filter(|p| p.account == trading && (p.unit == "obj-ACME") == unit_is_object)
            .map(|p| match p.direction {
                Direction::Debit => p.amount,
                Direction::Credit => -p.amount,
            })
            .fold(Decimal::ZERO, |a, b| a + b)
    };
    assert_eq!(signed(true), held, "the ledger holds what the book holds");
    assert_eq!(
        -signed(false),
        fill.quantity * fill.price + fill.costs,
        "cash out is notional plus the fee the venue charged"
    );
    Ok(())
}

// --- negatives ----------------------------------------------------------------

#[test]
fn a_replayed_submission_adds_no_record_and_no_second_booking() -> Result<()> {
    let mut platform = warm_platform()?;
    let original = buy(&mut platform, dec!("100"), "replay");
    let replay = original.clone();
    platform.submit_order(original, later())?;
    let cash = platform.cash();
    let positions = platform.positions_at_cost();
    assert_eq!(platform.pending_desk_outcomes().len(), 1, "premise");

    assert!(platform.submit_order(replay, later()).is_err());
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        1,
        "the replay added a ledger record"
    );
    assert_eq!(platform.cash(), cash);
    assert_eq!(platform.positions_at_cost(), positions);
    Ok(())
}

#[test]
fn a_refused_order_leaves_no_ledger_record() -> Result<()> {
    let mut platform = warm_platform()?;
    // Premise: an order of this shape fills when it is sized to fit, so the
    // refusal below is the size.
    let fits = buy(&mut platform, dec!("100"), "fits");
    platform.submit_order(fits, later())?;
    assert_eq!(platform.pending_desk_outcomes().len(), 1, "premise");

    let too_big = buy(&mut platform, dec!("100000000"), "toobig");
    assert!(platform.submit_order(too_big, later()).is_err());
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        1,
        "a refusal produced a fill record"
    );
    Ok(())
}

// The desk declining a fill the ledger would refuse (a negative fee, a fill
// naming another order) cannot be driven from here: the platform's broker is
// `pub(crate)` and the simulated broker never produces one, which is itself
// the finding. It is proved in `qip-kernel`'s `desk_ledger` unit tests with a
// venue that lies.

// --- determinism ----------------------------------------------------------------

#[test]
fn the_chain_is_a_fold_of_the_event_log_and_identical_across_two_runs() -> Result<()> {
    let run = || -> Result<Platform> {
        let mut platform = warm_platform()?;
        for tag in ["a", "b", "c"] {
            let order = buy(&mut platform, dec!("10"), tag);
            platform.submit_order(order, later())?;
        }
        Ok(platform)
    };
    let first = run()?;
    let second = run()?;
    let live: Vec<_> = first
        .pending_desk_outcomes()
        .iter()
        .map(|o| o.record.clone())
        .collect();
    assert_eq!(live.len(), 3, "premise: three fills, three records");
    let again: Vec<_> = second
        .pending_desk_outcomes()
        .iter()
        .map(|o| o.record.clone())
        .collect();
    assert_eq!(live, again, "two identical runs, two identical identities");

    let booked = fills_booked(&first);
    assert_eq!(booked.len(), 3, "premise: the log booked all three");
    assert_eq!(
        outcomes_from(&booked, "central-desk"),
        live,
        "a restart rebuilding from the log yields the live chain"
    );
    let sequences: Vec<u64> = live.iter().map(|r| r.entry.sequence).collect();
    assert_eq!(sequences, vec![0, 1, 2]);
    Ok(())
}

#[test]
fn a_restart_continues_the_desk_chain_and_the_order_counter_so_no_id_is_reused() -> Result<()> {
    // The failure this prevents: a second process starting its chain again at
    // sequence 0 (the ledger would see a conflict at a position it holds), or
    // reading its first order, `ord-1`/`fill-1` like the first process's, as a
    // replay of the old fill and leaving a real fill out of the book. The
    // second is now prevented at the source: `resume_book` continues the order
    // counter past the log's, so the first order after a restart is `ord-2`
    // and the desk's chain correlation ids (a hash of the order id under a
    // session that never changes) stay distinct.
    let path = std::env::temp_dir()
        .join(format!("qip-slice-desk-restart-{}", std::process::id()))
        .join("events.jsonl");
    let _ = std::fs::remove_dir_all(path.parent().expect("a parent"));
    let boot = |at: Timestamp| -> Result<Platform> {
        let config = PlatformConfig::default().with_event_log_file(&path);
        let (context, _clock) = Context::deterministic(at, config.seed);
        Platform::new(config, context, Telemetry::silent(), universe(), limits())
    };

    let mut first = boot(start())?;
    let order = buy(&mut first, dec!("10"), "before");
    first.submit_order(order, later())?;
    let before = first.take_desk_outcomes();
    assert_eq!(before.len(), 1, "premise: the first process filled once");
    let first_order_id = first.orders().fills()[0].order_id.clone();
    let first_quantity = first.orders().fills()[0].quantity;
    drop(first);

    let restarted_at = start().saturating_add(Duration::from_hours(1));
    let mut second = boot(restarted_at)?;
    // `take_desk_outcomes` is destructive and never acknowledged, so the log
    // holds no watermark and the restart re-queues the fill it took: the
    // ledger has been told of nothing the fabric did not acknowledge.
    assert_eq!(
        second
            .pending_desk_outcomes()
            .iter()
            .map(|o| o.record.clone())
            .collect::<Vec<_>>(),
        before.iter().map(|o| o.record.clone()).collect::<Vec<_>>(),
        "premise: a restart re-queues what was never acknowledged"
    );
    let order = buy(&mut second, dec!("10"), "after");
    assert_ne!(
        order.order_id, first_order_id,
        "the order id counter restarted: two orders now share one lineage id"
    );
    second.submit_order(order, restarted_at.saturating_add(Duration::from_mins(3)))?;
    assert_eq!(
        second.positions_at_cost()["obj-ACME"].0,
        first_quantity + second.orders().fills()[0].quantity,
        "the book is the log's fill plus the new one"
    );
    let after = second.take_desk_outcomes();
    assert_eq!(
        after.len(),
        2,
        "the new fill was recorded beside the re-queued one, not dropped as a replay"
    );
    assert_eq!(after[1].record.entry.sequence, 1, "the chain continued");
    assert_eq!(
        after[1]
            .record
            .entry
            .expected_digest(&before[0].record.journal_digest)?,
        after[1].record.entry.digest,
        "and it chains onto the first process's tail"
    );
    Ok(())
}
