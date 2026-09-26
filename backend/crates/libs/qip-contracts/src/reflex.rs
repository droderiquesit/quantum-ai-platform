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
//!
//! **That versioned digest is [`chain_digest_v2`]**, and every new entry is
//! sealed under it. Each [`JournalEntry`] names its [`ChainVersion`], so v1
//! entries keep verifying under v1's rules and v2 entries are never checked
//! under them. The P1 bodies ADR 0100 §5 names — [`OutcomeRecord`],
//! [`ChainSpan`] and [`Gap`] — live here too, as data; their topic bindings
//! are `qip-events`'.

use crate::message::BookSide;
use qip_core::Timestamp;
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

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
    ///
    /// `side`, `quote_unit` and `fee` are the posting fields (red-team B2):
    /// without a side a fill cannot be booked as a debit or a credit, without
    /// a quote unit its price is a number in no currency, and a ledger that
    /// posts from this entry had nothing to post from. `side` uses the wire's
    /// own reading (`FillRecord::side`): `Ask` bought, `Bid` sold.
    ///
    /// All three are optional and skipped when absent, for the reason
    /// `OrderSent`'s ADR 0084 fields are: the v1 chain hashes the serialised
    /// decision, so an absent field written back as `null` would move every
    /// digest a cell has already sealed. **An absent `fee` means the venue
    /// reported none, never that the fee was zero** (LEDGER-019): a fee is
    /// only ever what a venue said it charged, as a `Decimal` rendered to
    /// text, and a reader that books a missing fee as zero has invented a
    /// venue fact.
    Filled {
        order_id: String,
        venue: String,
        object: String,
        quantity: String,
        price: String,
        simulated: bool,
        shares: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        side: Option<BookSide>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote_unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee: Option<String>,
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

/// Which rules an entry's digest was sealed under.
///
/// Carried on the entry rather than inferred, so a verifier never checks one
/// version's digest under the other's rules: a v1 digest recomputed as v2
/// (or the reverse) always mismatches, and a verifier that guessed would
/// report a sound chain as broken — or, worse, be taught to try both and
/// accept whichever passes, which accepts a sub-second edit to any entry by
/// relabelling it v1.
///
/// Absent on the wire means v1, and v1 is never written back, so every entry
/// sealed before v2 existed re-serialises byte-identically. An unknown
/// version is refused at deserialisation rather than read as either.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ChainVersion {
    /// [`chain_digest_v1`]: declaration-order JSON, whole seconds, and the
    /// kind hashed in place of a body that would not serialise. Verified,
    /// never written.
    #[default]
    V1,
    /// [`chain_digest_v2`]: canonical JSON, nanoseconds, and a refusal in
    /// place of a body that has no canonical form.
    V2,
}

impl ChainVersion {
    /// Whether this is v1 — the `skip_serializing_if` that keeps v1 entries
    /// byte-identical.
    pub fn is_v1(&self) -> bool {
        *self == Self::V1
    }
}

/// A decision with its position in the chain.
///
/// `Serialize` is written out rather than derived because the two versions
/// store `at` differently, and each for a reason. A v1 entry writes the
/// instant through `Timestamp`'s own RFC 3339 form, exactly as it always has.
/// A v2 entry writes it as integer nanoseconds, because v2 hashes
/// nanoseconds and `Timestamp`'s text form keeps only milliseconds: a v2
/// entry written as text would lose the digits its digest covers and fail
/// to verify the moment it came back from a mirror. `Deserialize` reads
/// either form, which `Timestamp` already does.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct JournalEntry {
    pub sequence: u64,
    pub at: Timestamp,
    pub decision: Decision,
    /// The chain digest, under the rules `version` names.
    pub digest: String,
    #[serde(default)]
    pub version: ChainVersion,
}

impl Serialize for JournalEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        // Field order is the derived order this type had under v1; a v1
        // entry must come out byte-for-byte as it went in.
        let fields = if self.version.is_v1() { 4 } else { 5 };
        let mut entry = serializer.serialize_struct("JournalEntry", fields)?;
        entry.serialize_field("sequence", &self.sequence)?;
        match self.version {
            ChainVersion::V1 => entry.serialize_field("at", &self.at)?,
            ChainVersion::V2 => entry.serialize_field("at", &self.at.as_nanos())?,
        }
        entry.serialize_field("decision", &self.decision)?;
        entry.serialize_field("digest", &self.digest)?;
        if !self.version.is_v1() {
            entry.serialize_field("version", &self.version)?;
        }
        entry.end()
    }
}

impl JournalEntry {
    /// The digest this entry should carry if it chains onto `previous`,
    /// computed under the rules its own `version` names and no other.
    ///
    /// An `Err` is a v2 entry whose decision has no canonical form, which
    /// no v2 writer seals — so a verifier reads it as a break, not a pass.
    pub fn expected_digest(&self, previous: &str) -> Result<String> {
        match self.version {
            ChainVersion::V1 => Ok(chain_digest_v1(
                previous,
                self.sequence,
                self.at,
                &self.decision,
            )),
            ChainVersion::V2 => chain_digest_v2(previous, self.sequence, self.at, &self.decision),
        }
    }
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
///
/// **Nothing new seals under this.** It is kept, unchanged, to verify what
/// was sealed before [`chain_digest_v2`], and its three defects are kept with
/// it because fixing any of them here would break those entries: it hashes
/// declaration-order JSON (M13), whole seconds so a sub-second edit to `at`
/// verifies (F3), and a decision that would not serialise is hashed as its
/// bare kind (F4).
pub fn chain_digest_v1(
    previous: &str,
    sequence: u64,
    at: Timestamp,
    decision: &Decision,
) -> String {
    let body = serde_json::to_string(decision).unwrap_or_else(|_| decision.kind().to_string());
    qip_core::sha256_hex(format!("{previous}|{sequence}|{}|{body}", at.as_secs()).as_bytes())
}

/// The gate a journal refuses under when a decision has no canonical form.
pub const GATE_JOURNAL_ENCODING: &str = "journal_encoding";

/// The v2 chain digest: `sha256("v2" | previous | sequence | at_nanos |
/// canonical(decision))`.
///
/// Three changes from [`chain_digest_v1`], each closing a named hole.
/// **Canonical JSON** (M13): the body is [`canonical_json`], keys sorted at
/// every depth, so the digest names the decision's content and not the order
/// some serialiser happened to write it in. **Nanoseconds** (F3): v1 hashed
/// whole seconds, so moving an entry's instant by half a second left its
/// digest unchanged and the chain verified an edit to exactly the fact an
/// ordering dispute turns on. **A `Result`** (F4): v1 hashed a body that
/// would not serialise as its bare kind, so any two such decisions of one
/// kind sealed to the same digest and the chain vouched for contents nobody
/// could read. The `v2` prefix separates the two preimage spaces outright.
pub fn chain_digest_v2(
    previous: &str,
    sequence: u64,
    at: Timestamp,
    decision: &Decision,
) -> Result<String> {
    let body = canonical_decision(decision)?;
    Ok(digest_v2_over(previous, sequence, at, &body))
}

fn digest_v2_over(previous: &str, sequence: u64, at: Timestamp, body: &str) -> String {
    qip_core::sha256_hex(format!("v2|{previous}|{sequence}|{}|{body}", at.as_nanos()).as_bytes())
}

/// The canonical JSON a v2 digest is taken over, refused where the decision
/// has no form that reads back as itself.
///
/// Serialising is not enough to be recordable. `serde_json` writes a
/// non-finite `f64` as `null` without an error, and a `null` does not read
/// back as a number: an entry sealed over it verifies forever and cannot be
/// replayed into the decision it names. So the body is decoded back into a
/// [`Decision`] and re-encoded, and anything that does not come back to the
/// same bytes is refused. Equality is taken on the bytes, not on the
/// decoded value, because `Timestamp`'s text form keeps milliseconds: a
/// sub-millisecond instant inside a decision reads back truncated, and the
/// truncated form is what the record holds and what the digest covers —
/// stable on every later read, which is what replay needs.
pub fn canonical_decision(decision: &Decision) -> Result<String> {
    let value = serde_json::to_value(decision).map_err(|error| {
        Error::schema(format!(
            "a {} decision would not serialise: {error}",
            decision.kind()
        ))
    })?;
    let body = canonical_json(&value);
    let decoded = Decision::deserialize(&value).map_err(|error| {
        Error::schema(format!(
            "a {} decision serialises to a form that does not read back ({error}); a non-finite \
             number is the usual cause — refuse it where it is produced",
            decision.kind()
        ))
    })?;
    let again = serde_json::to_value(&decoded).map_err(|error| {
        Error::schema(format!(
            "a {} decision read back but would not serialise again: {error}",
            decision.kind()
        ))
    })?;
    if canonical_json(&again) != body {
        return Err(Error::schema(format!(
            "a {} decision does not read back as the same bytes, so its record could not be \
             replayed into it",
            decision.kind()
        )));
    }
    Ok(body)
}

/// Seal `decision` under v2 at `sequence`, returning what was actually
/// sealed and its digest.
///
/// Total, because the journal's `record` cannot fail: a decision was taken,
/// and the chain must say *something* at this sequence. What it never says
/// is a digest over the kind alone (v1's F4). A decision with no canonical
/// form is replaced by a [`Decision::Refused`] under
/// [`GATE_JOURNAL_ENCODING`] naming its kind and why, so the gap is on the
/// chain as a refusal a reader can find rather than as a digest nobody can
/// reproduce from the body beside it. The refusal's own body is built from
/// two strings with `serde_json::Value`'s infallible display, which is the
/// exact form [`canonical_json`] gives it, so this path cannot itself fail.
pub fn seal_v2(
    previous: &str,
    sequence: u64,
    at: Timestamp,
    decision: Decision,
) -> (Decision, String) {
    match canonical_decision(&decision) {
        Ok(body) => {
            let digest = digest_v2_over(previous, sequence, at, &body);
            (decision, digest)
        }
        Err(error) => {
            let reason = format!(
                "a {} decision was not recorded as itself: {}",
                decision.kind(),
                error.message()
            );
            let body = refusal_body(GATE_JOURNAL_ENCODING, &reason);
            let digest = digest_v2_over(previous, sequence, at, &body);
            (
                Decision::Refused {
                    gate: GATE_JOURNAL_ENCODING.to_string(),
                    reason,
                },
                digest,
            )
        }
    }
}

/// The canonical JSON of `Decision::Refused { gate, reason }`, built without
/// a fallible serialiser: keys in sorted order, strings escaped by
/// `serde_json::Value`'s `Display`, which is what [`canonical_json`] uses for
/// a string.
fn refusal_body(gate: &str, reason: &str) -> String {
    format!(
        "{{\"Refused\":{{\"gate\":{},\"reason\":{}}}}}",
        serde_json::Value::String(gate.to_string()),
        serde_json::Value::String(reason.to_string())
    )
}

/// The P1 record written beside a journal entry that carries an outcome
/// (ADR 0100 §5: fills, cancels, settlement records).
///
/// P2 carries every entry and may be shed under overload; P1 may not. This
/// is the copy that survives, and it carries the entry's own sequence and
/// digest so it can be matched to the chain it came from — an outcome that
/// cannot be placed on its chain is an outcome nobody can prove was not
/// inserted. Data only: the topic binding lives in `qip-events`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub cell: String,
    /// The persisted, monotonic session counter (ADR 0100 §4), not a start
    /// time: a second-granular start collides in a crash loop.
    pub session: u64,
    pub journal_sequence: u64,
    pub journal_digest: String,
    pub entry: JournalEntry,
}

/// A P1 continuity record spanning a run of non-outcome journal entries
/// (ADR 0100 §5's "`ChainSpan` continuity records").
///
/// Lets P1 stay chain-continuous without carrying every entry: the next P1
/// record's chain must start where `tail_digest` ends. Custody of the run,
/// not a proof of it — an unkeyed chain detects an edited byte, not a
/// rewrite that recomputed every hash (ADR 0043). `first_seq..=last_seq`,
/// both inclusive. Data only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpan {
    pub cell: String,
    pub session: u64,
    pub first_seq: u64,
    pub last_seq: u64,
    pub tail_digest: String,
}

/// An explicit gap a producer declares on a stream (ADR 0100 §4: "a shed
/// window is an explicit `Gap` record the broker accepts").
///
/// Silence and a gap read identically to a consumer that is not told, and
/// the difference is whether a replay says UNREPRODUCIBLE or quietly
/// reproduces something else. `from_seq..=to_seq`, both inclusive, and
/// `reason` names why only the producer knows. Data only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub stream: String,
    pub from_seq: u64,
    pub to_seq: u64,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hand_built_refusal_body_is_byte_for_byte_its_canonical_json() {
        // `seal_v2` hashes the refusal through `refusal_body`, and a verifier
        // recomputes it through `canonical_decision`. If the two ever differ
        // every refused entry reads as a chain break. A reason carrying a
        // quote, a backslash and a newline exercises the escaping, which is
        // where a hand-built form would drift first.
        let reason = "a \"quoted\" \\ reason\nwith a newline";
        let refused = Decision::Refused {
            gate: GATE_JOURNAL_ENCODING.to_string(),
            reason: reason.to_string(),
        };
        let canonical = canonical_decision(&refused).expect("a refusal is encodable");
        assert!(
            canonical.contains("\\\""),
            "premise: the reason needs escaping"
        );
        assert_eq!(refusal_body(GATE_JOURNAL_ENCODING, reason), canonical);
    }

    #[test]
    fn a_v1_entry_serialises_with_no_version_and_its_instant_as_text() {
        // A v1 entry sealed before v2 existed must come out byte-for-byte as
        // it went in, or the mirror's record of it changes shape on re-write.
        let entry = JournalEntry {
            sequence: 3,
            at: Timestamp::from_secs(1_700_000_000),
            decision: Decision::StrategyWithdrawn {
                strategy: "alpha".to_string(),
            },
            digest: "d".to_string(),
            version: ChainVersion::V1,
        };
        assert_eq!(
            serde_json::to_string(&entry).expect("serialises"),
            "{\"sequence\":3,\"at\":\"2023-11-14T22:13:20.000Z\",\"decision\":\
             {\"StrategyWithdrawn\":{\"strategy\":\"alpha\"}},\"digest\":\"d\"}"
        );
        let v2 = JournalEntry {
            version: ChainVersion::V2,
            at: Timestamp::from_nanos(1_700_000_000_000_000_001),
            ..entry
        };
        let text = serde_json::to_string(&v2).expect("serialises");
        assert_eq!(
            text,
            "{\"sequence\":3,\"at\":1700000000000000001,\"decision\":\
             {\"StrategyWithdrawn\":{\"strategy\":\"alpha\"}},\"digest\":\"d\",\"version\":\"v2\"}"
        );
        let back: JournalEntry = serde_json::from_str(&text).expect("reads back");
        assert_eq!(
            back, v2,
            "a v2 entry lost a digit of its instant on the way through"
        );
    }
}
