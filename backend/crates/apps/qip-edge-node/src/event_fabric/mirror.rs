//! ADR 0100 §1: the node's composition behind the existing
//! `qip_edge::journal::Mirror` seam — the fabric gains no new mirror type.
//! SLICE-24.
//!
//! [`FabricMirror`] is what `Cell::flush` ships through in fabric mode, and
//! it does one thing: offer the batch to the spool writer's bounded channel
//! without waiting (ADR 0100 §6, FABRIC-002/003). It lives in the node, so
//! `qip-edge` gains no fabric type and the cell cannot reach a channel, a
//! spool or a socket through it.
//!
//! # A full channel is a refusal, not a wait
//!
//! `qip_edge::journal::ship` marks entries shipped — and, on a journal built
//! with `trimmed_on_ship`, drops them — only when the mirror returns `Ok`.
//! So when the channel is full this mirror returns `Err` naming the cause,
//! the batch is dropped here, and every entry in it stays unshipped in the
//! journal, chained and in memory, to be offered again by the next flush.
//! Nothing is lost and nothing waits. Each refusal is counted, by cause, so
//! a writer that has fallen behind shows as a number rather than as a
//! journal that is growing for no stated reason.

use crate::event_fabric::writer::{Handoff, HandoffSender, Offer};
use qip_core::error::{Error, Result};
use qip_edge::journal::{Mirror, MirrorBatch};

/// A [`Mirror`] that hands journal batches to the spool writer.
#[derive(Debug)]
pub struct FabricMirror {
    cell: String,
    sender: HandoffSender,
    accepted: u64,
    refused_full: u64,
    refused_closed: u64,
}

impl FabricMirror {
    /// A mirror for `cell`, offering to the writer behind `sender`.
    pub fn new(cell: impl Into<String>, sender: HandoffSender) -> Result<Self> {
        let cell = cell.into();
        if cell.trim().is_empty() {
            return Err(Error::invalid(
                "a fabric mirror needs the cell whose journal it ships",
            ));
        }
        Ok(Self {
            cell,
            sender,
            accepted: 0,
            refused_full: 0,
            refused_closed: 0,
        })
    }

    /// Batches the writer's channel took.
    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    /// Batches refused because the channel was full.
    pub fn refused_full(&self) -> u64 {
        self.refused_full
    }

    /// Batches refused because the writer had gone.
    pub fn refused_closed(&self) -> u64 {
        self.refused_closed
    }
}

impl Mirror for FabricMirror {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        if batch.cell != self.cell {
            return Err(Error::invalid(format!(
                "a batch from cell {} was handed to the fabric mirror for cell {}",
                batch.cell, self.cell
            )));
        }
        match self.sender.offer(Handoff::Journal(batch)) {
            Offer::Accepted => {
                self.accepted = self.accepted.saturating_add(1);
                Ok(())
            }
            Offer::Full(_) => {
                self.refused_full = self.refused_full.saturating_add(1);
                Err(Error::denied(
                    "the spool writer's handoff channel is full; the entries stay unshipped in \
                     the journal and the next flush offers them again — the decision thread does \
                     not wait for the writer (ADR 0100 §6)",
                ))
            }
            Offer::Closed(_) => {
                self.refused_closed = self.refused_closed.saturating_add(1);
                Err(Error::io(
                    "the spool writer has stopped; the entries stay unshipped in the journal, and \
                     a writer that has stopped also stops the pressure heartbeat, which halts new \
                     exposure",
                ))
            }
        }
    }
}
