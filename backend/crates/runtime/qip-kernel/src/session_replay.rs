//! Blueprint §34.4's simulated rung: the producer of
//! [`SimulationEvidence`] the ladder never had.
//!
//! `qip_lifecycle::venue_ladder::SimulatedGate` is the ceiling rung of the
//! venue promotion ladder and it reads a [`SimulationEvidence`] — replayed
//! sessions, reconciliation breaks, and the clip a crossing cost is measured
//! at. **Nothing in any binary constructed one.** The gate could only ever
//! refuse, every venue stopped at the observed rung, and the rung's checks
//! were reachable only by a test handing them a literal. A ceiling rung that
//! no production path can reach is the same defect as a limit that cannot
//! fire: it reads as a control and is not one.
//!
//! This module closes that, in the runtime rather than in either service,
//! because the evidence is a comparison between two services' types —
//! `qip_execution_engine::session`'s recording and `qip_lifecycle`'s
//! judgement — and the runtime is where two services meet.
//!
//! # What a recorded session is here
//!
//! It is not a new file format and it is not a second copy of the event log.
//! A session is the traffic of **one pass at one venue**: the instructions
//! [`qip_execution_engine::oms::OrderManager::submit`] issued, and the
//! answers the broker returned, sealed when the stage that issued them ends.
//! `OrderManager` records both halves as they happen and keeps a bounded
//! window of sealed sessions; the recording exists because the platform was
//! already holding both facts and throwing one of them away.
//!
//! # What a replay refuses to treat as evidence
//!
//! The gate's three checks can all be satisfied by a venue that did nothing,
//! and each refusal below closes one of those doors. They are refusals rather
//! than adjustments because the alternative to each is a figure nobody took.
//!
//! * **A session with no fills is not replayed.** Nothing to reconcile
//!   reconciles perfectly. Five empty sessions would clear the rung on a
//!   venue that never traded — and would clear it *more easily* than a venue
//!   that actually trades, which is the same inversion that makes an absent
//!   latency read as zero the fastest venue on the ladder.
//! * **A saturated session is not replayed.** It holds a true prefix of a
//!   pass and not the whole of it, so it can find a break and can never
//!   establish the absence of one, and "reconciliation verified" is a claim
//!   about absence.
//! * **The same recording twice is one session.** Counted on the
//!   fingerprint, so a caller that replayed one recording five times gets
//!   one. Otherwise the figure the gate reads is the caller's loop counter.
//! * **No latency and no fee come out of a replay, ever.** They are not
//!   absent by omission: `SimulationEvidence` has no field for either, and
//!   this module deliberately does not widen the `VenueMeasurement` the
//!   observed rung reads. The desk's simulated broker stamps `at +
//!   settings.latency` on a fill rather than timing it, so a latency taken
//!   from a replayed fill would be the configured value with extra steps — a
//!   number checking itself. A venue's latency is measured at the adapter or
//!   it is [`None`].
//! * **A recording holding a fill the venue did not report as simulated
//!   produces no evidence at all.** Not a break among breaks: a live fill in
//!   a paper deployment is the alarm three other layers exist to prevent, and
//!   the ladder must not be able to weigh it against anything.
//!
//! # Why the reconciliation is a comparison and not a re-derivation
//!
//! The desk's side of each check is the instruction as issued — quantity,
//! side, venue, instant — and the venue's side is the fill as the broker
//! returned it, recorded *before* `Order::apply_fill` had the chance to
//! reject it. That ordering is the whole reason the count can be non-zero:
//! `OrderManager::submit` already throws an overfill away, so a replay over
//! the fills the book accepted would report zero breaks forever.
//!
//! # What this cannot do
//!
//! It cannot promote a venue, and nothing here names a rung.
//! `attempt_promotion` computes its target from the rung below and refuses
//! anything above `VENUE_PROMOTION_CEILING`, which is `GateStage::Paper`.
//! Evidence is an argument to a gate; there is no quantity of it that adds a
//! rung, and this module has no parameter that names one.

use qip_core::Decimal;
use qip_execution_engine::session::RecordedSession;
use qip_lifecycle::venue_ladder::SimulationEvidence;
use std::collections::{BTreeMap, BTreeSet};

/// A fill on an order the desk never issued at this venue.
pub const BREAK_UNSENT_FILL: &str = "unsent_fill";
/// Filled quantity beyond what the desk instructed.
pub const BREAK_OVERFILLED: &str = "overfilled";
/// A fill stamped at a venue other than the one the session belongs to.
pub const BREAK_MISROUTED_FILL: &str = "misrouted_fill";
/// A fill the venue stamped before the instruction that caused it.
pub const BREAK_FILL_BEFORE_INSTRUCTION: &str = "fill_before_instruction";
/// A fill on an order the venue had already reported a later fill for.
///
/// Per order rather than per session: two orders issued in one pass have
/// independent round trips and either may answer first, so a single sequence
/// across the pass would charge a break on every session the desk sent more
/// than one order in.
pub const BREAK_OUT_OF_SEQUENCE: &str = "out_of_sequence";
/// A fill with a non-positive quantity or price. Not a trade.
pub const BREAK_UNPRICED_FILL: &str = "unpriced_fill";

/// What the replay pass found.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayOutcome {
    /// The evidence the simulated rung reads, or [`None`] where the corpus
    /// cannot support any — which is a different statement from evidence
    /// that fell short, and the summary says which.
    pub evidence: Option<SimulationEvidence>,
    /// One line for the LEARN stage's record, always present. A replay that
    /// ran and said nothing is indistinguishable from one nobody wired in.
    pub summary: String,
    /// Findings an operator acts on. A reconciliation break is one; a corpus
    /// that is merely still small is not.
    pub problems: Vec<String>,
}

/// Replay the recorded sessions held for `venue`.
///
/// Takes the sessions by reference from the recorder that sealed them. There
/// is no constructor for a [`RecordedSession`] outside that recorder, so a
/// caller cannot hand this function a session the platform never had.
pub fn replay(sessions: &[&RecordedSession], venue: &str) -> ReplayOutcome {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    // Ordered by the break's own name so the summary reads the same way on
    // every pass. A report that reorders between cycles is a report nobody
    // can diff.
    let mut breaks: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut replayed = 0usize;
    let mut skipped_empty = 0usize;
    let mut skipped_saturated = 0usize;
    let mut duplicates = 0usize;
    let mut largest_clip = Decimal::ZERO;

    for session in sessions {
        if session.venue() != venue {
            continue;
        }
        if !seen.insert(session.fingerprint()) {
            duplicates += 1;
            continue;
        }
        if session.is_saturated() {
            skipped_saturated += 1;
            continue;
        }
        if session.fills().is_empty() {
            skipped_empty += 1;
            continue;
        }
        // Fail closed and fail whole. A recording holding a fill the venue
        // did not report as simulated is not evidence with a blemish on it;
        // it is the thing the paper-trading boundary exists to make
        // impossible, and the ladder must not be able to weigh it against
        // anything.
        if let Some(live) = session.fills().iter().find(|fill| !fill.simulated) {
            return ReplayOutcome {
                evidence: None,
                summary: format!(
                    "no session at {venue} is replayable: the recording holds fill {} which the \
                     venue did not report as simulated, and no promotion evidence is produced \
                     from a session containing one",
                    live.fill_id.as_str()
                ),
                problems: vec![format!(
                    "venue {venue} reported fill {} as not simulated; this deployment is paper \
                     trading and a non-simulated fill is an alarm rather than evidence",
                    live.fill_id.as_str()
                )],
            };
        }

        replayed += 1;
        let instructed = instructed_quantities(session);
        let mut filled: BTreeMap<&str, Decimal> = BTreeMap::new();
        let mut latest: BTreeMap<&str, qip_core::Timestamp> = BTreeMap::new();
        for fill in session.fills() {
            // Per order, and deliberately not across the session. Two orders
            // issued in the same pass have independent round trips and the
            // second may answer first at any venue; a single sequence over
            // the whole pass charged a break on every session the desk sent
            // more than one order in, which is a control that fires always.
            // Within one order the venue's own partial fills are sequential,
            // and one arriving before its predecessor means the recording and
            // the venue disagree about what happened when.
            match latest.entry(fill.order_id.as_str()) {
                std::collections::btree_map::Entry::Occupied(mut seen) => {
                    if fill.at < *seen.get() {
                        *breaks.entry(BREAK_OUT_OF_SEQUENCE).or_default() += 1;
                    } else {
                        seen.insert(fill.at);
                    }
                }
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(fill.at);
                }
            }
            if fill.quantity <= Decimal::ZERO || fill.price <= Decimal::ZERO {
                *breaks.entry(BREAK_UNPRICED_FILL).or_default() += 1;
            }
            if fill.venue != venue {
                *breaks.entry(BREAK_MISROUTED_FILL).or_default() += 1;
            }
            let Some(instruction) = instructed.get(fill.order_id.as_str()) else {
                *breaks.entry(BREAK_UNSENT_FILL).or_default() += 1;
                continue;
            };
            if fill.at < instruction.at {
                *breaks.entry(BREAK_FILL_BEFORE_INSTRUCTION).or_default() += 1;
            }
            let running = filled
                .entry(fill.order_id.as_str())
                .or_insert(Decimal::ZERO);
            // A running total that will not add is itself the finding. It
            // can only overflow past a quantity the desk instructed, so it is
            // charged as an overfill rather than dropped — a total that
            // stopped accumulating would make every later fill on the order
            // read as being within the instruction.
            match running.checked_add(fill.quantity) {
                Some(total) => {
                    *running = total;
                    if total > instruction.quantity {
                        *breaks.entry(BREAK_OVERFILLED).or_default() += 1;
                    }
                }
                None => *breaks.entry(BREAK_OVERFILLED).or_default() += 1,
            }
            if fill.quantity > largest_clip {
                largest_clip = fill.quantity;
            }
        }
    }

    let total: usize = breaks.values().sum();
    let skipped = describe_skipped(skipped_empty, skipped_saturated, duplicates);
    if replayed == 0 || largest_clip <= Decimal::ZERO {
        return ReplayOutcome {
            evidence: None,
            summary: format!(
                "no recorded session at {venue} can be replayed into promotion evidence: {}",
                if replayed == 0 {
                    format!("nothing was sealed that held a fill{skipped}")
                } else {
                    format!(
                        "{replayed} session(s) were replayed and none held a fill with a \
                         positive quantity, so there is no clip to measure a crossing cost \
                         at{skipped}"
                    )
                }
            ),
            problems: Vec::new(),
        };
    }

    let found = if breaks.is_empty() {
        "no reconciliation break".to_string()
    } else {
        let named: Vec<String> = breaks
            .iter()
            .map(|(name, count)| format!("{count} {name}"))
            .collect();
        named.join(", ")
    };
    ReplayOutcome {
        evidence: Some(SimulationEvidence {
            replayed_sessions: replayed,
            reconciliation_breaks: total,
            reference_clip: largest_clip,
        }),
        summary: format!(
            "{replayed} recorded session(s) at {venue} replayed against a reference clip of \
             {largest_clip}, finding {found}{skipped}"
        ),
        problems: breaks
            .iter()
            .map(|(name, count)| {
                format!(
                    "replaying {venue}'s recorded sessions found {count} {name} break(s): the \
                     desk's instructions and the venue's answers do not agree, and §34.4's \
                     simulated rung reads verified as none"
                )
            })
            .collect(),
    }
}

/// The desk's instruction per order id, taking the largest quantity where one
/// order was issued more than once in a pass.
///
/// The largest rather than the last, because the comparison this feeds must
/// not invent a break: an order re-issued at a smaller size would otherwise
/// make its earlier fills read as an overfill. A break found under the most
/// generous reading of the instruction is a break.
fn instructed_quantities(session: &RecordedSession) -> BTreeMap<&str, Instructed> {
    let mut instructed: BTreeMap<&str, Instructed> = BTreeMap::new();
    for instruction in session.instructions() {
        let entry = instructed
            .entry(instruction.order_id.as_str())
            .or_insert(Instructed {
                quantity: instruction.quantity,
                at: instruction.at,
            });
        if instruction.quantity > entry.quantity {
            entry.quantity = instruction.quantity;
        }
        if instruction.at < entry.at {
            entry.at = instruction.at;
        }
    }
    instructed
}

#[derive(Clone, Copy, Debug)]
struct Instructed {
    quantity: Decimal,
    at: qip_core::Timestamp,
}

/// What the pass declined to replay, said out loud.
///
/// A skipped session is not a problem and it is not nothing: a window full of
/// empty sessions and a window holding no sessions at all are different facts
/// about a deployment, and only one of them says the venue is being asked for
/// orders.
fn describe_skipped(empty: usize, saturated: usize, duplicates: usize) -> String {
    let mut parts = Vec::new();
    if empty > 0 {
        parts.push(format!(
            "{empty} sealed session(s) held no fill and reconcile nothing"
        ));
    }
    if saturated > 0 {
        parts.push(format!(
            "{saturated} sealed session(s) dropped entries at their bound and cannot establish \
             the absence of a break"
        ));
    }
    if duplicates > 0 {
        parts.push(format!(
            "{duplicates} session(s) repeated a fingerprint already replayed and were counted \
             once"
        ));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("; {}", parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::ids::{FillId, ObjectId, OrderId};
    use qip_core::{Decimal, Timestamp, dec};
    use qip_execution_engine::order::{Fill, Side};
    use qip_execution_engine::session::{RecordedInstruction, SessionRecorder};

    const VENUE: &str = "XVENUE";

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000 + offset)
    }

    fn instruction(order: &str, quantity: Decimal, when: Timestamp) -> RecordedInstruction {
        RecordedInstruction {
            order_id: OrderId::from_string(order),
            object_id: ObjectId::from_string("OBJ"),
            side: Side::Buy,
            quantity,
            venue: VENUE.to_string(),
            at: when,
        }
    }

    fn fill(id: &str, order: &str, quantity: Decimal, when: Timestamp) -> Fill {
        Fill {
            fill_id: FillId::from_string(id),
            order_id: OrderId::from_string(order),
            at: when,
            quantity,
            price: dec!("100"),
            costs: dec!("0.1"),
            venue: VENUE.to_string(),
            simulated: true,
        }
    }

    /// `count` clean sessions: one instruction, one fill that matches it.
    fn clean_recorder(count: usize) -> SessionRecorder {
        let mut recorder = SessionRecorder::default();
        for index in 0..count {
            let order = format!("order-{index}");
            let offset = index as i64;
            recorder.instruct(instruction(&order, dec!("10"), at(offset)));
            recorder.answer(
                VENUE,
                fill(&format!("fill-{index}"), &order, dec!("10"), at(offset)),
                at(offset),
            );
            recorder.close(VENUE, at(offset));
        }
        recorder
    }

    #[test]
    fn a_window_of_sessions_in_which_nothing_filled_produces_no_evidence_at_all() {
        // The refusal this module exists to make, and the one that decides
        // whether the ceiling rung is a control. Nothing to reconcile
        // reconciles perfectly, so five empty sessions would clear §34.4's
        // simulated rung for a venue that never traded — and clear it more
        // easily than a venue that actually trades does.
        let mut recorder = SessionRecorder::default();
        for index in 0..8 {
            recorder.instruct(instruction(
                &format!("order-{index}"),
                dec!("10"),
                at(index),
            ));
            recorder.close(VENUE, at(index));
        }
        let sessions = recorder.sessions(VENUE);
        assert_eq!(
            sessions.len(),
            8,
            "the premise: eight sessions were sealed, so the refusal below is about their \
             contents and not about an empty window"
        );
        let outcome = replay(&sessions, VENUE);
        assert!(
            outcome.evidence.is_none(),
            "a venue that filled nothing produced promotion evidence: {:?}",
            outcome.evidence
        );
        assert!(
            outcome.summary.contains("held no fill"),
            "the summary does not name why nothing was replayed: {}",
            outcome.summary
        );

        // The premise, and the half that makes this a gate rather than a
        // refusal generator: sessions that did fill do produce evidence.
        let traded = clean_recorder(8);
        let evidence = replay(&traded.sessions(VENUE), VENUE)
            .evidence
            .expect("sessions that filled are evidence");
        assert_eq!(evidence.replayed_sessions, 8);
        assert_eq!(evidence.reconciliation_breaks, 0);
    }

    #[test]
    fn one_recording_replayed_many_times_counts_once() {
        // The failure: satisfying a gate that asks for five replayed
        // sessions by handing it one recording five times. The figure would
        // then be the caller's loop counter rather than a measurement of the
        // corpus.
        let recorder = clean_recorder(1);
        let one = recorder.sessions(VENUE);
        assert_eq!(one.len(), 1, "the premise: one recording was sealed");
        let repeated: Vec<&RecordedSession> = std::iter::repeat_n(one[0], 9).collect();
        let outcome = replay(&repeated, VENUE);
        let evidence = outcome.evidence.expect("one session is still a session");
        assert_eq!(
            evidence.replayed_sessions, 1,
            "one recording presented nine times was counted as {} sessions",
            evidence.replayed_sessions
        );
        assert!(
            outcome.summary.contains("repeated a fingerprint"),
            "the summary does not say the repeats were discarded: {}",
            outcome.summary
        );
        // The premise for the dedup itself: nine genuinely different
        // recordings do count nine.
        let nine = clean_recorder(9);
        let distinct = replay(&nine.sessions(VENUE), VENUE)
            .evidence
            .expect("nine recordings are evidence");
        assert_eq!(distinct.replayed_sessions, 9);
    }

    #[test]
    fn a_fill_beyond_what_the_desk_instructed_is_a_reconciliation_break() {
        // The break the recording is shaped to make findable at all. The OMS
        // throws an overfill away at the seam, so a replay over the fills the
        // *book* accepted would report zero breaks on every session forever.
        let mut recorder = SessionRecorder::default();
        recorder.instruct(instruction("order-1", dec!("10"), at(0)));
        recorder.answer(VENUE, fill("fill-1", "order-1", dec!("10"), at(0)), at(0));
        recorder.answer(VENUE, fill("fill-2", "order-1", dec!("4"), at(1)), at(1));
        recorder.close(VENUE, at(1));
        let outcome = replay(&recorder.sessions(VENUE), VENUE);
        let evidence = outcome.evidence.expect("a session that filled is evidence");
        assert_eq!(
            evidence.reconciliation_breaks, 1,
            "a venue that filled 14 on an instruction of 10 reconciled: {evidence:?}"
        );
        assert!(
            outcome
                .problems
                .iter()
                .any(|problem| problem.contains(BREAK_OVERFILLED)),
            "the break was counted and not named: {:?}",
            outcome.problems
        );
        // The premise: the same two fills within the instruction reconcile,
        // so the break above is about the excess and not about two fills.
        let mut within = SessionRecorder::default();
        within.instruct(instruction("order-1", dec!("14"), at(0)));
        within.answer(VENUE, fill("fill-1", "order-1", dec!("10"), at(0)), at(0));
        within.answer(VENUE, fill("fill-2", "order-1", dec!("4"), at(1)), at(1));
        within.close(VENUE, at(1));
        let clean = replay(&within.sessions(VENUE), VENUE)
            .evidence
            .expect("evidence");
        assert_eq!(clean.reconciliation_breaks, 0);
    }

    #[test]
    fn a_fill_on_an_order_the_desk_never_issued_is_a_reconciliation_break() {
        let mut recorder = SessionRecorder::default();
        recorder.instruct(instruction("order-1", dec!("10"), at(0)));
        recorder.answer(VENUE, fill("fill-1", "order-1", dec!("10"), at(0)), at(0));
        recorder.answer(VENUE, fill("fill-9", "order-9", dec!("5"), at(1)), at(1));
        recorder.close(VENUE, at(1));
        let outcome = replay(&recorder.sessions(VENUE), VENUE);
        let evidence = outcome.evidence.expect("evidence");
        assert_eq!(evidence.reconciliation_breaks, 1);
        assert!(
            outcome
                .problems
                .iter()
                .any(|problem| problem.contains(BREAK_UNSENT_FILL)),
            "{:?}",
            outcome.problems
        );
    }

    #[test]
    fn a_recording_holding_a_fill_the_venue_did_not_call_simulated_produces_no_evidence() {
        // Not a break among breaks. A non-simulated fill in a paper
        // deployment is what the three paper layers exist to prevent, and a
        // ladder able to weigh one against clean sessions would be a fourth
        // way in that none of the three watches.
        let mut recorder = SessionRecorder::default();
        for index in 0..8 {
            let order = format!("order-{index}");
            recorder.instruct(instruction(&order, dec!("10"), at(index)));
            let mut answer = fill(&format!("fill-{index}"), &order, dec!("10"), at(index));
            if index == 7 {
                answer.simulated = false;
            }
            recorder.answer(VENUE, answer, at(index));
            recorder.close(VENUE, at(index));
        }
        let sessions = recorder.sessions(VENUE);
        assert_eq!(sessions.len(), 8, "the premise: eight sessions were sealed");
        let outcome = replay(&sessions, VENUE);
        assert!(
            outcome.evidence.is_none(),
            "a corpus holding a live fill produced promotion evidence: {:?}",
            outcome.evidence
        );
        assert!(
            outcome
                .problems
                .iter()
                .any(|problem| problem.contains("not simulated")),
            "the live fill was discarded without being raised: {:?}",
            outcome.problems
        );
        // The premise: the same eight sessions with every fill simulated do
        // produce evidence, so the refusal is about the flag.
        let clean = clean_recorder(8);
        assert!(replay(&clean.sessions(VENUE), VENUE).evidence.is_some());
    }

    #[test]
    fn a_saturated_session_is_not_replayed_because_it_cannot_show_the_absence_of_a_break() {
        let mut recorder = SessionRecorder::default();
        recorder.instruct(instruction("order-1", dec!("10"), at(0)));
        for index in 0..(qip_execution_engine::session::SESSION_ENTRY_LIMIT + 5) {
            recorder.answer(
                VENUE,
                fill(&format!("fill-{index}"), "order-1", dec!("1"), at(0)),
                at(0),
            );
        }
        recorder.close(VENUE, at(0));
        let sessions = recorder.sessions(VENUE);
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].is_saturated(), "the premise: it saturated");
        let outcome = replay(&sessions, VENUE);
        assert!(
            outcome.evidence.is_none(),
            "a truncated recording was replayed as a complete one"
        );
        assert!(
            outcome.summary.contains("dropped entries"),
            "{}",
            outcome.summary
        );
    }

    #[test]
    fn the_reference_clip_is_the_largest_clip_the_venue_actually_absorbed() {
        // A cost figure with no size attached is not a figure, and the size
        // has to be one the venue was actually asked for. Defaulting it to
        // zero would price a decentralised venue's crossing cost at a clip
        // nobody ever sent, which is the cheapest clip there is.
        let mut recorder = SessionRecorder::default();
        recorder.instruct(instruction("order-1", dec!("30"), at(0)));
        recorder.answer(VENUE, fill("fill-1", "order-1", dec!("7"), at(0)), at(0));
        recorder.answer(VENUE, fill("fill-2", "order-1", dec!("23"), at(1)), at(1));
        recorder.close(VENUE, at(1));
        let evidence = replay(&recorder.sessions(VENUE), VENUE)
            .evidence
            .expect("evidence");
        assert_eq!(
            evidence.reference_clip,
            dec!("23"),
            "the reference clip is not the largest clip the venue absorbed"
        );
        assert_eq!(evidence.reconciliation_breaks, 0);
    }

    #[test]
    fn a_later_fill_on_one_order_arriving_before_its_predecessor_is_a_break_and_two_orders_are_not()
    {
        // Both halves matter and the second is the one that was wrong first:
        // asserting a single time sequence across a whole pass charged a
        // break on every session in which the desk sent two orders, because
        // two round trips answer in whatever order they finish. A control
        // that fires always is how an operator learns that findings are
        // noise.
        let mut crossed = SessionRecorder::default();
        crossed.instruct(instruction("order-1", dec!("20"), at(0)));
        crossed.answer(VENUE, fill("fill-1", "order-1", dec!("10"), at(5)), at(5));
        crossed.answer(VENUE, fill("fill-2", "order-1", dec!("10"), at(2)), at(5));
        crossed.close(VENUE, at(5));
        let evidence = replay(&crossed.sessions(VENUE), VENUE)
            .evidence
            .expect("evidence");
        assert_eq!(
            evidence.reconciliation_breaks, 1,
            "one order's fills ran backwards and reconciled"
        );

        // Two orders whose answers cross. Not a break.
        let mut interleaved = SessionRecorder::default();
        interleaved.instruct(instruction("order-1", dec!("10"), at(0)));
        interleaved.instruct(instruction("order-2", dec!("10"), at(0)));
        interleaved.answer(VENUE, fill("fill-1", "order-1", dec!("10"), at(5)), at(5));
        interleaved.answer(VENUE, fill("fill-2", "order-2", dec!("10"), at(2)), at(5));
        interleaved.close(VENUE, at(5));
        let clean = replay(&interleaved.sessions(VENUE), VENUE)
            .evidence
            .expect("evidence");
        assert_eq!(
            clean.reconciliation_breaks, 0,
            "two orders answering out of order were charged a break"
        );
    }

    #[test]
    fn a_session_at_another_venue_is_not_evidence_about_this_one() {
        let mut recorder = SessionRecorder::default();
        recorder.instruct(instruction("order-1", dec!("10"), at(0)));
        recorder.answer(VENUE, fill("fill-1", "order-1", dec!("10"), at(0)), at(0));
        recorder.close(VENUE, at(0));
        // Everything the recorder holds, not just this venue's.
        let outcome = replay(&recorder.all(), "XOTHER");
        assert!(
            outcome.evidence.is_none(),
            "one venue's sessions were counted as another's"
        );
        // The premise: the same window does answer for the venue it belongs to.
        assert!(replay(&recorder.all(), VENUE).evidence.is_some());
    }
}
