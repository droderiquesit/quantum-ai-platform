//! Confirmation depth and what it does and does not promise.
//!
//! An exchange fill is a fact. A chain fill is a probability that rises with
//! every block built on top of it and collapses to zero if the block is
//! reorganised out. The difference cannot be hidden behind an accessor, so
//! nothing in this crate returns derived state without being told the depth
//! the caller needs: a strategy hedging an inventory imbalance may act on the
//! head and accept the risk, while anything that moves real custody must not.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::block::BlockNumber;

/// The confirmation depth a caller requires before it will act on state.
///
/// A newtype rather than a bare `u32` so that a depth cannot be defaulted,
/// inferred or passed positionally by accident. Constructing one is the moment
/// a caller states its risk appetite, and that moment is visible in a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Confirmations(u32);

impl Confirmations {
    /// The chain head, with no confirmations at all.
    ///
    /// Named rather than spelled `0` so that reading head state is an explicit
    /// acceptance of reorg risk rather than an omission.
    pub const AT_RISK: Self = Self(0);

    pub const fn exactly(blocks: u32) -> Self {
        Self(blocks)
    }

    pub const fn depth(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for Confirmations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} confirmations", self.0)
    }
}

/// How settled a piece of chain state is, against what the caller asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Finality {
    /// Seen in the mempool and in no block. It may never be included, and its
    /// position if included is decided by someone else.
    Pending,
    /// Included, but shallower than the caller requires.
    Included { confirmations: u32, required: u32 },
    /// Included at or beyond the required depth.
    Confirmed { confirmations: u32, required: u32 },
    /// The including block was reorganised away. Everything derived from it is
    /// void — not stale, void.
    Orphaned { was_at: BlockNumber },
}

impl Finality {
    /// Classify a block's depth against a requirement.
    pub const fn of_block(number: BlockNumber, head: BlockNumber, required: Confirmations) -> Self {
        let confirmations = number.depth_below(head) as u32;
        if confirmations >= required.depth() {
            Self::Confirmed {
                confirmations,
                required: required.depth(),
            }
        } else {
            Self::Included {
                confirmations,
                required: required.depth(),
            }
        }
    }

    /// Whether a caller that stated this requirement may act on the state.
    pub const fn is_actionable(&self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }

    /// Whether the state is void rather than merely young.
    pub const fn is_void(&self) -> bool {
        matches!(self, Self::Orphaned { .. })
    }

    pub const fn confirmations(&self) -> Option<u32> {
        match self {
            Self::Included { confirmations, .. } | Self::Confirmed { confirmations, .. } => {
                Some(*confirmations)
            }
            Self::Pending | Self::Orphaned { .. } => None,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Included { .. } => "included",
            Self::Confirmed { .. } => "confirmed",
            Self::Orphaned { .. } => "orphaned",
        }
    }
}
