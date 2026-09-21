//! What a recorded session is, and who is allowed to say one happened.
//!
//! Blueprint §34.4's simulated rung — the ceiling of the venue promotion
//! ladder — reads "traded in sim against recorded and replayed data,
//! reconciliation verified". `qip_lifecycle::venue_ladder::SimulationEvidence`
//! has existed for that rung since the ladder was built and **nothing in any
//! binary has ever constructed one**, because the platform recorded no session
//! to replay. The ceiling rung was therefore reachable only by a test handing
//! the gate a literal, which is the shape of a control that reads as
//! protection and is not.
//!
//! This module is the recording half. It holds, per venue, the instructions
//! the desk issued and the answers the venue gave back, seals them into a
//! [`RecordedSession`] when the pass that issued them ends, and keeps a
//! bounded window of those. `qip-kernel`'s `session_replay` is the replaying
//! half; this one judges nothing.
//!
//! # Why this records the venue's answer and not the book's
//!
//! [`crate::oms::OrderManager::submit`] already reconciles once, at the seam:
//! it calls `Order::apply_fill` on each fill and pushes a break when the order
//! refuses one. So by the time a `SubmissionResult` exists, `result.fills` is
//! the set of fills the **book accepted** — an overfill has already been
//! thrown away. A recorder fed that set could never find an overfill on
//! replay, and would report zero breaks on every session forever: a number
//! checking itself, which is precisely the fallacy §34.4's rungs exist to
//! refuse. So [`SessionRecorder::answer`] is called with the fill the broker
//! returned, before the book has had a chance to reject it, and the replay
//! compares the desk's instruction against the venue's raw answer.
//!
//! # What this refuses to record
//!
//! * **A session in which nothing happened.** Sealing one would put an empty
//!   record in the window, and an empty session reconciles perfectly. Five of
//!   them would clear the simulated rung on the strength of a venue that never
//!   traded.
//! * **The same session twice.** A recording is identified by a fingerprint
//!   over its own contents ([`RecordedSession::fingerprint`]), and a
//!   fingerprint already in the window is refused. Without this, a caller that
//!   sealed one recording five times would satisfy a gate asking for five
//!   replayed sessions — the loop counter passing as a measurement of the
//!   corpus. This is the same discipline the event bus applies to a
//!   re-delivering feed, for the same reason.
//! * **A fill it has already seen.** `SubmissionResult::fills` carries the
//!   order's *accumulated* fills, so a resubmitted order re-presents its
//!   earlier ones; dedup is on [`crate::order::Fill::fill_id`], so a fill
//!   recorded twice is one fill and the fingerprint over the session is
//!   stable.
//!
//! # Bounds
//!
//! The window is a fixed number of sealed sessions ([`SESSION_WINDOW`] by
//! default) and the oldest is evicted when a new one arrives. An open session
//! is bounded too, by [`SESSION_ENTRY_LIMIT`] instructions and the same number
//! of fills: a venue that answers without ever being asked, or a pass that
//! never ends, must not become an unbounded buffer in the order path. Past the
//! limit the session stops accepting entries and is marked saturated, and a
//! saturated session is sealed like any other — what it holds is true, there
//! is simply more of it that was not kept, and the replay says so rather than
//! presenting a truncated session as a complete one.

use crate::order::{Fill, Side};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Sealed sessions kept per recorder.
///
/// Thirty-two rather than the five the simulated rung asks for, so that a
/// window still holds a corpus after the oldest sessions have aged out, and
/// small enough that the whole window is a few thousand entries in the worst
/// case. A window sized at the gate's own minimum would mean every eviction
/// took the venue below its evidence.
pub const SESSION_WINDOW: usize = 32;

/// Instructions, and separately fills, one open session will hold.
///
/// This sits in the order path. A bound that is large enough never to be
/// reached in a cycle and small enough to be stated is the whole requirement;
/// what it must not be is absent.
pub const SESSION_ENTRY_LIMIT: usize = 512;

/// One instruction the desk issued to a venue, as issued.
///
/// The desk's half of the reconciliation. Deliberately not derived from any
/// fill: the point of the comparison is that the two sides were recorded
/// independently, and a quantity read back out of the answer would make the
/// check a tautology.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedInstruction {
    pub order_id: OrderId,
    pub object_id: ObjectId,
    pub side: Side,
    /// What the desk asked for. Always positive.
    pub quantity: Decimal,
    /// The venue it was issued to, as the broker names itself.
    pub venue: String,
    /// When the desk issued it.
    pub at: Timestamp,
}

impl RecordedInstruction {
    /// The canonical line this instruction contributes to a fingerprint.
    fn canonical(&self) -> String {
        format!(
            "i|{}|{}|{}|{}|{}|{}\n",
            self.order_id.as_str(),
            self.object_id.as_str(),
            self.side.as_str(),
            self.quantity,
            self.venue,
            self.at.as_nanos()
        )
    }
}

/// The canonical line a fill contributes to a fingerprint.
fn canonical_fill(fill: &Fill) -> String {
    format!(
        "f|{}|{}|{}|{}|{}|{}|{}|{}\n",
        fill.fill_id.as_str(),
        fill.order_id.as_str(),
        fill.at.as_nanos(),
        fill.quantity,
        fill.price,
        fill.costs,
        fill.venue,
        fill.simulated
    )
}

/// One venue's traffic across one pass, sealed.
///
/// There is no public constructor. A session exists only because
/// [`SessionRecorder`] observed one happening and sealed it, so a fingerprint
/// cannot be attached to contents it was not computed from, and a caller
/// cannot hand the replay a session the platform never had.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedSession {
    venue: String,
    opened_at: Timestamp,
    closed_at: Timestamp,
    instructions: Vec<RecordedInstruction>,
    fills: Vec<Fill>,
    saturated: bool,
    fingerprint: String,
}

impl RecordedSession {
    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub const fn opened_at(&self) -> Timestamp {
        self.opened_at
    }

    pub const fn closed_at(&self) -> Timestamp {
        self.closed_at
    }

    /// The desk's half, in the order the desk issued it.
    pub fn instructions(&self) -> &[RecordedInstruction] {
        &self.instructions
    }

    /// The venue's half, in the order the venue reported it.
    ///
    /// A `Vec` and not a set: the sequence is part of the recording, and a
    /// replay that reorders is not a replay.
    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }

    /// Whether the open session hit [`SESSION_ENTRY_LIMIT`] and stopped
    /// accepting entries.
    ///
    /// A saturated session holds a true prefix of what happened and not the
    /// whole of it, so a reconciliation over it can find a break and cannot
    /// establish the absence of one — the replay refuses to count it.
    pub const fn is_saturated(&self) -> bool {
        self.saturated
    }

    /// A stable hash over everything this session holds.
    ///
    /// The identity a window dedups on. Computed from the contents rather
    /// than from a counter, so the same session presented twice is refused
    /// however it arrived.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// A session being written.
#[derive(Clone, Debug)]
struct OpenSession {
    venue: String,
    opened_at: Timestamp,
    instructions: Vec<RecordedInstruction>,
    fills: Vec<Fill>,
    fill_ids: BTreeSet<String>,
    saturated: bool,
}

/// What [`SessionRecorder::close`] did.
///
/// Returned rather than swallowed because two of the three arms are findings:
/// a pass that issued orders and sealed nothing, and a pass whose recording
/// the window had already seen, are both worth a line on the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealOutcome {
    /// A session was sealed into the window.
    Sealed,
    /// There was no open session, or it held neither an instruction nor a
    /// fill. Nothing happened at this venue, which is not a recording.
    NothingHappened,
    /// The window already holds a session with this fingerprint. The
    /// recording is discarded rather than counted a second time.
    AlreadyRecorded,
}

/// The bounded window of sealed sessions, per venue.
#[derive(Clone, Debug)]
pub struct SessionRecorder {
    open: BTreeMap<String, OpenSession>,
    /// Keyed on the seal sequence, so iteration is in the order sessions were
    /// sealed. A map keyed on the fingerprint would iterate in hash order,
    /// and a replay that reorders is not a replay.
    closed: BTreeMap<u64, RecordedSession>,
    fingerprints: BTreeSet<String>,
    sequence: u64,
    capacity: usize,
}

impl Default for SessionRecorder {
    fn default() -> Self {
        Self {
            open: BTreeMap::new(),
            closed: BTreeMap::new(),
            fingerprints: BTreeSet::new(),
            sequence: 0,
            capacity: SESSION_WINDOW,
        }
    }
}

impl SessionRecorder {
    /// A recorder holding at most `capacity` sealed sessions.
    ///
    /// Refuses zero rather than substituting a default. A recorder that keeps
    /// nothing would report "no session has ever been replayed" forever, which
    /// reads identically to a platform that never traded, and the caller that
    /// asked for zero would never learn it had asked for something incoherent.
    pub fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a session recorder holding zero sessions records nothing; give it a capacity of \
                 at least one, or do not construct one",
            ));
        }
        Ok(Self {
            capacity,
            ..Self::default()
        })
    }

    /// How many sealed sessions the window holds.
    pub fn len(&self) -> usize {
        self.closed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.closed.is_empty()
    }

    /// Record that the desk issued an instruction to a venue.
    pub fn instruct(&mut self, instruction: RecordedInstruction) {
        let venue = instruction.venue.clone();
        let at = instruction.at;
        let session = self.session_mut(&venue, at);
        if session.instructions.len() >= SESSION_ENTRY_LIMIT {
            session.saturated = true;
            return;
        }
        session.instructions.push(instruction);
    }

    /// Record the answer a venue gave, exactly as it gave it.
    ///
    /// Takes `simulated` from the caller rather than from `fill.simulated`,
    /// because the broker's own word on that is the one thing the OMS already
    /// refuses to take: `OrderManager::submit` overwrites the flag from
    /// `Broker::is_simulated` and this recorder is fed the corrected fill.
    ///
    /// A fill for a venue with no open session opens one. That is deliberate:
    /// a venue answering an instruction nobody issued is the single most
    /// important thing a reconciliation can find, and a recorder that dropped
    /// it would guarantee the finding could never be made.
    pub fn answer(&mut self, venue: &str, fill: Fill, at: Timestamp) {
        let session = self.session_mut(venue, at);
        if session.fill_ids.contains(fill.fill_id.as_str()) {
            return;
        }
        if session.fills.len() >= SESSION_ENTRY_LIMIT {
            session.saturated = true;
            return;
        }
        session.fill_ids.insert(fill.fill_id.as_str().to_string());
        session.fills.push(fill);
    }

    fn session_mut(&mut self, venue: &str, at: Timestamp) -> &mut OpenSession {
        self.open
            .entry(venue.to_string())
            .or_insert_with(|| OpenSession {
                venue: venue.to_string(),
                opened_at: at,
                instructions: Vec::new(),
                fills: Vec::new(),
                fill_ids: BTreeSet::new(),
                saturated: false,
            })
    }

    /// Seal the open session at `venue`.
    pub fn close(&mut self, venue: &str, at: Timestamp) -> SealOutcome {
        // A pass in which nothing happened is not a recording, and there is
        // exactly one test for that rather than two: a session exists in
        // `open` only because [`Self::instruct`] or [`Self::answer`] put it
        // there, and each of those pushes its entry immediately — the
        // saturation arms are reached only past [`SESSION_ENTRY_LIMIT`],
        // which a session one entry old has not reached. So an open session
        // always holds at least one entry, and a second guard on emptiness
        // here would be a branch nothing could take, which this repository's
        // own standing example says is worse than no guard: it reads as a
        // control and can never fire.
        let Some(open) = self.open.remove(venue) else {
            return SealOutcome::NothingHappened;
        };
        let mut canonical = format!(
            "v|{}|{}|{}|{}\n",
            open.venue,
            open.opened_at.as_nanos(),
            at.as_nanos(),
            open.saturated
        );
        for instruction in &open.instructions {
            canonical.push_str(&instruction.canonical());
        }
        for fill in &open.fills {
            canonical.push_str(&canonical_fill(fill));
        }
        let fingerprint = sha256_hex(canonical.as_bytes());
        if self.fingerprints.contains(&fingerprint) {
            return SealOutcome::AlreadyRecorded;
        }
        let session = RecordedSession {
            venue: open.venue,
            opened_at: open.opened_at,
            closed_at: at,
            instructions: open.instructions,
            fills: open.fills,
            saturated: open.saturated,
            fingerprint: fingerprint.clone(),
        };
        while self.closed.len() >= self.capacity {
            let Some((oldest, evicted)) = self.closed.pop_first() else {
                break;
            };
            let _ = oldest;
            self.fingerprints.remove(evicted.fingerprint());
        }
        self.sequence += 1;
        self.fingerprints.insert(fingerprint);
        self.closed.insert(self.sequence, session);
        SealOutcome::Sealed
    }

    /// Seal every open session. The end of a pass, whatever it touched.
    pub fn close_all(&mut self, at: Timestamp) -> Vec<(String, SealOutcome)> {
        let venues: Vec<String> = self.open.keys().cloned().collect();
        venues
            .into_iter()
            .map(|venue| {
                let outcome = self.close(&venue, at);
                (venue, outcome)
            })
            .collect()
    }

    /// The sealed sessions for one venue, oldest first.
    pub fn sessions(&self, venue: &str) -> Vec<&RecordedSession> {
        self.closed
            .values()
            .filter(|session| session.venue == venue)
            .collect()
    }

    /// Every sealed session, oldest first.
    pub fn all(&self) -> Vec<&RecordedSession> {
        self.closed.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_core::ids::FillId;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn instruction(order: &str, quantity: Decimal) -> RecordedInstruction {
        RecordedInstruction {
            order_id: OrderId::from_string(order),
            object_id: ObjectId::from_string("OBJ"),
            side: Side::Buy,
            quantity,
            venue: "XVENUE".to_string(),
            at: now(),
        }
    }

    fn fill(id: &str, order: &str, quantity: Decimal) -> Fill {
        Fill {
            fill_id: FillId::from_string(id),
            order_id: OrderId::from_string(order),
            at: now(),
            quantity,
            price: dec!("100"),
            costs: dec!("0.1"),
            venue: "XVENUE".to_string(),
            simulated: true,
        }
    }

    #[test]
    fn a_pass_in_which_nothing_happened_is_not_sealed_as_a_session() {
        // The refusal that keeps an empty recording out of the window. An
        // empty session reconciles perfectly, so five of them would clear
        // §34.4's simulated rung for a venue that never traded.
        let mut recorder = SessionRecorder::default();
        assert_eq!(
            recorder.close("XVENUE", now()),
            SealOutcome::NothingHappened
        );
        assert!(recorder.is_empty());

        // The premise: the same recorder does seal a pass in which something
        // did happen, so the refusal above is about emptiness and not about
        // the recorder being inert.
        recorder.instruct(instruction("order-1", dec!("10")));
        recorder.answer("XVENUE", fill("fill-1", "order-1", dec!("10")), now());
        assert_eq!(recorder.close("XVENUE", now()), SealOutcome::Sealed);
        assert_eq!(recorder.len(), 1);
    }

    #[test]
    fn the_same_recording_presented_twice_is_one_session_and_not_two() {
        // The failure this prevents: a caller satisfying a gate that asks for
        // five replayed sessions by sealing one recording five times. The
        // count would then be the loop counter rather than a measurement of
        // the corpus.
        let mut recorder = SessionRecorder::default();
        for _ in 0..5 {
            recorder.instruct(instruction("order-1", dec!("10")));
            recorder.answer("XVENUE", fill("fill-1", "order-1", dec!("10")), now());
            recorder.close("XVENUE", now());
        }
        assert_eq!(
            recorder.len(),
            1,
            "the window holds {} copies of one recording",
            recorder.len()
        );
        // The premise: a genuinely different session does seal, so the
        // dedup above is keyed on the contents and not on the venue.
        recorder.instruct(instruction("order-2", dec!("10")));
        recorder.answer("XVENUE", fill("fill-2", "order-2", dec!("10")), now());
        assert_eq!(recorder.close("XVENUE", now()), SealOutcome::Sealed);
        assert_eq!(recorder.len(), 2);
    }

    #[test]
    fn a_fill_the_recorder_has_already_seen_is_one_fill() {
        // `SubmissionResult::fills` carries the order's accumulated fills, so
        // a resubmitted order re-presents its earlier ones. Recording them
        // twice would show the venue overfilling every order it ever
        // partially filled.
        let mut recorder = SessionRecorder::default();
        recorder.instruct(instruction("order-1", dec!("10")));
        for _ in 0..4 {
            recorder.answer("XVENUE", fill("fill-1", "order-1", dec!("4")), now());
        }
        recorder.close("XVENUE", now());
        let sealed = recorder.sessions("XVENUE");
        assert_eq!(sealed.len(), 1);
        assert_eq!(
            sealed[0].fills().len(),
            1,
            "one fill was recorded {} times",
            sealed[0].fills().len()
        );
    }

    #[test]
    fn a_fill_at_a_venue_that_was_never_instructed_is_still_recorded() {
        // The most important thing a reconciliation can find is a fill on an
        // order nobody sent. A recorder that dropped it because no session
        // was open would guarantee the finding could never be made.
        let mut recorder = SessionRecorder::default();
        recorder.answer("XGHOST", fill("fill-9", "order-9", dec!("1")), now());
        assert_eq!(recorder.close("XGHOST", now()), SealOutcome::Sealed);
        let sealed = recorder.sessions("XGHOST");
        assert_eq!(sealed.len(), 1);
        assert!(sealed[0].instructions().is_empty());
        assert_eq!(sealed[0].fills().len(), 1);
    }

    #[test]
    fn the_window_is_bounded_and_evicts_the_oldest_session_first() {
        let mut recorder = SessionRecorder::new(3).expect("a positive capacity");
        for index in 0..10u32 {
            recorder.instruct(instruction(&format!("order-{index}"), dec!("10")));
            recorder.answer(
                "XVENUE",
                fill(
                    &format!("fill-{index}"),
                    &format!("order-{index}"),
                    dec!("10"),
                ),
                now(),
            );
            recorder.close("XVENUE", now());
        }
        assert_eq!(recorder.len(), 3, "the window grew past its capacity");
        let sealed = recorder.sessions("XVENUE");
        assert_eq!(
            sealed[0].instructions()[0].order_id.as_str(),
            "order-7",
            "the window kept the oldest sessions rather than the newest"
        );
        assert_eq!(sealed[2].instructions()[0].order_id.as_str(), "order-9");
    }

    #[test]
    fn a_recorder_asked_to_keep_nothing_is_refused_rather_than_given_a_default() {
        let refused = SessionRecorder::new(0);
        let error = refused.expect_err("a capacity of zero records nothing");
        assert!(
            error.message().contains("at least one"),
            "the refusal does not name what to do instead: {}",
            error.message()
        );
        // The premise: one is admitted, so the refusal is about zero.
        assert!(SessionRecorder::new(1).is_ok());
    }

    #[test]
    fn an_open_session_stops_accepting_entries_rather_than_growing_without_bound() {
        // This sits in the order path. A venue answering without ever being
        // asked must not become an unbounded buffer.
        let mut recorder = SessionRecorder::default();
        for index in 0..(SESSION_ENTRY_LIMIT + 50) {
            recorder.answer(
                "XVENUE",
                fill(&format!("fill-{index}"), "order-1", dec!("1")),
                now(),
            );
        }
        recorder.close("XVENUE", now());
        let sealed = recorder.sessions("XVENUE");
        assert_eq!(sealed[0].fills().len(), SESSION_ENTRY_LIMIT);
        assert!(
            sealed[0].is_saturated(),
            "a session that dropped entries did not say so"
        );
    }
}
