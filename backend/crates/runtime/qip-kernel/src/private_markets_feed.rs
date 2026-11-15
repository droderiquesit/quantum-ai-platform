//! The private-markets slot the desk's Private Markets Brain reads
//! (R12-ASSET-3018).
//!
//! A pure function over the kernel's own books, kept out of `platform.rs` so
//! it can be tested against a book directly. [`Platform`](crate::platform::Platform)
//! calls it every REASON, before the panel is convened, and writes the result
//! into the slot the desk's gate reads.

use qip_financial::cashflow::CommitmentBook;
use qip_financial::valuation::AssetValuation;
use qip_investment_agents::desk::{
    PrivateCommitmentReading, PrivateMarkReading, PrivateMarketsView,
};
use std::collections::BTreeMap;

/// The slot as it reads at `now`: every commitment with its open and overdue
/// notices, every illiquid mark with whether it is past its review date and
/// the confidence it carries.
///
/// A mark whose confidence cannot be computed reads as zero, the restrictive
/// answer: an uncertain mark is never reported as a confident one.
pub fn reading(
    commitments: &CommitmentBook,
    marks: &BTreeMap<String, AssetValuation>,
    now: qip_core::Timestamp,
) -> PrivateMarketsView {
    PrivateMarketsView {
        observed: true,
        commitments: commitments
            .iter()
            .map(|commitment| PrivateCommitmentReading {
                subject: commitment.subject().to_string(),
                unfunded: commitment.unfunded(),
                calls_open: commitment.calls().count(),
                calls_overdue: commitment
                    .calls()
                    .filter(|call| call.is_overdue_at(now))
                    .count(),
            })
            .collect(),
        marks: marks
            .iter()
            .map(|(asset, mark)| PrivateMarkReading {
                asset: asset.clone(),
                stale: mark.is_stale(now),
                confidence: mark.confidence_at(now).unwrap_or(0.0),
            })
            .collect(),
    }
}
