//! Producer state machine: epochs, dense sequences, verified duplicates,
//! carry-over. ADR 0100 §4.
//!
//! This is the broker's half of ADR 0100 §4's ordering rule, applied to a
//! batch already stamped by a drain (`qip_events::event_fabric::codec`'s
//! `producer_id`, `producer_epoch` and `base_sequence` — drain-owned fields,
//! assigned before the batch ever reaches this table). [`ProducerTable`]
//! answers one question per batch: does this `(producer_id, epoch,
//! base_sequence)` extend this producer's dense stream at this partition, or
//! not — and if not, is it a verified retry, a conflict, a fenced
//! incarnation, or simply unverifiable?
//!
//! # Pure, and recovered by replay rather than by a snapshot
//!
//! No I/O and no clock: every decision is a function of the batch's own
//! fields and the state this table already holds. A process rebuilding this
//! table after a restart does so by calling [`ProducerTable::admit`] again
//! for every batch on the tail segment, in order — recovery is the caller
//! replaying history through the same admission rule live traffic uses, not
//! a second code path that has to be kept in sync with this one.
//!
//! # The rule, in order
//!
//! 1. **Fencing first, before sequence is even read.** If `epoch` is older
//!    than the highest epoch this table has ever *appended* a batch under,
//!    the batch is refused outright — [`Admission::FencedEpoch`] — no matter
//!    what its sequence claims. The producer that sent it is a superseded
//!    incarnation; nothing it says about ordering is still true. Fencing
//!    takes effect the moment the successor epoch's first batch is
//!    successfully appended, not merely observed: an epoch that shows up but
//!    fails its own sequence check (case 3 below) has not yet fenced
//!    anything, because nothing has been recorded under it yet.
//! 2. **The dense continuation.** `expected` is one past the last sequence
//!    this table has appended for this `(producer_id, partition)`, or `0` if
//!    it has never appended one — computed the same way whether `epoch`
//!    matches the recorded epoch or is newer. If `base_sequence == expected`,
//!    the batch is appended, the recorded epoch becomes `epoch`, and the
//!    window below remembers it. **This is the only path that carries
//!    sequences across epochs**: a restarted producer's first batch under
//!    its new epoch continues exactly where the old one left off rather than
//!    resetting to zero, so a crash between "wrote a batch" and "the ack
//!    arrived" neither loses that batch's slot nor claims it twice.
//! 3. **Ahead of the stream.** `base_sequence > expected` is a hole this
//!    table cannot fill — [`Admission::OutOfOrder`]. This is refused, not
//!    remembered as broken: state is untouched, so the correct continuation
//!    (the missing batch, or an explicit `Gap` record covering it — see
//!    below) still appends normally whenever it arrives. A hole must never
//!    become a wall the stream cannot get past.
//! 4. **Behind the stream: verify, or refuse.** `base_sequence < expected`
//!    can only be acknowledged as a duplicate by finding the *exact* batch
//!    this table already appended at that slot — same epoch, same
//!    `base_sequence`, still inside the window. Its record count and payload
//!    hash must also match, or the slot has two different claimants
//!    ([`Admission::Conflict`]), which is not a duplicate and must not be
//!    acknowledged as one. A same-epoch match at that sequence with the
//!    identical count and hash is a verified retry ([`Admission::Duplicate`])
//!    of a lost acknowledgement, not new data. Anything this table cannot
//!    find evidence for — aged out of the window, or from an epoch that
//!    never got that far — is [`Admission::OutsideWindow`]: refused rather
//!    than acknowledged on trust. An unverified duplicate is worse than a
//!    refusal, because the caller cannot tell it apart from a genuine one.
//!
//! Checking epoch **before** re-deriving `expected` is what red-team finding
//! M1 found missing: a dedup rule that matched on `base_sequence` alone,
//! "regardless of epoch", could acknowledge a new incarnation's genuinely
//! different records as a duplicate of an old incarnation's, because the two
//! numbers happened to coincide. Matching on `(epoch, base_sequence)`
//! together, plus the payload hash, is what turns "same slot, different
//! content" into a conflict this table can name instead of a duplicate it
//! would silently misfile.
//!
//! # No identifier here claims exactly-once (FABRIC-043)
//!
//! [`Admission::Duplicate`] is a **verified** repeat of a batch this table
//! has already recorded — it is detection, sized against a bounded memory,
//! not a guarantee that no duplicate can ever slip past it. A retry that
//! arrives after the window has moved on is refused
//! ([`Admission::OutsideWindow`]), not silently accepted as new data and not
//! silently accepted as a duplicate either; ADR 0100 §4 is explicit that the
//! window is bounded and finite, and nothing in this module's API, error
//! messages or documentation is permitted to describe the result as
//! exactly-once delivery.
//!
//! # No special case for a `Gap`
//!
//! ADR 0100 §4: a shed window is "an explicit `Gap` record the broker
//! accepts" — an ordinary `Topic::EventFabricGap` record, carrying the next
//! dense sequence in its payload, sent through the same drain-stamped path
//! as any other batch. Because the drain assigns `base_sequence` densely at
//! the moment it writes that record, the record already occupies exactly the
//! next slot in the sequence space; there is no numeric hole in this table's
//! counter for a flag to describe. Giving `admit` a `Gap` variant would
//! therefore branch on a distinction that carries no information here — the
//! red-team note calls a drain-time Gap flag "vacuous" for exactly this
//! reason — and would create two paths for one rule instead of one.
//!
//! # The window is bounded; the producer roster is not
//!
//! [`WINDOW_SIZE`] bounds how many of one producer's most recent appended
//! batches this table remembers *per `(producer_id, partition)`* — the
//! "five-batch window" ADR 0100 §4 and red-team finding m4 require, so
//! verified-duplicate memory cannot grow without limit for one stream. The
//! number of distinct `(producer_id, partition)` pairs this table can hold is
//! not separately bounded here; that count is the size of the producing
//! fleet, a deployment fact the broker's admission control (a later packet)
//! is the right place to cap, not a limit this pure table should guess at.

use std::collections::{BTreeMap, VecDeque};

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::ContentHash;

/// How many of a producer's most recent appended batches this table
/// remembers per `(producer_id, partition)`. ADR 0100 §4 and red-team
/// finding m4: a retry older than this many batches is refused rather than
/// acknowledged on trust, because this table's memory of it is gone.
pub const WINDOW_SIZE: usize = 5;

/// One producer's admission outcome for one batch. See the module
/// documentation for the order these are decided in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The batch extends the dense stream. `next_sequence` is the only
    /// `base_sequence` this table will accept next from this producer at
    /// this partition.
    Appended {
        /// One past the last sequence this batch's records now cover:
        /// `base_sequence + record_count`.
        next_sequence: u64,
    },
    /// A verified retry: the same epoch, the same `base_sequence`, the same
    /// record count and the same payload hash as a batch this table already
    /// appended. Acknowledged without appending again. See the module
    /// documentation's FABRIC-043 note: this is a bounded, best-effort
    /// detection, not a claim of exactly-once delivery.
    Duplicate,
    /// The same `(epoch, base_sequence)` this table already appended, but
    /// with a different record count or a different payload hash — two
    /// different sets of records claiming one slot in the dense stream.
    /// Refused rather than treated as a duplicate, which would silently keep
    /// whichever copy arrived first and discard the other's content
    /// (red-team finding M1).
    Conflict,
    /// `epoch` is older than the highest epoch this table has appended a
    /// batch under. The producer that sent this batch is a superseded
    /// incarnation; `current_epoch` names the epoch that fenced it.
    FencedEpoch {
        /// The epoch this table currently recognises for this producer at
        /// this partition.
        current_epoch: u64,
    },
    /// `base_sequence` is behind the dense stream and no window entry can
    /// verify it as a duplicate — aged out of the window, or from an epoch
    /// that never appended anything at that slot. Refused rather than
    /// acknowledged unverified.
    OutsideWindow,
    /// `base_sequence` is ahead of the dense stream: a hole this table
    /// cannot fill from what it has been shown. `expected` names the only
    /// sequence this table will append next; state is left untouched, so
    /// the correct continuation still appends normally whenever it arrives.
    OutOfOrder {
        /// The sequence this table is actually waiting for.
        expected: u64,
    },
}

/// Identifies one producer's sequence stream at one partition — the unit
/// this table tracks state for (ADR 0100 §4: "dense per `(stream,
/// partition)`"; the objective this table implements: "a pure per-(producer,
/// partition) table").
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ProducerPartition {
    producer_id: String,
    partition: String,
}

/// One appended batch, remembered only long enough to verify a retry of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedBatch {
    epoch: u64,
    base_sequence: u64,
    record_count: u64,
    payload_hash: ContentHash,
}

#[derive(Debug, Default)]
struct ProducerState {
    /// The highest epoch this table has appended a batch under for this
    /// producer at this partition. `None` until the first append.
    epoch: Option<u64>,
    /// One past the last sequence appended, carried across epochs. `None`
    /// until the first append, at which point the dense stream starts at 0.
    last_sequence: Option<u64>,
    /// The most recent up-to-`WINDOW_SIZE` appended batches, oldest first.
    window: VecDeque<CachedBatch>,
}

/// A pure per-`(producer, partition)` table of ADR 0100 §4's ordering rule.
/// See the module documentation for the full decision order.
#[derive(Debug, Default)]
pub struct ProducerTable {
    producers: BTreeMap<ProducerPartition, ProducerState>,
}

impl ProducerTable {
    /// An empty table: no producer has appended anything yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The dense sequence this table last appended for `producer_id` at
    /// `partition`, or `None` if it has never appended one. Read-only, so a
    /// test — or a caller checking its own recovery — can observe what this
    /// table believes without going through [`Self::admit`]'s side effects.
    pub fn last_sequence(&self, producer_id: &str, partition: &str) -> Option<u64> {
        self.producers
            .get(&lookup_key(producer_id, partition))
            .and_then(|state| state.last_sequence)
    }

    /// The epoch this table currently recognises for `producer_id` at
    /// `partition`, or `None` if it has never appended a batch there.
    pub fn current_epoch(&self, producer_id: &str, partition: &str) -> Option<u64> {
        self.producers
            .get(&lookup_key(producer_id, partition))
            .and_then(|state| state.epoch)
    }

    /// Offer one batch already stamped with a drain-assigned producer
    /// identity and sequence. See the module documentation for the decision
    /// order this follows.
    ///
    /// `record_count` is the number of records the batch carries — the
    /// dense stream advances by exactly this many slots on a successful
    /// append. Refuses (rather than silently treating as a no-op) a batch
    /// claiming zero records, and refuses rather than wraps on any addition
    /// that would overflow a `u64` sequence: a peer supplies every one of
    /// these numbers over the wire, and a wrapped sequence is a silent
    /// reordering, not a rare edge case to shrug off.
    pub fn admit(
        &mut self,
        producer_id: &str,
        partition: &str,
        epoch: u64,
        base_sequence: u64,
        record_count: u64,
        payload_hash: ContentHash,
    ) -> Result<Admission> {
        if record_count == 0 {
            return Err(Error::invalid(
                "a batch must carry at least one record; there is nothing to append",
            ));
        }

        let state = self
            .producers
            .entry(lookup_key(producer_id, partition))
            .or_default();

        // Fencing is checked before the sequence is even read: an epoch
        // older than the one this table has already recorded is refused no
        // matter what it claims about ordering, because the incarnation
        // that sent it no longer speaks for this stream.
        if let Some(current_epoch) = state.epoch
            && epoch < current_epoch
        {
            return Ok(Admission::FencedEpoch { current_epoch });
        }

        let expected = match state.last_sequence {
            Some(last) => last.checked_add(1).ok_or_else(|| {
                Error::numeric(format!(
                    "producer {producer_id} at partition {partition} has reached the highest \
                     sequence a u64 can carry; refusing rather than wrapping to 0, which would \
                     be read as the start of a fresh stream"
                ))
            })?,
            None => 0,
        };

        if base_sequence > expected {
            // A hole this table cannot fill. State is left untouched — this
            // must not become a wall: the correct continuation, or an
            // explicit Gap record covering the missing slots, still appends
            // normally the next time it arrives (ADR 0100 §4, red-team
            // finding M2).
            return Ok(Admission::OutOfOrder { expected });
        }

        if base_sequence == expected {
            let next_sequence = base_sequence.checked_add(record_count).ok_or_else(|| {
                Error::numeric(format!(
                    "batch for producer {producer_id} at partition {partition} starting at \
                     sequence {base_sequence} with {record_count} records would overflow a u64 \
                     sequence"
                ))
            })?;
            state.epoch = Some(epoch);
            state.last_sequence = Some(next_sequence - 1);
            state.window.push_back(CachedBatch {
                epoch,
                base_sequence,
                record_count,
                payload_hash,
            });
            while state.window.len() > WINDOW_SIZE {
                state.window.pop_front();
            }
            return Ok(Admission::Appended { next_sequence });
        }

        // base_sequence < expected: only a batch this table has already
        // appended, in this exact epoch, at this exact slot, can be
        // verified. Anything else — a stale epoch's slot, or a sequence the
        // window no longer remembers — is refused rather than guessed at.
        match state
            .window
            .iter()
            .find(|cached| cached.epoch == epoch && cached.base_sequence == base_sequence)
        {
            Some(cached)
                if cached.record_count == record_count && cached.payload_hash == payload_hash =>
            {
                Ok(Admission::Duplicate)
            }
            Some(_) => Ok(Admission::Conflict),
            None => Ok(Admission::OutsideWindow),
        }
    }
}

fn lookup_key(producer_id: &str, partition: &str) -> ProducerPartition {
    ProducerPartition {
        producer_id: producer_id.to_string(),
        partition: partition.to_string(),
    }
}
