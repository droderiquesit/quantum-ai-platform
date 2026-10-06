//! Batch reconstruction that says where it cannot be believed.
//!
//! Live, a sequence gap becomes a reset and the book withholds prices until it
//! is rebuilt (`VenueState::reset`). That is an in-process state change: a
//! later replay or training job reading the same history sees a book that looks
//! whole. [`replay`] carries the integrity finding *out* on its output, one
//! flag per message and one [`UnreliablePeriod`] per episode, so nothing
//! downstream can treat a flagged span as ground truth without having been
//! handed the flag.
//!
//! A period opens at the first message that follows a missing sequence, or that
//! arrives with a sequence lower than one already seen, and closes at the next
//! `Reset` message, which is the venue declaring that a full rebuild follows.
//! While it is open, order and level messages are not applied: the book they
//! would edit is already wrong, and applying them to it would manufacture a
//! second error. Trades, status changes and auction updates still apply, because
//! a gap in depth does not un-print a trade.
//!
//! The close assumes the snapshot that follows a `Reset` is complete. This
//! module cannot see that it is; a feed whose snapshots can themselves be cut
//! short needs the venue's end-of-snapshot marker, which the canonical schema
//! does not yet carry.

use crate::venue::VenueState;
use qip_contracts::{MarketMessage, MessageBody};
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Why a span of history is not to be believed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrityFault {
    /// Sequences `missing_from..=missing_to` never arrived.
    Gap { missing_from: u64, missing_to: u64 },
    /// A message arrived behind one already applied.
    OutOfOrder { sequence: u64, expected: u64 },
}

/// One episode of unreliable history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreliablePeriod {
    pub fault: IntegrityFault,
    /// Venue time of the first message after the fault.
    pub from: Timestamp,
    /// Venue time of the `Reset` that ended it; `None` if the input ended first.
    pub until: Option<Timestamp>,
    /// Messages replayed while the period was open.
    pub messages: usize,
}

/// The flag carried for each message replayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayStep {
    pub stream_sequence: u64,
    pub unreliable: bool,
}

/// Everything a downstream consumer needs to know about the replay's integrity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayOutput {
    pub steps: Vec<ReplayStep>,
    pub periods: Vec<UnreliablePeriod>,
}

impl ReplayOutput {
    /// Whether every message was replayed inside a reliable span.
    pub fn is_clean(&self) -> bool {
        self.periods.is_empty()
    }
}

fn is_book_edit(body: &MessageBody) -> bool {
    !matches!(
        body,
        MessageBody::Trade { .. }
            | MessageBody::StatusChange { .. }
            | MessageBody::AuctionUpdate { .. }
            | MessageBody::Reset { .. }
    )
}

/// Replay `messages` into `state`, flagging every span that cannot be trusted.
pub fn replay(state: &mut VenueState, messages: &[MarketMessage]) -> Result<ReplayOutput> {
    let mut output = ReplayOutput::default();
    let mut last: BTreeMap<String, u64> = BTreeMap::new();
    let mut open: Option<UnreliablePeriod> = None;

    for message in messages {
        let stream = message.origin.stream_key();
        let sequence = message.origin.sequence;
        let is_reset = matches!(message.body, MessageBody::Reset { .. });

        if is_reset {
            // A reset restarts the stream, so what it follows is not a gap.
            if let Some(mut period) = open.take() {
                period.until = Some(message.venue_time);
                output.periods.push(period);
            }
            last.insert(stream, sequence);
        } else {
            match last.get(&stream).copied() {
                // Several facts can share one wire message, so equal is fine.
                Some(previous) if sequence > previous.saturating_add(1) => {
                    open.get_or_insert_with(|| UnreliablePeriod {
                        fault: IntegrityFault::Gap {
                            missing_from: previous + 1,
                            missing_to: sequence - 1,
                        },
                        from: message.venue_time,
                        until: None,
                        messages: 0,
                    });
                    last.insert(stream, sequence);
                }
                Some(previous) if sequence < previous => {
                    open.get_or_insert_with(|| UnreliablePeriod {
                        fault: IntegrityFault::OutOfOrder {
                            sequence,
                            expected: previous + 1,
                        },
                        from: message.venue_time,
                        until: None,
                        messages: 0,
                    });
                }
                _ => {
                    last.insert(stream, sequence);
                }
            }
        }

        let unreliable = open.is_some();
        if let Some(period) = open.as_mut() {
            period.messages += 1;
        }
        if !(unreliable && is_book_edit(&message.body)) {
            state.apply(message)?;
        }
        if is_reset {
            state.resynchronised(message.venue_time);
        }
        output.steps.push(ReplayStep {
            stream_sequence: sequence,
            unreliable,
        });
    }
    output.periods.extend(open);
    Ok(output)
}
