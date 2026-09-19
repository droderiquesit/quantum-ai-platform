//! The cell's local decision record, and the mirror that ships it.
//!
//! A cell decides without asking the centre, so the centre can only ever learn
//! what a cell did after the fact. That makes the record the whole audit story
//! for everything the platform does at speed, and it has to survive the cell
//! dying: hash-chained here, drained to durable storage by an explicit
//! [`Mirror::ship`] call that never happens on the hot path.
//!
//! Nothing here writes to a file during [`crate::Cell::on_bytes`] or
//! [`crate::Cell::work`]. That is not an optimisation, it is the reason the
//! mirror is asynchronous at all: a decision loop that blocks on a disk is a
//! decision loop whose latency is a storage system's problem.

use qip_core::error::{Error, Result};
use qip_core::{Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One thing the cell did, or refused to do.
///
/// Refusals are first-class. A cell that records only its trades can answer
/// "why did this happen" and not "why did nothing happen", and the second
/// question is the one asked after a quiet morning that should not have been.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Decision {
    /// Bytes arrived and decoded into this many messages on this feed.
    Ingested {
        feed: String,
        decoded: usize,
        skipped: usize,
    },
    /// A sequence gap was detected and the affected books reset.
    GapDetected { stream: String, detail: String },
    /// A strategy emitted a signal.
    SignalRaised {
        strategy: String,
        object: String,
        kind: String,
        conviction_shrunk_f64: f64,
    },
    /// An opportunity was priced and its net edge computed.
    EdgePriced {
        opportunity: String,
        net: String,
        positive: bool,
    },
    /// An order was sent to a venue.
    OrderSent {
        order_id: String,
        venue: String,
        quantity: String,
        simulated: bool,
    },
    /// The venue reported part or all of an order traded, and the cell
    /// booked it.
    ///
    /// Distinct from [`Self::OrderSent`] on purpose, and the distinction is
    /// the whole record: an order sent is a request the venue accepted, and
    /// a fill is a venue fact about what traded. The chain once carried only
    /// the first and every reader took it for the second. `shares` is the
    /// pro-rata attribution of this fill's quantity to the strategies whose
    /// intent the order carried, summing to `quantity` exactly, so the
    /// journal alone answers who traded what. Decimals as strings, as
    /// everywhere in this enum.
    Filled {
        order_id: String,
        venue: String,
        object: String,
        quantity: String,
        price: String,
        simulated: bool,
        shares: Vec<(String, String)>,
    },
    /// A resting order passed its time to live and the cell withdrew what
    /// remained, `withdrawn` being the venue's own answer to the cancel.
    /// Whatever filled before this is in its own [`Self::Filled`] entries;
    /// this closes the order without claiming anything about them.
    OrderExpired {
        order_id: String,
        venue: String,
        withdrawn: String,
    },
    /// A resting order was withdrawn because the cell is halted, rather
    /// than because its own time to live ran out (§29.2).
    ///
    /// Distinct from [`Self::OrderExpired`] on purpose, and the distinction is
    /// the record: the two are the same action for entirely different reasons,
    /// and an incident review reading a chain full of expiries cannot see the
    /// moment a kill switch emptied the book. `withdrawn` is the venue's own
    /// answer to the cancel, as it is there.
    MassCancelled {
        order_id: String,
        venue: String,
        withdrawn: String,
    },
    /// Something was refused, with the gate that refused it.
    Refused { gate: String, reason: String },
    /// The venue and the cell's book disagree about a fill.
    ReconciliationBreak { detail: String },
    /// The cell halted or resumed.
    HaltChanged { halted: bool, reason: String },
    /// A verified policy payload was applied by atomic swap.
    ///
    /// `narrowed` names the capabilities the payload leaves less than fresh,
    /// in order, so the reason a cell sized small is reconstructable from the
    /// journal alone.
    PolicyApplied {
        sequence: u64,
        halted: bool,
        narrowed: Vec<String>,
    },
    /// A capital envelope the centre issued was verified and installed.
    ///
    /// Recorded like a decision because it is one: it is the moment the cell's
    /// authority changed, and "why was this cell allowed to commit that much"
    /// is a question the journal has to answer as precisely as "why did this
    /// trade". The approver and the expiry are carried because those are the
    /// two facts an incident review asks for first.
    CapitalRenewed {
        strategy: String,
        approver: String,
        expires_at: Timestamp,
    },
    /// Two or more strategies' intents offset and the offsetting part was
    /// crossed inside the cell instead of reaching a venue (§27.1).
    ///
    /// The blueprint calls this a ledger entry rather than an optimisation
    /// detail, and a regulatory expectation: an internal cross is a trade
    /// between two of the platform's own strategies, and a trade nobody can
    /// point at afterwards is the thing an examiner asks about. Both sides and
    /// the price are named for that reason — "who traded with whom, at what
    /// price, and who decided the price" has to be answerable from the chain
    /// alone.
    ///
    /// `price` is the prevailing mid at the netting instant, which is a price
    /// neither side chose. Decimals are carried as strings for the same reason
    /// the rest of this enum does: the journal is a record, and a record that
    /// reformats a number is a record of a different number.
    CrossedInternally {
        object: String,
        venue: String,
        quantity: String,
        price: String,
        /// The strategies on the buying side, and on the selling side. Both,
        /// because a cross with one named side is not a cross anybody can
        /// check.
        bought: Vec<String>,
        sold: Vec<String>,
    },
    /// A found cycle was assigned one of blueprint §30.2's eight execution
    /// paths (ADR 0068).
    ///
    /// Recorded at the router's own seam rather than at the send, and for
    /// every cycle the router assigned rather than for every cycle that
    /// traded, so the chain holds the classification whether or not a later
    /// gate vetoed the cycle. The pairing is the point: a cycle that reached
    /// the router leaves either this entry or a [`Self::Refused`] under the
    /// `path_router` gate, and never neither. An assignment that appeared
    /// only for cycles that traded would answer "how did this execute" and
    /// not "what did the router think of what it saw", and the second is the
    /// question asked when the cell is quiet.
    ///
    /// An entry here is **not** a claim that anything was sent. §30.2's
    /// assignment is a classification — which coordination mechanism and
    /// which latency budget the cycle would be executed under — and nothing
    /// in `qip-routing`'s path vocabulary can name a venue or produce an
    /// order.
    ///
    /// `path` is §30.2's own row number and `path_name` the identifier, both,
    /// because an operator reads the table by number and everything else here
    /// by name. `eligible` is every path the composition admitted, in order,
    /// so a replay can tell a cycle that had one possible path from one where
    /// the preference chose between several — and the preference is the
    /// caller's, which is exactly the fact a later argument about routing will
    /// turn on.
    CyclePathAssigned {
        cycle_id: String,
        path: u8,
        path_name: String,
        eligible: Vec<String>,
        rationale: String,
    },
    /// Blueprint §33.1's extension for the assigned path held (§31.1, §33.1).
    ///
    /// §33.1: *"Every verdict, including silence, is logged."* This is the
    /// half that held; the half that refused is a `Refused` entry under the
    /// `path_extension` gate, so both outcomes are on the chain and neither
    /// has to be inferred from the other's absence.
    ///
    /// `has_row` is the fact a count of these entries would otherwise hide:
    /// §33.1's table starts at path 3, so for paths 1 and 2 the honest
    /// record is that the blueprint asks for no additional check — which is
    /// a different thing from a check that passed, and reads identically in
    /// any log that stores only success.
    PathExtensionChecked {
        cycle_id: String,
        path: u8,
        has_row: bool,
        rationale: String,
    },
    /// Every leg of an arbitrage cycle was sent (§30, §27.2).
    ///
    /// Recorded once the last leg is past the venue call, naming the orders
    /// that make up the atomic set, so a reader of the chain can tell which
    /// `order_sent` entries belong together without re-running the scan.
    /// The net edge is the scanner's, in units of the instrument the cycle
    /// started from, carried as a string for the reason every other decimal
    /// here is.
    CycleCommitted {
        cycle_id: String,
        orders: Vec<String>,
        net: String,
    },
    /// A leg of an arbitrage cycle went out smaller than the scanner priced
    /// it, because an earlier leg of the same cycle filled short (§32.1).
    ///
    /// Recorded at the moment the size is chosen and before the leg is sent,
    /// with the planned size beside the one that went out, so the chain
    /// answers "why is this order not the size the cycle was admitted at"
    /// without anybody having to re-derive it from the fills. `fraction` is
    /// what the cycle can still complete at — the minimum over every leg the
    /// venues have answered on, not this leg alone — and is carried as a
    /// string for the reason every other decimal here is.
    CycleDecomposed {
        cycle_id: String,
        leg: usize,
        planned: String,
        size: String,
        fraction: String,
    },
    /// One leg of an arbitrage cycle was sent alone and left to rest, the
    /// rest of the cycle held back until the venue answers it (§32.1's
    /// passive-first mechanism).
    ///
    /// Recorded at the instant the choice is taken and before the leg is
    /// sent, because "why did only one leg of this cycle reach a venue" is a
    /// question a chain reader will ask of the very next entry. `median` is
    /// the measured fill time in milliseconds that made this venue the
    /// slowest of the cycle's — the evidence, not the conclusion, so a
    /// replay can tell a cell that rested on measurement from one that
    /// rested on a tie nobody broke. A statistic rather than money, so it is
    /// a number here and not a decimal string.
    CycleRested {
        cycle_id: String,
        leg: usize,
        venue: String,
        order_id: String,
        median_millis: i64,
    },
    /// A cycle whose resting leg was withdrawn without filling anything, so
    /// no leg of it ever became a position (§32.1).
    ///
    /// This is the entry that distinguishes passive-first from a delay. Under
    /// the all-at-once discipline the fast legs would already be crossed
    /// against a slow leg that never filled, and the cell would be holding
    /// the difference. `reason` names what closed the resting order — its own
    /// time to live elapsing, or a mass cancel on a halt — because the two
    /// send an operator to different places.
    CycleAbandoned {
        cycle_id: String,
        leg: usize,
        venue: String,
        reason: String,
    },
    /// A deployed strategy was withdrawn from the cell, its envelope handed
    /// back to the caller.
    ///
    /// Recorded because it is the moment the cell stopped being able to act
    /// on that strategy's signals, and "why did this strategy go quiet" has
    /// the same standing as "why did it trade": a plan that dropped it is
    /// the usual answer, and the answer belongs in the chain, not in the
    /// node's log.
    StrategyWithdrawn { strategy: String },
    /// The region table was re-based to this cell's share of its region's
    /// grant, as a verified policy payload's grant manifest named it
    /// (ADR 0039).
    ///
    /// Recorded because it is the moment the cell's total authority changed,
    /// which has the same standing as [`Self::CapitalRenewed`] for one
    /// strategy. `grants` is how many verified envelopes the manifest named
    /// and the cell counted; `deficit` is non-zero when the share fell below
    /// what the cell had already held or committed, which zeroes `free` and
    /// un-sends nothing — a stated ledger state, and the reason the cell was
    /// then refused under `region_reservation` until its orders settled.
    /// Decimals as strings, as everywhere in this enum.
    RegionShareApplied {
        sequence: u64,
        grants: usize,
        share: String,
        bound: String,
        free: String,
        deficit: String,
    },
    /// What the cell has been told about the other regions changed (§36.3).
    ///
    /// Recorded because it is the moment the cell stopped — or started —
    /// taking one side of a cross-region mirror, and "why did this cell stop
    /// mirroring" has the same standing as "why did it trade". `source` is
    /// the reading's own kind and `regions` the names it carried, which is
    /// empty for the unreadable reading: that one darkens every region other
    /// than this cell's own and names none, so a reader who saw only a list
    /// would think nothing had changed.
    RegionOutlookChanged {
        source: String,
        regions: Vec<String>,
        detail: String,
    },
    /// The cell was told to reconcile against every venue before resuming
    /// (§36.3's node-crash row).
    ///
    /// The venues are named, because the discipline clears venue by venue
    /// and a chain that recorded only "reconciliation required" could not
    /// say which venue was still outstanding when the cell was quiet.
    ReconciliationRequired { reason: String, venues: Vec<String> },
    /// One venue's own account agreed with the cell's record, after a
    /// restart (§36.3).
    ///
    /// `open` and `quotes` are what the venue said it was holding — both
    /// zero for the ordinary clean answer — and `pending` names the venues
    /// still to answer. `resumed` is the fact the pending list implies and
    /// this states, so a replay does not have to infer the moment the cell
    /// was allowed to form an order again from an empty vector.
    VenueReconciled {
        venue: String,
        open: usize,
        quotes: usize,
        pending: Vec<String>,
        resumed: bool,
    },
}

impl Decision {
    /// A short label for the kind of decision, for counting without matching.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Ingested { .. } => "ingested",
            Self::GapDetected { .. } => "gap_detected",
            Self::SignalRaised { .. } => "signal_raised",
            Self::EdgePriced { .. } => "edge_priced",
            Self::OrderSent { .. } => "order_sent",
            Self::Filled { .. } => "filled",
            Self::OrderExpired { .. } => "order_expired",
            Self::MassCancelled { .. } => "mass_cancelled",
            Self::Refused { .. } => "refused",
            Self::ReconciliationBreak { .. } => "reconciliation_break",
            Self::HaltChanged { .. } => "halt_changed",
            Self::PolicyApplied { .. } => "policy_applied",
            Self::CapitalRenewed { .. } => "capital_renewed",
            Self::CrossedInternally { .. } => "crossed_internally",
            Self::CyclePathAssigned { .. } => "cycle_path_assigned",
            Self::PathExtensionChecked { .. } => "path_extension_checked",
            Self::CycleCommitted { .. } => "cycle_committed",
            Self::CycleDecomposed { .. } => "cycle_decomposed",
            Self::CycleRested { .. } => "cycle_rested",
            Self::CycleAbandoned { .. } => "cycle_abandoned",
            Self::StrategyWithdrawn { .. } => "strategy_withdrawn",
            Self::RegionShareApplied { .. } => "region_share_applied",
            Self::RegionOutlookChanged { .. } => "region_outlook_changed",
            Self::ReconciliationRequired { .. } => "reconciliation_required",
            Self::VenueReconciled { .. } => "venue_reconciled",
        }
    }
}

/// A decision with its position in the chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub sequence: u64,
    pub at: Timestamp,
    pub decision: Decision,
    /// `sha256(previous_digest | sequence | at | decision)`.
    pub digest: String,
}

/// An append-only, hash-chained record of everything the cell decided.
///
/// The chain is what lets the centre detect a cell that dropped entries: a
/// mirror batch whose first entry does not chain onto the last one received is
/// a gap, whatever the sequence numbers claim.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    entries: Vec<JournalEntry>,
    /// How many entries have been handed to a mirror. Entries are kept after
    /// shipping so a replay can reconstruct the session; a production cell
    /// would trim behind an acknowledged watermark.
    shipped: usize,
}

impl Journal {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest an empty chain starts from.
    ///
    /// A fixed, named value rather than an empty string, so a batch that
    /// claims to be the first is distinguishable from one whose predecessor
    /// went missing.
    pub const GENESIS: &'static str = "genesis";

    pub fn record(&mut self, decision: Decision, at: Timestamp) -> &JournalEntry {
        let sequence = self.entries.len() as u64;
        let previous = self
            .entries
            .last()
            .map_or(Self::GENESIS.to_string(), |entry| entry.digest.clone());
        let digest = chain_digest(&previous, sequence, at, &decision);
        self.entries.push(JournalEntry {
            sequence,
            at,
            decision,
            digest,
        });
        self.entries
            .last()
            .unwrap_or_else(|| unreachable!("an entry was just pushed"))
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many decisions of each kind the cell has recorded.
    pub fn tally(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for entry in &self.entries {
            *counts.entry(entry.decision.kind()).or_insert(0) += 1;
        }
        counts
    }

    /// Verify the chain, returning the sequence where it first breaks.
    pub fn verify(&self) -> std::result::Result<(), u64> {
        let mut previous = Self::GENESIS.to_string();
        for entry in &self.entries {
            let expected = chain_digest(&previous, entry.sequence, entry.at, &entry.decision);
            if expected != entry.digest {
                return Err(entry.sequence);
            }
            previous = entry.digest.clone();
        }
        Ok(())
    }

    /// Everything not yet handed to a mirror.
    pub fn unshipped(&self) -> &[JournalEntry] {
        &self.entries[self.shipped.min(self.entries.len())..]
    }

    fn mark_shipped(&mut self, count: usize) {
        self.shipped = (self.shipped + count).min(self.entries.len());
    }
}

fn chain_digest(previous: &str, sequence: u64, at: Timestamp, decision: &Decision) -> String {
    // The decision is hashed through its serialized form so the chain covers
    // every field. Hashing a summary would let a field change without the
    // digest noticing, which is the failure a chain exists to prevent.
    let body = serde_json::to_string(decision).unwrap_or_else(|_| decision.kind().to_string());
    sha256_hex(format!("{previous}|{sequence}|{}|{body}", at.as_secs()).as_bytes())
}

/// A batch of journal entries, carrying enough chain to be checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MirrorBatch {
    pub cell: String,
    pub at: Timestamp,
    /// The digest the first entry chains onto — [`Journal::GENESIS`] for the
    /// first batch of a session.
    pub chains_onto: String,
    pub entries: Vec<JournalEntry>,
    /// Stream watermarks as of this batch, so the centre knows how far the
    /// cell had consumed when it made these decisions.
    pub watermarks: Vec<(String, u64)>,
}

impl MirrorBatch {
    /// Whether this batch chains onto `previous_digest` and is internally
    /// consistent.
    pub fn verify_against(&self, previous_digest: &str) -> Result<()> {
        if self.chains_onto != previous_digest {
            return Err(Error::invalid(format!(
                "mirror batch from {} chains onto {} but the last received was {previous_digest}",
                self.cell, self.chains_onto
            )));
        }
        let mut previous = self.chains_onto.clone();
        for entry in &self.entries {
            let expected = chain_digest(&previous, entry.sequence, entry.at, &entry.decision);
            if expected != entry.digest {
                return Err(Error::invalid(format!(
                    "mirror batch from {} breaks its chain at sequence {}",
                    self.cell, entry.sequence
                )));
            }
            previous = entry.digest.clone();
        }
        Ok(())
    }

    /// The digest a following batch must chain onto.
    pub fn tail_digest(&self) -> String {
        self.entries
            .last()
            .map_or_else(|| self.chains_onto.clone(), |entry| entry.digest.clone())
    }
}

/// Somewhere durable a cell ships its journal to.
///
/// Called only from [`crate::Cell::flush`], never from the hot path. An
/// implementation is free to block; that is the whole point of it being here
/// rather than inline.
pub trait Mirror: std::fmt::Debug {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()>;

    /// What this would need in production, empty when it is usable as is.
    fn required_configuration(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A mirror that keeps batches in memory, for tests and for a cell whose
/// durable target is unreachable.
#[derive(Debug, Default)]
pub struct MemoryMirror {
    batches: Vec<MirrorBatch>,
}

impl MemoryMirror {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn batches(&self) -> &[MirrorBatch] {
        &self.batches
    }

    /// Verify every batch chains onto the last, in order.
    ///
    /// What the centre does on receipt, exercised here so the property is
    /// tested without a durable store.
    pub fn verify_continuity(&self) -> Result<()> {
        let mut previous = Journal::GENESIS.to_string();
        for batch in &self.batches {
            batch.verify_against(&previous)?;
            previous = batch.tail_digest();
        }
        Ok(())
    }
}

impl Mirror for MemoryMirror {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        self.batches.push(batch);
        Ok(())
    }
}

/// A mirror that appends JSON batches to a file.
#[derive(Debug)]
pub struct FileMirror {
    path: std::path::PathBuf,
}

impl FileMirror {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Mirror for FileMirror {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(&batch).map_err(|error| {
            Error::schema(format!("a mirror batch would not serialize: {error}"))
        })?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| {
                Error::io(format!(
                    "cannot open the mirror at {}: {error}",
                    self.path.display()
                ))
            })?;
        writeln!(file, "{line}")
            .map_err(|error| Error::io(format!("cannot write to the mirror: {error}")))
    }
}

/// Drain a journal into a batch and hand it to a mirror.
///
/// Public so a caller holding a journal without a whole [`crate::Cell`] can
/// ship it, and so the chaining property can be tested without one.
pub fn ship(
    journal: &mut Journal,
    mirror: &mut dyn Mirror,
    cell: &str,
    watermarks: Vec<(String, u64)>,
    now: Timestamp,
) -> Result<usize> {
    let pending = journal.unshipped().to_vec();
    if pending.is_empty() {
        return Ok(0);
    }
    let chains_onto = journal
        .entries()
        .get(pending[0].sequence as usize)
        .and_then(|first| {
            first
                .sequence
                .checked_sub(1)
                .and_then(|previous| journal.entries().get(previous as usize))
        })
        .map_or(Journal::GENESIS.to_string(), |entry| entry.digest.clone());

    let count = pending.len();
    mirror.ship(MirrorBatch {
        cell: cell.to_string(),
        at: now,
        chains_onto,
        entries: pending,
        watermarks,
    })?;
    journal.mark_shipped(count);
    Ok(count)
}
