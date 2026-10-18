//! R12-LEDGER-10179 as its own verification states it: "any redelivery/crash
//! schedule yields single-delivery balances", for the records the desks
//! actually produce.
//!
//! The store already has this property for cell-shaped records
//! (`qip-ledgerd/tests/store.rs`). This suite holds it for desk records built
//! by the kernel's `DeskLedgerFeed` on all three desk chains (`desk`,
//! `desk-api`, `desk-deep`) — the records `qip-desk-ship` sends — interleaved
//! on one partition as three processes would produce them, then delivered
//! with duplicates, re-sent runs, rewinds and a crash that cuts the store's
//! write-ahead log at an arbitrary byte. Each case is compared against the
//! balances a single clean delivery books and against the refold of the
//! fills by the pure posting rule.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::ledger::{Account, Direction};
use qip_core::rng::Rng;
use qip_core::testing::Property;
use qip_core::{Clock, Decimal, ManualClock, Timestamp, Xoshiro256, sha256_hex};
use qip_execution_engine::order::Side;
use qip_kernel::desk_ledger::DeskLedgerFeed;
use qip_kernel::platform::FillBooked;
use qip_ledgerd::store::{Delivery, Disposition, LedgerStore, P1Record};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_portfolio::ledger::{PaperFill, post};
use qip_storage::EngineConfig;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const PARTITION: &str = "reflex-outcomes.local/0";
const CHAINS: [&str; 3] = ["desk", "desk-api", "desk-deep"];

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-desk-ledger-property-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn open(dir: &Path) -> LedgerStore {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    LedgerStore::open(
        dir,
        EngineConfig::new(clock),
        LedgerTelemetry::new(Arc::new(Metrics::new("qip-ledgerd"))),
    )
    .expect("the store opens")
}

fn wal_path(dir: &Path) -> PathBuf {
    std::fs::read_dir(dir)
        .expect("store dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("wal."))
        })
        .expect("a write-ahead log")
}

/// Desk records for every chain, `count` fills each, interleaved on one
/// partition preserving each chain's own order.
fn clean_stream(rng: &mut Xoshiro256) -> Vec<P1Record> {
    let mut per_chain: Vec<Vec<P1Record>> = Vec::new();
    for chain in CHAINS {
        let mut feed = DeskLedgerFeed::on_chain(chain).expect("a desk chain");
        let count = 1 + rng.below(6);
        for n in 0..count {
            let quantity = Decimal::from_int(1 + rng.below(9) as i64);
            let price = Decimal::from_int(90 + rng.below(20) as i64);
            let booked = FillBooked {
                order_id: format!("ord-{n}"),
                fill_id: format!("fill-{n}"),
                object_id: "obj-ACME".to_string(),
                venue: "paper-xnys".to_string(),
                side: if rng.below(2) == 0 {
                    Side::Buy
                } else {
                    Side::Sell
                },
                quantity,
                price,
                costs: Decimal::parse("0.5").expect("literal"),
                at: Timestamp::from_secs(1_790_000_000 + n as i64),
                simulated: true,
            };
            feed.admit(&booked, "central-desk", "cycle")
                .expect("admitted");
        }
        per_chain.push(
            feed.pending()
                .iter()
                .map(|o| P1Record::Outcome(Box::new(o.record.clone())))
                .collect(),
        );
    }
    let mut cursors = vec![0usize; per_chain.len()];
    let mut out = Vec::new();
    while cursors.iter().zip(&per_chain).any(|(c, v)| *c < v.len()) {
        let open: Vec<usize> = (0..per_chain.len())
            .filter(|&i| cursors[i] < per_chain[i].len())
            .collect();
        let pick = open[rng.below(open.len() as u64) as usize];
        out.push(per_chain[pick][cursors[pick]].clone());
        cursors[pick] += 1;
    }
    out
}

/// The clean stream with runs of earlier records re-sent at later offsets,
/// as a desk shipper does after a crash between send and acknowledgement.
fn dirty_stream(rng: &mut Xoshiro256, clean: &[P1Record]) -> Vec<P1Record> {
    let mut records = clean.to_vec();
    for _ in 0..1 + rng.below(3) {
        let end = 1 + rng.below(records.len() as u64) as usize;
        let start = rng.below(end as u64) as usize;
        let run: Vec<P1Record> = records[start..end].to_vec();
        let insert_at = end + rng.below((records.len() - end + 1) as u64) as usize;
        for (k, record) in run.into_iter().enumerate() {
            records.insert(insert_at + k, record);
        }
    }
    records
}

fn schedule(records: &[P1Record]) -> Vec<Delivery> {
    records
        .iter()
        .enumerate()
        .map(|(offset, record)| Delivery {
            partition: PARTITION.to_string(),
            offset: offset as u64,
            record_hash: sha256_hex(&serde_json::to_vec(record).expect("serialises")),
            record: record.clone(),
        })
        .collect()
}

fn consume(store: &LedgerStore, partition: &[Delivery]) -> Vec<Disposition> {
    let mut cursor = store.resume_offset(PARTITION).expect("resume offset");
    let mut seen = Vec::new();
    let mut steps = 0usize;
    while let Some(delivery) = partition.get(cursor as usize) {
        steps += 1;
        assert!(steps < 100_000, "the consumer never reached the end");
        let disposition = store.apply(delivery).expect("apply");
        cursor = match disposition {
            Disposition::ReReadFrom { offset } => offset,
            _ => cursor + 1,
        };
        seen.push(disposition);
    }
    seen
}

fn refold(records: &[P1Record]) -> BTreeMap<(Account, String), Decimal> {
    let mut balances = BTreeMap::new();
    for record in records {
        if let P1Record::Outcome(outcome) = record {
            let paper =
                PaperFill::try_from(outcome.as_ref()).expect("a desk record is a paper fill");
            for posting in post(&paper).expect("a desk fill posts").postings() {
                let entry = balances
                    .entry((posting.account.clone(), posting.unit.clone()))
                    .or_insert(Decimal::ZERO);
                *entry = match posting.direction {
                    Direction::Debit => *entry + posting.amount,
                    Direction::Credit => *entry - posting.amount,
                };
            }
        }
    }
    balances
}

/// Mutation: in `LedgerStore::apply`, treat a re-sent record at the chain's
/// last committed position as a new one (`position.first + 1 >= next`) — the
/// first re-sent run parks a clean desk chain for integrity and the property
/// fails on its first case.
#[test]
fn for_any_redelivery_and_crash_schedule_three_desks_records_book_single_delivery_balances() {
    let mut saw_duplicate = false;
    let mut saw_cut_inside = false;
    let mut saw_every_chain = false;
    Property::new("desk records: any redelivery/crash schedule books single-delivery balances")
        .cases(40)
        .for_all(
            |rng| {
                let clean = clean_stream(rng);
                let dirty = dirty_stream(rng, &clean);
                let cut = rng.below(1_000_000);
                let rewinds: Vec<u64> = (0..3).map(|_| rng.below(dirty.len() as u64)).collect();
                (clean, dirty, cut, rewinds)
            },
            |(clean, dirty, cut, rewinds)| {
                let expected = refold(clean);
                let cells: std::collections::BTreeSet<String> = clean
                    .iter()
                    .filter_map(|r| match r {
                        P1Record::Outcome(o) => Some(o.cell.clone()),
                        P1Record::Span(_) => None,
                    })
                    .collect();
                saw_every_chain |= cells.len() == CHAINS.len();

                let clean_dir = temp_dir("clean");
                {
                    let store = open(&clean_dir);
                    consume(&store, &schedule(clean));
                    let booked = store.balances().map_err(|e| e.to_string())?;
                    if booked != expected {
                        return Err(format!("premise: clean delivery booked {booked:?}"));
                    }
                }

                let partition = schedule(dirty);
                let dir = temp_dir("dirty");
                {
                    let store = open(&dir);
                    let first = consume(&store, &partition);
                    saw_duplicate |= first.contains(&Disposition::Duplicate);
                    for from in rewinds {
                        for delivery in &partition[*from as usize..] {
                            store.apply(delivery).map_err(|e| e.to_string())?;
                        }
                    }
                }
                let wal = wal_path(&dir);
                let bytes = std::fs::read(&wal).map_err(|e| e.to_string())?;
                let keep = 16 + (*cut as usize) % (bytes.len() - 16 + 1);
                saw_cut_inside |= keep < bytes.len();
                std::fs::write(&wal, &bytes[..keep]).map_err(|e| e.to_string())?;
                let store = open(&dir);
                consume(&store, &partition);

                let booked = store.balances().map_err(|e| e.to_string())?;
                let parked = store.parked().map_err(|e| e.to_string())?;
                let events = store.events().map_err(|e| e.to_string())?.len();
                let _ = std::fs::remove_dir_all(&dir);
                let _ = std::fs::remove_dir_all(&clean_dir);
                if !parked.is_empty() {
                    return Err(format!("a clean desk chain was parked: {parked:?}"));
                }
                if events != clean.len() {
                    return Err(format!(
                        "{events} ledger events for {} desk fills",
                        clean.len()
                    ));
                }
                if booked != expected {
                    return Err(format!(
                        "after a cut at {keep} of {} bytes the ledger booked {booked:?}, a \
                         single clean delivery {expected:?}",
                        bytes.len()
                    ));
                }
                for chain in &cells {
                    let tail = store
                        .tail(chain)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("no tail for {chain}"))?;
                    let count = clean
                        .iter()
                        .filter(|r| matches!(r, P1Record::Outcome(o) if &o.cell == chain))
                        .count();
                    if tail.sequence != count as u64 - 1 {
                        return Err(format!("{chain}'s tail is {} of {count}", tail.sequence));
                    }
                }
                Ok(())
            },
        );
    assert!(saw_duplicate, "premise: some case delivered a duplicate");
    assert!(saw_cut_inside, "premise: some case cut the log short");
    assert!(
        saw_every_chain,
        "premise: some case carried all three desk chains"
    );
}
