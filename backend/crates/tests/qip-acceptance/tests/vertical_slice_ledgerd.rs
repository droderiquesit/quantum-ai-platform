//! The vertical slice, last hop: a P1 `OutcomeRecord` into the real
//! `qip-ledgerd` store apply path.
//!
//! ```text
//! desk: Platform::submit_order -> take_desk_outcomes -> OutcomeRecord (cell = "desk")
//!                                                          |
//!                                                          v
//!                            LedgerStore::apply (chain check, paper fence, PaperFill,
//!                            post, fold, one atomic batch) -> balances, chain, cursor
//! ```
//!
//! This closed FINDING G5: `qip-ledgerd` was not reachable from
//! `qip-acceptance`, so the chain's last leg was covered only by
//! `apps/qip-ledgerd/tests/fabric.rs`, with records shaped by hand. Here the
//! records are the ones the platform itself emitted. The cell half of the same
//! hop is `vertical_slice_cell_ledger.rs`.
//!
//! The store is the production type over the local backend in a temporary
//! directory; the broker that delivers the spool is not run, and a
//! `Delivery` is built the way `consumer.rs` builds one.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::ledger::Account;
use qip_contracts::reflex::{
    ChainVersion, Decision as Recorded, JournalEntry, OutcomeRecord, seal_v2,
};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Decimal, ManualClock, ObjectId, dec, sha256_hex};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::desk_ledger::{DESK_CELL, DeskOutcome};
use qip_kernel::{Platform, PlatformConfig};
use qip_ledgerd::store::{Delivery, Disposition, GENESIS, LedgerStore, P1Record, ParkReason};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{MetricValue, Metrics, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_storage::EngineConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const PARTITION: &str = "p0";

// --- fixtures: the desk, as vertical_slice_outcome.rs builds it ---------------

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

/// A warm desk that has bought twice, and what it emitted.
fn desk_with_two_fills() -> Result<(Platform, Vec<DeskOutcome>)> {
    let mut platform = warm_platform()?;
    for (quantity, tag) in [(dec!("100"), "first"), (dec!("40"), "second")] {
        let order = buy(&mut platform, quantity, tag);
        platform.submit_order(order, later())?;
    }
    let outcomes = platform.take_desk_outcomes();
    assert_eq!(outcomes.len(), 2, "premise: two fills, two records");
    Ok((platform, outcomes))
}

// --- the store -----------------------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-slice-ledgerd-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn open(dir: &Path) -> (LedgerStore, Arc<Metrics>) {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let store = LedgerStore::open(
        dir,
        EngineConfig::new(clock),
        LedgerTelemetry::new(metrics.clone()),
    )
    .expect("the store opens");
    (store, metrics)
}

fn counter(metrics: &Metrics, name: &str) -> u64 {
    metrics
        .snapshot()
        .series
        .iter()
        .filter(|s| s.name == name)
        .map(|s| match &s.value {
            MetricValue::Counter(v) => *v,
            other => panic!("{name} is not a counter: {other:?}"),
        })
        .sum()
}

fn delivery(offset: u64, record: &OutcomeRecord) -> Delivery {
    let record = P1Record::Outcome(Box::new(record.clone()));
    Delivery {
        partition: PARTITION.to_string(),
        offset,
        record_hash: sha256_hex(&serde_json::to_vec(&record).expect("serialises")),
        record,
    }
}

/// `record` with its decision rewritten and re-sealed at the same position, so
/// the chain is intact and only the fill is wrong: what a producer that built
/// a bad fill honestly would send.
fn resealed(
    record: &OutcomeRecord,
    previous: &str,
    edit: impl FnOnce(&mut Recorded),
) -> OutcomeRecord {
    let mut decision = record.entry.decision.clone();
    edit(&mut decision);
    let (decision, digest) = seal_v2(previous, record.entry.sequence, record.entry.at, decision);
    OutcomeRecord {
        cell: record.cell.clone(),
        session: record.session,
        journal_sequence: record.entry.sequence,
        journal_digest: digest.clone(),
        entry: JournalEntry {
            sequence: record.entry.sequence,
            at: record.entry.at,
            decision,
            digest,
            version: ChainVersion::V2,
        },
    }
}

fn trading() -> Account {
    Account::Trading {
        cell: DESK_CELL.to_string(),
        strategy: "central-desk".to_string(),
    }
}

// --- the happy path ---------------------------------------------------------------

#[test]
fn desk_fills_are_booked_by_the_real_store_and_agree_with_the_platforms_book() -> Result<()> {
    let (platform, outcomes) = desk_with_two_fills()?;
    let (store, _) = open(&temp_dir("happy"));
    for (offset, outcome) in outcomes.iter().enumerate() {
        let disposition = store
            .apply(&delivery(offset as u64, &outcome.record))
            .unwrap();
        assert!(
            matches!(disposition, Disposition::Posted { .. }),
            "{disposition:?}"
        );
    }

    // Double entry: every unit closes to zero across all accounts.
    let trial = store.trial_balance().unwrap();
    assert!(!trial.is_empty(), "premise: something was booked");
    assert!(trial.values().all(|total| total.is_zero()), "{trial:?}");

    // The trading account holds what the platform's book holds...
    let held = platform.positions_at_cost()["obj-ACME"].0;
    let filled: Decimal = platform
        .orders()
        .fills()
        .iter()
        .map(|fill| fill.quantity)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(held, filled, "premise: the book holds both fills");
    assert!(held.is_positive());
    assert_eq!(store.balance(&trading(), "obj-ACME").unwrap(), held);

    // ...and its cash is minus quantity times price, plus the venue's fee.
    let fills = platform.orders().fills();
    assert_eq!(fills.len(), 2);
    let paid: Decimal = fills
        .iter()
        .map(|fill| fill.quantity * fill.price + fill.costs)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(store.balance(&trading(), "USD").unwrap(), -paid);

    // The chain tail is the second record, and the cursor is past it.
    let tail = store.tail(DESK_CELL).unwrap().expect("a tail");
    assert_eq!(tail.sequence, 1);
    assert_eq!(tail.digest, outcomes[1].record.journal_digest);
    assert_eq!(store.resume_offset(PARTITION).unwrap(), 2);
    Ok(())
}

#[test]
fn the_same_record_applied_twice_posts_once() -> Result<()> {
    let (_platform, outcomes) = desk_with_two_fills()?;
    let (store, metrics) = open(&temp_dir("idempotent"));
    let first = delivery(0, &outcomes[0].record);
    assert!(matches!(
        store.apply(&first).unwrap(),
        Disposition::Posted { .. }
    ));
    let balances = store.balances().unwrap();
    let events = store.events().unwrap().len();

    assert_eq!(store.apply(&first).unwrap(), Disposition::Duplicate);
    // And a redelivery at a new offset, as a replayed batch would arrive.
    let again = delivery(7, &outcomes[0].record);
    assert_eq!(store.apply(&again).unwrap(), Disposition::Duplicate);
    assert_eq!(
        store.balances().unwrap(),
        balances,
        "a duplicate moved money"
    );
    assert_eq!(store.events().unwrap().len(), events, "a duplicate posted");
    assert_eq!(counter(&metrics, names::LEDGER_DUPLICATES), 2);
    Ok(())
}

#[test]
fn a_restart_and_a_refold_yield_the_balances_the_live_store_held() -> Result<()> {
    let (_platform, outcomes) = desk_with_two_fills()?;
    let dir = temp_dir("restart");
    let live = {
        let (store, _) = open(&dir);
        for (offset, outcome) in outcomes.iter().enumerate() {
            store
                .apply(&delivery(offset as u64, &outcome.record))
                .unwrap();
        }
        let live = store.balances().unwrap();
        assert!(!live.is_empty(), "premise");
        assert_eq!(store.refold().unwrap(), live, "refold equals the live rows");
        live
    };
    let (reopened, _) = open(&dir);
    assert_eq!(
        reopened.balances().unwrap(),
        live,
        "restart changed balances"
    );
    assert_eq!(reopened.refold().unwrap(), live);
    assert_eq!(reopened.resume_offset(PARTITION).unwrap(), 2);
    // Re-offered after the restart, the whole partition is a no-op.
    for (offset, outcome) in outcomes.iter().enumerate() {
        assert_eq!(
            reopened
                .apply(&delivery(offset as u64, &outcome.record))
                .unwrap(),
            Disposition::Duplicate
        );
    }
    assert_eq!(reopened.balances().unwrap(), live);
    Ok(())
}

// --- negatives: the store refuses, parks the cell, books nothing ---------------

#[test]
fn a_non_simulated_desk_fill_is_refused_by_the_paper_fence_and_books_nothing() -> Result<()> {
    let (_platform, outcomes) = desk_with_two_fills()?;
    let (store, metrics) = open(&temp_dir("live"));
    let live = resealed(&outcomes[0].record, GENESIS, |decision| {
        if let Recorded::Filled { simulated, .. } = decision {
            *simulated = false;
        }
    });
    let disposition = store.apply(&delivery(0, &live)).unwrap();
    assert_eq!(
        disposition,
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert_eq!(counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED), 1);
    assert!(store.balances().unwrap().is_empty(), "a live fill booked");
    assert!(store.events().unwrap().is_empty());
    assert!(store.tail(DESK_CELL).unwrap().is_none());
    // The cell stays parked: a good record behind it is held, not applied.
    let held = store.apply(&delivery(1, &outcomes[1].record)).unwrap();
    assert!(matches!(held, Disposition::Held { .. }), "{held:?}");
    assert!(store.balances().unwrap().is_empty());
    Ok(())
}

#[test]
fn a_malformed_outcome_parks_the_partition_and_nothing_is_booked() -> Result<()> {
    let (_platform, outcomes) = desk_with_two_fills()?;
    for (label, edit) in [
        (
            "no quote unit",
            Box::new(|d: &mut Recorded| {
                if let Recorded::Filled { quote_unit, .. } = d {
                    *quote_unit = None;
                }
            }) as Box<dyn Fn(&mut Recorded)>,
        ),
        (
            "instrument quoted in itself",
            Box::new(|d: &mut Recorded| {
                if let Recorded::Filled { object, .. } = d {
                    *object = "USD".to_string();
                }
            }),
        ),
        (
            "shares that do not sum to the fill",
            Box::new(|d: &mut Recorded| {
                if let Recorded::Filled { shares, .. } = d {
                    shares[0].1 = "1".to_string();
                }
            }),
        ),
    ] {
        let (store, _) = open(&temp_dir("malformed"));
        let bad = resealed(&outcomes[0].record, GENESIS, |d| edit(d));
        let disposition = store.apply(&delivery(0, &bad)).unwrap();
        assert_eq!(
            disposition,
            Disposition::Parked {
                reason: ParkReason::RefusedFill
            },
            "{label}"
        );
        assert!(store.balances().unwrap().is_empty(), "{label}: booked");
        let behind = store.apply(&delivery(1, &outcomes[1].record)).unwrap();
        assert!(
            matches!(behind, Disposition::Held { .. }),
            "{label}: {behind:?}"
        );
        assert!(
            store.balances().unwrap().is_empty(),
            "{label}: held record booked"
        );
    }
    Ok(())
}

#[test]
fn a_record_whose_fill_was_edited_after_sealing_breaks_the_chain_and_books_nothing() -> Result<()> {
    let (_platform, outcomes) = desk_with_two_fills()?;
    let (store, _) = open(&temp_dir("tamper"));
    let mut forged = outcomes[0].record.clone();
    if let Recorded::Filled { price, .. } = &mut forged.entry.decision {
        *price = "1".to_string();
    }
    let disposition = store.apply(&delivery(0, &forged)).unwrap();
    assert_eq!(
        disposition,
        Disposition::Parked {
            reason: ParkReason::Integrity
        }
    );
    assert!(store.balances().unwrap().is_empty());
    Ok(())
}
