//! The ledger store: balances, chain tails and consumer offsets move in one
//! batch per record, duplicates move nothing, and a problem parks one cell.
//!
//! ADR 0100 §4 is the contract. Each test names the failure it prevents and
//! the mutation it was verified against.

use qip_contracts::ledger::{Account, Direction, LedgerEvent};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{
    ChainSpan, ChainVersion, Decision, JournalEntry, OutcomeRecord, seal_v2,
};
use qip_core::rng::Rng;
use qip_core::testing::Property;
use qip_core::{Clock, Decimal, ManualClock, Timestamp, Xoshiro256, sha256_hex};
use qip_ledgerd::store::{Delivery, Disposition, GENESIS, LedgerStore, P1Record, ParkReason};
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
