//! The reflex journal contract: what an edge cell decided, and the v1 hash
//! chain over it.
//!
//! ADR 0100 §1 and red-team M13: the ledger and the API have to read a cell's
//! journal without depending on `qip-edge`, because `qip-edge` also carries
//! the order manager, the venue adapters and the cell itself — exactly the
//! things `api_boundary.rs`'s `FORBIDDEN_CRATES` keeps the application layer
//! away from. [`Decision`] and [`JournalEntry`] used to be declared in
//! `qip-edge::journal`, which put every reader of the audit trail behind that
//! boundary too. They live here instead, and `qip-edge::journal` re-exports
//! them, so `qip-edge` still has exactly one definition to write against.
//!
//! **This move is verbatim.** [`chain_digest_v1`] hashes a [`Decision`]
//! through its serialized form, so the chain covers declaration order and
//! every `#[serde(...)]` attribute exactly as written below — reordering a
//! field, renaming one, or changing a `skip_serializing_if` changes every
//! digest a cell has ever sealed. Nothing here may be tidied without also
//! being a new, deliberately versioned digest.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

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
    ///
    /// `release_at` and `equalised` are ADR 0084's: the instant the gateway
    /// was told not to release the order before, and whether the schedule
    /// that produced it had a median for every venue in the set. Both are
    /// absent on entries sealed before the fields existed, and the chain is
    /// hash-linked over the serialised entry, so an absent field is *not*
    /// written back on re-serialisation — `skip_serializing_if` — or every
    /// old journal would fail to verify. A reader treats an absent
    /// `release_at` as "released at the entry's own instant, unequalised",
    /// which is what such an order was, and never as zero.
    OrderSent {
        order_id: String,
        venue: String,
        quantity: String,
        simulated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release_at: Option<Timestamp>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        equalised: bool,
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
    /// The venue a signal's intent was reasoned at, chosen among the venues
    /// whose book for the instrument was usable at the pass instant, by the
    /// tightest quoted spread (ADR 0078, §27.2's consolidation).
    ///
    /// `candidates` is every venue compared and the spread it was compared
    /// on, in venue order, decimals as strings — so a reader can verify the
    /// pick from the chain alone. A pick a replay cannot verify is a pick
    /// nobody can audit.
    VenueChosen {
        object: String,
        venue: String,
        candidates: Vec<(String, String)>,
    },
    /// ADR 0080: the applied policy named a retired strategy's lot and the
    /// cell built a reduce-only intent for it. Its own kind rather than a
    /// `SignalRaised`, because no strategy raised anything: the instruction
    /// came down the policy wire and the size came from this cell's book.
    /// `flatten_by` is what the centre asked, `held` what the cell held, and
    /// `signed_size` the smaller of the two in the instruction's direction —
    /// three numbers because a reader of a partial unwind needs to see that
    /// the cell chose the lot over the instruction, not that it misread one.
    /// Every quantity is a `Decimal` rendered to text, as `Filled` renders
    /// its own.
    DispositionIntent {
        strategy: String,
        object: String,
        venue: String,
        flatten_by: String,
        held: String,
        signed_size: String,
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
            Self::VenueChosen { .. } => "venue_chosen",
            Self::DispositionIntent { .. } => "disposition_intent",
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

/// The v1 chain digest (ADR 0100 §1): `sha256(previous_digest | sequence | at
/// | decision)`.
///
/// The decision is hashed through its serialized form so the chain covers
/// every field. Hashing a summary would let a field change without the
/// digest noticing, which is the failure a chain exists to prevent. Named
/// `_v1` because it is now a contract two crates read: `qip-edge` seals with
/// it and anything auditing a mirrored journal must reproduce it exactly, so
/// a future change to what is hashed is a `_v2` beside this one, not an edit
/// to it — every digest already sealed under v1 has to keep verifying.
pub fn chain_digest_v1(
    previous: &str,
    sequence: u64,
    at: Timestamp,
    decision: &Decision,
) -> String {
    let body = serde_json::to_string(decision).unwrap_or_else(|_| decision.kind().to_string());
    qip_core::sha256_hex(format!("{previous}|{sequence}|{}|{body}", at.as_secs()).as_bytes())
}
