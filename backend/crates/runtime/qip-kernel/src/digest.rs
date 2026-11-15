//! Build the per-cell [`FusedDigest`] from an opportunity map (R12-TICK-2086).
//!
//! The map is derived from licensed tick data, so the licence decides which
//! region may hold it. [`build_for_cell`] calls
//! [`OpportunityMap::authorise_for`] first and builds nothing when it refuses,
//! so a digest for a region the data may not enter does not exist to be sent.

use crate::fusion::OpportunityMap;
use qip_ai::registry::ModelCard;
use qip_contracts::fused_digest::{FusedDigest, FusedDigestEntry};
use qip_core::error::{Error, Result};
use qip_core::Duration;

/// Distil `map` for `cell` in `region`: at most `max_entries` of the map's
/// already-ranked entries, valid for `valid_for` after the map's instant.
/// Refuses a region the card's entitlements do not permit, and a zero bound.
pub fn build_for_cell(
    map: &OpportunityMap,
    card: &ModelCard,
    cell: &str,
    region: &str,
    valid_for: Duration,
    max_entries: usize,
) -> Result<FusedDigest> {
    map.authorise_for(card, region)?;
    if max_entries == 0 || valid_for <= Duration::ZERO {
        return Err(Error::invalid(
            "a fused digest needs a positive entry bound and validity; state them rather than \
             shipping an empty or instantly stale one",
        ));
    }
    let digest = FusedDigest {
        cell: cell.to_string(),
        region: region.to_string(),
        as_of: map.as_of,
        valid_until: map.as_of.saturating_add(valid_for),
        entries: map
            .entries
            .iter()
            .take(max_entries)
            .map(|e| FusedDigestEntry {
                instrument: e.instrument.clone(),
                score: e.score,
                expires_at: e.expires_at,
            })
            .collect(),
        source_digest: map.source_digest.clone(),
    };
    digest.validate()?;
    Ok(digest)
}
