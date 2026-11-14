//! The pass's `MarketStatePack` (ADR 0428, amending ADR 0151 decision 2).
//!
//! A pass that applied tape events ships, inside its `FeatureDigest`, one
//! pack built over exactly those events, so a consumer on the other side of
//! `reflex.feature_digested` can tell a state built over a clean tape from
//! one built over a tape with a hole in it. The integrity flags are computed
//! by `MarketStatePack::new` from the events themselves; nothing here can
//! claim a clean tape it did not read.

use qip_contracts::market_event::MarketEvent;
use qip_contracts::market_state::{MarketStatePack, StateKind};
use qip_core::Timestamp;
use qip_core::error::Result;

/// The pack for one pass, or `None` when the pass applied no event — a pass
/// over no tape records "no tape", never a clean one (ADR 0151 decision 2).
///
/// Interval and horizon are the pass's own (ADR 0428 decision 3): `from` is
/// the earlier of the previous pass's instant and the earliest applied event
/// time, `to` the later of this pass's instant and the latest applied event
/// time, and the horizon is their distance. Every term is a time the pass
/// already holds; nothing here is a constant. The lineage names the one
/// upstream artifact the pack was derived from — the applied tape, by the
/// digest the `FeatureDigest` already carries over its source hashes — so a
/// replay can tie the pack to the bytes it describes.
///
/// A refusal from the constructor (an empty interval on a first pass whose
/// every event is stamped at the pass's own instant) is returned, not
/// swallowed; the caller ships no pack, which ADR 0428 decision 4 makes an
/// unattested pass, never a clean one.
pub fn market_state_of(
    previous_pass: Option<Timestamp>,
    now: Timestamp,
    applied: &[MarketEvent],
    applied_digest: &str,
) -> Result<Option<MarketStatePack>> {
    let (Some(earliest), Some(latest)) = (
        applied.iter().map(MarketEvent::event_time).min(),
        applied.iter().map(MarketEvent::event_time).max(),
    ) else {
        return Ok(None);
    };
    let from = previous_pass.map_or(earliest, |previous| previous.min(earliest));
    let to = now.max(latest);
    MarketStatePack::new(
        StateKind::Book,
        from,
        to,
        to.since(from),
        vec![format!("reflex.market_event.applied@{applied_digest}")],
        applied,
    )
    .map(Some)
}
