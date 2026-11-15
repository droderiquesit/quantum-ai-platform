//! The consumer of a pass's `MarketStatePack` (ADR 0428 decision 5).
//!
//! The edge node ships, inside `reflex.feature_digested`, a pack saying
//! whether the tape a pass applied had a hole in it. A reader that never looks
//! at it makes the flag decoration: fusion would weigh the traces of a venue
//! whose sequence numbers skipped exactly as it weighs a clean one.
//!
//! This module may only *remove*. Given the traces fusion is about to read it
//! returns a subset of them, never a trace it was not given and never a point
//! it did not receive, so no input weight can rise on account of an
//! attestation. An absent pack over applied events is *unattested* and is
//! treated as flagged everywhere (ADR 0428 decision 4): the producer refuses
//! to build a pack over an empty interval, so reading that refusal as a clean
//! tape would turn a first-pass quirk into a way to launder a flag.

use crate::fusion::MarketTrace;
use qip_contracts::market_state::{IntegrityFlag, MarketStatePack};
use qip_contracts::replay::FeatureDigest;
use std::collections::BTreeSet;

/// What a pass's digest lets a consumer believe about the tape behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TapeAttestation {
    /// The pass applied no event. It makes no claim about any tape, so there
    /// is nothing to remove a weight on.
    NoTape,
    /// A pack over the applied tape and it carried no integrity flag.
    Clean,
    /// A pack naming sequence gaps; these venues each had one.
    Flagged { venues: BTreeSet<String> },
    /// Events were applied and no pack describes them.
    Unattested,
}

impl TapeAttestation {
    /// Read the attestation a digest carries.
    pub fn of_digest(digest: &FeatureDigest) -> Self {
        Self::of(digest.applied, digest.market_state.as_ref())
    }

    /// `applied` events and the pack that should describe them.
    pub fn of(applied: u64, pack: Option<&MarketStatePack>) -> Self {
        match (applied, pack) {
            (0, _) => Self::NoTape,
            (_, None) => Self::Unattested,
            (_, Some(pack)) => {
                let venues: BTreeSet<String> = pack
                    .integrity()
                    .iter()
                    .map(|flag| {
                        let IntegrityFlag::SequenceGap { stream, .. } = flag;
                        // A stream key is `venue/feed/partition`; the venue is
                        // the first delimited token, compared whole so that
                        // `XNYS` does not flag `XNYSX`.
                        stream.split('/').next().unwrap_or_default().to_string()
                    })
                    .collect();
                if venues.is_empty() {
                    Self::Clean
                } else {
                    Self::Flagged { venues }
                }
            }
        }
    }

    /// Whether a trace on `venue` may be weighed.
    pub fn admits(&self, venue: &str) -> bool {
        match self {
            Self::NoTape | Self::Clean => true,
            Self::Flagged { venues } => !venues.contains(venue),
            Self::Unattested => false,
        }
    }

    /// The traces fusion may read: those `self` admits, in their order. Never
    /// adds or alters one.
    pub fn admit(&self, traces: Vec<MarketTrace>) -> Vec<MarketTrace> {
        traces
            .into_iter()
            .filter(|trace| self.admits(&trace.venue))
            .collect()
    }
}
