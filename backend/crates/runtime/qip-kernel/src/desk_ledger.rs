//! The desk's fills, put on the one ledger there is.
//!
//! Until this existed a desk [`Fill`](qip_execution_engine::order::Fill)
//! reached `Platform::capital` and the outcome capture and nothing else:
//! `qip-ledgerd` consumes exactly one shape, the P1 [`OutcomeRecord`] a cell
//! spools, so the double-entry ledger booked every edge-cell fill and none of
//! the desk's. The desk does not get a ledger of its own — a second one would
//! be a second source of truth for what the first already holds — it emits
//! the same `OutcomeRecord`, on a chain of its own (`cell = "desk"`), and
//! `qip-ledgerd` stays the sole writer.
//!
//! # What makes a record trustworthy
//!
//! * **Identity is a function of the fill.** A record's chain position is its
//!   ordinal among the desk's booked fills and its digest chains onto the
//!   previous one under v2, exactly as `qip-edge` seals a cell's journal. The
//!   chain is therefore a pure fold of the `FillBooked` records already in
//!   the event log, so a restart rebuilds the same positions and a replay
//!   yields the same identity ([`DeskLedgerFeed::fold_booked`]).
//! * **Recording is idempotent on `(order_id, fill_id)`.** The OMS already
//!   refuses a replayed order id and an order refuses a repeated fill id; this
//!   is the third lock, for a fill batch offered twice by a caller that is
//!   neither. A duplicate adds no record and, in the platform, books nothing.
//! * **What the ledger holds is a journalled watermark.** A record leaves the
//!   queue only when the fabric has acknowledged it
//!   ([`DeskLedgerFeed::acknowledge`], through
//!   `Platform::acknowledge_desk_outcomes`), and that acknowledgement is
//!   written to the same hash-chained event log the fills are in. A restart
//!   folds the fills at or below the highest watermark
//!   ([`DeskLedgerFeed::fold_booked`]) and re-queues every fill after it
//!   ([`DeskLedgerFeed::requeue_booked`]) with the sequence and digest it had
//!   before the crash. A crash before the acknowledgement was logged resends
//!   a batch the ledger already holds, which it absorbs as a duplicate; a
//!   crash before shipping resends everything; neither leaves a hole.
//! * **A record exists only if the ledger would take it.** [`outcome_for`]
//!   runs [`PaperFill::try_from`] and [`post`] — the very functions
//!   `LedgerStore::apply` runs — before it returns a record, so a fill the
//!   ledger would park its whole cell for (a live fill, a notional off the
//!   nine-digit grid, a negative fee) is refused here, where the desk can say
//!   no, instead of in the ledger's partition where it stops every later
//!   record behind it.

use crate::platform::FillBooked;
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{ChainVersion, Decision, JournalEntry, OutcomeRecord, seal_v2};
use qip_core::error::{Error, Result};
use qip_execution_engine::order::Side;
use qip_portfolio::ledger::{PaperFill, post};
use std::collections::BTreeSet;

/// The chain the fast brain's desk fills sit on, and the default for any
/// platform that names none. A cell is named for where it runs; a desk is a
/// central process's own book, and no cell carries a desk's name. Every other
/// process that books fills names its own chain
/// ([`crate::config::PlatformConfig::desk_chain`]): two desks on one chain
/// fork the ledger.
pub const DESK_CELL: &str = "desk";

/// Refuse a chain name the ledger could not hold as an account name, or one
/// a cell could also carry.
pub fn validate_chain(chain: &str) -> Result<()> {
    if chain.is_empty()
        || !chain
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Error::invalid(format!(
            "desk chain `{chain}` must be non-empty lowercase ASCII letters, digits and `-`: it \
             becomes a ledger account name and a fabric producer identity"
        )));
    }
    if !chain.starts_with(DESK_CELL) {
        return Err(Error::invalid(format!(
            "desk chain `{chain}` must start with `{DESK_CELL}`, so no regional cell's name can \
             collide with it"
        )));
    }
    Ok(())
}

/// The digest a chain's first entry chains onto: the same text as
/// `qip_edge::Journal::GENESIS` and `qip_ledgerd::store::GENESIS`, restated
/// because neither crate may be depended on from here.
const GENESIS: &str = "genesis";

/// One session, for ever. The chain's position is rebuilt from the event log
/// on every start, so a restart continues the chain rather than beginning a
/// second one, and a session counter that has to be remembered across
/// restarts is a thing that can be forgotten. What keeps chain correlation ids
/// from repeating across restarts is therefore the order id, not the session:
/// `resume_book` continues the order counter past the log's highest.
pub const DESK_SESSION: u64 = 0;

/// The unit the desk's prices are quoted in. The same currency the capital
/// fabric's demand observation is filed under.
pub const DESK_QUOTE_UNIT: &str = "USD";

/// Fills the feed will remember for duplicate detection. Past this the desk
/// refuses to book a new fill, loudly, rather than forgetting old ones and
/// letting a replay through: archive the event log and restart.
pub const DESK_FEED_CAPACITY: usize = 100_000;

/// Records waiting to be taken by the ledger's consumer. Past this a fill is
/// refused, for the reason above — dropping the oldest would be the desk
/// choosing which fill the ledger never hears about.
pub const DESK_OUTBOX_CAPACITY: usize = 10_000;

/// A record and the cycle it came out of.
#[derive(Clone, Debug, PartialEq)]
pub struct DeskOutcome {
    pub record: OutcomeRecord,
    /// The correlation id the order's decisions were captured under, so the
    /// ledger posting can be walked back to the cycle that caused it. The
    /// record itself has no such field — a cell's journal carries none —
    /// and the broker envelope that ships it takes this as its lineage.
    pub correlation_id: String,
}

impl DeskOutcome {
    /// The correlation id the spool envelope must carry for `qip-ledgerd`'s
    /// consumer to accept this record: the chain id its body derives. The
    /// cycle's own id is [`Self::correlation_id`], kept for joining the
    /// posting back to the decision that placed the order.
    pub fn chain_correlation(&self) -> Option<qip_core::CorrelationId> {
        self.record.correlation()
    }
}

/// What [`DeskLedgerFeed::admit`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// A new record, at this chain position.
    Recorded { sequence: u64 },
    /// This `(order_id, fill_id)` is already on the chain; nothing was added.
    Duplicate,
    /// The fill is valid and the chain advanced over it, but there was no room
    /// to queue or remember its record, so it is not in [`DeskLedgerFeed::pending`].
    ///
    /// Room is a delivery limit, not a verdict on the fill: the venue holds the
    /// position whether or not anything has drained this queue, so the caller
    /// books it and surfaces the gap. Nothing is lost, because the chain is a
    /// fold of the `FillBooked` log and [`outcomes_from`] re-derives this
    /// record, with the same sequence and digest, from that log.
    Overflowed { sequence: u64 },
}

/// Build the record for one booked fill at `sequence`, chained onto
/// `previous`, or refuse it with the reason the ledger would.
pub fn outcome_for(
    booked: &FillBooked,
    strategy: &str,
    sequence: u64,
    previous: &str,
) -> Result<OutcomeRecord> {
    outcome_on(DESK_CELL, booked, strategy, sequence, previous)
}

/// [`outcome_for`] on the named desk chain.
pub fn outcome_on(
    chain: &str,
    booked: &FillBooked,
    strategy: &str,
    sequence: u64,
    previous: &str,
) -> Result<OutcomeRecord> {
    let decision = Decision::Filled {
        order_id: booked.order_id.clone(),
        venue: booked.venue.clone(),
        object: booked.object_id.clone(),
        quantity: booked.quantity.to_string(),
        price: booked.price.to_string(),
        simulated: booked.simulated,
        shares: vec![(strategy.to_string(), booked.quantity.to_string())],
        side: Some(match booked.side {
            Side::Buy => BookSide::Ask,
            Side::Sell => BookSide::Bid,
        }),
        quote_unit: Some(DESK_QUOTE_UNIT.to_string()),
        fee: Some(booked.costs.to_string()),
    };
    let (decision, digest) = seal_v2(previous, sequence, booked.at, decision);
    let record = OutcomeRecord {
        cell: chain.to_string(),
        session: DESK_SESSION,
        journal_sequence: sequence,
        journal_digest: digest.clone(),
        entry: JournalEntry {
            sequence,
            at: booked.at,
            decision,
            digest,
            version: ChainVersion::V2,
        },
    };
    // The ledger's own two steps, run now. `post` is pure, so a record that
    // passes here posts identically there.
    let fill = PaperFill::try_from(&record)?;
    post(&fill)?;
    Ok(record)
}

/// The desk's chain tail, its duplicate index and the records not yet taken.
#[derive(Debug)]
pub struct DeskLedgerFeed {
    /// The chain this feed's records sit on; see [`DESK_CELL`].
    chain: String,
    next_sequence: u64,
    tail_digest: Option<String>,
    seen: BTreeSet<(String, String)>,
    outbox: Vec<DeskOutcome>,
    /// The highest chain sequence a consumer has acknowledged as delivered.
    /// Held so an acknowledgement that goes backwards is a refusal rather than
    /// a silent no-op: a shipper whose bookkeeping has fallen behind the
    /// feed's is a shipper that will resend, and that should be seen.
    acknowledged_through: Option<u64>,
    /// Whether the chain has advanced over a record that was never queued: a
    /// fill that found no room. Set by the two seams that report
    /// [`Admission::Overflowed`], cleared by [`Self::fill_gap`] once every
    /// position between the acknowledged watermark and the chain's tail is
    /// queued. The platform heals it when an acknowledgement makes room.
    gap: bool,
}

impl Default for DeskLedgerFeed {
    fn default() -> Self {
        Self {
            chain: DESK_CELL.to_string(),
            next_sequence: 0,
            tail_digest: None,
            seen: BTreeSet::new(),
            outbox: Vec::new(),
            acknowledged_through: None,
            gap: false,
        }
    }
}

impl DeskLedgerFeed {
    /// A feed on the fast brain's chain, [`DESK_CELL`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A feed on a named chain, refused if the name is not one
    /// [`validate_chain`] admits.
    pub fn on_chain(chain: &str) -> Result<Self> {
        validate_chain(chain)?;
        Ok(Self {
            chain: chain.to_string(),
            ..Self::default()
        })
    }

    /// The chain this feed's records sit on.
    pub fn chain(&self) -> &str {
        &self.chain
    }

    /// The sequence the next record will carry; equal to the number recorded.
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn pending(&self) -> &[DeskOutcome] {
        &self.outbox
    }

    /// Whether the next fill would be refused for want of room.
    pub fn is_full(&self) -> bool {
        self.seen.len() >= DESK_FEED_CAPACITY || self.outbox.len() >= DESK_OUTBOX_CAPACITY
    }

    fn previous(&self) -> &str {
        self.tail_digest.as_deref().unwrap_or(GENESIS)
    }

    /// Record one booked fill, or refuse it. Nothing changes on a refusal and
    /// nothing changes on a duplicate. A fill that is valid but finds no room
    /// advances the chain and is reported as [`Admission::Overflowed`].
    pub fn admit(
        &mut self,
        booked: &FillBooked,
        strategy: &str,
        correlation: &str,
    ) -> Result<Admission> {
        let key = (booked.order_id.clone(), booked.fill_id.clone());
        if self.seen.contains(&key) {
            return Ok(Admission::Duplicate);
        }
        let record = outcome_on(
            &self.chain,
            booked,
            strategy,
            self.next_sequence,
            self.previous(),
        )?;
        let next = self.next_sequence.checked_add(1).ok_or_else(|| {
            Error::numeric("the desk's ledger chain has no sequence left to give".to_string())
        })?;
        // Past every fallible step: the writes below cannot half-happen. The
        // chain advances whether or not there is room to queue the record,
        // because it must stay equal to the fold of the event log; only the
        // queue and the duplicate index are bounded.
        let sequence = self.next_sequence;
        self.next_sequence = next;
        self.tail_digest = Some(record.journal_digest.clone());
        let room_to_remember = self.seen.len() < DESK_FEED_CAPACITY;
        let room_to_queue = self.outbox.len() < DESK_OUTBOX_CAPACITY;
        if room_to_remember {
            self.seen.insert(key);
        }
        if !(room_to_remember && room_to_queue) {
            self.gap = true;
            return Ok(Admission::Overflowed { sequence });
        }
        self.outbox.push(DeskOutcome {
            record,
            correlation_id: correlation.to_string(),
        });
        Ok(Admission::Recorded { sequence })
    }

    /// Advance the chain over a fill the event log already holds and the
    /// ledger already has (it is at or below the journalled watermark),
    /// without queueing it and without remembering its ids. The restart path;
    /// the fills after the watermark go through [`Self::requeue_booked`].
    ///
    /// The ids are deliberately not remembered. `OrderManager::next_order_id`
    /// and the simulated broker's fill ids count from one in every process, so
    /// `(ord-1, fill-1)` in the log is a different fill from `(ord-1, fill-1)`
    /// this process mints. Seeding the duplicate index from the log would
    /// read the new fill as a replay of the old and leave it out of the book.
    /// Within a process the OMS keeps ids unique, which is the scope the
    /// duplicate index is sound over.
    pub fn fold_booked(&mut self, booked: &FillBooked, strategy: &str) -> Result<u64> {
        let record = outcome_on(
            &self.chain,
            booked,
            strategy,
            self.next_sequence,
            self.previous(),
        )?;
        let next = self.next_sequence.checked_add(1).ok_or_else(|| {
            Error::numeric("the desk's ledger chain has no sequence left to give".to_string())
        })?;
        self.next_sequence = next;
        self.tail_digest = Some(record.journal_digest);
        Ok(next - 1)
    }

    /// Queue the record for a fill the event log holds but the ledger has
    /// not been told of: the restart path for everything after the
    /// acknowledged watermark. The chain advances exactly as [`Self::admit`]
    /// advances it, so the record has the sequence and digest it had before
    /// the crash; the ids are **not** remembered, for the reason
    /// [`Self::fold_booked`] gives, and a fill that finds no room advances
    /// the chain and is reported as [`Admission::Overflowed`].
    pub fn requeue_booked(
        &mut self,
        booked: &FillBooked,
        strategy: &str,
        correlation: &str,
    ) -> Result<Admission> {
        let record = outcome_on(
            &self.chain,
            booked,
            strategy,
            self.next_sequence,
            self.previous(),
        )?;
        let next = self.next_sequence.checked_add(1).ok_or_else(|| {
            Error::numeric("the desk's ledger chain has no sequence left to give".to_string())
        })?;
        let sequence = self.next_sequence;
        self.next_sequence = next;
        self.tail_digest = Some(record.journal_digest.clone());
        if self.outbox.len() >= DESK_OUTBOX_CAPACITY {
            self.gap = true;
            return Ok(Admission::Overflowed { sequence });
        }
        self.outbox.push(DeskOutcome {
            record,
            correlation_id: correlation.to_string(),
        });
        Ok(Admission::Recorded { sequence })
    }

    /// Note that the ledger holds every record through `through_sequence`,
    /// on the strength of the event log's watermark. Only a restart calls
    /// this, before it re-queues what follows.
    pub fn resume_acknowledged(&mut self, through_sequence: u64) {
        self.acknowledged_through = Some(through_sequence);
    }

    /// Whether [`Self::acknowledge`] through `through_sequence` would move the
    /// watermark (`Ok(true)`), would be a no-op repeat (`Ok(false)`), or is
    /// refused, without doing it. The platform journals the watermark between
    /// this check and the change, so the log never holds an acknowledgement
    /// the feed would have refused.
    pub fn check_acknowledge(&self, through_sequence: u64) -> Result<bool> {
        if let Some(already) = self.acknowledged_through {
            if through_sequence < already {
                return Err(Error::invalid(format!(
                    "acknowledgement through chain position {through_sequence} goes backwards: \
                     position {already} is already acknowledged"
                )));
            }
            if already == through_sequence {
                return Ok(false);
            }
        }
        let newest = self.outbox.last().map(|o| o.record.entry.sequence);
        match newest {
            Some(newest) if through_sequence <= newest => Ok(true),
            _ => Err(Error::invalid(format!(
                "acknowledgement through chain position {through_sequence} is beyond what \
                 the desk queued (newest queued: {newest:?}); acknowledging it would \
                 discard a record that has not been delivered"
            ))),
        }
    }

    /// Whether the chain has a position that was never queued and has not
    /// been healed.
    pub fn has_gap(&self) -> bool {
        self.gap
    }

    /// Whether room has opened for the queue to take a missing record.
    pub fn has_room(&self) -> bool {
        self.outbox.len() < DESK_OUTBOX_CAPACITY
    }

    /// Queue the records the chain advanced over without queueing, from
    /// `derived`: the whole chain re-derived from the event log, in order, so
    /// `derived[n]` is the record at position `n`. Fills each position above
    /// the acknowledged watermark that is neither queued nor beyond the
    /// chain's tail, while there is room, keeping the queue in chain order.
    /// Returns how many were queued.
    ///
    /// Refused, with nothing queued, when `derived` does not agree with the
    /// chain: shorter than the chain, or a record that does not hash onto the
    /// one before it or into the record already queued after it. A healed
    /// record that differs from the one a restart would re-queue is a chain
    /// the ledger would park, and the restart is then the only safe repair.
    pub fn fill_gap(&mut self, derived: &[DeskOutcome]) -> Result<usize> {
        if !self.gap {
            return Ok(0);
        }
        if (derived.len() as u64) < self.next_sequence {
            return Err(Error::invalid(format!(
                "the event log re-derives {} desk records and the chain has advanced to \
                 position {}; the log does not hold the whole chain, so the gap cannot be \
                 healed live — restart from a complete log",
                derived.len(),
                self.next_sequence
            )));
        }
        let first = self.acknowledged_through.map_or(0, |through| through + 1);
        let queued: BTreeSet<u64> = self
            .outbox
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect();
        let mut added = 0usize;
        let mut still_missing = false;
        for sequence in first..self.next_sequence {
            if queued.contains(&sequence) {
                continue;
            }
            if self.outbox.len() >= DESK_OUTBOX_CAPACITY {
                still_missing = true;
                continue;
            }
            let outcome = derived.get(sequence as usize).ok_or_else(|| {
                Error::invalid(format!("no re-derived record at chain position {sequence}"))
            })?;
            if outcome.record.entry.sequence != sequence {
                return Err(Error::invalid(format!(
                    "the re-derived record at index {sequence} names chain position {}",
                    outcome.record.entry.sequence
                )));
            }
            // It must sit between its neighbours on the chain, not merely
            // carry the right number.
            if sequence > 0 {
                let before = &derived[sequence as usize - 1];
                if outcome
                    .record
                    .entry
                    .expected_digest(&before.record.journal_digest)?
                    != outcome.record.entry.digest
                {
                    return Err(Error::invalid(format!(
                        "the re-derived record at chain position {sequence} does not chain \
                         onto the one before it"
                    )));
                }
            }
            if let Some(next) = self
                .outbox
                .iter()
                .find(|o| o.record.entry.sequence == sequence + 1)
                && next
                    .record
                    .entry
                    .expected_digest(&outcome.record.journal_digest)?
                    != next.record.entry.digest
            {
                return Err(Error::invalid(format!(
                    "the re-derived record at chain position {sequence} is not the one the \
                     queued record after it was chained onto"
                )));
            }
            let at = self
                .outbox
                .partition_point(|o| o.record.entry.sequence < sequence);
            self.outbox.insert(at, outcome.clone());
            added += 1;
        }
        self.gap = still_missing;
        Ok(added)
    }

    /// Hand over everything queued, oldest first, leaving the queue empty.
    ///
    /// Destructive: what is returned is no longer queued, so a caller that
    /// then fails to deliver it has lost it from here. Production delivery
    /// uses [`Self::pending`] and [`Self::acknowledge`], which drop a record
    /// only after the fabric has it; this is for tests and operators.
    pub fn drain(&mut self) -> Vec<DeskOutcome> {
        std::mem::take(&mut self.outbox)
    }

    /// The highest chain sequence acknowledged so far.
    pub fn acknowledged_through(&self) -> Option<u64> {
        self.acknowledged_through
    }

    /// Drop every queued record at or below chain `through_sequence`, because
    /// the consumer holds them. Returns how many were dropped.
    ///
    /// Refused, with nothing changed, when `through_sequence` is beyond the
    /// newest record queued (an acknowledgement for something this feed never
    /// held would silently discard the next record to arrive) or below an
    /// earlier acknowledgement (the consumer is resending what it has already
    /// been told is safe). Re-acknowledging the same sequence is a harmless
    /// no-op.
    pub fn acknowledge(&mut self, through_sequence: u64) -> Result<usize> {
        if !self.check_acknowledge(through_sequence)? {
            return Ok(0);
        }
        let before = self.outbox.len();
        self.outbox
            .retain(|outcome| outcome.record.entry.sequence > through_sequence);
        self.acknowledged_through = Some(through_sequence);
        Ok(before - self.outbox.len())
    }
}

/// The chain re-derived with each record's cycle correlation: `booked` is
/// `(fill, correlation)` in log order. The same fold [`outcomes_from`] runs,
/// keeping the correlation the live path attached.
pub fn outcomes_with_correlation<'a>(
    booked: impl IntoIterator<Item = (&'a FillBooked, &'a str)>,
    strategy: &str,
) -> Vec<DeskOutcome> {
    outcomes_with_correlation_on(DESK_CELL, booked, strategy)
}

/// [`outcomes_with_correlation`] on the named desk chain.
pub fn outcomes_with_correlation_on<'a>(
    chain: &str,
    booked: impl IntoIterator<Item = (&'a FillBooked, &'a str)>,
    strategy: &str,
) -> Vec<DeskOutcome> {
    let mut outcomes: Vec<DeskOutcome> = Vec::new();
    for (fill, correlation) in booked {
        let previous = outcomes
            .last()
            .map_or(GENESIS, |last| last.record.journal_digest.as_str());
        let sequence = outcomes.len() as u64;
        if let Ok(record) = outcome_on(chain, fill, strategy, sequence, previous) {
            outcomes.push(DeskOutcome {
                record,
                correlation_id: correlation.to_string(),
            });
        }
    }
    outcomes
}

/// The records for `booked` fills, in the order given, positioned from zero.
///
/// A pure fold: the same fills yield the same records on every call, which
/// is what lets a consumer that lost its place ask for the chain again. A
/// fill the ledger would refuse is skipped exactly as the live path skipped
/// it — it never held a position — so the positions agree.
pub fn outcomes_from<'a>(
    booked: impl IntoIterator<Item = &'a FillBooked>,
    strategy: &str,
) -> Vec<OutcomeRecord> {
    let mut records: Vec<OutcomeRecord> = Vec::new();
    for fill in booked {
        let previous = records
            .last()
            .map_or(GENESIS, |last| last.journal_digest.as_str());
        let sequence = records.len() as u64;
        if let Ok(record) = outcome_for(fill, strategy, sequence, previous) {
            records.push(record);
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::{Decimal, Timestamp, dec};
    use qip_portfolio::ledger::PaperFill;

    fn booked(order: &str, fill: &str, quantity: Decimal, price: Decimal) -> FillBooked {
        FillBooked {
            order_id: order.to_string(),
            fill_id: fill.to_string(),
            object_id: "obj-ACME".to_string(),
            venue: "paper-xnys".to_string(),
            side: Side::Buy,
            quantity,
            price,
            costs: dec!("1.5"),
            at: Timestamp::from_secs(1_760_000_100),
            simulated: true,
        }
    }

    #[test]
    fn the_same_fills_yield_the_same_chain_however_often_it_is_rebuilt() {
        // The failure this prevents: a record identity that depends on when or
        // how many times it was built, so a replay posts a different event.
        let fills = vec![
            booked("o1", "f1", dec!("10"), dec!("100")),
            booked("o2", "f2", dec!("5"), dec!("101")),
        ];
        let first = outcomes_from(&fills, "central-desk");
        let second = outcomes_from(&fills, "central-desk");
        assert_eq!(first.len(), 2, "premise: both fills are recordable");
        assert_eq!(first, second);
        assert_eq!(first[1].entry.sequence, 1);
        let expected = first[1]
            .entry
            .expected_digest(&first[0].journal_digest)
            .expect("v2 digests verify");
        assert_eq!(expected, first[1].entry.digest, "the chain links");
    }

    #[test]
    fn a_fill_offered_twice_is_recorded_once() {
        // The failure this prevents: a replayed fill batch booking twice.
        let mut feed = DeskLedgerFeed::new();
        let fill = booked("o1", "f1", dec!("10"), dec!("100"));
        assert_eq!(
            feed.admit(&fill, "central-desk", "corr-1").unwrap(),
            Admission::Recorded { sequence: 0 }
        );
        assert_eq!(
            feed.admit(&fill, "central-desk", "corr-1").unwrap(),
            Admission::Duplicate
        );
        assert_eq!(feed.pending().len(), 1);
        assert_eq!(feed.next_sequence(), 1);
        assert_eq!(feed.pending()[0].correlation_id, "corr-1");
    }

    #[test]
    fn a_requeued_fill_has_the_identity_it_had_and_its_ids_are_not_remembered() {
        // The failure this prevents: a restart re-queueing under a different
        // digest (the ledger would see a second fact), or remembering the
        // ids so the new process's own (ord-1, fill-1) reads as a replay.
        let fills: Vec<FillBooked> = (0..3)
            .map(|n| booked(&format!("o{n}"), "f1", dec!("1"), dec!("100")))
            .collect();
        let mut live = DeskLedgerFeed::new();
        for fill in &fills {
            live.admit(fill, "central-desk", "c").unwrap();
        }
        let mut resumed = DeskLedgerFeed::new();
        resumed.fold_booked(&fills[0], "central-desk").unwrap();
        resumed.resume_acknowledged(0);
        for fill in &fills[1..] {
            assert!(matches!(
                resumed
                    .requeue_booked(fill, "central-desk", "logged")
                    .unwrap(),
                Admission::Recorded { .. }
            ));
        }
        let records = |feed: &DeskLedgerFeed| -> Vec<_> {
            feed.pending().iter().map(|o| o.record.clone()).collect()
        };
        assert_eq!(records(&resumed), records(&live)[1..].to_vec());
        assert_eq!(resumed.next_sequence(), live.next_sequence());
        // The same ids are a new fill to this process.
        assert!(matches!(
            resumed.admit(&fills[1], "central-desk", "c").unwrap(),
            Admission::Recorded { .. }
        ));
    }

    #[test]
    fn requeueing_past_the_bound_advances_the_chain_and_reports_the_overflow() {
        let mut feed = DeskLedgerFeed::new();
        for n in 0..DESK_OUTBOX_CAPACITY {
            let fill = booked(&format!("o{n}"), "f", dec!("1"), dec!("100"));
            assert!(matches!(
                feed.requeue_booked(&fill, "central-desk", "c").unwrap(),
                Admission::Recorded { .. }
            ));
        }
        let extra = booked("extra", "f", dec!("1"), dec!("100"));
        assert_eq!(
            feed.requeue_booked(&extra, "central-desk", "c").unwrap(),
            Admission::Overflowed {
                sequence: DESK_OUTBOX_CAPACITY as u64
            }
        );
        assert_eq!(feed.pending().len(), DESK_OUTBOX_CAPACITY);
        assert_eq!(feed.next_sequence(), DESK_OUTBOX_CAPACITY as u64 + 1);
    }

    // --- the watermark through a restart of the real platform -------------------

    fn log_path(label: &str) -> std::path::PathBuf {
        // Under the workspace's ignored `target/`: a kernel source file may not
        // read the process environment, and a log needs a directory.
        let dir = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../target/desk-watermark"
        ))
        .join(format!("{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("events.jsonl")
    }

    fn platform_over(path: &std::path::Path) -> Result<Platform> {
        let start = Timestamp::from_secs(1_760_000_000);
        let id = ObjectId::from_string("obj-ACME");
        let mut universe = Universe::new();
        universe.insert(
            FinancialObject::builder(
                id.clone(),
                "ACME",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("desk-ledger", start))
            .build(start)?,
        )?;
        let config = PlatformConfig::default().with_event_log_file(path);
        let seed = config.seed;
        let context = Context::new(Arc::new(ManualClock::new(start)), seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe,
            LimitSet::new("desk-ledger"),
        )
    }

    fn sequences(platform: &Platform) -> Vec<u64> {
        platform
            .pending_desk_outcomes()
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect()
    }

    #[test]
    fn a_restart_requeues_what_was_booked_and_never_acknowledged_with_its_old_identity() {
        // The failure this prevents: fills in the book and the log, never
        // shipped, never re-queued, and the next record a hole the ledger
        // parks on. Crash windows (a) and (b): before any ship, and after the
        // fabric acked but before the watermark was logged — both are a log
        // with no watermark.
        let path = log_path("requeue");
        let before = {
            let mut first = platform_over(&path).unwrap();
            submit_one(&mut first).unwrap();
            submit_one(&mut first).unwrap();
            let before: Vec<_> = first.pending_desk_outcomes().to_vec();
            assert_eq!(before.len(), 2, "premise: two fills queued");
            before
        };
        let second = platform_over(&path).unwrap();
        let after: Vec<_> = second
            .pending_desk_outcomes()
            .iter()
            .map(|o| o.record.clone())
            .collect();
        assert_eq!(
            after,
            before.iter().map(|o| o.record.clone()).collect::<Vec<_>>(),
            "the re-queued records are not the ones the crash lost"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_restart_does_not_requeue_what_the_logged_watermark_says_the_ledger_has() {
        // Crash window (c), and the watermark's idempotence: the fabric acked
        // and the watermark was logged; only the fill after it comes back.
        let path = log_path("covered");
        {
            let mut first = platform_over(&path).unwrap();
            for _ in 0..3 {
                submit_one(&mut first).unwrap();
            }
            assert_eq!(first.acknowledge_desk_outcomes(1).unwrap(), 2);
            // Re-acknowledging writes nothing: the log does not grow without progress.
            let records = first.event_log().records().len();
            assert_eq!(first.acknowledge_desk_outcomes(1).unwrap(), 0);
            assert_eq!(first.event_log().records().len(), records);
        }
        let mut second = platform_over(&path).unwrap();
        assert_eq!(sequences(&second), vec![2]);
        // The refusal of a backwards ack survives the restart.
        assert!(second.acknowledge_desk_outcomes(0).is_err());
        // (e) Ids are not remembered: this process mints its own order ids from
        // one again, and the fill books as new at the next chain position.
        submit_one(&mut second).unwrap();
        assert_eq!(sequences(&second), vec![2, 3]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_watermark_beyond_the_fills_in_the_log_is_refused_at_resume() {
        // Crash window (d): a truncated or corrupt log must not be trusted
        // into dropping fills the ledger never saw.
        let path = log_path("beyond");
        {
            let mut first = platform_over(&path).unwrap();
            submit_one(&mut first).unwrap();
            let now = first.context().now();
            first
                .journal_record(
                    crate::platform::DeskLedgerAcknowledged {
                        through_sequence: 5,
                    },
                    "kernel/desk-ledger",
                    now,
                )
                .unwrap();
        }
        let error = platform_over(&path).unwrap_err();
        assert!(
            error
                .message()
                .contains("truncated or the watermark is wrong"),
            "{}",
            error.message()
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn feed_of(count: usize) -> DeskLedgerFeed {
        let mut feed = DeskLedgerFeed::new();
        for n in 0..count {
            let fill = booked(&format!("o{n}"), "f", dec!("1"), dec!("100"));
            assert!(matches!(
                feed.admit(&fill, "central-desk", "c").unwrap(),
                Admission::Recorded { .. }
            ));
        }
        feed
    }

    #[test]
    fn an_acknowledgement_drops_exactly_the_records_through_it() {
        // The failure this prevents: an ack that drops the whole queue, so a
        // record queued after the shipper peeked is discarded undelivered.
        let mut feed = feed_of(5);
        assert_eq!(feed.acknowledge(2).unwrap(), 3);
        let left: Vec<u64> = feed
            .pending()
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect();
        assert_eq!(left, vec![3, 4]);
        assert_eq!(feed.acknowledged_through(), Some(2));
        // Re-acknowledging the same position is a no-op, not an error.
        assert_eq!(feed.acknowledge(2).unwrap(), 0);
        assert_eq!(feed.pending().len(), 2);
    }

    #[test]
    fn an_acknowledgement_beyond_what_was_queued_is_refused_and_drops_nothing() {
        // The failure this prevents: acknowledging position 9 of a queue that
        // ends at 4 and so discarding records 5..=9 when they arrive.
        let mut feed = feed_of(5);
        let error = feed.acknowledge(5).unwrap_err();
        assert!(error.message().contains("beyond"), "{}", error.message());
        assert_eq!(feed.pending().len(), 5, "a refusal moved the queue");
        assert_eq!(feed.acknowledged_through(), None);
        // And nothing at all queued: any position is beyond it.
        let mut empty = DeskLedgerFeed::new();
        assert!(empty.acknowledge(0).is_err());
    }

    #[test]
    fn an_acknowledgement_that_goes_backwards_is_refused() {
        let mut feed = feed_of(5);
        feed.acknowledge(3).unwrap();
        let error = feed.acknowledge(1).unwrap_err();
        assert!(error.message().contains("backwards"), "{}", error.message());
        assert_eq!(feed.pending().len(), 1);
        assert_eq!(feed.acknowledged_through(), Some(3));
    }

    #[test]
    fn a_peek_leaves_the_queue_and_only_an_acknowledgement_shrinks_it() {
        let mut feed = feed_of(3);
        assert_eq!(feed.pending().len(), 3);
        assert_eq!(feed.pending().len(), 3, "peeking took something");
        feed.acknowledge(2).unwrap();
        assert!(feed.pending().is_empty());
        assert!(!feed.is_full());
    }

    #[test]
    fn a_non_simulated_fill_is_refused_and_leaves_the_feed_untouched() {
        // The failure this prevents: a live fill reaching the ledger's
        // partition and parking the desk's whole chain behind it.
        let mut feed = DeskLedgerFeed::new();
        let mut live = booked("o1", "f1", dec!("10"), dec!("100"));
        live.simulated = false;
        let error = feed.admit(&live, "central-desk", "c").unwrap_err();
        assert!(
            error.message().contains("not marked simulated"),
            "{}",
            error.message()
        );
        assert_eq!(feed.next_sequence(), 0);
        assert!(feed.pending().is_empty());
        // And the refusal did not burn the id: the same fill, corrected, records.
        live.simulated = true;
        assert!(matches!(
            feed.admit(&live, "central-desk", "c").unwrap(),
            Admission::Recorded { sequence: 0 }
        ));
    }

    #[test]
    fn a_notional_off_the_grid_is_refused_before_it_can_park_the_ledger() {
        // 0.000000001 x 0.5 needs a tenth fractional digit.
        let mut feed = DeskLedgerFeed::new();
        let fill = booked("o1", "f1", dec!("0.000000001"), dec!("0.5"));
        let error = feed.admit(&fill, "central-desk", "c").unwrap_err();
        assert!(
            error.message().contains("fractional digits"),
            "{}",
            error.message()
        );
        assert!(feed.pending().is_empty());
    }

    #[test]
    fn a_record_built_for_a_fill_is_the_one_the_ledger_books() {
        let fill = booked("o1", "f1", dec!("10"), dec!("100"));
        let record = outcome_for(&fill, "central-desk", 0, GENESIS).unwrap();
        assert_eq!(record.cell, DESK_CELL);
        let paper = PaperFill::try_from(&record).unwrap();
        assert_eq!(paper.quantity(), dec!("10"));
        assert_eq!(paper.price(), dec!("100"));
        assert_eq!(paper.source().order_id, "o1");
    }

    /// A feed with the bound's worth queued, one fill that overflowed, and the
    /// fills in log order.
    fn overflowed_feed() -> (DeskLedgerFeed, Vec<FillBooked>) {
        let mut feed = DeskLedgerFeed::new();
        let mut all = Vec::new();
        for n in 0..=DESK_OUTBOX_CAPACITY {
            let fill = booked(&format!("h{n}"), "f", dec!("1"), dec!("100"));
            feed.admit(&fill, "central-desk", "c").unwrap();
            all.push(fill);
        }
        assert!(feed.has_gap(), "premise: the last fill found no room");
        assert_eq!(feed.pending().len(), DESK_OUTBOX_CAPACITY, "premise");
        (feed, all)
    }

    fn derive(all: &[FillBooked]) -> Vec<DeskOutcome> {
        outcomes_with_correlation(all.iter().map(|f| (f, "c")), "central-desk")
    }

    /// The failure this prevents: the position the full queue skipped staying
    /// missing until a restart, with the shipper refusing every record behind
    /// it. Acknowledging the queue makes room; the missing record is then
    /// queued at its own position and the gap is closed.
    ///
    /// Mutation: have `fill_gap` return before inserting — nothing is queued
    /// and the first assertion fails.
    #[test]
    fn an_acknowledgement_makes_room_and_the_skipped_record_is_queued_at_its_position() {
        let (mut feed, all) = overflowed_feed();
        assert_eq!(
            feed.fill_gap(&derive(&all)).unwrap(),
            0,
            "no room yet: nothing may be queued past the bound"
        );
        assert!(feed.has_gap(), "a heal with no room cleared the gap");
        feed.acknowledge(DESK_OUTBOX_CAPACITY as u64 - 1).unwrap();
        assert!(feed.pending().is_empty(), "premise: the queue drained");

        assert_eq!(feed.fill_gap(&derive(&all)).unwrap(), 1);
        assert!(!feed.has_gap());
        let sequences: Vec<u64> = feed
            .pending()
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect();
        assert_eq!(sequences, vec![DESK_OUTBOX_CAPACITY as u64]);
        // It is the record a restart would have queued.
        assert_eq!(feed.pending()[0], derive(&all)[DESK_OUTBOX_CAPACITY]);
    }

    /// The healed record is queued in chain order when records behind it are
    /// already queued, and a log that does not agree with the chain heals
    /// nothing.
    ///
    /// Mutation: skip the chaining check against the queued successor — the
    /// forged-log assertion fails.
    #[test]
    fn a_log_that_disagrees_with_the_chain_heals_nothing() {
        let (mut feed, all) = overflowed_feed();
        feed.acknowledge(DESK_OUTBOX_CAPACITY as u64 - 1).unwrap();
        let after = booked("after-gap", "f", dec!("1"), dec!("100"));
        feed.admit(&after, "central-desk", "c").unwrap();
        assert_eq!(
            feed.pending().len(),
            1,
            "premise: one record behind the gap"
        );

        let mut forged = all.clone();
        forged[DESK_OUTBOX_CAPACITY] = booked("forged", "f", dec!("2"), dec!("100"));
        forged.push(after.clone());
        assert!(feed.fill_gap(&derive(&forged)).is_err());
        assert_eq!(feed.pending().len(), 1, "a refused heal queued something");
        assert!(feed.has_gap());

        let mut honest = all;
        honest.push(after);
        assert_eq!(feed.fill_gap(&derive(&honest)).unwrap(), 1);
        let sequences: Vec<u64> = feed
            .pending()
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect();
        assert_eq!(
            sequences,
            vec![DESK_OUTBOX_CAPACITY as u64, DESK_OUTBOX_CAPACITY as u64 + 1]
        );
    }

    #[test]
    fn a_full_outbox_overflows_without_dropping_the_oldest_or_forking_the_chain() {
        let mut feed = DeskLedgerFeed::new();
        let mut all: Vec<FillBooked> = Vec::new();
        for n in 0..DESK_OUTBOX_CAPACITY {
            let fill = booked(&format!("o{n}"), "f", dec!("1"), dec!("100"));
            let admission = feed.admit(&fill, "central-desk", "c").unwrap();
            assert!(matches!(admission, Admission::Recorded { .. }), "premise");
            all.push(fill);
        }
        assert!(feed.is_full(), "premise: the outbox is at capacity");

        let extra = booked("extra", "f", dec!("1"), dec!("100"));
        let admission = feed.admit(&extra, "central-desk", "c").unwrap();
        assert_eq!(
            admission,
            Admission::Overflowed {
                sequence: DESK_OUTBOX_CAPACITY as u64
            },
            "a valid fill with no room must not be refused: the venue holds it"
        );
        all.push(extra);
        assert_eq!(
            feed.pending().len(),
            DESK_OUTBOX_CAPACITY,
            "the overflowed record was queued past the bound"
        );
        assert_eq!(
            feed.next_sequence(),
            DESK_OUTBOX_CAPACITY as u64 + 1,
            "the chain did not advance over the overflowed fill"
        );

        // Nothing is lost: the chain re-derived from the log has the overflowed
        // record at the position the live chain gave it, and the live tail is
        // the re-derived tail, so a restart cannot fork from what was running.
        let rederived = outcomes_from(all.iter(), "central-desk");
        assert_eq!(rederived.len(), DESK_OUTBOX_CAPACITY + 1);
        assert_eq!(
            rederived.last().map(|r| r.entry.sequence),
            Some(DESK_OUTBOX_CAPACITY as u64)
        );
        let next = booked("next", "f", dec!("1"), dec!("100"));
        let live = {
            let mut resumed = DeskLedgerFeed::new();
            for fill in &all {
                resumed.fold_booked(fill, "central-desk").unwrap();
            }
            resumed
        };
        assert_eq!(live.next_sequence(), feed.next_sequence());

        let first = feed.drain();
        assert_eq!(first[0].record.entry.sequence, 0, "the oldest survived");
        assert!(!feed.is_full());
        // After a drain the next fill is queued again, chained onto the
        // overflowed one, not onto the last record that was queued.
        match feed.admit(&next, "central-desk", "c").unwrap() {
            Admission::Recorded { sequence } => {
                assert_eq!(sequence, DESK_OUTBOX_CAPACITY as u64 + 1)
            }
            other => panic!("expected the fill to be queued after a drain, got {other:?}"),
        }
        let queued = feed.pending()[0].record.clone();
        assert_eq!(
            queued.journal_digest,
            outcomes_from(all.iter().chain(std::iter::once(&next)), "central-desk")
                .last()
                .unwrap()
                .journal_digest,
            "the record after the overflow chained onto a tail the log does not hold"
        );
    }

    // --- through the platform, with a venue that misbehaves -----------------

    use crate::PlatformConfig;
    use crate::platform::Platform;
    use qip_core::time::Duration;
    use qip_core::{Context, ManualClock, ObjectId};
    use qip_execution_engine::broker::{
        Broker, SimulatedBroker, SimulationSettings, VenueCapabilities,
    };
    use qip_execution_engine::order::{Fill, Order};
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::universe::Universe;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use std::sync::Arc;

    /// How a venue misreports a fill.
    #[derive(Clone, Copy, Debug)]
    enum Lie {
        /// The venue reports exactly what it did.
        Honest,
        NegativeFee,
        OtherOrder,
    }

    #[derive(Debug)]
    struct LyingBroker {
        inner: SimulatedBroker,
        lie: Lie,
    }

    impl Broker for LyingBroker {
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn is_simulated(&self) -> bool {
            self.inner.is_simulated()
        }
        fn is_available(&self) -> bool {
            self.inner.is_available()
        }
        fn capabilities(&self) -> VenueCapabilities {
            self.inner.capabilities()
        }
        fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>> {
            let mut fills = self.inner.submit(order, at)?;
            for fill in &mut fills {
                match self.lie {
                    Lie::Honest => {}
                    Lie::NegativeFee => fill.costs = Decimal::ZERO - dec!("1"),
                    Lie::OtherOrder => {
                        fill.order_id = qip_core::OrderId::from_string("someone-elses-order");
                    }
                }
            }
            Ok(fills)
        }
        fn cancel(&mut self, order: &Order, at: Timestamp) -> Result<()> {
            self.inner.cancel(order, at)
        }
    }

    fn platform_with(lie: Lie) -> Platform {
        let start = Timestamp::from_secs(1_760_000_000);
        let id = ObjectId::from_string("obj-ACME");
        let mut universe = Universe::new();
        universe
            .insert(
                FinancialObject::builder(
                    id.clone(),
                    "ACME",
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("desk-ledger", start))
                .build(start)
                .expect("valid instrument"),
            )
            .expect("insertable");
        let config = PlatformConfig::default();
        let seed = config.seed;
        let context = Context::new(Arc::new(ManualClock::new(start)), seed);
        let mut platform = Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe,
            LimitSet::new("desk-ledger"),
        )
        .expect("a platform");
        platform.broker = Box::new(LyingBroker {
            inner: SimulatedBroker::new(SimulationSettings::default(), seed),
            lie,
        });
        platform
    }

    fn submit_one(platform: &mut Platform) -> Result<()> {
        let start = Timestamp::from_secs(1_760_000_000);
        let now = start.saturating_add(Duration::from_mins(3));
        let order = platform.order_from(
            ObjectId::from_string("obj-ACME"),
            Side::Buy,
            dec!("100"),
            dec!("100"),
            "prop-lie",
            vec!["hyp-lie".to_string()],
            now,
        );
        platform.submit_order(order, now)
    }

    #[test]
    fn a_fill_the_ledger_would_refuse_is_declined_and_moves_neither_the_book_nor_the_ledger() {
        // The failure this prevents: the book taking a fill the ledger parks its
        // chain for, so cash and the double-entry ledger disagree for good.
        let mut platform = platform_with(Lie::NegativeFee);
        let cash = platform.cash();
        let result = submit_one(&mut platform);
        assert!(
            result.is_ok(),
            "premise: the venue accepted the order: {result:?}"
        );
        assert!(
            !platform.orders().fills().is_empty(),
            "premise: the venue reported a fill"
        );
        assert!(platform.pending_desk_outcomes().is_empty());
        assert_eq!(platform.cash(), cash);
        assert!(platform.positions_at_cost().is_empty());
        assert_eq!(platform.unledgered_fills().len(), 1);
        assert!(
            platform.unledgered_fills()[0].contains("negative fee"),
            "{:?}",
            platform.unledgered_fills()
        );
        assert_eq!(
            platform
                .telemetry()
                .metrics
                .snapshot()
                .counter_total(qip_observability::metrics::names::DESK_FILLS_UNLEDGERED),
            1
        );
    }

    #[test]
    fn a_full_feed_does_not_stop_the_book_following_the_venue() {
        // The failure this prevents: nothing in production drains the feed, so
        // after DESK_OUTBOX_CAPACITY fills a desk that DECLINED on capacity
        // would stop booking what the venue filled, and cash and positions
        // would drift from the venue with nothing wrong with any fill.
        let mut platform = platform_with(Lie::Honest);
        for n in 0..DESK_OUTBOX_CAPACITY {
            let fill = booked(&format!("pre{n}"), "f", dec!("1"), dec!("100"));
            platform
                .desk_ledger_mut()
                .admit(&fill, "central-desk", "c")
                .unwrap();
        }
        assert!(platform.desk_ledger_mut().is_full(), "premise: no room");
        let cash = platform.cash();
        let result = submit_one(&mut platform);
        assert!(result.is_ok(), "premise: the venue accepted: {result:?}");
        assert!(
            !platform.orders().fills().is_empty(),
            "premise: the venue reported a fill"
        );

        // The book followed the venue.
        assert!(
            platform.positions_at_cost().contains_key("obj-ACME"),
            "the fill was not booked because the feed was full"
        );
        assert!(
            platform.cash() < cash,
            "cash did not move for a booked fill"
        );
        // The gap is surfaced and counted, and the bound held.
        assert_eq!(platform.pending_desk_outcomes().len(), DESK_OUTBOX_CAPACITY);
        assert!(
            platform
                .unledgered_fills()
                .iter()
                .any(|line| line.contains("not queued")),
            "{:?}",
            platform.unledgered_fills()
        );
        assert_eq!(
            platform
                .telemetry()
                .metrics
                .snapshot()
                .counter_total(qip_observability::metrics::names::DESK_FILLS_UNLEDGERED),
            1
        );
        // And the chain advanced over it, so a restart folding the log lands on
        // the same position.
        assert_eq!(
            platform.desk_ledger_mut().next_sequence(),
            DESK_OUTBOX_CAPACITY as u64 + 1
        );
    }

    #[test]
    fn a_fill_that_names_another_order_is_declined_as_a_mismatch() {
        let mut platform = platform_with(Lie::OtherOrder);
        let cash = platform.cash();
        let _ = submit_one(&mut platform);
        assert!(platform.pending_desk_outcomes().is_empty());
        assert_eq!(platform.cash(), cash);
        assert!(
            platform
                .unledgered_fills()
                .iter()
                .any(|l| l.contains("arrived in the submission")),
            "{:?}",
            platform.unledgered_fills()
        );
    }
}
