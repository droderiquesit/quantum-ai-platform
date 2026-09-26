//! Hybrid Logical Clock for causally-consistent event timestamps.
//!
//! See ADR 0100 §1 for the event fabric's architecture and §4 for ordering:
//! "Per-partition order only. There is no global order, and none is
//! claimed." A partition's clock only ever has to agree with itself and with
//! the producers that write to it, never with the wall clock or with another
//! partition's — so [`PartitionClock`] takes every physical reading as a
//! caller-supplied argument rather than reading an ambient clock, which
//! would make the same partition disagree with itself between a live run and
//! a replay of the same inputs.
//!
//! Moved here from the record codec — originally SLICE-06's — so that this
//! envelope, and everything built on it, does not have to wait on that
//! packet. The codec still carries a stamped reading as the two plain
//! integers [`HlcTimestamp`] already is; nothing about the wire format
//! changes.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// A hybrid logical clock reading: a physical instant and a tie-breaking
/// counter for events that land in the same instant, or that must be
/// ordered ahead of a physical clock that has not yet ticked.
///
/// Ordered by physical time first and the counter second — the field order
/// the derive uses — so two readings compare the way the algorithm intends
/// without a hand-written `Ord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HlcTimestamp {
    pub physical: Timestamp,
    pub logical: u64,
}

impl HlcTimestamp {
    pub const fn new(physical: Timestamp, logical: u64) -> Self {
        Self { physical, logical }
    }
}

/// One partition's hybrid logical clock.
///
/// Holds only the last reading it produced or accepted. Every call supplies
/// its own physical time, so nothing here reaches for `qip_core::time::Clock`
/// — a clock read here rather than passed in would make a replay's clock
/// disagree with the live run it is replaying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartitionClock {
    last: HlcTimestamp,
}

impl PartitionClock {
    /// Start a partition's clock at `initial`, with no local events recorded
    /// yet.
    pub const fn new(initial: Timestamp) -> Self {
        Self {
            last: HlcTimestamp {
                physical: initial,
                logical: 0,
            },
        }
    }

    /// The clock's last reading.
    pub const fn last(&self) -> HlcTimestamp {
        self.last
    }

    /// Advance the clock for an event produced locally at `now`.
    ///
    /// Never decreases: a `now` at or behind the clock's own physical time
    /// only advances the logical counter, which is what two events minted in
    /// the same nanosecond — or a caller whose `now` briefly regressed —
    /// both need in order to still be ordered against everything already
    /// stamped.
    pub fn tick(&mut self, now: Timestamp) -> HlcTimestamp {
        self.last = if now > self.last.physical {
            HlcTimestamp::new(now, 0)
        } else {
            HlcTimestamp::new(self.last.physical, self.last.logical + 1)
        };
        self.last
    }

    /// Merge in a reading a producer attached to a record this partition is
    /// receiving at `now`, refusing one whose physical time is more than
    /// `cap` ahead of `now`.
    ///
    /// The refusal is the whole point of the cap. The fabric's HMAC only
    /// signs P0 watermarks (ADR 0100 §7); an ordinary producer's clock is
    /// unauthenticated, so a broken or forged one claiming to be hours ahead
    /// must not be adopted even in part. Adopting it — even clamped to the
    /// cap — would still move this partition's clock forward on the word of
    /// a reading nothing has verified, after which every legitimate local
    /// event sorts behind it for as long as the partition exists. Refusing
    /// leaves the caller free to discard the record, quarantine the
    /// producer, or re-derive its own physical time and try again — all of
    /// which are answers this clock cannot give on its own.
    pub fn receive(
        &mut self,
        now: Timestamp,
        producer: HlcTimestamp,
        cap: Duration,
    ) -> Result<HlcTimestamp> {
        if producer.physical.since(now) > cap {
            return Err(Error::denied(format!(
                "producer clock {} is more than {cap:?} ahead of {now}: refusing to adopt it",
                producer.physical
            )));
        }
        let physical = now.max(self.last.physical).max(producer.physical);
        let logical = if physical == self.last.physical && physical == producer.physical {
            self.last.logical.max(producer.logical) + 1
        } else if physical == self.last.physical {
            self.last.logical + 1
        } else if physical == producer.physical {
            producer.logical + 1
        } else {
            0
        };
        self.last = HlcTimestamp::new(physical, logical);
        Ok(self.last)
    }
}
