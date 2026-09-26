//! ADR 0100 §8: records the exogenous inputs a pass actually applied — tape
//! events, control frames and clock ticks — so replay can re-drive `run_pass`
//! from them verbatim. SLICE-24.
//!
//! [`RecordedInputs`] runs on the decision thread, so it hands each pass's
//! [`PassInputs`] to the spool writer the same way the mirror hands a journal
//! batch: an offer that never waits. Unlike the journal, the inputs have no
//! other home to stay in when the channel is full, so they wait in a backlog
//! here, bounded by the spool's own exhaustion line from
//! [`Thresholds`] — the journal's thresholds, so an operator sizes one budget
//! and not two.
//!
//! # Overflow is a gap, never a silence (red-team M8)
//!
//! A pass whose inputs would take the backlog past its bound is dropped and
//! its number is added to an [`InputGap`] queued behind what the backlog
//! still holds. The writer writes that gap on P2, where the missing passes
//! would have been, and on P1, where it cannot be shed. A replay that meets
//! it refuses the window as unreproducible; a replay that met silence would
//! re-drive the passes either side and report the pass in between as
//! reproduced when nothing about it was recorded.
//!
//! Control frames and clock ticks reach this module inside the marker
//! (`PassMarker::control`, `now_ns`) and the readings the pass took inside
//! `PassMarker::readings`; the marker is handed over as the pass built it
//! and converted by nothing here (SLICE-55 builds it).

use crate::event_fabric::pressure::Thresholds;
use crate::event_fabric::telemetry::OutboxTelemetry;
use crate::event_fabric::writer::{Handoff, HandoffSender, InputGap, Offer, PassInputs};
use qip_core::error::{Error, Result};
use std::collections::VecDeque;
use std::sync::Arc;

/// What [`RecordedInputs::record`] did with one pass's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Recorded {
    /// The writer's channel took them.
    HandedOff,
    /// The channel was full or held back; they wait in the backlog.
    Queued,
    /// The backlog was at its bound; the pass is now inside a gap.
    Dropped,
}

/// The decision thread's recorder of what each pass applied.
#[derive(Debug)]
pub struct RecordedInputs {
    sender: HandoffSender,
    thresholds: Thresholds,
    telemetry: Arc<OutboxTelemetry>,
    /// Handoffs not yet taken, in the order they must land, each with the
    /// bytes it counts against the bound. A gap counts nothing: it is small,
    /// and consecutive drops extend one gap rather than adding another, so
    /// there is at most one gap per queued pass plus one.
    backlog: VecDeque<(Handoff, u64)>,
    backlog_bytes: u64,
}

impl RecordedInputs {
    pub fn new(
        sender: HandoffSender,
        thresholds: Thresholds,
        telemetry: Arc<OutboxTelemetry>,
    ) -> Self {
        Self {
            sender,
            thresholds,
            telemetry,
            backlog: VecDeque::new(),
            backlog_bytes: 0,
        }
    }

    /// Bytes the backlog holds.
    pub fn backlog_bytes(&self) -> u64 {
        self.backlog_bytes
    }

    /// Handoffs the backlog holds, gaps included.
    pub fn backlog_len(&self) -> usize {
        self.backlog.len()
    }

    /// Record one pass's inputs: offer the backlog first, in order, then
    /// these; queue them if the channel is full; drop them into a gap if the
    /// backlog is at its bound.
    ///
    /// An `Err` means the writer has gone, or the inputs would not
    /// serialise to be measured — both are faults the caller must see, not
    /// drops this recorder may hide.
    pub fn record(&mut self, inputs: PassInputs) -> Result<Recorded> {
        let bytes = measure(&inputs)?;
        self.pump()?;
        if !self.backlog.is_empty() {
            return Ok(self.enqueue_or_drop(Box::new(inputs), bytes));
        }
        match self.sender.offer(Handoff::Pass(Box::new(inputs))) {
            Offer::Accepted => Ok(Recorded::HandedOff),
            Offer::Full(Handoff::Pass(inputs)) => Ok(self.enqueue_or_drop(inputs, bytes)),
            Offer::Full(_) => Err(Error::invalid(
                "the handoff channel returned a different value than it was offered",
            )),
            Offer::Closed(_) => Err(closed()),
        }
    }

    /// Offer the backlog to the writer, in order, until the channel is full.
    pub fn pump(&mut self) -> Result<()> {
        while let Some((handoff, bytes)) = self.backlog.pop_front() {
            match self.sender.offer(handoff) {
                Offer::Accepted => {
                    self.backlog_bytes = self.backlog_bytes.saturating_sub(bytes);
                }
                Offer::Full(back) => {
                    self.backlog.push_front((back, bytes));
                    return Ok(());
                }
                Offer::Closed(back) => {
                    self.backlog.push_front((back, bytes));
                    return Err(closed());
                }
            }
        }
        Ok(())
    }

    fn enqueue_or_drop(&mut self, inputs: Box<PassInputs>, bytes: u64) -> Recorded {
        let bound = self.thresholds.exhaust_bytes();
        if let Some(total) = self.backlog_bytes.checked_add(bytes)
            && total <= bound
        {
            self.backlog.push_back((Handoff::Pass(inputs), bytes));
            self.backlog_bytes = total;
            return Recorded::Queued;
        }
        let pass = inputs.marker.pass;
        let events = inputs.applied.len() as u64;
        let now_ns = inputs.marker.now_ns;
        if let Some((Handoff::InputGap(gap), _)) = self.backlog.back_mut() {
            gap.from_pass = gap.from_pass.min(pass);
            gap.to_pass = gap.to_pass.max(pass);
            gap.passes = gap.passes.saturating_add(1);
            gap.events = gap.events.saturating_add(events);
            gap.last_now_ns = gap.last_now_ns.max(now_ns);
        } else {
            self.telemetry.input_gap();
            self.backlog.push_back((
                Handoff::InputGap(InputGap {
                    from_pass: pass,
                    to_pass: pass,
                    passes: 1,
                    events,
                    last_now_ns: now_ns,
                    bound_bytes: bound,
                }),
                0,
            ));
        }
        Recorded::Dropped
    }
}

/// The bytes one pass's inputs count against the bound: their JSON length,
/// the same order of size the spool will hold for them.
fn measure(inputs: &PassInputs) -> Result<u64> {
    let mut bytes = serde_json::to_vec(&inputs.marker)?.len() as u64;
    for event in &inputs.applied {
        bytes = bytes
            .checked_add(serde_json::to_vec(event)?.len() as u64)
            .ok_or_else(|| Error::numeric("one pass's inputs overflow a u64 byte count"))?;
    }
    Ok(bytes)
}

fn closed() -> Error {
    Error::io(
        "the spool writer has stopped, so a pass's inputs have nowhere to be recorded; a writer \
         that has stopped also stops the pressure heartbeat, which halts new exposure",
    )
}
