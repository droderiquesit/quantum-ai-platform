//! What a cross-region mirror reports about itself while it is behind or cut
//! off: lag, whether remote knowledge is stale, and where to replay from on
//! return (ADR 0119 §2, R12-RES-7114).
//!
//! No socket and no clock: `now` and the source head arrive as arguments, so
//! the source region's produce path cannot be reached from here and cannot
//! wait on anything here.
//!
//! # The failure this prevents
//!
//! A mirror whose link is cut stops hearing the source's head. If lag were
//! computed only from the last head it heard, a cut link would read as "zero
//! lag, fresh" while the source kept committing. So staleness has two causes,
//! and either one makes remote knowledge [`Knowledge::Stale`]: the measured lag
//! is past its bound, or the mirror has not heard from the source for longer
//! than the age bound (it cannot know the lag, which is not the same as the
//! lag being small).

use qip_core::error::{Error, Result};
use qip_observability::metrics::{Metrics, labels, names};

use super::mirror::MirrorCheckpoint;

/// The stated bound on mirror lag for one stream. Both figures are the
/// operator's to commit with the stream's policy; there is no default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LagBound {
    max_records: u64,
    max_seconds: u64,
}

impl LagBound {
    /// Refuses a zero bound: it would mark every stream stale at all times,
    /// which is a mirror switched off by configuration.
    pub fn new(max_records: u64, max_seconds: u64) -> Result<Self> {
        if max_records == 0 || max_seconds == 0 {
            return Err(Error::invalid(
                "a mirror lag bound needs a positive record count and age; zero marks every stream stale",
            ));
        }
        Ok(Self {
            max_records,
            max_seconds,
        })
    }
}

/// How far behind the source the target is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MirrorLag {
    /// Committed source records not yet acknowledged by the target.
    pub records: u64,
    /// Age in seconds of the oldest uncopied committed record; zero when
    /// nothing is uncopied.
    pub seconds: u64,
}

/// Whether a global consumer may read remote knowledge as current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knowledge {
    Fresh,
    /// Past the bound, or the source has not been heard from within the age
    /// bound. A typed state, not an absent value.
    Stale(StaleCause),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleCause {
    LagPastBound,
    SourceSilent,
}

/// What the mirror last learnt of the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceView {
    /// One past the last committed source offset, as last heard.
    pub committed_through: u64,
    /// Commit time (unix seconds) of the oldest committed record at or above
    /// the checkpoint, when there is one.
    pub oldest_uncopied_at: Option<u64>,
    /// When the mirror last heard from the source (unix seconds).
    pub heard_at: u64,
}

/// Lag and freshness at `now`. Refuses a checkpoint ahead of the source's
/// committed bound (the mirror holds what the source never committed) and a
/// `now` before the last contact.
pub fn assess(
    checkpoint: &MirrorCheckpoint,
    view: &SourceView,
    bound: LagBound,
    now: u64,
) -> Result<(MirrorLag, Knowledge)> {
    if checkpoint.next_offset > view.committed_through {
        return Err(Error::invalid(format!(
            "checkpoint {} is past the source's committed bound {}; the mirror copied a record the source never committed",
            checkpoint.next_offset, view.committed_through
        )));
    }
    if now < view.heard_at {
        return Err(Error::invalid(
            "now is before the last contact with the source; pass a monotonic time",
        ));
    }
    let records = view.committed_through - checkpoint.next_offset;
    let seconds = match (records, view.oldest_uncopied_at) {
        (0, _) => 0,
        (_, Some(at)) => now.saturating_sub(at),
        // Records are uncopied but their age was not supplied: refuse rather
        // than report zero seconds behind.
        (_, None) => {
            return Err(Error::invalid(
                "uncopied records with no oldest-uncopied time; lag seconds would be a guess",
            ));
        }
    };
    let lag = MirrorLag { records, seconds };
    let knowledge = if now - view.heard_at > bound.max_seconds {
        Knowledge::Stale(StaleCause::SourceSilent)
    } else if records > bound.max_records || seconds > bound.max_seconds {
        Knowledge::Stale(StaleCause::LagPastBound)
    } else {
        Knowledge::Fresh
    };
    Ok((lag, knowledge))
}

/// Publish the lag series for one source stream (R12-OBS-9148). Labels are
/// the caller's configured region and stream; a free-form label is refused by
/// the caller's construction, not here.
pub fn record(metrics: &Metrics, source_region: &str, stream: &str, lag: &MirrorLag) {
    let l = || labels([("source_region", source_region), ("stream", stream)]);
    metrics.gauge(names::FABRIC_MIRROR_LAG_RECORDS, l(), lag.records as f64);
    metrics.gauge(names::FABRIC_MIRROR_LAG_SECONDS, l(), lag.seconds as f64);
}

/// Where the mirror resumes from on return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayRoute {
    /// The source fabric still retains the checkpoint.
    Fabric,
    /// The fabric trimmed past the checkpoint; the archive still holds it.
    Archive,
    /// Neither holds the checkpoint: offsets `from..to` are lost to the
    /// mirror and are journaled as a gap, never skipped silently.
    Lost { from: u64, to: u64 },
}

/// Choose the replay route. `retained_from` is the oldest offset the source
/// fabric still serves; `archive_from` the oldest the archive holds, if any.
/// Both routes verify the batch chain through `plan_copy` before producing.
pub fn replay_route(
    checkpoint: &MirrorCheckpoint,
    retained_from: u64,
    archive_from: Option<u64>,
) -> ReplayRoute {
    let next = checkpoint.next_offset;
    if next >= retained_from {
        ReplayRoute::Fabric
    } else if archive_from.is_some_and(|a| a <= next) {
        ReplayRoute::Archive
    } else {
        ReplayRoute::Lost {
            from: next,
            to: archive_from.map_or(retained_from, |a| a.min(retained_from)),
        }
    }
}
