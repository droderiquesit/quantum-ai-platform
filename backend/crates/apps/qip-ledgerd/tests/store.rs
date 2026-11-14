//! The ledger store: balances, chain tails and consumer offsets move in one
//! batch per record, duplicates move nothing, and a problem parks one cell.
//!
//! ADR 0100 §4 is the contract. Each test names the failure it prevents and
//! the mutation it was verified against.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::ledger::{Account, Direction, LedgerEvent, SettlementState};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{
    ChainSpan, ChainVersion, Decision, JournalEntry, OutcomeRecord, seal_v2,
};
use qip_core::rng::Rng;
use qip_core::testing::Property;
use qip_core::{Clock, Decimal, ManualClock, Timestamp, Xoshiro256, sha256_hex};
use qip_ledgerd::store::{
    Delivery, Disposition, GENESIS, LedgerStore, OrderKind, OrderRow, OrderState, P1Record,
    ParkReason,
};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::{MetricValue, Metrics, names};
use qip_portfolio::ledger::{PaperFill, post};
use qip_storage::EngineConfig;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const PARTITION: &str = "p0";
const UNIT: &str = "USD";
const VENUE: &str = "sim-xnys";

// --- fixtures ---------------------------------------------------------------

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-ledger-store-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn config() -> EngineConfig {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    EngineConfig::new(clock)
}

fn open(dir: &Path) -> (LedgerStore, Arc<Metrics>) {
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let store = LedgerStore::open(dir, config(), LedgerTelemetry::new(metrics.clone()))
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

fn fill(order: &str, quantity: u64, price: &str, side: BookSide, simulated: bool) -> Decision {
    Decision::Filled {
        order_id: order.to_string(),
        venue: VENUE.to_string(),
        object: "OBJ-ACME".to_string(),
        quantity: quantity.to_string(),
        price: price.to_string(),
        simulated,
        shares: vec![("alpha".to_string(), quantity.to_string())],
        side: Some(side),
        quote_unit: Some(UNIT.to_string()),
        fee: None,
        decision_sequence: None,
    }
}

fn noise(n: u64) -> Decision {
    Decision::EdgePriced {
        opportunity: format!("opp-{n}"),
        net: "0.1".to_string(),
        positive: true,
    }
}

/// One cell's journal for one session, sealed exactly as `qip-edge` seals
/// it: sequence from 0, chained onto genesis under v2.
struct Journal {
    cell: String,
    session: u64,
    entries: Vec<JournalEntry>,
}

impl Journal {
    fn new(cell: &str, session: u64) -> Self {
        Self {
            cell: cell.to_string(),
            session,
            entries: Vec::new(),
        }
    }

    fn previous(&self) -> String {
        self.entries
            .last()
            .map_or_else(|| GENESIS.to_string(), |e| e.digest.clone())
    }

    /// Seal an entry without recording it: what a *different* record at the
    /// next position would carry.
    fn seal_next(&self, decision: Decision) -> JournalEntry {
        let sequence = self.entries.len() as u64;
        let at = Timestamp::from_secs(1_790_000_000 + sequence as i64);
        let (decision, digest) = seal_v2(&self.previous(), sequence, at, decision);
        JournalEntry {
            sequence,
            at,
            decision,
            digest,
            version: ChainVersion::V2,
        }
    }

    fn outcome(&mut self, decision: Decision) -> P1Record {
        let entry = self.seal_next(decision);
        self.entries.push(entry.clone());
        P1Record::Outcome(Box::new(self.wrap(entry)))
    }

    fn wrap(&self, entry: JournalEntry) -> OutcomeRecord {
        OutcomeRecord {
            cell: self.cell.clone(),
            session: self.session,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry,
        }
    }

    fn span(&mut self, count: u64) -> P1Record {
        let first = self.entries.len() as u64;
        for i in 0..count {
            let entry = self.seal_next(noise(first + i));
            self.entries.push(entry);
        }
        P1Record::Span(ChainSpan {
            cell: self.cell.clone(),
            session: self.session,
            first_seq: first,
            last_seq: first + count - 1,
            tail_digest: self.previous(),
        })
    }
}

fn hash_of(record: &P1Record) -> String {
    sha256_hex(&serde_json::to_vec(record).expect("a record serialises"))
}

/// A partition whose offsets are the indices of `records`.
fn schedule(records: &[P1Record]) -> Vec<Delivery> {
    records
        .iter()
        .enumerate()
        .map(|(offset, record)| Delivery {
            partition: PARTITION.to_string(),
            offset: offset as u64,
            record_hash: hash_of(record),
            record: record.clone(),
        })
        .collect()
}

fn at(offset: u64, record: &P1Record) -> Delivery {
    Delivery {
        partition: PARTITION.to_string(),
        offset,
        record_hash: hash_of(record),
        record: record.clone(),
    }
}

/// A consumer that honours the store: resumes where it says, re-reads when
/// it says. Returns every disposition in order.
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

/// Balances from the outcome stream alone, by the pure posting rule — the
/// ledger's answer recomputed by something that never touched the store.
fn refold(records: &[P1Record]) -> BTreeMap<(Account, String), Decimal> {
    let mut balances = BTreeMap::new();
    for record in records {
        if let P1Record::Outcome(outcome) = record
            && let Decision::Filled { .. } = outcome.entry.decision
        {
            let paper =
                PaperFill::try_from(outcome.as_ref()).expect("a clean stream holds paper fills");
            fold_event(&mut balances, &post(&paper).expect("a paper fill posts"));
        }
    }
    balances
}

fn fold_event(balances: &mut BTreeMap<(Account, String), Decimal>, event: &LedgerEvent) {
    for posting in event.postings() {
        let entry = balances
            .entry((posting.account.clone(), posting.unit.clone()))
            .or_insert(Decimal::ZERO);
        *entry = match posting.direction {
            Direction::Debit => *entry + posting.amount,
            Direction::Credit => *entry - posting.amount,
        };
    }
}

/// A clean P1 stream for two cells, one of which restarts into a second
/// session, interleaved preserving each cell's order. Also returns each
/// record's `(cell, session)` so a test can perturb within one chain only.
fn clean_stream(rng: &mut Xoshiro256) -> Vec<P1Record> {
    let mut per_cell: Vec<Vec<P1Record>> = Vec::new();
    for (cell, sessions) in [("cell-a", vec![1u64, 2]), ("cell-b", vec![5u64])] {
        let mut records = Vec::new();
        for session in sessions {
            let mut journal = Journal::new(cell, session);
            let items = 2 + rng.below(5);
            for n in 0..items {
                match rng.below(4) {
                    0 => records.push(journal.span(1 + rng.below(3))),
                    1 => records.push(journal.outcome(Decision::OrderExpired {
                        order_id: format!("{cell}-{session}-x{n}"),
                        venue: VENUE.to_string(),
                        withdrawn: "1".to_string(),
                    })),
                    _ => {
                        let side = if rng.below(2) == 0 {
                            BookSide::Ask
                        } else {
                            BookSide::Bid
                        };
                        let price = format!("{}.{:02}", 1 + rng.below(500), rng.below(100));
                        records.push(journal.outcome(fill(
                            &format!("{cell}-{session}-o{n}"),
                            1 + rng.below(50),
                            &price,
                            side,
                            true,
                        )));
                    }
                }
            }
        }
        per_cell.push(records);
    }
    let mut merged = Vec::new();
    let mut heads = vec![0usize; per_cell.len()];
    while heads.iter().zip(&per_cell).any(|(h, r)| *h < r.len()) {
        let live: Vec<usize> = (0..per_cell.len())
            .filter(|&i| heads[i] < per_cell[i].len())
            .collect();
        let pick = live[rng.below(live.len() as u64) as usize];
        merged.push(per_cell[pick][heads[pick]].clone());
        heads[pick] += 1;
    }
    merged
}

fn chain_of(record: &P1Record) -> (String, u64) {
    match record {
        P1Record::Outcome(o) => (o.cell.clone(), o.session),
        P1Record::Span(s) => (s.cell.clone(), s.session),
    }
}

/// The clean stream with records re-appended after broker loss and adjacent
/// records of one chain swapped, so a gap opens and is later filled.
fn dirty_stream(rng: &mut Xoshiro256, clean: &[P1Record]) -> Vec<P1Record> {
    let mut records = clean.to_vec();
    // Swap a record with the next record of the same (cell, session), which
    // may sit further along the partition.
    for _ in 0..rng.below(3) {
        let i = rng.below(records.len() as u64) as usize;
        let chain = chain_of(&records[i]);
        if let Some(j) = (i + 1..records.len()).find(|&j| chain_of(&records[j]) == chain) {
            records.swap(i, j);
        }
    }
    // Re-append a run of earlier records at new offsets, as a producer does
    // when the broker lost what it had acknowledged.
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

// --- tests --------------------------------------------------------------------

#[test]
fn for_any_duplication_redelivery_and_crash_between_records_the_balances_equal_a_single_clean_delivery()
 {
    // The failure this prevents: an offset that becomes durable before the
    // postings it stands for. A crash between the two leaves a consumer that
    // resumes past a fill nobody booked, and the ledger is short by exactly
    // that fill with nothing on it to say so. The crash is injected the way
    // a crash happens to this engine — the write-ahead log cut at an
    // arbitrary byte, the torn record discarded on reopen — so the test
    // needs no hook in the code it tests.
    //
    // Mutation verified: committing the offset in its own batch before the
    // postings fails this property.
    let mut saw_duplicate = false;
    let mut saw_reread = false;
    let mut saw_cut_inside = false;
    Property::new("crash between records leaves balances equal to a clean delivery")
        .cases(48)
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
                if expected.is_empty() {
                    // A stream with no fill proves nothing about balances.
                    return Ok(());
                }

                // Premise: a single clean delivery books exactly the refold.
                let clean_dir = temp_dir("clean");
                {
                    let (store, _) = open(&clean_dir);
                    consume(&store, &schedule(clean));
                    let booked = store.balances().map_err(|e| e.to_string())?;
                    if booked != expected {
                        return Err(format!("premise: clean delivery booked {booked:?}"));
                    }
                }

                // A dirty delivery, with redeliveries from arbitrary offsets.
                let partition = schedule(dirty);
                let dir = temp_dir("dirty");
                {
                    let (store, _) = open(&dir);
                    let first = consume(&store, &partition);
                    saw_duplicate |= first.contains(&Disposition::Duplicate);
                    saw_reread |= first
                        .iter()
                        .any(|d| matches!(d, Disposition::ReReadFrom { .. }));
                    for from in rewinds {
                        for delivery in &partition[*from as usize..] {
                            store.apply(delivery).map_err(|e| e.to_string())?;
                        }
                    }
                }

                // Crash: cut the log at an arbitrary byte, then reopen and
                // resume where the store says.
                let wal = wal_path(&dir);
                let bytes = std::fs::read(&wal).map_err(|e| e.to_string())?;
                let keep = 16 + (*cut as usize) % (bytes.len() - 16 + 1);
                saw_cut_inside |= keep < bytes.len();
                std::fs::write(&wal, &bytes[..keep]).map_err(|e| e.to_string())?;
                let (store, _) = open(&dir);
                consume(&store, &partition);

                let booked = store.balances().map_err(|e| e.to_string())?;
                let parked = store.parked().map_err(|e| e.to_string())?;
                let _ = std::fs::remove_dir_all(&dir);
                let _ = std::fs::remove_dir_all(&clean_dir);
                if !parked.is_empty() {
                    return Err(format!("a clean chain left cells parked: {parked:?}"));
                }
                if booked != expected {
                    return Err(format!(
                        "after a cut at {keep} of {} bytes the ledger booked {booked:?}, \
                         a clean delivery {expected:?}",
                        bytes.len()
                    ));
                }
                Ok(())
            },
        );
    assert!(saw_duplicate, "premise: some case delivered a duplicate");
    assert!(saw_reread, "premise: some case opened and filled a gap");
    assert!(saw_cut_inside, "premise: some case cut the log short");
}

#[test]
fn a_record_that_repeats_a_committed_sequence_with_the_same_digest_moves_nothing() {
    // The failure this prevents: a record re-appended after broker loss,
    // arriving at a new offset, booked a second time — every position it
    // touched doubled. Mutation verified: re-posting duplicates fails this.
    let dir = temp_dir("duplicate");
    let (store, metrics) = open(&dir);
    let mut journal = Journal::new("cell-a", 1);
    let first = journal.outcome(fill("o-1", 10, "100.5", BookSide::Ask, true));

    assert_eq!(
        store.apply(&at(0, &first)).expect("apply"),
        Disposition::Posted { postings: 4 },
        "premise: the fill is booked once"
    );
    let once = store.balances().expect("balances");
    assert!(!once.is_empty(), "premise: the fill moved balances");
    let tail = store.tail("cell-a").expect("tail");

    // The same record at a later offset, and again at the same offset.
    assert_eq!(
        store.apply(&at(7, &first)).expect("apply"),
        Disposition::Duplicate
    );
    assert_eq!(
        store.apply(&at(0, &first)).expect("apply"),
        Disposition::Duplicate
    );
    assert_eq!(
        store.balances().expect("balances"),
        once,
        "a duplicate moved a balance"
    );
    assert_eq!(
        store.events().expect("events").len(),
        1,
        "a duplicate was booked"
    );
    assert_eq!(
        store.tail("cell-a").expect("tail"),
        tail,
        "a duplicate moved the tail"
    );
    assert_eq!(
        store.resume_offset(PARTITION).expect("offset"),
        8,
        "the offset still moved"
    );
    assert_eq!(counter(&metrics, names::LEDGER_DUPLICATES), 2);
    assert!(
        store.parked().expect("parked").is_empty(),
        "a duplicate parked a cell"
    );
}

#[test]
fn a_record_that_repeats_a_committed_sequence_with_a_different_digest_parks_that_cell_and_no_other()
{
    // The failure this prevents (M10): one cell's integrity break stopping
    // every cell that shares its partition. Mutation verified: parking the
    // whole partition fails this.
    let dir = temp_dir("conflict");
    let (store, _) = open(&dir);
    let mut a = Journal::new("cell-a", 1);
    let mut b = Journal::new("cell-b", 1);
    let a0 = a.outcome(fill("a-0", 10, "10", BookSide::Ask, true));
    let b0 = b.outcome(fill("b-0", 5, "20", BookSide::Bid, true));
    assert!(matches!(
        store.apply(&at(0, &a0)).expect("apply"),
        Disposition::Posted { .. }
    ));
    assert!(matches!(
        store.apply(&at(1, &b0)).expect("apply"),
        Disposition::Posted { .. }
    ));

    // A different record sealed at cell-a's sequence 0: same position, a
    // different digest.
    let forged = P1Record::Outcome(Box::new(Journal::new("cell-a", 1).wrap(
        Journal::new("cell-a", 1).seal_next(fill("a-forged", 99, "10", BookSide::Ask, true)),
    )));
    let P1Record::Outcome(original) = &a0 else {
        unreachable!()
    };
    let P1Record::Outcome(other) = &forged else {
        unreachable!()
    };
    assert_eq!(
        other.entry.sequence, original.entry.sequence,
        "premise: same position"
    );
    assert_ne!(
        other.entry.digest, original.entry.digest,
        "premise: different digest"
    );

    let before = store.balances().expect("balances");
    assert_eq!(
        store.apply(&at(2, &forged)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::Integrity
        }
    );
    assert_eq!(
        store.balances().expect("balances"),
        before,
        "the conflict moved a balance"
    );

    // cell-b keeps being applied; cell-a's next valid record is held.
    let b1 = b.outcome(fill("b-1", 3, "20", BookSide::Ask, true));
    let a1 = a.outcome(fill("a-1", 4, "10", BookSide::Bid, true));
    assert!(
        matches!(
            store.apply(&at(3, &b1)).expect("apply"),
            Disposition::Posted { .. }
        ),
        "the other cell on the partition was stopped too"
    );
    assert_eq!(
        store.apply(&at(4, &a1)).expect("apply"),
        Disposition::Held {
            reason: ParkReason::Integrity
        }
    );
    let parked = store.parked().expect("parked");
    assert_eq!(
        parked.keys().collect::<Vec<_>>(),
        vec!["cell-a"],
        "only the conflicting cell"
    );
    assert_eq!(parked["cell-a"].offset, 2);
    let note = store
        .journal()
        .expect("journal")
        .into_iter()
        .find(|n| n.event == "integrity");
    assert!(
        note.is_some_and(|n| n.operator_action.contains("LedgerStore::release")),
        "the integrity park is journaled naming the operator action"
    );
}

#[test]
fn a_gap_in_a_cells_chain_parks_the_cell_until_the_missing_records_arrive() {
    // The failure this prevents: a record booked across a gap, so the ledger
    // holds sequence 2 and not sequence 1 and no later check can say which
    // fill is missing. Mutation verified: posting across the gap fails this.
    let dir = temp_dir("gap");
    let (store, _) = open(&dir);
    let mut a = Journal::new("cell-a", 1);
    let r0 = a.outcome(fill("a-0", 10, "10", BookSide::Ask, true));
    let r1 = a.outcome(fill("a-1", 20, "11", BookSide::Ask, true));
    let r2 = a.outcome(fill("a-2", 30, "12", BookSide::Bid, true));
    let r3 = a.outcome(fill("a-3", 40, "13", BookSide::Ask, true));

    assert!(matches!(
        store.apply(&at(0, &r0)).expect("apply"),
        Disposition::Posted { .. }
    ));
    let after_r0 = store.balances().expect("balances");
    assert!(!after_r0.is_empty(), "premise: r0 was booked");

    assert_eq!(
        store.apply(&at(1, &r2)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::Gap
        }
    );
    assert_eq!(
        store.apply(&at(2, &r3)).expect("apply"),
        Disposition::Held {
            reason: ParkReason::Gap
        }
    );
    assert_eq!(
        store.balances().expect("balances"),
        after_r0,
        "a record posted across the gap"
    );
    assert_eq!(
        store.tail("cell-a").expect("tail").map(|t| t.sequence),
        Some(0)
    );
    assert_eq!(
        store.parked().expect("parked")["cell-a"].reason,
        ParkReason::Gap
    );
    assert!(
        store.release("cell-a", "operator-1").is_err(),
        "a gap is not released by hand"
    );

    // The missing record arrives: applied, and the store asks for a re-read
    // from where the gap parked the cell.
    assert_eq!(
        store.apply(&at(3, &r1)).expect("apply"),
        Disposition::ReReadFrom { offset: 1 }
    );
    assert!(
        store.parked().expect("parked").is_empty(),
        "the filled gap still parks the cell"
    );
    assert!(matches!(
        store.apply(&at(1, &r2)).expect("apply"),
        Disposition::Posted { .. }
    ));
    assert!(matches!(
        store.apply(&at(2, &r3)).expect("apply"),
        Disposition::Posted { .. }
    ));
    assert_eq!(
        store.apply(&at(3, &r1)).expect("apply"),
        Disposition::Duplicate
    );
    assert_eq!(
        store.balances().expect("balances"),
        refold(&[r0, r1, r2, r3]),
        "the re-read chain books what a clean delivery books"
    );
}

#[test]
fn the_same_offset_arriving_with_a_different_record_hash_is_an_integrity_break_never_a_skip() {
    // The failure this prevents (M3): an offset rule deciding what the chain
    // should. A bare `offset <= consumed` skip would drop a record
    // legitimately re-appended below the consumed offset, and would read the
    // broker saying two things about one offset as nothing at all. Mutation
    // verified: skipping offsets at or below the consumed one fails this.
    let dir = temp_dir("same-offset");
    let (store, _) = open(&dir);
    let mut a = Journal::new("cell-a", 1);
    let mut b = Journal::new("cell-b", 1);
    let a0 = a.outcome(fill("a-0", 10, "10", BookSide::Ask, true));
    let b0 = b.outcome(fill("b-0", 5, "20", BookSide::Bid, true));
    let a1 = a.outcome(fill("a-1", 4, "10", BookSide::Bid, true));
    let b1 = b.outcome(fill("b-1", 6, "21", BookSide::Ask, true));

    assert!(matches!(
        store.apply(&at(5, &a0)).expect("apply"),
        Disposition::Posted { .. }
    ));
    assert!(matches!(
        store.apply(&at(6, &b0)).expect("apply"),
        Disposition::Posted { .. }
    ));
    assert_eq!(
        store.resume_offset(PARTITION).expect("offset"),
        7,
        "premise: consumed through 6"
    );

    // M3: a record re-appended below the consumed offset, never seen before,
    // is applied by chain continuity rather than skipped by its offset.
    assert!(
        matches!(
            store.apply(&at(2, &a1)).expect("apply"),
            Disposition::Posted { .. }
        ),
        "a new record below the consumed offset was skipped"
    );

    // Offset 2 again, now carrying a different record: b1 would chain
    // cleanly, and it still must not be applied or skipped.
    assert_ne!(hash_of(&a1), hash_of(&b1), "premise: two different records");
    let before = store.balances().expect("balances");
    assert_eq!(
        store.apply(&at(2, &b1)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::Integrity
        }
    );
    assert_eq!(
        store.balances().expect("balances"),
        before,
        "the conflicting record was booked"
    );
    let parked = store.parked().expect("parked");
    assert_eq!(parked.keys().collect::<Vec<_>>(), vec!["cell-b"]);
    assert_eq!(parked["cell-b"].reason, ParkReason::Integrity);
    assert!(
        store
            .journal()
            .expect("journal")
            .iter()
            .any(|n| n.event == "integrity" && n.detail.contains("two things about one position")),
        "the break is journaled for what it is"
    );
}

#[test]
fn refolding_the_outcome_stream_from_its_first_record_equals_the_stored_balances() {
    // The failure this prevents: a balance that moved without a posting
    // behind it, so the balances table and the ledger's own history disagree
    // and nothing reading either can tell which is right. The stored
    // balances must equal both a refold of the outcome stream by the pure
    // posting rule and a refold of the events the store kept. Mutation
    // verified: incrementing a balance without its posting fails this.
    Property::new("stored balances equal the refold of the stream")
        .cases(24)
        .for_all(clean_stream, |records| {
            let expected = refold(records);
            if expected.is_empty() {
                return Ok(());
            }
            let dir = temp_dir("refold");
            let booked = {
                let (store, _) = open(&dir);
                consume(&store, &schedule(records));
                let booked = store.balances().map_err(|e| e.to_string())?;
                let mut from_events = BTreeMap::new();
                for event in store.events().map_err(|e| e.to_string())? {
                    fold_event(&mut from_events, &event);
                }
                if from_events != booked {
                    return Err(format!(
                        "the stored events fold to {from_events:?}, the balances say {booked:?}"
                    ));
                }
                booked
            };
            // And after a restart, from disk.
            let (reopened, _) = open(&dir);
            let from_disk = reopened.balances().map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&dir);
            if booked != expected || from_disk != expected {
                return Err(format!(
                    "the stream refolds to {expected:?}; the store holds {booked:?}, \
                     and {from_disk:?} after reopening"
                ));
            }
            Ok(())
        });
}

#[test]
fn a_fill_not_marked_simulated_parks_that_cell_counts_it_and_posts_nothing() {
    // The failure this prevents: the fourth paper fence proven only at the
    // type, with `qip_ledger_live_fill_refused_total` registered and nothing
    // that fires it. Mutation verified: skipping the record and advancing
    // silently leaves the counter at 0 and the cell unparked, and fails this.
    let dir = temp_dir("live");
    let (store, metrics) = open(&dir);

    // Premise: the same fill marked simulated is booked.
    {
        let paper_dir = temp_dir("paper");
        let (paper, _) = open(&paper_dir);
        let mut j = Journal::new("cell-a", 1);
        let record = j.outcome(fill("a-0", 10, "10", BookSide::Ask, true));
        assert!(matches!(
            paper.apply(&at(0, &record)).expect("apply"),
            Disposition::Posted { .. }
        ));
    }

    let mut a = Journal::new("cell-a", 1);
    let mut b = Journal::new("cell-b", 1);
    let live = a.outcome(fill("a-0", 10, "10", BookSide::Ask, false));
    assert_eq!(
        counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED),
        0,
        "premise: nothing counted"
    );

    assert_eq!(
        store.apply(&at(0, &live)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert_eq!(counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED), 1);
    assert!(
        store.balances().expect("balances").is_empty(),
        "a live fill moved a balance"
    );
    assert!(
        store.events().expect("events").is_empty(),
        "a live fill was booked"
    );
    assert_eq!(
        store.tail("cell-a").expect("tail"),
        None,
        "a live fill advanced the chain"
    );
    assert_eq!(
        store.parked().expect("parked")["cell-a"].reason,
        ParkReason::LiveFill
    );
    assert!(
        store
            .journal()
            .expect("journal")
            .iter()
            .any(|n| n.event == "live_fill" && n.operator_action.contains("LedgerStore::release")),
        "the live fill is journaled naming the operator action"
    );

    // Only that cell: the partition advances and cell-b is booked.
    let b0 = b.outcome(fill("b-0", 5, "20", BookSide::Bid, true));
    assert!(matches!(
        store.apply(&at(1, &b0)).expect("apply"),
        Disposition::Posted { .. }
    ));
    assert_eq!(store.resume_offset(PARTITION).expect("offset"), 2);

    // Released by an operator, the re-read meets the same fence again.
    assert_eq!(store.release("cell-a", "operator-1").expect("release"), 0);
    assert_eq!(store.resume_offset(PARTITION).expect("offset"), 0);
    assert_eq!(
        store.apply(&at(0, &live)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert_eq!(counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED), 2);
    assert!(
        store
            .events()
            .expect("events")
            .iter()
            .all(|e| e.source().cell == "cell-b")
    );
}

#[test]
fn a_gap_parked_cell_rereads_from_its_parked_offset_once_the_missing_record_arrives() {
    // The failure this prevents: "park until the gap fills" with no account
    // of how a filled gap unparks the cell. Resuming from the current offset
    // would leave every record held behind the gap unposted for ever. The
    // store must answer the parked offset, and a restart in the middle of the
    // re-read must resume inside it. Mutation verified: resuming from the
    // current offset leaves the held records unposted and fails this.
    let mut a = Journal::new("cell-a", 1);
    let mut b = Journal::new("cell-b", 1);
    let a0 = a.outcome(fill("a-0", 10, "10", BookSide::Ask, true));
    let a1 = a.outcome(fill("a-1", 20, "11", BookSide::Ask, true));
    let a2 = a.span(3);
    let a3 = a.outcome(fill("a-3", 30, "12", BookSide::Bid, true));
    let b0 = b.outcome(fill("b-0", 5, "20", BookSide::Bid, true));
    let b1 = b.outcome(fill("b-1", 7, "21", BookSide::Ask, true));
    let clean = vec![
        a0.clone(),
        b0.clone(),
        a1.clone(),
        a2.clone(),
        b1.clone(),
        a3.clone(),
    ];
    // a1 is late: a2 and a3 arrive first and are held behind the gap.
    let late = vec![a0, a2, b0, a3, b1, a1];
    let partition = schedule(&late);

    let dir = temp_dir("reread");
    {
        let (store, _) = open(&dir);
        for delivery in &partition[..5] {
            store.apply(delivery).expect("apply");
        }
        assert_eq!(
            store.parked().expect("parked")["cell-a"].offset,
            1,
            "premise: parked at 1"
        );
        assert_eq!(
            store.apply(&partition[5]).expect("apply"),
            Disposition::ReReadFrom { offset: 1 },
            "the filled gap must send the consumer back to where it parked"
        );
        // Crash before the re-read: the resume offset is the parked one.
        assert_eq!(store.resume_offset(PARTITION).expect("offset"), 1);
    }
    let (store, _) = open(&dir);
    let resumed = consume(&store, &partition);
    assert!(
        resumed
            .iter()
            .filter(|d| matches!(d, Disposition::Posted { .. }))
            .count()
            == 1
            && resumed.contains(&Disposition::Advanced),
        "the held span and fill were re-read and applied: {resumed:?}"
    );
    assert!(store.parked().expect("parked").is_empty());
    assert_eq!(
        store.tail("cell-a").expect("tail").map(|t| t.sequence),
        Some(5)
    );
    assert_eq!(
        store.balances().expect("balances"),
        refold(&clean),
        "the re-read ledger differs from a clean delivery"
    );
}

#[test]
fn ledger_watermarks_are_signed_and_verifiable() {
    // Ledger periodically signs (previous_digest, state_digest, timestamp) to
    // form an unbreakable chain. The signature is HMAC-SHA256.
    // Mutation: if signature is not verified, this test fails.
    let signing_key = b"stable-ledger-key-12345";
    let dir = temp_dir("watermark");
    let (store, _) = open(&dir);

    // Record first watermark
    let timestamp_1 = 1_000_000_000u64;
    let state_digest_1 = "digest-first".to_string();
    store
        .record_watermark(timestamp_1, state_digest_1.clone(), signing_key)
        .expect("record first watermark");

    // Verify it exists and is valid
    let watermark_1 = store
        .current_watermark()
        .expect("get watermark")
        .expect("watermark exists");
    assert_eq!(watermark_1.timestamp, timestamp_1, "timestamp stored");
    assert_eq!(watermark_1.state_digest, state_digest_1, "digest stored");
    assert!(watermark_1.verify(signing_key), "watermark verifies");
    assert!(
        !watermark_1.verify(b"wrong-key"),
        "wrong key fails verification"
    );

    // Record second watermark, which chains onto the first
    let timestamp_2 = 1_000_001_000u64;
    let state_digest_2 = "digest-second".to_string();
    store
        .record_watermark(timestamp_2, state_digest_2.clone(), signing_key)
        .expect("record second watermark");

    let watermark_2 = store
        .current_watermark()
        .expect("get watermark")
        .expect("watermark exists");
    assert_eq!(watermark_2.timestamp, timestamp_2, "second timestamp");
    assert_eq!(watermark_2.state_digest, state_digest_2, "second digest");
    assert_eq!(
        watermark_2.previous_digest, watermark_1.signature,
        "chains to previous"
    );
    assert!(watermark_2.verify(signing_key), "second watermark verifies");
}

#[test]
fn corrupted_watermark_fails_verification() {
    // Mutation: if verify() doesn't check the digest, this test fails.
    let signing_key = b"immutable-ledger-key";
    let dir = temp_dir("chain");
    let (store, _) = open(&dir);

    store
        .record_watermark(1_000, "digest-1".to_string(), signing_key)
        .expect("record 1");
    store
        .record_watermark(2_000, "digest-2".to_string(), signing_key)
        .expect("record 2");
    store
        .record_watermark(3_000, "digest-3".to_string(), signing_key)
        .expect("record 3");

    let wm3 = store
        .current_watermark()
        .expect("get watermark")
        .expect("watermark exists");

    // Verify: signature is correct
    assert!(wm3.verify(signing_key), "watermark 3 verifies");

    // If digest were corrupted, verification fails
    let mut corrupted = wm3.clone();
    corrupted.state_digest = "tampered-digest".to_string();
    assert!(
        !corrupted.verify(signing_key),
        "modified watermark fails verification"
    );

    // The previous_digest links to watermark 2's signature
    assert!(
        !wm3.previous_digest.is_empty(),
        "watermark 3 chains to previous"
    );
}

#[test]
fn ledger_exports_current_state_including_watermarks() {
    // Mutation: if export doesn't include balances, this test fails.
    let mut a = Journal::new("cell-a", 1);
    let a0 = a.outcome(fill("a-0", 100, "10.50", BookSide::Ask, true));
    let a1 = a.outcome(fill("a-1", 50, "11.25", BookSide::Bid, true));

    let dir = temp_dir("export");
    let (store, _) = open(&dir);
    let partition = schedule(&[a0, a1]);
    for delivery in &partition {
        store.apply(delivery).expect("apply");
    }

    // Sign and record a watermark before export
    let signing_key = b"ledger-secret-key";
    store
        .record_watermark(2_000_000_000, "state-v1-hash".to_string(), signing_key)
        .expect("record watermark");

    // Export state
    let exported = store.export_state().expect("export state");
    assert!(!exported.balances.is_empty(), "exported contains balances");
    assert!(
        exported.chain_records_count > 0,
        "exported contains chain records"
    );
    assert_eq!(
        exported
            .current_watermark
            .as_ref()
            .map(|w| w.state_digest.clone()),
        Some("state-v1-hash".to_string()),
        "exported watermark has correct digest"
    );
}

/// R12-LEDGER-8137. Consecutive deliveries for one cell used to write
/// `chain/<cell>/<session>/<first>`, a key that only grows, so every apply
/// for the cell landed at the end of one key range: the hotspot the journal
/// and watermark keys had already been spread to avoid. The chain key is
/// now sharded over (session, first), and the chain still reads back in
/// sequence order, because order comes from the key's position segments.
///
/// Mutation: drop the shard segment from `chain_key` and the keys sort in
/// write order again, so the test fails.
#[test]
fn consecutive_deliveries_for_one_cell_never_write_chain_keys_that_sort_in_write_order_and_the_chain_reads_back_in_sequence_order()
 {
    let dir = temp_dir("chain-hotspot");
    let mut journal = Journal::new("cell-a", 1);
    let records: Vec<P1Record> = (0..12)
        .map(|n| {
            journal.outcome(fill(
                &format!("cell-a-1-o{n}"),
                1 + n,
                "100.00",
                BookSide::Bid,
                true,
            ))
        })
        .collect();
    {
        let (store, _) = open(&dir);
        consume(&store, &schedule(&records));
        let events = store.fill_events().expect("the chain reads back");
        assert_eq!(
            events.len(),
            12,
            "premise: twelve fills, one booked event each"
        );
        let records = store.chain_records().expect("the chain reads back");
        let lasts: Vec<u64> = records.iter().map(|record| record.last_seq).collect();
        assert_eq!(
            lasts,
            (0..12).collect::<Vec<u64>>(),
            "the chain does not read back in sequence order"
        );
    }
    let raw = qip_storage::DurableStore::open(&dir, config()).expect("the store reopens");
    let keys: Vec<String> = raw
        .scan_prefix("chain/")
        .expect("scan")
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        keys.len(),
        12,
        "premise: one chain record per delivery: {keys:?}"
    );
    let firsts: Vec<u64> = keys
        .iter()
        .map(|key| key.rsplit('/').next().unwrap().parse().unwrap())
        .collect();
    assert!(
        firsts.windows(2).any(|pair| pair[0] > pair[1]),
        "the chain's keys sort in the order they were written: {keys:?}"
    );
}

// --- settlement state (R12-LEDGER-5011, ADR 0130 decision 1-2, ADR 0440) ----

fn settled(order: &str, settlement_date: Timestamp, simulated: bool) -> Decision {
    Decision::Settled {
        order_id: order.to_string(),
        venue: VENUE.to_string(),
        settlement_date,
        simulated,
    }
}

fn settling(dir: &Path) -> (LedgerStore, Arc<Metrics>) {
    let (store, metrics) = open(dir);
    let store = store
        .with_settlement_cycle_venues([VENUE.to_string()])
        .expect("a named venue is admitted");
    (store, metrics)
}

fn trading(strategy: &str) -> Account {
    Account::Trading {
        cell: "cell-a".to_string(),
        strategy: strategy.to_string(),
    }
}

fn in_settlement(strategy: &str) -> Account {
    Account::InSettlement {
        cell: "cell-a".to_string(),
        strategy: strategy.to_string(),
    }
}

fn last_event(store: &LedgerStore) -> LedgerEvent {
    store
        .fill_events()
        .expect("events")
        .pop()
        .expect("an event was booked")
}

#[test]
fn a_sale_at_a_settlement_cycle_venue_holds_its_proceeds_in_settlement_until_the_venue_settles_them()
 {
    // The failure this prevents: the ledger calling a sale's proceeds
    // spendable on the trade date while the cell's own settlement gate says
    // they are not (ADR 0130's context). Mutation verified twice: posting
    // the cash leg to `trading:` in `post_in_settlement` is refused by
    // `LedgerEvent::staged` (a stage with nothing in settlement) and parks
    // the cell, failing the first `Posted` assertion; dropping the
    // `in_settlement:` cash leg leaves the event unbalanced in USD, which
    // `LedgerEvent::new` refuses, and parks it the same way (the
    // requirement's "drop the posting and the balance check fails").
    let dir = temp_dir("settle-sale");
    let (store, _) = settling(&dir);
    let mut j = Journal::new("cell-a", 1);
    let sale = j.outcome(fill("a-0", 10, "10", BookSide::Bid, true));
    assert_eq!(
        store.apply(&at(0, &sale)).expect("apply"),
        Disposition::Posted { postings: 4 }
    );

    // Before the settlement: the units left the book, the cash did not
    // arrive, and the event says why.
    assert_eq!(
        store.balance(&trading("alpha"), "OBJ-ACME").expect("units"),
        Decimal::from_int(-10),
        "premise: the position moved at the trade date"
    );
    assert_eq!(
        store.balance(&trading("alpha"), UNIT).expect("cash"),
        Decimal::ZERO,
        "unsettled proceeds were booked as spendable cash"
    );
    assert_eq!(
        store
            .balance(&in_settlement("alpha"), UNIT)
            .expect("pending"),
        Decimal::from_int(100)
    );
    let pending = last_event(&store);
    assert_eq!(pending.settlement_state(), SettlementState::InSettlement);
    assert_eq!(
        store
            .pending_settlement("cell-a", "a-0")
            .expect("pending")
            .len(),
        1
    );

    // The pending row survives a restart: a process that lost it would
    // refuse the venue's settlement as settling nothing.
    drop(store);
    let (store, _) = settling(&dir);

    let date = Timestamp::from_secs(1_790_172_800);
    let settlement = j.outcome(settled("a-0", date, true));
    assert_eq!(
        store.apply(&at(1, &settlement)).expect("apply"),
        Disposition::Posted { postings: 2 }
    );
    assert_eq!(
        store.balance(&trading("alpha"), UNIT).expect("cash"),
        Decimal::from_int(100),
        "the settlement did not reach the book"
    );
    assert_eq!(
        store
            .balance(&in_settlement("alpha"), UNIT)
            .expect("pending"),
        Decimal::ZERO,
        "the settlement left cash in settlement"
    );
    assert_eq!(
        last_event(&store).settlement_state(),
        SettlementState::Settled {
            settlement_date: date,
            fills: vec![0],
        }
    );
    assert!(
        store
            .pending_settlement("cell-a", "a-0")
            .expect("pending")
            .is_empty(),
        "a settled order is still held as pending"
    );
    assert!(
        store
            .trial_balance()
            .expect("trial balance")
            .values()
            .all(|total| *total == Decimal::ZERO),
        "the trial balance does not close"
    );
    // The fill's own event still reads as it was booked: in settlement. A
    // settlement is a new event, never an edit of the fill (LEDGER-017).
    assert_eq!(
        store.fill_events().expect("events")[0].settlement_state(),
        SettlementState::InSettlement
    );
}

#[test]
fn a_purchase_filled_in_parts_keeps_its_cash_in_the_book_until_one_settlement_pays_for_every_part()
{
    // The failure this prevents: a purchase whose cash left the book at the
    // trade date, and an order filled twice whose settlement paid for one
    // part and stranded the other. Mutation verified: settling only the
    // last held fill in `post_settlement` books two postings instead of four
    // and fails the `Posted` assertion, stranding the first part's 50 USD.
    let dir = temp_dir("settle-buy");
    let (store, _) = settling(&dir);
    let mut j = Journal::new("cell-a", 1);
    let first = j.outcome(fill("a-0", 5, "10", BookSide::Ask, true));
    let second = j.outcome(fill("a-0", 3, "10", BookSide::Ask, true));
    store.apply(&at(0, &first)).expect("apply");
    store.apply(&at(1, &second)).expect("apply");
    assert_eq!(
        store.balance(&trading("alpha"), UNIT).expect("cash"),
        Decimal::ZERO,
        "a purchase paid before settlement"
    );
    assert_eq!(
        store.balance(&in_settlement("alpha"), UNIT).expect("owed"),
        Decimal::from_int(-80),
        "premise: both parts are owed in settlement"
    );

    let date = Timestamp::from_secs(1_790_172_800);
    let settlement = j.outcome(settled("a-0", date, true));
    assert_eq!(
        store.apply(&at(2, &settlement)).expect("apply"),
        Disposition::Posted { postings: 4 }
    );
    assert_eq!(
        store.balance(&trading("alpha"), UNIT).expect("cash"),
        Decimal::from_int(-80)
    );
    assert_eq!(
        store.balance(&in_settlement("alpha"), UNIT).expect("owed"),
        Decimal::ZERO
    );
    assert_eq!(
        last_event(&store).settlement_state(),
        SettlementState::Settled {
            settlement_date: date,
            fills: vec![0, 1],
        }
    );
}

#[test]
fn a_fill_at_a_venue_with_no_declared_cycle_books_its_cash_at_once_and_reads_as_immediate() {
    // The failure this prevents: every venue's cash held pending because
    // one venue settles on a cycle. Mutation verified: making the store post
    // every fill through `post_in_settlement` leaves 0 USD in the book and
    // fails the cash assertion.
    let dir = temp_dir("settle-instant");
    let (store, _) = open(&dir);
    let store = store
        .with_settlement_cycle_venues(["sim-other".to_string()])
        .expect("admitted");
    let mut j = Journal::new("cell-a", 1);
    let sale = j.outcome(fill("a-0", 10, "10", BookSide::Bid, true));
    store.apply(&at(0, &sale)).expect("apply");
    assert_eq!(
        store.balance(&trading("alpha"), UNIT).expect("cash"),
        Decimal::from_int(100)
    );
    assert_eq!(
        last_event(&store).settlement_state(),
        SettlementState::Immediate
    );
    assert!(
        store
            .pending_settlement("cell-a", "a-0")
            .expect("pending")
            .is_empty()
    );
}

#[test]
fn a_settlement_of_an_order_with_nothing_in_settlement_parks_the_cell_and_moves_nothing() {
    // The failure this prevents: a venue's settlement booked against cash the
    // ledger never held pending, which would double the proceeds, or a
    // refused settlement silently stepped over. Mutation verified: the store
    // advancing past a refused settlement instead of parking returns
    // `Advanced` and fails the `Parked` assertion; holding every fill
    // pending whatever its venue books this settlement and fails it too.
    let dir = temp_dir("settle-nothing");
    let (store, _) = open(&dir);
    let mut j = Journal::new("cell-a", 1);
    let sale = j.outcome(fill("a-0", 10, "10", BookSide::Bid, true));
    store.apply(&at(0, &sale)).expect("apply");
    let before = store.balances().expect("balances");
    let settlement = j.outcome(settled("a-0", Timestamp::from_secs(1_790_172_800), true));
    assert_eq!(
        store.apply(&at(1, &settlement)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::RefusedFill
        }
    );
    assert_eq!(store.balances().expect("balances"), before);
    assert_eq!(store.fill_events().expect("events").len(), 1);
}

#[test]
fn a_settlement_not_marked_simulated_is_refused_as_live_and_leaves_the_cash_in_settlement() {
    // The fourth paper fence, for the new record: a `Settled` that does not
    // say it is simulated is refused exactly as a live fill is. Mutation
    // verified: removing the `simulated` check in `PaperSettlement::try_from`
    // books it, moves 100 USD to the book and fails every assertion below.
    let dir = temp_dir("settle-live");
    let (store, metrics) = settling(&dir);
    let mut j = Journal::new("cell-a", 1);
    let sale = j.outcome(fill("a-0", 10, "10", BookSide::Bid, true));
    store.apply(&at(0, &sale)).expect("apply");
    let live = j.outcome(settled("a-0", Timestamp::from_secs(1_790_172_800), false));
    assert_eq!(
        store.apply(&at(1, &live)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert_eq!(counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED), 1);
    assert_eq!(
        store
            .balance(&in_settlement("alpha"), UNIT)
            .expect("pending"),
        Decimal::from_int(100)
    );
    assert_eq!(
        store.balance(&trading("alpha"), UNIT).expect("cash"),
        Decimal::ZERO
    );
}

#[test]
fn a_blank_settlement_cycle_venue_is_refused() {
    // A blank name matches no fill, so a declaration of one reads as a
    // configured cycle while being none. Mutation verified: removing the
    // `refuse_blank` call admits it and fails this.
    let dir = temp_dir("settle-blank");
    let (store, _) = open(&dir);
    assert!(store.with_settlement_cycle_venues([String::new()]).is_err());
}

// --- the order register (ADR 0490) ------------------------------------------

fn sent(order: &str, quantity: u64, simulated: bool) -> Decision {
    Decision::OrderSent {
        order_id: order.to_string(),
        venue: VENUE.to_string(),
        quantity: quantity.to_string(),
        simulated,
        release_at: None,
        equalised: false,
        grants: Vec::new(),
        grounds: Vec::new(),
    }
}

fn hedged(order: &str, quantity: u64, simulated: bool) -> Decision {
    Decision::Hedged {
        order_id: order.to_string(),
        venue: VENUE.to_string(),
        object: "OBJ-ACME".to_string(),
        quantity: quantity.to_string(),
        simulated,
        grounds: vec!["evidence:journal:feed_silent:3".to_string()],
    }
}

fn row(store: &LedgerStore, order: &str) -> OrderRow {
    store
        .orders()
        .expect("orders")
        .into_iter()
        .find(|r| r.order_id == order)
        .unwrap_or_else(|| panic!("no register row for {order}"))
}

#[test]
fn a_hedge_opens_a_register_row_before_it_fills_and_its_fills_move_the_row_and_the_balances() {
    // The failure this prevents: R12-LEDGER-10114's "unledgered hedges are
    // positions the books do not know". Before ADR 0490 `Decision::Hedged`
    // reached P1 and the ledger advanced past it writing nothing, so a hedge
    // that had not yet filled existed nowhere in the central books.
    // Mutation verified: deleting the `Decision::Hedged` arm of
    // `register_move` fails on the missing row for `a-1`; deleting the fill
    // arm's addition leaves the intent `Open` after its whole fill; dropping
    // the register row from the batch fails on the first row read.
    let dir = temp_dir("register");
    let (store, _) = open(&dir);
    let mut j = Journal::new("cell-a", 1);
    let mut offset = 0;
    let mut records = Vec::new();
    let mut deliver = |store: &LedgerStore, record: P1Record| {
        let disposition = store.apply(&at(offset, &record)).expect("apply");
        offset += 1;
        records.push(record);
        disposition
    };

    // An intent and its fill: the row opens, then closes as filled.
    deliver(&store, j.outcome(sent("a-0", 10, true)));
    assert_eq!(row(&store, "a-0").state, OrderState::Open);
    assert_eq!(row(&store, "a-0").kind, OrderKind::Intent);
    deliver(
        &store,
        j.outcome(fill("a-0", 10, "10", BookSide::Ask, true)),
    );
    let intent = row(&store, "a-0");
    assert_eq!(intent.state, OrderState::Filled);
    assert_eq!(intent.filled, Decimal::parse("10").unwrap());

    // The hedge: known to the books the moment it is sent, before any fill.
    let before_hedge = store.balances().expect("balances");
    assert!(
        !before_hedge.is_empty(),
        "premise: the intent's fill was booked"
    );
    assert_eq!(
        deliver(&store, j.outcome(hedged("a-1", 10, true))),
        Disposition::Advanced
    );
    let hedge = row(&store, "a-1");
    assert_eq!(hedge.kind, OrderKind::Hedge);
    assert_eq!(hedge.state, OrderState::Open);
    assert_eq!(hedge.object.as_deref(), Some("OBJ-ACME"));
    assert_eq!(hedge.quantity, Decimal::parse("10").unwrap());
    assert_eq!(hedge.filled, Decimal::ZERO);
    assert_eq!(
        store.balances().expect("balances"),
        before_hedge,
        "a hedge that has not filled moves no balance; the register is a memo"
    );

    // A partial fill of the hedge moves the balances and the row together.
    deliver(&store, j.outcome(fill("a-1", 4, "10", BookSide::Bid, true)));
    let partial = store.balances().expect("balances");
    assert_ne!(partial, before_hedge, "the hedge's fill moved no balance");
    let hedge = row(&store, "a-1");
    assert_eq!(hedge.state, OrderState::Open);
    assert_eq!(hedge.filled, Decimal::parse("4").unwrap());

    deliver(&store, j.outcome(fill("a-1", 6, "10", BookSide::Bid, true)));
    let hedge = row(&store, "a-1");
    assert_eq!(hedge.state, OrderState::Filled);
    assert_eq!(hedge.filled, Decimal::parse("10").unwrap());

    // A hedge withdrawn before it filled closes as withdrawn, not filled.
    deliver(&store, j.outcome(hedged("a-2", 5, true)));
    deliver(
        &store,
        j.outcome(Decision::MassCancelled {
            order_id: "a-2".to_string(),
            venue: VENUE.to_string(),
            withdrawn: "5".to_string(),
        }),
    );
    let withdrawn = row(&store, "a-2");
    assert_eq!(withdrawn.state, OrderState::Withdrawn);
    assert_eq!(withdrawn.withdrawn.as_deref(), Some("5"));
    assert_eq!(withdrawn.filled, Decimal::ZERO);
    assert_eq!(
        store.balances().expect("balances"),
        refold(&records),
        "the register changed nothing a refold of the stream disagrees with"
    );

    // A redelivery moves nothing, the register included.
    let rows = store.orders().expect("orders");
    let last = records.len() as u64 - 1;
    assert_eq!(
        store
            .apply(&at(last, &records[last as usize]))
            .expect("apply"),
        Disposition::Duplicate
    );
    assert_eq!(store.orders().expect("orders"), rows);
}

#[test]
fn a_hedge_not_marked_simulated_parks_its_cell_and_opens_no_register_row() {
    // The failure this prevents: the fourth paper fence held for fills and
    // settlements only, so a hedge sent to a live venue was advanced past
    // as if it were noise and would have opened a register row as an
    // ordinary open hedge. Mutation verified: removing the `Hedged` arm of
    // `live_intent` lets the record advance (`Advanced`, not `Parked`) and
    // opens a row, failing both assertions.
    let dir = temp_dir("live-hedge");
    let (store, metrics) = open(&dir);
    let mut j = Journal::new("cell-a", 1);

    // Premise: the same hedge marked simulated is admitted.
    {
        let paper_dir = temp_dir("paper-hedge");
        let (paper, _) = open(&paper_dir);
        let mut p = Journal::new("cell-a", 1);
        assert_eq!(
            paper
                .apply(&at(0, &p.outcome(hedged("a-0", 3, true))))
                .expect("apply"),
            Disposition::Advanced
        );
        assert_eq!(paper.orders().expect("orders").len(), 1);
    }

    let live = j.outcome(hedged("a-0", 3, false));
    assert_eq!(
        store.apply(&at(0, &live)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert_eq!(counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED), 1);
    assert!(store.orders().expect("orders").is_empty());
    assert_eq!(store.tail("cell-a").expect("tail"), None);
    assert!(
        store
            .journal()
            .expect("journal")
            .iter()
            .any(|n| n.event == "live_fill" && n.detail.contains("hedge a-0")),
        "the refusal names the hedge"
    );
}

#[test]
fn an_order_sent_not_marked_simulated_parks_its_cell_and_opens_no_register_row() {
    // The same fence for an intent. Mutation verified: removing the
    // `OrderSent` arm of `live_intent` advances the record and fails this.
    let dir = temp_dir("live-intent");
    let (store, _) = open(&dir);
    let mut j = Journal::new("cell-a", 1);
    assert_eq!(
        store
            .apply(&at(0, &j.outcome(sent("a-0", 3, false))))
            .expect("apply"),
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert!(store.orders().expect("orders").is_empty());
}

#[test]
fn an_order_opened_twice_in_one_session_parks_the_cell_and_keeps_the_first_row() {
    // The failure this prevents: a second opening silently replacing the
    // first row would forget the fills booked against it, and the register
    // would understate a position the balances hold. Mutation verified:
    // removing the already-open check advances the record and overwrites
    // the row's `filled` with zero, failing both assertions.
    let dir = temp_dir("reopen");
    let (store, _) = open(&dir);
    let mut j = Journal::new("cell-a", 1);
    store
        .apply(&at(0, &j.outcome(hedged("a-0", 5, true))))
        .expect("apply");
    store
        .apply(&at(
            1,
            &j.outcome(fill("a-0", 2, "10", BookSide::Ask, true)),
        ))
        .expect("apply");
    assert_eq!(
        store
            .apply(&at(2, &j.outcome(hedged("a-0", 5, true))))
            .expect("apply"),
        Disposition::Parked {
            reason: ParkReason::Integrity
        }
    );
    assert_eq!(row(&store, "a-0").filled, Decimal::parse("2").unwrap());

    // The same order id in a later session is a different order: the cell
    // numbers orders from one at every start.
    let other_dir = temp_dir("reopen-session");
    let (other, _) = open(&other_dir);
    let mut first = Journal::new("cell-a", 1);
    let mut second = Journal::new("cell-a", 2);
    other
        .apply(&at(0, &first.outcome(hedged("a-0", 5, true))))
        .expect("apply");
    assert_eq!(
        other
            .apply(&at(1, &second.outcome(hedged("a-0", 5, true))))
            .expect("apply"),
        Disposition::Advanced
    );
    assert_eq!(other.orders().expect("orders").len(), 2);
}

// --- custody statements on the stream (ADR 0494, R12-LEDGER-10185) ----------

const OBJECT: &str = "OBJ-ACME";

fn custody(holdings: &[(&str, &str)], simulated: bool) -> Decision {
    Decision::CustodyStatement {
        venue: VENUE.to_string(),
        holdings: holdings
            .iter()
            .map(|(unit, quantity)| ((*unit).to_string(), (*quantity).to_string()))
            .collect(),
        cash_units: vec![UNIT.to_string()],
        simulated,
    }
}

fn custody_breaks(metrics: &Metrics, direction: &str) -> u64 {
    metrics
        .snapshot()
        .series
        .iter()
        .filter(|s| {
            s.name == names::LEDGER_CUSTODY_BREAKS
                && s.labels.get("direction").map(String::as_str) == Some(direction)
        })
        .map(|s| match &s.value {
            MetricValue::Counter(v) => *v,
            other => panic!("not a counter: {other:?}"),
        })
        .sum()
}

fn custody_notes(store: &LedgerStore) -> Vec<String> {
    store
        .journal()
        .expect("journal")
        .into_iter()
        .filter(|note| note.event == "custody_break")
        .map(|note| note.detail)
        .collect()
}

/// Two fills on the cell's chain, so the ledger holds a non-zero position
/// the statement can agree or disagree with; returns the store, the metrics,
/// the journal and the ledger's own holding of the instrument.
fn holding_ten(label: &str) -> (LedgerStore, Arc<Metrics>, Journal, Decimal, PathBuf) {
    let dir = temp_dir(label);
    let (store, metrics) = open(&dir);
    let mut j = Journal::new("cell-a", 1);
    let first = j.outcome(fill("a-0", 6, "10", BookSide::Ask, true));
    let second = j.outcome(fill("a-1", 4, "10", BookSide::Ask, true));
    store.apply(&at(0, &first)).expect("apply");
    store.apply(&at(1, &second)).expect("apply");
    let held = store.balance(&trading("alpha"), OBJECT).expect("holding");
    // The premise: the ledger holds something, so "agrees" is not the
    // comparison of two zeros.
    assert_eq!(held.abs(), Decimal::from_int(10));
    (store, metrics, j, held, dir)
}

#[test]
fn a_custody_statement_on_the_stream_that_agrees_with_the_booked_fills_records_no_break() {
    // The failure this prevents: a comparison that reads every statement as a
    // break — a cash unit counted on one side only, or the statement read
    // before the fills it covers — halting every cell on its first
    // statement. Mutation verified: dropping `cash_units` from
    // `CustodyStatement::from_entry` counts the USD cash leg as a
    // `ledger_over_venue`/`venue_over_ledger` break and fails here.
    let (store, metrics, mut j, held, _dir) = holding_ten("custody-agrees");
    let statement = j.outcome(custody(&[(OBJECT, &held.to_string())], true));
    assert_eq!(
        store.apply(&at(2, &statement)).expect("apply"),
        Disposition::CustodyCompared { breaks: 0 }
    );
    assert!(custody_notes(&store).is_empty());
    assert_eq!(custody_breaks(&metrics, "ledger_over_venue"), 0);
    assert_eq!(custody_breaks(&metrics, "venue_over_ledger"), 0);
    // The chain advanced onto the statement: it is a record, not a side
    // channel, and the next entry follows it.
    assert_eq!(store.resume_offset(PARTITION).expect("offset"), 3);
}

#[test]
fn a_seeded_divergence_between_the_venue_statement_and_the_ledger_is_one_journaled_break_counted_by_direction_and_never_an_adjustment()
 {
    // The failure this prevents: the ledger and the custodian disagreeing
    // about what the cell holds while nothing notices, or the ledger
    // "fixing" itself to the statement and erasing the evidence. Mutation
    // verified: making `apply` skip `compare_custody` leaves the
    // disposition `Advanced` and no note or counter; swapping the
    // comparison's direction arms in `custody::compare` counts the break
    // under `venue_over_ledger` and fails the direction assertion.
    let (store, metrics, mut j, held, _dir) = holding_ten("custody-breaks");
    let sign = if held.is_positive() { 1 } else { -1 };
    let short_by_three = held
        .checked_sub(Decimal::from_int(3 * sign))
        .expect("arithmetic");
    let statement = j.outcome(custody(&[(OBJECT, &short_by_three.to_string())], true));
    assert_eq!(
        store.apply(&at(2, &statement)).expect("apply"),
        Disposition::CustodyCompared { breaks: 1 }
    );
    let notes = custody_notes(&store);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].starts_with(&format!("{OBJECT}: ")), "{notes:?}");
    // The comparison is signed: the side with the larger signed quantity is
    // the one that holds more.
    let expected = if held > short_by_three {
        "ledger_over_venue"
    } else {
        "venue_over_ledger"
    };
    let other = if expected == "ledger_over_venue" {
        "venue_over_ledger"
    } else {
        "ledger_over_venue"
    };
    assert_eq!(custody_breaks(&metrics, expected), 1);
    assert_eq!(custody_breaks(&metrics, other), 0);
    // Never an adjustment: the ledger still books what the fills said.
    assert_eq!(
        store.balance(&trading("alpha"), OBJECT).expect("holding"),
        held
    );

    // A venue that holds more than the ledger booked is the other direction.
    let over = held
        .checked_add(Decimal::from_int(5 * sign))
        .expect("arithmetic");
    let second = j.outcome(custody(&[(OBJECT, &over.to_string())], true));
    assert_eq!(
        store.apply(&at(3, &second)).expect("apply"),
        Disposition::CustodyCompared { breaks: 1 }
    );
    assert_eq!(custody_breaks(&metrics, expected), 1);
    assert_eq!(custody_breaks(&metrics, other), 1);
}

#[test]
fn a_fill_journaled_after_the_statement_is_not_on_the_ledger_side_of_it() {
    // The failure this prevents: a statement compared against fills the
    // venue had not yet booked when it took the statement, which reads every
    // busy pass as a break. The statement's own entry is applied and
    // compared; a later fill is booked; the same statement is then compared
    // again out of band, under a second venue name so no mark makes it
    // `Stale`, on a ledger that now holds the later fill — and must agree.
    // Mutation verified: removing the chain-position filter in
    // `compare_custody` counts the later fill and answers `Broken`.
    let (store, metrics, mut j, held, _dir) = holding_ten("custody-position");
    let statement_record = j.outcome(custody(&[(OBJECT, &held.to_string())], true));
    let later = j.outcome(fill("a-2", 7, "10", BookSide::Ask, true));
    let P1Record::Outcome(outcome) = &statement_record else {
        panic!("an outcome")
    };
    let mut statement = qip_contracts::ledger::custody::CustodyStatement::from_entry(
        "cell-a",
        outcome.session,
        &outcome.entry,
    )
    .expect("parses")
    .expect("a statement");
    assert_eq!(
        store.apply(&at(2, &statement_record)).expect("apply"),
        Disposition::CustodyCompared { breaks: 0 }
    );
    assert_eq!(
        store.reconcile_custody(&statement).expect("compares"),
        qip_ledgerd::store::CustodyReconciliation::Stale,
        "a statement compared when it was applied is not compared twice"
    );
    store.apply(&at(3, &later)).expect("apply");
    assert_ne!(
        store.balance(&trading("alpha"), OBJECT).expect("holding"),
        held,
        "premise: the later fill moved the ledger"
    );
    statement.venue = "sim-second-name".to_string();
    assert_eq!(
        store.reconcile_custody(&statement).expect("compares"),
        qip_ledgerd::store::CustodyReconciliation::Agrees
    );
    assert!(custody_notes(&store).is_empty());
    assert_eq!(custody_breaks(&metrics, "ledger_over_venue"), 0);
    assert_eq!(custody_breaks(&metrics, "venue_over_ledger"), 0);
}

#[test]
fn a_custody_statement_not_marked_simulated_parks_the_cell_as_live_and_compares_nothing() {
    // The paper fence for the new record (ADR 0130 decision 6): a statement
    // that does not say it is simulated is refused exactly as a live fill
    // is. Mutation verified: removing the `!statement.simulated` park in
    // `apply` compares it — disposition `CustodyCompared` — and fails here.
    let (store, metrics, mut j, held, _dir) = holding_ten("custody-live");
    let live = j.outcome(custody(&[(OBJECT, "0")], false));
    assert_eq!(
        store.apply(&at(2, &live)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::LiveFill
        }
    );
    assert_eq!(counter(&metrics, names::LEDGER_LIVE_FILL_REFUSED), 1);
    assert!(custody_notes(&store).is_empty());
    assert_eq!(custody_breaks(&metrics, "ledger_over_venue"), 0);
    assert_eq!(
        store.balance(&trading("alpha"), OBJECT).expect("holding"),
        held
    );
}

#[test]
fn a_custody_statement_whose_holding_is_not_an_exact_decimal_parks_the_cell() {
    // A statement nobody can read is not a statement that agrees. Mutation
    // verified: treating an unparsable holding as absent in `from_entry`
    // reads "ten" as zero, compares, and fails the disposition below.
    let (store, _metrics, mut j, _held, _dir) = holding_ten("custody-garbled");
    let garbled = j.outcome(custody(&[(OBJECT, "ten")], true));
    assert_eq!(
        store.apply(&at(2, &garbled)).expect("apply"),
        Disposition::Parked {
            reason: ParkReason::RefusedFill
        }
    );
    assert!(custody_notes(&store).is_empty());
}
