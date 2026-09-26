//! The ledger's durable state: balances, per-cell chain tails and
//! per-partition consumer offsets, moved together or not at all (ADR 0100 §4).
//!
//! # One record, one batch
//!
//! [`LedgerStore::apply`] turns one P1 delivery into exactly one
//! [`WriteBatch`] handed to [`DurableStore::commit`], which writes it as one
//! write-ahead-log record and `fsync`s it. Whatever that delivery changes —
//! the event's postings, every balance they touch, the cell's chain tail, a
//! park, a journal note, and the partition's consumed offset with the record
//! hash — is in that one batch. The failure this prevents is the one a
//! two-step write has: an offset committed ahead of its postings survives a
//! crash that the postings do not, the consumer resumes past a fill nobody
//! booked, and the ledger is short by exactly that fill for ever with nothing
//! on it to say so. With one batch a crash leaves the old state or the new,
//! and `tests/store.rs` proves it by cutting the log at random bytes.
//!
//! # Deduplication is chain continuity, never an offset rule
//!
//! Each cell's journal is a hash chain per `(cell, session)`. A record is
//! placed on it by its sequence and digest, not by where the broker happened
//! to put it (red-team M3: a bare `offset <= consumed` rule silently drops a
//! record legitimately re-appended after broker loss).
//!
//! * `seq == tail + 1`, chaining onto the tail's digest — applied.
//! * `seq <= tail` with the digest the ledger already holds at that sequence
//!   — a duplicate; only the offset moves.
//! * `seq <= tail` with any other digest — an integrity break; the cell is
//!   parked until an operator releases it.
//! * `seq > tail + 1` — a gap; the cell is parked at this offset while the
//!   partition keeps advancing for every other cell (M10). When the missing
//!   record arrives the store applies it and answers
//!   [`Disposition::ReReadFrom`] the parked offset, and the held records are
//!   re-read and applied, idempotent by the same continuity rule.
//! * A new session starts at sequence 0 on [`GENESIS`] and needs a strictly
//!   greater session counter; a record from an older session is a duplicate
//!   of something already held or an integrity break, never a fresh start.
//!
//! The same offset arriving with a different record hash is an integrity
//! break, never a skip: the broker has said two different things about one
//! position and the ledger will not pick one.
//!
//! What the duplicate check reads is the ledger's own chain records — the
//! postings it booked and the spans it accepted, which it keeps anyway
//! because they *are* the ledger (LEDGER-017). There is no separate set of
//! seen event ids and no [`qip_events`] `EventLog`, whose bounded capacity
//! would make deduplication stop working on the day it filled (M11).
//!
//! # The fourth paper fence, at the store
//!
//! A `Filled` outcome is turned into postings only through
//! [`PaperFill::try_from`], which refuses a fill whose `simulated` flag is not
//! true (ADR 0100 §9). The store answers that refusal by parking the cell,
//! counting it on `qip_ledger_live_fill_refused_total`, and posting nothing.
//! It is checked before the chain position, so a live fill is refused as
//! live wherever it sits.
//!
//! # No update, no delete
//!
//! There is no method that changes or removes a posting or a balance
//! (LEDGER-017). The only key this module ever deletes is a park, when the
//! gap it recorded fills or an operator releases it.

use crate::telemetry::LedgerTelemetry;
use qip_contracts::ledger::{Account, Direction, LedgerEvent};
use qip_contracts::reflex::{ChainSpan, Decision, OutcomeRecord};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_portfolio::ledger::{PaperFill, post};
use qip_storage::{DurableStore, EngineConfig, KeyValueStore, WriteBatch};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// The digest a session's first journal entry chains onto.
///
/// The same text as `qip_edge::Journal::GENESIS`, which seals every cell's
/// journal; this crate does not depend on `qip-edge`, so the value is
/// restated here and a change to either is a change to the chain contract.
pub const GENESIS: &str = "genesis";

/// The `kind` label on `qip_ledger_duplicates_total`: a duplicate found by
/// chain continuity, the only kind this store recognises.
const DUPLICATE_BY_CHAIN: &str = "chain";

const PREFIX_BALANCE: &str = "balance/";
const PREFIX_CHAIN: &str = "chain/";
const PREFIX_TAIL: &str = "tail/";
const PREFIX_PARK: &str = "park/";
const PREFIX_CURSOR: &str = "cursor/";
const PREFIX_JOURNAL: &str = "journal/";
const KEY_JOURNAL_NEXT: &str = "meta/journal_next";

// --- what is delivered ------------------------------------------------------

/// One record on the P1 outcomes stream (ADR 0100 §5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum P1Record {
    /// A journal entry that carries an outcome: a fill, a cancel, a break.
    /// Boxed because it is four times the size of a span.
    Outcome(Box<OutcomeRecord>),
    /// A continuity record spanning a run of non-outcome entries.
    Span(ChainSpan),
}

impl P1Record {
    /// The cell whose chain this record sits on.
    pub fn cell(&self) -> &str {
        match self {
            Self::Outcome(outcome) => &outcome.cell,
            Self::Span(span) => &span.cell,
        }
    }
}

/// A P1 record as the broker delivered it: where, and what it hashed to.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    /// The broker partition; several cells may share one.
    pub partition: String,
    pub offset: u64,
    /// The broker's hash of the record's bytes. Compared, never computed
    /// here, so two deliveries of one offset are judged by what the broker
    /// said each time.
    pub record_hash: String,
    pub record: P1Record,
}

// --- what the store answers --------------------------------------------------

/// Why a cell is parked, from least to most severe.
///
/// Ordered so that a second park on an already-parked cell keeps the more
/// severe reason: a gap that turns out to hide a digest conflict must not be
/// released by the gap filling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    /// A record arrived past the next sequence. Unparks itself when the
    /// missing record arrives.
    Gap,
    /// A fill that could not be posted honestly: malformed, inexact, or a
    /// balance that would overflow. Held until an operator releases it.
    RefusedFill,
    /// Two different records claim one chain position, one offset carries two
    /// hashes, a digest does not chain, or a session went backwards. Held
    /// until an operator releases it.
    Integrity,
    /// A fill not marked simulated — the fourth paper fence. Held until an
    /// operator releases it.
    LiveFill,
}

impl ParkReason {
    /// Whether only an operator can clear this park. Everything but a gap.
    pub fn holds_until_release(self) -> bool {
        !matches!(self, Self::Gap)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Gap => "gap",
            Self::RefusedFill => "refused_fill",
            Self::Integrity => "integrity",
            Self::LiveFill => "live_fill",
        }
    }
}

/// A parked cell: why, and the offset its held records start at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Park {
    pub cell: String,
    pub partition: String,
    pub reason: ParkReason,
    /// The earliest offset held for this cell; re-reading starts here.
    pub offset: u64,
    /// For a gap, the session whose next sequence is awaited. Only a record
    /// of that session at that sequence fills the gap; a record of any other
    /// session is held, or a new session could close an old session's gap
    /// and strand its missing record as a stale one.
    pub awaiting_session: Option<u64>,
}

/// What [`LedgerStore::apply`] did with one delivery. Every variant was
/// committed durably before it was returned, including the offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// A fill was booked as one balanced event of this many postings.
    Posted { postings: usize },
    /// The chain advanced on a record that books nothing: a non-fill outcome
    /// or a continuity span.
    Advanced,
    /// Already on the chain with the same digest; only the offset moved.
    Duplicate,
    /// This record parked its cell; nothing else moved but the offset.
    Parked { reason: ParkReason },
    /// The cell was already parked; this record is held, not applied.
    Held { reason: ParkReason },
    /// This record filled a gap and was applied. The consumer must re-read
    /// the partition from `offset`, where the cell's held records start.
    ReReadFrom { offset: u64 },
}

/// A durable note of a park, a gap filled, or a release, naming what an
/// operator must do — or that nobody need do anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalNote {
    pub cell: String,
    pub partition: String,
    pub offset: u64,
    /// `gap`, `refused_fill`, `integrity`, `live_fill`, `gap_filled` or
    /// `released`.
    pub event: String,
    pub detail: String,
    pub operator_action: String,
}

/// A cell's chain position: the last sequence applied and its digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainTail {
    pub session: u64,
    pub sequence: u64,
    pub digest: String,
}

// --- stored rows --------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BalanceRow {
    account: Account,
    unit: String,
    /// Debits minus credits. Signed, so a trading account that has paid out
    /// cash reads negative in that unit rather than being split in two.
    amount: Decimal,
}

/// One record the ledger applied, keyed by `(cell, session, first_seq)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChainRecord {
    last_seq: u64,
    digest: String,
    offset: u64,
    record_hash: String,
    event: Option<LedgerEvent>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cursor {
    /// The next offset a consumer resuming this partition must read.
    next: u64,
    /// The most recent delivery applied and the hash it carried.
    last: Option<(u64, String)>,
}

/// A record's claim about where it sits on its cell's chain.
struct Position {
    session: u64,
    first: u64,
    last: u64,
    digest: String,
}

// --- the store ----------------------------------------------------------------

/// The ledger's single writer over one [`DurableStore`].
#[derive(Debug)]
pub struct LedgerStore {
    store: DurableStore,
    telemetry: LedgerTelemetry,
    /// Serialises read-decide-commit. The engine serialises commits, but the
    /// decision reads the tail, the park and the cursor first, and two
    /// deliveries deciding on the same tail would both apply.
    writer: Mutex<()>,
}

impl LedgerStore {
    /// Open the ledger at `directory`, recovering whatever the last process
    /// committed.
    pub fn open(
        directory: impl AsRef<Path>,
        config: EngineConfig,
        telemetry: LedgerTelemetry,
    ) -> Result<Self> {
        let store = DurableStore::open(directory, config)?;
        let ledger = Self {
            store,
            telemetry,
            writer: Mutex::new(()),
        };
        // A restarted process reports the parks it inherited, not zero.
        ledger.record_parked_keys()?;
        Ok(ledger)
    }

    /// Apply one delivery, committing everything it changes in one batch.
    ///
    /// An `Err` means nothing moved — not the balances, not the tail, not
    /// the offset — and the same delivery may be offered again.
    pub fn apply(&self, delivery: &Delivery) -> Result<Disposition> {
        refuse_blank("partition", &delivery.partition)?;
        refuse_blank("record hash", &delivery.record_hash)?;
        let cell = delivery.record.cell();
        refuse_blank("cell", cell)?;

        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let cursor = self.cursor(&delivery.partition)?;
        let park = self.park(cell)?;

        if let Some((offset, hash)) = &cursor.last
            && *offset == delivery.offset
            && *hash != delivery.record_hash
        {
            return self.park_cell(
                delivery,
                &cursor,
                park,
                ParkReason::Integrity,
                format!(
                    "offset {} was consumed with record hash {hash} and has arrived again \
                     with {}; the broker has said two things about one position",
                    delivery.offset, delivery.record_hash
                ),
            );
        }

        // The fourth paper fence, before the chain: a live fill is refused as
        // live wherever it sits, and never waits behind a gap to be counted.
        let fill = match &delivery.record {
            P1Record::Outcome(outcome) => match &outcome.entry.decision {
                Decision::Filled { simulated, .. } => {
                    Some((PaperFill::try_from(outcome.as_ref()), *simulated))
                }
                _ => None,
            },
            P1Record::Span(_) => None,
        };
        if let Some((Err(refusal), false)) = &fill {
            self.telemetry.live_fill_refused();
            return self.park_cell(
                delivery,
                &cursor,
                park,
                ParkReason::LiveFill,
                refusal.message().to_string(),
            );
        }

        let position = match position(&delivery.record) {
            Ok(position) => position,
            Err(refusal) => {
                return self.park_cell(
                    delivery,
                    &cursor,
                    park,
                    ParkReason::Integrity,
                    refusal.message().to_string(),
                );
            }
        };
        let tail = self.tail(cell)?;

        // Where the next record of this session must start, and what it must
        // chain onto. `None` means the record's session is older than the
        // tail's: it can only be something already held.
        let expected = match &tail {
            None => Some((0, GENESIS.to_string())),
            Some(t) if position.session > t.session => Some((0, GENESIS.to_string())),
            Some(t) if position.session == t.session => match t.sequence.checked_add(1) {
                Some(next) => Some((next, t.digest.clone())),
                None => {
                    return self.park_cell(
                        delivery,
                        &cursor,
                        park,
                        ParkReason::Integrity,
                        format!(
                            "the chain tail is at sequence {}, and nothing can follow it",
                            t.sequence
                        ),
                    );
                }
            },
            Some(_) => None,
        };

        let (next, previous) = match expected {
            Some((next, previous)) if position.first >= next => (next, previous),
            _ => return self.duplicate_or_conflict(delivery, &cursor, park, &position),
        };

        if position.first > next {
            if let Some(held) = park {
                return self.hold(delivery, &cursor, held.reason);
            }
            return self.park_cell_awaiting(
                delivery,
                &cursor,
                None,
                ParkReason::Gap,
                Some(position.session),
                format!(
                    "sequence {} arrived in session {} where {next} was next; the cell waits \
                     for {next}",
                    position.first, position.session
                ),
            );
        }

        if let Some(held) = &park
            && (held.reason.holds_until_release()
                || held.awaiting_session != Some(position.session))
        {
            return self.hold(delivery, &cursor, held.reason);
        }

        if let P1Record::Outcome(outcome) = &delivery.record
            && let Err(detail) = chains_onto(outcome, &previous)
        {
            return self.park_cell(delivery, &cursor, park, ParkReason::Integrity, detail);
        }

        let event = match fill {
            None => None,
            Some((Err(refusal), _)) => {
                return self.park_cell(
                    delivery,
                    &cursor,
                    park,
                    ParkReason::RefusedFill,
                    refusal.message().to_string(),
                );
            }
            Some((Ok(paper), _)) => match post(&paper) {
                Ok(event) => Some(event),
                Err(refusal) => {
                    self.telemetry.unbalanced_refused();
                    return self.park_cell(
                        delivery,
                        &cursor,
                        park,
                        ParkReason::RefusedFill,
                        refusal.message().to_string(),
                    );
                }
            },
        };

        let mut batch = WriteBatch::new();
        if let Some(event) = &event {
            match self.fold(event) {
                Ok(rows) => {
                    for (key, row) in rows {
                        batch = batch.put_as(key, &row)?;
                    }
                }
                Err(refusal) => {
                    return self.park_cell(
                        delivery,
                        &cursor,
                        park,
                        ParkReason::RefusedFill,
                        refusal.message().to_string(),
                    );
                }
            }
        }

        batch = batch.put_as(
            tail_key(cell),
            &ChainTail {
                session: position.session,
                sequence: position.last,
                digest: position.digest.clone(),
            },
        )?;
        batch = batch.put_as(
            chain_key(cell, position.session, position.first),
            &ChainRecord {
                last_seq: position.last,
                digest: position.digest.clone(),
                offset: delivery.offset,
                record_hash: delivery.record_hash.clone(),
                event: event.clone(),
            },
        )?;

        let mut advanced = advance(&cursor, delivery)?;
        let mut reread = None;
        if let Some(gap) = &park {
            // Only a gap park reaches here; a holding park returned above.
            advanced.next = advanced.next.min(gap.offset);
            reread = Some(gap.offset);
            batch = batch.delete(park_key(cell));
            batch = self.note(
                batch,
                JournalNote {
                    cell: cell.to_string(),
                    partition: gap.partition.clone(),
                    offset: gap.offset,
                    event: "gap_filled".to_string(),
                    detail: format!(
                        "sequence {} arrived at offset {} and closed the gap",
                        position.first, delivery.offset
                    ),
                    operator_action: format!(
                        "none: the partition is re-read from offset {}",
                        gap.offset
                    ),
                },
            )?;
        }
        batch = batch.put_as(cursor_key(&delivery.partition), &advanced)?;
        self.store.commit(batch)?;

        self.telemetry.commit("success");
        if reread.is_some() {
            self.record_parked_keys()?;
        }
        Ok(match (reread, &event) {
            (Some(offset), _) => Disposition::ReReadFrom { offset },
            (None, Some(event)) => Disposition::Posted {
                postings: event.postings().len(),
            },
            (None, None) => Disposition::Advanced,
        })
    }

    /// The offset a consumer resuming `partition` must read next.
    pub fn resume_offset(&self, partition: &str) -> Result<u64> {
        Ok(self.cursor(partition)?.next)
    }

    /// Every balance, keyed by account and unit, as debits minus credits.
    pub fn balances(&self) -> Result<BTreeMap<(Account, String), Decimal>> {
        let mut balances = BTreeMap::new();
        for (_, value) in self.store.scan_prefix(PREFIX_BALANCE)? {
            let row: BalanceRow = decode(value)?;
            balances.insert((row.account, row.unit), row.amount);
        }
        Ok(balances)
    }

    /// Every event booked, in chain order per cell and session.
    pub fn events(&self) -> Result<Vec<LedgerEvent>> {
        let mut events = Vec::new();
        for (_, value) in self.store.scan_prefix(PREFIX_CHAIN)? {
            let record: ChainRecord = decode(value)?;
            events.extend(record.event);
        }
        Ok(events)
    }

    /// A cell's chain tail, if anything of it has been applied.
    pub fn tail(&self, cell: &str) -> Result<Option<ChainTail>> {
        self.read(&tail_key(cell))
    }

    /// Every parked cell, by cell.
    pub fn parked(&self) -> Result<BTreeMap<String, Park>> {
        let mut parks = BTreeMap::new();
        for (_, value) in self.store.scan_prefix(PREFIX_PARK)? {
            let park: Park = decode(value)?;
            parks.insert(park.cell.clone(), park);
        }
        Ok(parks)
    }

    /// Every journal note, oldest first.
    pub fn journal(&self) -> Result<Vec<JournalNote>> {
        self.store
            .scan_prefix(PREFIX_JOURNAL)?
            .into_iter()
            .map(|(_, value)| decode(value))
            .collect()
    }

    /// An operator releases a cell held by an integrity break, a refused
    /// fill or a live fill, and the partition's resume offset moves back to
    /// the parked offset so the held records are re-read.
    ///
    /// Refused for a gap park — a gap unparks when the missing record
    /// arrives, and releasing it by hand would let the next record post
    /// across the gap — and for a cell that is not parked. Releasing does not
    /// excuse the record that caused the park: re-read, a live fill parks the
    /// cell again.
    pub fn release(&self, cell: &str, operator: &str) -> Result<u64> {
        refuse_blank("operator", operator)?;
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let Some(park) = self.park(cell)? else {
            return Err(Error::invalid(format!(
                "cell {cell} is not parked; there is nothing to release"
            )));
        };
        if !park.reason.holds_until_release() {
            return Err(Error::denied(format!(
                "cell {cell} is parked on a gap at offset {}; it unparks when the missing \
                 record arrives, and releasing it by hand would post across the gap",
                park.offset
            )));
        }
        let mut cursor = self.cursor(&park.partition)?;
        cursor.next = cursor.next.min(park.offset);
        let mut batch = WriteBatch::new()
            .delete(park_key(cell))
            .put_as(cursor_key(&park.partition), &cursor)?;
        batch = self.note(
            batch,
            JournalNote {
                cell: cell.to_string(),
                partition: park.partition.clone(),
                offset: park.offset,
                event: "released".to_string(),
                detail: format!("a {} park released by {operator}", park.reason.as_str()),
                operator_action: format!("re-read the partition from offset {}", park.offset),
            },
        )?;
        self.store.commit(batch)?;
        self.record_parked_keys()?;
        Ok(park.offset)
    }

    // --- decisions --------------------------------------------------------

    /// A record at or behind the tail: the same record again, or a conflict.
    fn duplicate_or_conflict(
        &self,
        delivery: &Delivery,
        cursor: &Cursor,
        park: Option<Park>,
        position: &Position,
    ) -> Result<Disposition> {
        let cell = delivery.record.cell();
        let held: Option<ChainRecord> =
            self.read(&chain_key(cell, position.session, position.first))?;
        let detail = match held {
            Some(record)
                if record.last_seq == position.last && record.digest == position.digest =>
            {
                if record.offset == delivery.offset && record.record_hash != delivery.record_hash {
                    format!(
                        "offset {} holds sequence {} with record hash {} and has arrived again \
                         with {}",
                        delivery.offset, position.first, record.record_hash, delivery.record_hash
                    )
                } else {
                    let batch = WriteBatch::new()
                        .put_as(cursor_key(&delivery.partition), &advance(cursor, delivery)?)?;
                    self.store.commit(batch)?;
                    self.telemetry.duplicate(DUPLICATE_BY_CHAIN);
                    self.telemetry.commit("duplicate");
                    return Ok(Disposition::Duplicate);
                }
            }
            Some(record) => format!(
                "session {} sequence {} is held with digest {} through {}, and a record claiming \
                 digest {} through {} has arrived for it",
                position.session,
                position.first,
                record.digest,
                record.last_seq,
                position.digest,
                position.last
            ),
            None => format!(
                "session {} sequence {} is behind the chain tail and the ledger holds no record \
                 starting there; a stale session or a re-framed span is not a fresh start",
                position.session, position.first
            ),
        };
        self.park_cell(delivery, cursor, park, ParkReason::Integrity, detail)
    }

    /// Hold a record for a parked cell: only the offset moves.
    fn hold(
        &self,
        delivery: &Delivery,
        cursor: &Cursor,
        reason: ParkReason,
    ) -> Result<Disposition> {
        let batch = WriteBatch::new()
            .put_as(cursor_key(&delivery.partition), &advance(cursor, delivery)?)?;
        self.store.commit(batch)?;
        Ok(Disposition::Held { reason })
    }

    /// Park the delivery's cell and nothing else, advancing the partition so
    /// every other cell keeps being applied (M10).
    fn park_cell(
        &self,
        delivery: &Delivery,
        cursor: &Cursor,
        existing: Option<Park>,
        reason: ParkReason,
        detail: String,
    ) -> Result<Disposition> {
        self.park_cell_awaiting(delivery, cursor, existing, reason, None, detail)
    }

    fn park_cell_awaiting(
        &self,
        delivery: &Delivery,
        cursor: &Cursor,
        existing: Option<Park>,
        reason: ParkReason,
        awaiting_session: Option<u64>,
        detail: String,
    ) -> Result<Disposition> {
        let cell = delivery.record.cell();
        let park = match existing {
            Some(held) => Park {
                cell: cell.to_string(),
                partition: held.partition,
                reason: held.reason.max(reason),
                offset: held.offset.min(delivery.offset),
                awaiting_session: held.awaiting_session,
            },
            None => Park {
                cell: cell.to_string(),
                partition: delivery.partition.clone(),
                reason,
                offset: delivery.offset,
                awaiting_session,
            },
        };
        let operator_action = if park.reason.holds_until_release() {
            format!(
                "investigate, then call LedgerStore::release naming the operator; the partition \
                 is re-read from offset {}",
                park.offset
            )
        } else {
            format!(
                "none: the cell unparks when its missing record arrives and re-reads from \
                 offset {}",
                park.offset
            )
        };
        let mut batch = WriteBatch::new()
            .put_as(park_key(cell), &park)?
            .put_as(cursor_key(&delivery.partition), &advance(cursor, delivery)?)?;
        batch = self.note(
            batch,
            JournalNote {
                cell: cell.to_string(),
                partition: delivery.partition.clone(),
                offset: delivery.offset,
                event: reason.as_str().to_string(),
                detail,
                operator_action,
            },
        )?;
        self.store.commit(batch)?;

        match reason {
            ParkReason::Integrity => self.telemetry.commit("conflict"),
            ParkReason::RefusedFill | ParkReason::LiveFill => self.telemetry.commit("refused"),
            ParkReason::Gap => {}
        }
        self.record_parked_keys()?;
        Ok(Disposition::Parked { reason })
    }

    /// The balance rows an event moves, each checked, or a refusal.
    fn fold(&self, event: &LedgerEvent) -> Result<BTreeMap<String, BalanceRow>> {
        let mut rows: BTreeMap<String, BalanceRow> = BTreeMap::new();
        for posting in event.postings() {
            let key = balance_key(&posting.account, &posting.unit);
            let current = match rows.get(&key) {
                Some(row) => row.amount,
                None => self
                    .read::<BalanceRow>(&key)?
                    .map_or(Decimal::ZERO, |row| row.amount),
            };
            let moved = match posting.direction {
                Direction::Debit => current.checked_add(posting.amount),
                Direction::Credit => current.checked_sub(posting.amount),
            };
            let Some(amount) = moved else {
                return Err(Error::numeric(format!(
                    "the {} balance of {} would overflow; the fill is refused rather than \
                     booked against a wrapped balance",
                    posting.unit, posting.account
                )));
            };
            rows.insert(
                key,
                BalanceRow {
                    account: posting.account.clone(),
                    unit: posting.unit.clone(),
                    amount,
                },
            );
        }
        Ok(rows)
    }

    // --- rows -------------------------------------------------------------

    fn note(&self, batch: WriteBatch, note: JournalNote) -> Result<WriteBatch> {
        let next: u64 = self.read(KEY_JOURNAL_NEXT)?.unwrap_or(0);
        let after = next
            .checked_add(1)
            .ok_or_else(|| Error::numeric("the ledger journal counter would wrap"))?;
        batch
            .put_as(format!("{PREFIX_JOURNAL}{next:020}"), &note)?
            .put_as(KEY_JOURNAL_NEXT, &after)
    }

    fn cursor(&self, partition: &str) -> Result<Cursor> {
        Ok(self.read(&cursor_key(partition))?.unwrap_or_default())
    }

    fn park(&self, cell: &str) -> Result<Option<Park>> {
        self.read(&park_key(cell))
    }

    fn read<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.store.get(key)?.map(decode).transpose()
    }

    fn record_parked_keys(&self) -> Result<()> {
        let parked = self.store.scan_prefix(PREFIX_PARK)?.len();
        self.telemetry.parked_keys(parked as u64);
        Ok(())
    }
}

// --- helpers --------------------------------------------------------------------

fn refuse_blank(what: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::invalid(format!(
            "a delivery needs a non-empty {what}; the ledger cannot place an anonymous record"
        )));
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| {
        Error::schema(format!(
            "a ledger row does not parse as what this build wrote: {e}"
        ))
    })
}

/// Where a record claims to sit, or a refusal of a span that runs backwards.
fn position(record: &P1Record) -> Result<Position> {
    match record {
        P1Record::Outcome(outcome) => Ok(Position {
            session: outcome.session,
            first: outcome.entry.sequence,
            last: outcome.entry.sequence,
            digest: outcome.entry.digest.clone(),
        }),
        P1Record::Span(span) => {
            if span.first_seq > span.last_seq {
                return Err(Error::invalid(format!(
                    "span {}..={} runs backwards",
                    span.first_seq, span.last_seq
                )));
            }
            Ok(Position {
                session: span.session,
                first: span.first_seq,
                last: span.last_seq,
                digest: span.tail_digest.clone(),
            })
        }
    }
}

/// Whether an outcome's entry is the one that follows `previous`.
///
/// A span is custody of a run, not proof of it (ADR 0043): its tail digest
/// cannot be recomputed without the entries it covers, so only an outcome's
/// digest is checked here. The next outcome after a span is checked against
/// the span's tail, which is where a forged span is caught.
fn chains_onto(outcome: &OutcomeRecord, previous: &str) -> std::result::Result<(), String> {
    if outcome.journal_sequence != outcome.entry.sequence
        || outcome.journal_digest != outcome.entry.digest
    {
        return Err(format!(
            "outcome cites entry {} ({}) but carries entry {} ({})",
            outcome.journal_sequence,
            outcome.journal_digest,
            outcome.entry.sequence,
            outcome.entry.digest
        ));
    }
    match outcome.entry.expected_digest(previous) {
        Ok(expected) if expected == outcome.entry.digest => Ok(()),
        Ok(expected) => Err(format!(
            "sequence {} carries digest {} but chains onto the tail as {expected}",
            outcome.entry.sequence, outcome.entry.digest
        )),
        Err(e) => Err(format!(
            "sequence {} has no digest this build can verify: {}",
            outcome.entry.sequence,
            e.message()
        )),
    }
}

/// The partition's cursor after this delivery has been consumed.
///
/// `next` never moves backwards here — a stray redelivery of an old offset
/// must not rewind a restart — and only a filled gap or a release lowers it,
/// explicitly.
fn advance(cursor: &Cursor, delivery: &Delivery) -> Result<Cursor> {
    let after = delivery.offset.checked_add(1).ok_or_else(|| {
        Error::numeric(format!(
            "offset {} is the last representable; nothing can follow it",
            delivery.offset
        ))
    })?;
    Ok(Cursor {
        next: cursor.next.max(after),
        last: Some((delivery.offset, delivery.record_hash.clone())),
    })
}

/// Hex, so a cell, partition, account or unit containing `/` cannot make two
/// keys collide or a prefix scan read another cell's rows.
fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn balance_key(account: &Account, unit: &str) -> String {
    format!(
        "{PREFIX_BALANCE}{}/{}",
        hex(&account.to_string()),
        hex(unit)
    )
}

fn chain_key(cell: &str, session: u64, first: u64) -> String {
    format!("{PREFIX_CHAIN}{}/{session:020}/{first:020}", hex(cell))
}

fn tail_key(cell: &str) -> String {
    format!("{PREFIX_TAIL}{}", hex(cell))
}

fn park_key(cell: &str) -> String {
    format!("{PREFIX_PARK}{}", hex(cell))
}

fn cursor_key(partition: &str) -> String {
    format!("{PREFIX_CURSOR}{}", hex(partition))
}
