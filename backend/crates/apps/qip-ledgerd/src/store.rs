//! The ledger's durable state: balances, per-cell chain tails and
//! per-partition consumer offsets, moved together or not at all (ADR 0100 §4).
//!
//! # One record, one batch
//!
//! [`LedgerStore::apply`] turns one P1 delivery into exactly one
//! [`LedgerBatch`] handed to [`LedgerBackend::commit`] — on the local store
//! one write-ahead-log record, `fsync`ed; on Spanner one read-write
//! transaction (ADR 0113). Whatever that delivery changes —
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
//!
//! # Balances are a fold of the history, and the store proves it at open
//!
//! A balance row is written in exactly one place — the batch that books the
//! event moving it — and is checked against the history every time the
//! ledger opens ([`LedgerStore::with_backend`]): every event the ledger holds,
//! fill or operation, is re-folded from nothing, and the result must equal
//! the stored rows exactly, key for key. A row with no history behind it, a
//! row whose amount the history does not produce, a row whose key does not
//! parse back to the account it holds, or a history with no row, stops the
//! ledger opening.
//!
//! The failure this replaces: `import_state`, a public method that wrote
//! balance rows straight from an export with no event behind them, "replacing
//! the current state entirely", and that silently skipped every key with more
//! than one `:` — which was every key a trading account renders to. It is
//! removed rather than repaired. A ledger is restored by replaying the stream
//! it was built from, through [`LedgerStore::apply`], and nothing else.
//!
//! # Operations
//!
//! Treasury, custody, inventory, collateral, margin, transfers, tax lots,
//! commitments, tax and mandate ownership arrive as a
//! [`LedgerOperation`] and are booked by [`LedgerStore::record_operation`]:
//! the operation's own event, its balance rows and its deduplication record
//! in one batch, keyed by the operation id, so a redelivered operation moves
//! nothing (R12-LEDGER-8024). An event that would take a quantity-holding
//! account below zero — releasing more than was reserved, closing more of a
//! lot than was opened — is refused and moves nothing.

use crate::backend::{LedgerBackend, LedgerBatch, LocalBackend};
use crate::telemetry::LedgerTelemetry;
use qip_capital::LedgerObligations;
use qip_contracts::ledger::{Account, Direction, LedgerEvent};
use qip_contracts::ledger_operation::{LedgerOperation, OperationKind, Phase};
use qip_contracts::reflex::{ChainSpan, Decision, OutcomeRecord};
use qip_core::error::{Error, Result};
use qip_core::hash::{constant_time_eq, hmac_sha256, to_hex};
use qip_core::{Decimal, Timestamp};
use qip_portfolio::ledger::{PaperFill, post};
use qip_storage::EngineConfig;
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
const PREFIX_WATERMARK: &str = "watermark/";
const PREFIX_OPERATION: &str = "operation/";
const PREFIX_LOT: &str = "lot/";
const KEY_PROBE: &str = "meta/probe";
const KEY_JOURNAL_NEXT: &str = "meta/journal_next";
const KEY_CURRENT_WATERMARK: &str = "meta/current_watermark";

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

/// What [`LedgerStore::record_operation`] did with one operation. Both were
/// committed durably before they were returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationDisposition {
    /// The operation was booked as one balanced event of this many postings.
    Posted { postings: usize },
    /// The ledger already holds this operation id with the same digest;
    /// nothing moved.
    Duplicate,
}

/// A ledger operation as the broker delivered it (ADR 0123): where, what it
/// hashed to, and the operation.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationDelivery {
    pub partition: String,
    pub offset: u64,
    pub record_hash: String,
    pub operation: LedgerOperation,
}

/// What [`LedgerStore::apply_operation`] did. Every arm moved the cursor.
#[derive(Clone, Debug, PartialEq)]
pub enum OperationApplied {
    /// Booked as one balanced event of this many postings.
    Posted { postings: usize },
    /// The ledger already holds this operation id with the same digest.
    Duplicate,
    /// Refused and noted; nothing was booked.
    Refused { detail: String },
}

/// What booking an operation would write.
enum PlannedOperation {
    Duplicate,
    Post { batch: LedgerBatch, postings: usize },
}

/// Why the ledger would not book an operation.
enum OperationRefusal {
    /// Another digest under an id the ledger holds.
    Conflict(Error),
    /// Anything else the ledger refuses: an unbalanced event, an overdraw,
    /// a lot opened twice.
    Refused(Error),
}

impl OperationRefusal {
    fn outcome(&self) -> &'static str {
        match self {
            Self::Conflict(_) => "conflict",
            Self::Refused(_) => "refused",
        }
    }

    fn into_error(self) -> Error {
        match self {
            Self::Conflict(error) | Self::Refused(error) => error,
        }
    }
}

/// One tax lot as the ledger holds it: what is left of it, and since when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaxLotView {
    pub cell: String,
    pub strategy: String,
    pub lot: String,
    pub unit: String,
    /// The lot's open quantity: the fold of its postings in `unit`.
    pub quantity: Decimal,
    pub cost_unit: String,
    /// The lot's remaining cost basis: the fold of its postings in
    /// `cost_unit`.
    pub cost: Decimal,
    /// When the lot was acquired, from the operation that opened it; the
    /// holding period runs from here.
    pub acquired_at: Timestamp,
}

/// Ledger state for export (backups, reconciliation). Read-only: there is
/// no import. See the module documentation for why.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerStateExport {
    /// All balances as "account:unit" -> decimal string.
    pub balances: BTreeMap<String, String>,
    /// Count of chain records applied (for verification).
    pub chain_records_count: u64,
    /// The current watermark if one exists.
    pub current_watermark: Option<LedgerWatermark>,
}

/// A signed watermark on the ledger state, chaining to the previous one.
///
/// Each watermark periodically signs (previous_digest, sequence, timestamp),
/// forming an unbreakable chain. The signature is HMAC-SHA256 keyed on a
/// stable secret known only to this ledger instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerWatermark {
    /// The UNIX timestamp in seconds when this watermark was created.
    pub timestamp: u64,
    /// The ledger state digest from all applied records up to this point.
    pub state_digest: String,
    /// HMAC-SHA256(signing_key, timestamp || state_digest || previous_digest)
    /// rendered as lowercase hex.
    pub signature: String,
    /// The digest of the previous watermark, for chaining. Empty on the first.
    pub previous_digest: String,
}

impl LedgerWatermark {
    /// Create a watermark, signing it with the given key.
    ///
    /// The key must be stable across restarts; it proves the ledger instance
    /// and anchors the chain.
    pub fn new(
        timestamp: u64,
        state_digest: String,
        previous_digest: String,
        signing_key: &[u8],
    ) -> Self {
        let message = format!("{}|{}|{}", timestamp, state_digest, previous_digest);
        let sig_bytes = hmac_sha256(signing_key, message.as_bytes());
        let signature = to_hex(&sig_bytes);

        Self {
            timestamp,
            state_digest,
            signature,
            previous_digest,
        }
    }

    /// Verify this watermark's signature against the given key.
    ///
    /// Returns true if the signature is valid, false otherwise.
    pub fn verify(&self, signing_key: &[u8]) -> bool {
        let message = format!(
            "{}|{}|{}",
            self.timestamp, self.state_digest, self.previous_digest
        );
        let expected_sig_bytes = hmac_sha256(signing_key, message.as_bytes());
        let expected_sig = to_hex(&expected_sig_bytes);
        constant_time_eq(self.signature.as_bytes(), expected_sig.as_bytes())
    }
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
pub struct ChainRecord {
    pub last_seq: u64,
    pub digest: String,
    pub offset: u64,
    pub record_hash: String,
    pub event: Option<LedgerEvent>,
}

/// One operation the ledger booked, keyed by its operation id. The event is
/// stored beside the operation so the history is re-foldable without
/// re-deriving it, and the operation beside the event so a lot's
/// acquisition time and a duplicate's digest are both answerable from the
/// ledger alone.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct OperationRecord {
    digest: String,
    operation: LedgerOperation,
    event: LedgerEvent,
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

/// The ledger's single writer over one [`LedgerBackend`].
#[derive(Debug)]
pub struct LedgerStore {
    store: Box<dyn LedgerBackend>,
    telemetry: LedgerTelemetry,
    /// Serialises read-decide-commit. The engine serialises commits, but the
    /// decision reads the tail, the park and the cursor first, and two
    /// deliveries deciding on the same tail would both apply.
    writer: Mutex<()>,
    /// When this process first saw each cell parked, so the age of the
    /// longest park is a measurement and not a guess. In memory and
    /// monotonic: a park inherited from before a restart is aged from the
    /// restart, which understates it, and that understatement is stated on
    /// [`Self::record_mismatch_age`] rather than hidden.
    parked_since: Mutex<BTreeMap<String, std::time::Instant>>,
}

impl LedgerStore {
    /// Open the ledger on the local store at `directory`, recovering
    /// whatever the last process committed. Tests and single-node
    /// development only; see [`crate::backend`].
    pub fn open(
        directory: impl AsRef<Path>,
        config: EngineConfig,
        telemetry: LedgerTelemetry,
    ) -> Result<Self> {
        Self::with_backend(Box::new(LocalBackend::open(directory, config)?), telemetry)
    }

    /// Open the ledger on `backend`, refusing to open one whose balances are
    /// not the fold of its own history (see the module documentation).
    pub fn with_backend(
        backend: Box<dyn LedgerBackend>,
        telemetry: LedgerTelemetry,
    ) -> Result<Self> {
        let ledger = Self {
            store: backend,
            telemetry,
            writer: Mutex::new(()),
            parked_since: Mutex::new(BTreeMap::new()),
        };
        ledger.verify_history()?;
        // A restarted process reports the parks it inherited, not zero.
        ledger.record_parked_keys()?;
        Ok(ledger)
    }

    /// `local` or `spanner`.
    pub fn backend_kind(&self) -> &'static str {
        self.store.kind()
    }

    /// Prove the store takes a write and gives it back: the readiness probe
    /// for a backend with no directory to probe. The row is the ledger's own
    /// metadata, never a balance.
    pub fn prove_writable(&self, token: &str) -> Result<()> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        self.store
            .commit(LedgerBatch::new().put_as(KEY_PROBE, &token)?)?;
        let read: Option<String> = self.read(KEY_PROBE)?;
        if read.as_deref() != Some(token) {
            return Err(Error::io(format!(
                "the {} ledger store did not return the probe it was just given; refusing to \
                 report a ledger ready that could book nothing",
                self.store.kind()
            )));
        }
        Ok(())
    }

    /// Re-fold every event the ledger holds and compare it, key for key,
    /// with the stored balance rows. Called by [`Self::with_backend`].
    ///
    /// Refuses — never skips — a balance row whose key does not decode to
    /// the account and unit the row holds, or whose account text does not
    /// parse; a row the history does not produce; an amount the history
    /// does not produce; and a history entry with no row.
    pub fn verify_history(&self) -> Result<()> {
        let refolded = self.refold()?;
        let mut stored: BTreeMap<(Account, String), Decimal> = BTreeMap::new();
        for (key, value) in self.store.scan_prefix(PREFIX_BALANCE)? {
            let row: BalanceRow = decode(value)?;
            let (account, unit) = parse_balance_key(&key)?;
            if account != row.account || unit != row.unit {
                return Err(Error::invalid(format!(
                    "balance row {key} is keyed as {account} in {unit} but holds {} in {}; the \
                     ledger will not open on a row filed under another account",
                    row.account, row.unit
                )));
            }
            stored.insert((account, unit), row.amount);
        }
        for ((account, unit), amount) in &stored {
            match refolded.get(&(account.clone(), unit.clone())) {
                Some(expected) if expected == amount => {}
                Some(expected) => {
                    return Err(Error::invalid(format!(
                        "the stored {unit} balance of {account} is {amount} and its history \
                         folds to {expected}; a balance written outside the event history is \
                         refused at open, and the ledger is restored by replaying its stream"
                    )));
                }
                None => {
                    return Err(Error::invalid(format!(
                        "the ledger holds a {unit} balance of {amount} for {account} and no \
                         event that posts to it; a balance written outside the event history \
                         is refused at open, and the ledger is restored by replaying its stream"
                    )));
                }
            }
        }
        if let Some(((account, unit), amount)) =
            refolded.iter().find(|(key, _)| !stored.contains_key(*key))
        {
            return Err(Error::invalid(format!(
                "the history folds {account} to {amount} {unit} and the ledger holds no balance \
                 row for it; a lost balance row is refused at open rather than read as zero"
            )));
        }
        Ok(())
    }

    /// Every balance as the history folds it, from nothing.
    pub fn refold(&self) -> Result<BTreeMap<(Account, String), Decimal>> {
        let mut balances: BTreeMap<(Account, String), Decimal> = BTreeMap::new();
        for event in self.events()? {
            for posting in event.postings() {
                let entry = balances
                    .entry((posting.account.clone(), posting.unit.clone()))
                    .or_insert(Decimal::ZERO);
                let moved = match posting.direction {
                    Direction::Debit => entry.checked_add(posting.amount),
                    Direction::Credit => entry.checked_sub(posting.amount),
                };
                *entry = moved.ok_or_else(|| {
                    Error::numeric(format!(
                        "refolding the {} balance of {} overflows",
                        posting.unit, posting.account
                    ))
                })?;
            }
        }
        Ok(balances)
    }

    /// The sum of every balance, per unit. Zero in every unit for a
    /// double-entry ledger whatever subledgers its events touched; a unit
    /// that does not close names a defect, not rounding.
    pub fn trial_balance(&self) -> Result<BTreeMap<String, Decimal>> {
        let mut totals: BTreeMap<String, Decimal> = BTreeMap::new();
        for ((_, unit), amount) in self.balances()? {
            let entry = totals.entry(unit.clone()).or_insert(Decimal::ZERO);
            *entry = entry
                .checked_add(amount)
                .ok_or_else(|| Error::numeric(format!("the trial balance in {unit} overflows")))?;
        }
        Ok(totals)
    }

    /// Book one operation: its event, the balance rows it moves and its
    /// deduplication record, in one batch.
    ///
    /// The same operation id with the same digest is a duplicate and moves
    /// nothing. The same id with another digest is refused — the ledger will
    /// not choose between two things one id was said to mean. An event that
    /// would overflow a balance or take a quantity-holding account below
    /// zero is refused. A refusal moves nothing.
    pub fn record_operation(&self, operation: &LedgerOperation) -> Result<OperationDisposition> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        match self.plan_operation(operation)? {
            Ok(PlannedOperation::Duplicate) => {
                self.telemetry.commit("duplicate");
                Ok(OperationDisposition::Duplicate)
            }
            Ok(PlannedOperation::Post { batch, postings }) => {
                self.store.commit(batch)?;
                self.telemetry.commit("success");
                Ok(OperationDisposition::Posted { postings })
            }
            Err(refusal) => {
                self.telemetry.commit(refusal.outcome());
                Err(refusal.into_error())
            }
        }
    }

    /// Book one operation the broker delivered (ADR 0123), and move the
    /// partition's cursor past it in the same commit, so the offset is never
    /// durable before the postings it stands for.
    ///
    /// A duplicate moves only the cursor. A refused operation — another
    /// digest under a held id, an overdraw, an unbalanced event — moves the
    /// cursor and a journal note naming the operation and the refusal, and
    /// nothing else: an operation is on no cell's chain, so there is no cell
    /// to park, and stalling the partition would hold every fill behind it.
    /// `Err` is the store failing and means nothing moved; the same delivery
    /// may be offered again.
    pub fn apply_operation(&self, delivery: &OperationDelivery) -> Result<OperationApplied> {
        refuse_blank("partition", &delivery.partition)?;
        refuse_blank("record hash", &delivery.record_hash)?;
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let cursor = self.cursor(&delivery.partition)?;
        let advanced = advance_to(&cursor, delivery.offset, &delivery.record_hash)?;
        let cursor_row = cursor_key(&delivery.partition);
        if let Some((offset, hash)) = &cursor.last
            && *offset == delivery.offset
            && *hash != delivery.record_hash
        {
            let detail = format!(
                "offset {} was consumed with record hash {hash} and has arrived again with {}; \
                 the broker has said two things about one position",
                delivery.offset, delivery.record_hash
            );
            return self.refuse_operation(delivery, &cursor_row, &advanced, detail);
        }
        match self.plan_operation(&delivery.operation)? {
            Ok(PlannedOperation::Duplicate) => {
                self.store
                    .commit(LedgerBatch::new().put_as(cursor_row, &advanced)?)?;
                self.telemetry.commit("duplicate");
                Ok(OperationApplied::Duplicate)
            }
            Ok(PlannedOperation::Post { batch, postings }) => {
                self.store.commit(batch.put_as(cursor_row, &advanced)?)?;
                self.telemetry.commit("success");
                Ok(OperationApplied::Posted { postings })
            }
            Err(refusal) => {
                let detail = refusal.into_error().message().to_string();
                self.refuse_operation(delivery, &cursor_row, &advanced, detail)
            }
        }
    }

    fn refuse_operation(
        &self,
        delivery: &OperationDelivery,
        cursor_row: &str,
        advanced: &Cursor,
        detail: String,
    ) -> Result<OperationApplied> {
        let batch = LedgerBatch::new().put_as(cursor_row.to_string(), advanced)?;
        let batch = self.note(
            batch,
            JournalNote {
                cell: delivery.operation.origin.clone(),
                partition: delivery.partition.clone(),
                offset: delivery.offset,
                event: "operation_refused".to_string(),
                detail: format!("operation {}: {detail}", delivery.operation.operation_id),
                operator_action: format!(
                    "nothing was booked for operation {}; correct it at its origin and send it \
                     under a new id",
                    delivery.operation.operation_id
                ),
            },
        )?;
        self.store.commit(batch)?;
        self.telemetry.commit("refused");
        Ok(OperationApplied::Refused { detail })
    }

    /// What booking `operation` would write, without writing it. The outer
    /// `Err` is the store failing; the inner one is the ledger refusing the
    /// operation, which moves nothing.
    fn plan_operation(
        &self,
        operation: &LedgerOperation,
    ) -> Result<std::result::Result<PlannedOperation, OperationRefusal>> {
        let digest = match operation.digest() {
            Ok(digest) => digest,
            Err(error) => return Ok(Err(OperationRefusal::Refused(error))),
        };
        let event = match operation.to_event() {
            Ok(event) => event,
            Err(error) => return Ok(Err(OperationRefusal::Refused(error))),
        };
        let key = operation_key(&operation.operation_id);
        if let Some(held) = self.read::<OperationRecord>(&key)? {
            if held.digest == digest {
                self.telemetry.duplicate("operation");
                return Ok(Ok(PlannedOperation::Duplicate));
            }
            return Ok(Err(OperationRefusal::Conflict(Error::invalid(format!(
                "operation {} is held with digest {} and has arrived again as {digest}; two \
                 different operations claim one id, and the ledger books neither again",
                operation.operation_id, held.digest
            )))));
        }

        // A lot is opened once. Opening one id twice would fold two
        // acquisitions into one balance with one acquisition time, and the
        // holding period of the second would be the first's.
        let opened_lot = match &operation.kind {
            OperationKind::TaxLot {
                book,
                lot,
                phase: Phase::Establish,
                ..
            } => {
                let lot_key = lot_key(&book.cell, &book.strategy, lot);
                if self.store.get(&lot_key)?.is_some() {
                    return Ok(Err(OperationRefusal::Refused(Error::invalid(format!(
                        "tax lot {lot} of {}/{} is already open; a lot id is opened once, and \
                         a second acquisition is a new lot",
                        book.cell, book.strategy
                    )))));
                }
                Some(lot_key)
            }
            _ => None,
        };

        let rows = match self.fold(&event) {
            Ok(rows) => rows,
            Err(refusal) => return Ok(Err(OperationRefusal::Refused(refusal))),
        };
        let mut batch = LedgerBatch::new();
        for (row_key, row) in rows {
            batch = batch.put_as(row_key, &row)?;
        }
        if let Some(lot_key) = opened_lot {
            batch = batch.put_as(lot_key, &operation.operation_id)?;
        }
        batch = batch.put_as(
            key,
            &OperationRecord {
                digest,
                operation: operation.clone(),
                event: event.clone(),
            },
        )?;
        Ok(Ok(PlannedOperation::Post {
            batch,
            postings: event.postings().len(),
        }))
    }

    /// Every tax lot the ledger has opened, with what is left of it. A lot
    /// is read from the balances its postings left — never from the
    /// operation's stated quantity — and its acquisition time from the
    /// operation that opened it.
    pub fn tax_lots(&self) -> Result<Vec<TaxLotView>> {
        let balances = self.balances()?;
        let mut lots: BTreeMap<(String, String, String), TaxLotView> = BTreeMap::new();
        for (_, value) in self.store.scan_prefix(PREFIX_OPERATION)? {
            let record: OperationRecord = decode(value)?;
            if let OperationKind::TaxLot {
                book,
                lot,
                unit,
                cost_unit,
                acquired_at,
                phase: Phase::Establish,
                ..
            } = &record.operation.kind
            {
                let account = Account::TaxLot {
                    cell: book.cell.clone(),
                    strategy: book.strategy.clone(),
                    lot: lot.clone(),
                };
                let held = |u: &str| {
                    balances
                        .get(&(account.clone(), u.to_string()))
                        .copied()
                        .unwrap_or(Decimal::ZERO)
                };
                lots.entry((book.cell.clone(), book.strategy.clone(), lot.clone()))
                    .or_insert_with(|| TaxLotView {
                        cell: book.cell.clone(),
                        strategy: book.strategy.clone(),
                        lot: lot.clone(),
                        unit: unit.clone(),
                        quantity: held(unit),
                        cost_unit: cost_unit.clone(),
                        cost: held(cost_unit),
                        acquired_at: *acquired_at,
                    });
            }
        }
        Ok(lots.into_values().collect())
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

        let mut batch = LedgerBatch::new();
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

    /// Every event booked: fills in chain order per cell and session, then
    /// operations in operation-id order.
    /// The fill events alone, in chain order: what the chains booked,
    /// without the operations (ADR 0123) [`Self::events`] also returns.
    pub fn fill_events(&self) -> Result<Vec<LedgerEvent>> {
        let mut events = Vec::new();
        for (_, value) in self.store.scan_prefix(PREFIX_CHAIN)? {
            let record: ChainRecord = decode(value)?;
            events.extend(record.event);
        }
        Ok(events)
    }

    pub fn events(&self) -> Result<Vec<LedgerEvent>> {
        let mut events = Vec::new();
        for (_, value) in self.store.scan_prefix(PREFIX_CHAIN)? {
            let record: ChainRecord = decode(value)?;
            events.extend(record.event);
        }
        for (_, value) in self.store.scan_prefix(PREFIX_OPERATION)? {
            let record: OperationRecord = decode(value)?;
            events.push(record.event);
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
        let mut batch = LedgerBatch::new()
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
                    let batch = LedgerBatch::new()
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
        let batch = LedgerBatch::new()
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
        let mut batch = LedgerBatch::new()
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

    /// Query the balance of a specific account in a unit, or Decimal::ZERO if not found.
    pub fn balance(&self, account: &Account, unit: &str) -> Result<Decimal> {
        let key = balance_key(account, unit);
        Ok(self
            .read::<BalanceRow>(&key)?
            .map_or(Decimal::ZERO, |row| row.amount))
    }

    /// Margin booked, collateral pledged and cash owed and not yet settled,
    /// folded from the stored balances in `unit` as of `as_of` — the reading
    /// the survival waterfall withholds from (R12-CAPITAL-4087,
    /// `qip_kernel::Platform::observe_ledger_obligations`).
    ///
    /// Read from [`Self::balances`], which the store proved equal to the
    /// refold of its history when it opened, so the figures are a fold of
    /// booked postings and not a second account kept beside them.
    pub fn obligations(&self, unit: &str, as_of: Timestamp) -> Result<LedgerObligations> {
        LedgerObligations::from_balances(&self.balances()?, unit, as_of)
    }

    /// Query all chain records (postings).
    pub fn chain_records(&self) -> Result<Vec<ChainRecord>> {
        let mut records = Vec::new();
        for (_, value) in self.store.scan_prefix(PREFIX_CHAIN)? {
            let record: ChainRecord = decode(value)?;
            records.push(record);
        }
        Ok(records)
    }

    /// Get the current watermark, if one has been recorded.
    pub fn current_watermark(&self) -> Result<Option<LedgerWatermark>> {
        self.read(KEY_CURRENT_WATERMARK)
    }

    /// Record a new watermark.
    ///
    /// This should be called periodically to create verifiable checkpoints
    /// of the ledger state. The watermark is signed with the ledger's
    /// stable signing key.
    pub fn record_watermark(
        &self,
        timestamp: u64,
        state_digest: String,
        signing_key: &[u8],
    ) -> Result<()> {
        let previous_digest = self
            .current_watermark()?
            .map(|w| w.signature.clone())
            .unwrap_or_default();

        let watermark = LedgerWatermark::new(timestamp, state_digest, previous_digest, signing_key);

        let batch = LedgerBatch::new()
            .put_as(KEY_CURRENT_WATERMARK, &watermark)?
            .put_as(format!("{PREFIX_WATERMARK}{:020}", timestamp), &watermark)?;

        self.store.commit(batch)?;
        Ok(())
    }

    /// Export the ledger's state as a JSON-serializable structure.
    pub fn export_state(&self) -> Result<LedgerStateExport> {
        let balances = self.balances()?;
        let chain_records = self.chain_records()?;
        let watermark = self.current_watermark()?;

        Ok(LedgerStateExport {
            balances: balances
                .into_iter()
                .map(|((account, unit), amount)| {
                    (format!("{}:{}", account, unit), amount.to_string())
                })
                .collect(),
            chain_records_count: chain_records.len() as u64,
            current_watermark: watermark,
        })
    }

    /// The balance rows an event moves, each checked, or a refusal: a
    /// balance that would overflow, or a quantity-holding account
    /// ([`Account::holds_a_quantity`]) that would go below zero.
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
                    "the {} balance of {} would overflow; the event is refused rather than \
                     booked against a wrapped balance",
                    posting.unit, posting.account
                )));
            };
            if amount.is_negative() && posting.account.holds_a_quantity() {
                return Err(Error::invalid(format!(
                    "{} holds {current} {} and this event would take it to {amount}; more \
                     cannot be released, withdrawn or closed than is held, and the event is \
                     refused rather than booked as a negative holding",
                    posting.account, posting.unit
                )));
            }
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

    fn note(&self, batch: LedgerBatch, note: JournalNote) -> Result<LedgerBatch> {
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
        let parked = self.parked()?;
        self.telemetry.parked_keys(parked.len() as u64);
        let now = std::time::Instant::now();
        let mut since = self.parked_since.lock().unwrap_or_else(|e| e.into_inner());
        since.retain(|cell, _| parked.contains_key(cell));
        for cell in parked.keys() {
            since.entry(cell.clone()).or_insert(now);
        }
        drop(since);
        self.record_mismatch_age();
        Ok(())
    }

    /// Record how long the longest-parked cell has been parked, zero when
    /// none is: the ledger's reconciliation mismatch age. Called whenever the
    /// park set changes and on every consumer step, because the age grows
    /// while nothing else happens.
    ///
    /// The clock starts when this process first saw the park, so a park
    /// inherited from before a restart reads younger than it is. The
    /// understatement runs towards "reconciled sooner than it was", which is
    /// the wrong direction for an alarm, and is why the persisted park does
    /// not yet carry its own time: that is a change to a stored row.
    pub fn record_mismatch_age(&self) {
        let since = self.parked_since.lock().unwrap_or_else(|e| e.into_inner());
        let age = since
            .values()
            .min()
            .map_or(0.0, |oldest| oldest.elapsed().as_secs_f64());
        drop(since);
        self.telemetry.mismatch_age_seconds(age);
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
    advance_to(cursor, delivery.offset, &delivery.record_hash)
}

fn advance_to(cursor: &Cursor, offset: u64, record_hash: &str) -> Result<Cursor> {
    let after = offset.checked_add(1).ok_or_else(|| {
        Error::numeric(format!(
            "offset {offset} is the last representable; nothing can follow it"
        ))
    })?;
    Ok(Cursor {
        next: cursor.next.max(after),
        last: Some((offset, record_hash.to_string())),
    })
}

/// Hex, so a cell, partition, account or unit containing `/` cannot make two
/// keys collide or a prefix scan read another cell's rows.
fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>()?;
    // Only the form `hex` writes: lowercase. An uppercase spelling decodes
    // to the same bytes and is a different key, which is a row filed twice.
    if hex(std::str::from_utf8(&bytes).ok()?) != text {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn balance_key(account: &Account, unit: &str) -> String {
    format!(
        "{PREFIX_BALANCE}{}/{}",
        hex(&account.to_string()),
        hex(unit)
    )
}

/// The account and unit a balance key names, or a refusal naming the key.
/// Never a skip: a key this cannot read is a row the ledger cannot account
/// for, and [`LedgerStore::verify_history`] stops the ledger on it.
pub fn parse_balance_key(key: &str) -> Result<(Account, String)> {
    let refuse = |why: &str| {
        Error::invalid(format!(
            "balance key {key:?} {why}; the ledger refuses a balance it cannot attribute rather \
             than skipping it"
        ))
    };
    let body = key
        .strip_prefix(PREFIX_BALANCE)
        .ok_or_else(|| refuse("is not under the balance prefix"))?;
    let (account_hex, unit_hex) = body
        .split_once('/')
        .ok_or_else(|| refuse("has no account/unit separator"))?;
    let account_text =
        unhex(account_hex).ok_or_else(|| refuse("has an account that is not hex"))?;
    let unit = unhex(unit_hex).ok_or_else(|| refuse("has a unit that is not hex"))?;
    if unit.is_empty() {
        return Err(refuse("names no unit"));
    }
    let account = account_text.parse::<Account>().map_err(|e| {
        refuse(&format!(
            "names no account this ledger keeps ({})",
            e.message()
        ))
    })?;
    Ok((account, unit))
}

fn operation_key(operation_id: &str) -> String {
    format!("{PREFIX_OPERATION}{}", hex(operation_id))
}

fn lot_key(cell: &str, strategy: &str, lot: &str) -> String {
    format!("{PREFIX_LOT}{}/{}/{}", hex(cell), hex(strategy), hex(lot))
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
