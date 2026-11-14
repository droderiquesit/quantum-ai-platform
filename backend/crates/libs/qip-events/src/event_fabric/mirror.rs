//! Mirror cursor: the idempotent, gap-detecting position a fabric mirror keeps
//! per source partition (ADR 0119 §1).
//!
//! I/O-free on purpose. The cursor takes a source sequence as an argument and
//! answers; it holds no socket and no handle to the source region's produce
//! path, so cutting the inter-region link can only stop `observe` being
//! called. It cannot make a local produce wait on a remote acknowledgement,
//! because nothing here is reachable from one.
//!
//! Failure prevented: a restart replaying records already acknowledged by the
//! target, and a missing source sequence skipped silently.

use qip_core::error::{Error, Result};

/// What the mirror must do with one source record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorStep {
    /// The next expected sequence: produce it, then `acknowledge` it.
    Apply,
    /// At or below the acknowledged position: drop it.
    Duplicate,
    /// Sequences `from..=to` are missing: journal a `Gap` record, never skip.
    Gap { from: u64, to: u64 },
}

/// The last source sequence the target has acknowledged for one partition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MirrorCursor {
    acknowledged: u64,
}

impl MirrorCursor {
    /// A cursor that has acknowledged nothing; source sequences start at 1.
    pub fn new() -> Self {
        Self { acknowledged: 0 }
    }

    /// Resume from a durable checkpoint.
    pub fn resume(acknowledged: u64) -> Self {
        Self { acknowledged }
    }

    /// The last acknowledged source sequence.
    pub fn acknowledged(&self) -> u64 {
        self.acknowledged
    }

    /// Classify a source sequence without moving the cursor.
    ///
    /// Source sequence 0 is refused: it names no record, and treating it as a
    /// duplicate would hide a corrupt source.
    pub fn observe(&self, sequence: u64) -> Result<MirrorStep> {
        if sequence == 0 {
            return Err(Error::invalid(
                "source sequence 0 names no record; sequences start at 1",
            ));
        }
        let next = self.acknowledged + 1;
        Ok(if sequence < next {
            MirrorStep::Duplicate
        } else if sequence == next {
            MirrorStep::Apply
        } else {
            MirrorStep::Gap {
                from: next,
                to: sequence - 1,
            }
        })
    }

    /// Record that the target acknowledged `sequence`. Only the next expected
    /// sequence, or one past a journaled gap, may advance the cursor.
    pub fn acknowledge(&mut self, sequence: u64) -> Result<()> {
        if sequence <= self.acknowledged {
            return Err(Error::invalid(format!(
                "sequence {sequence} is not past acknowledged {}",
                self.acknowledged
            )));
        }
        self.acknowledged = sequence;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_from_a_checkpoint_drops_duplicates_and_applies_the_next_record() {
        let cursor = MirrorCursor::resume(5);
        assert_eq!(cursor.acknowledged(), 5);
        assert_eq!(cursor.observe(5).unwrap(), MirrorStep::Duplicate);
        assert_eq!(cursor.observe(1).unwrap(), MirrorStep::Duplicate);
        assert_eq!(cursor.observe(6).unwrap(), MirrorStep::Apply);
    }

    #[test]
    fn a_missing_source_sequence_is_reported_as_a_gap_and_never_skipped() {
        let mut cursor = MirrorCursor::new();
        cursor.acknowledge(2).unwrap();
        assert_eq!(
            cursor.observe(6).unwrap(),
            MirrorStep::Gap { from: 3, to: 5 }
        );
        assert_eq!(cursor.acknowledged(), 2);
    }

    #[test]
    fn sequence_zero_and_a_backward_acknowledgement_are_refused() {
        let mut cursor = MirrorCursor::resume(3);
        assert!(cursor.observe(0).is_err());
        assert!(cursor.acknowledge(3).is_err());
        assert!(cursor.acknowledge(2).is_err());
        assert_eq!(cursor.acknowledged(), 3);
        cursor.acknowledge(4).unwrap();
        assert_eq!(cursor.acknowledged(), 4);
    }
}
