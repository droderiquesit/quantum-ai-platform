//! A dark region is the centre's word for silence (ADR 0079).
//!
//! Nothing here is a cell's claim. A crashed node publishes nothing, so the
//! fact "this region is dark" cannot arrive on the wire — a cell that can say
//! it is dark is not — and `CellStateDelta` gains no field for it. The centre
//! derives it instead, on every read, from the instant it last heard from
//! each cell and the window an operator stated in
//! `CentralConfig::region_dark_after`: a region is dark at `now` when the
//! centre has heard from at least one of its cells at some time, and from
//! none of them within the window. A region the centre has never heard from
//! is not dark; it is unknown, and unknown already receives nothing.
//!
//! The derivation is never a stored flag. What *is* stored is the set of
//! regions whose darkness the platform has journaled, so that the transition
//! — and only the transition — is written in each direction; it decides
//! nothing, and [`super::CentralPlane::dark_regions`] does not read it.
//!
//! What the reading is for, in the plane: `issue` refuses a grant into a
//! dark region; the region's share bound is frozen at the last value it had
//! while lit; the last book stays in the aggregate, in every concentration,
//! in `crowded`'s cell count and in `cells_behind`; and the feasibility slot
//! names the region so a cell suspends every mirror into it. Every effect is
//! a refusal, a retention or a journal entry. Nothing here can loosen a
//! bound, which is the invariant the ADR states and `tests/dark_regions.rs`
//! pins.

use qip_core::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// The instant the centre last heard from one cell, and the region the cell
/// said it was in.
///
/// `at` is the centre's instant of ingestion rather than the report's own
/// `at`: silence is measured on the centre's clock, because a cell's clock is
/// the one thing a silent cell cannot vouch for. The report's own instant is
/// still the one `qip-api`'s status page ages a book by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastHeard {
    pub region: String,
    pub at: Timestamp,
}

/// One region's darkness as derived at an instant: which cell the centre
/// last heard from, when, and the window that has elapsed since.
///
/// Carried on a refusal so an operator reading "refused: region dark" is
/// told the three facts the derivation was made from rather than the
/// conclusion alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionDarkness {
    pub region: String,
    /// The cell of this region the centre heard from most recently.
    pub last_heard_from: String,
    pub last_heard_at: Timestamp,
    /// The operator's window, restated so the record is self-contained.
    pub window: Duration,
}

impl RegionDarkness {
    /// The instant the region became dark: the last report plus the window.
    pub fn dark_since(&self) -> Timestamp {
        self.last_heard_at.saturating_add(self.window)
    }

    /// Why a grant, a share or a mirror is being refused, in one sentence
    /// an operator can act on.
    pub fn describe(&self) -> String {
        format!(
            "region {} is dark: the centre last heard from it through {} at {} and its window \
             is {:.0} second(s), so it has been dark since {}; nothing new enters a dark \
             region until one of its cells reports again",
            self.region,
            self.last_heard_from,
            self.last_heard_at.to_rfc3339(),
            self.window.as_secs_f64(),
            self.dark_since().to_rfc3339()
        )
    }
}

/// A change in a region's derived darkness between two reads.
///
/// Returned by [`super::CentralPlane::region_transitions`] for the platform
/// to journal, and acknowledged back through
/// [`super::CentralPlane::announce`] only once the record is in the log — so
/// a journal failure leaves the transition pending rather than lost, and the
/// next read offers it again under the same idempotency key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionTransition {
    /// The region was not dark at the last announced read and is now.
    WentDark(RegionDarkness),
    /// The region was announced dark and a report from `cell` at `heard_at`
    /// has cleared it.
    SpokeAgain {
        region: String,
        cell: String,
        heard_at: Timestamp,
    },
}

impl RegionTransition {
    pub fn region(&self) -> &str {
        match self {
            Self::WentDark(darkness) => &darkness.region,
            Self::SpokeAgain { region, .. } => region,
        }
    }
}

/// The `region.dark` record: the centre derived a region dark at `at`, on
/// the reading carried here.
///
/// The record of a derivation and not a second source of truth — replaying
/// the cell reports through `CentralPlane::ingest` re-derives the same
/// reading at the same instant. Keyed on the region and the last-heard
/// instant, so a journal that failed and is retried writes once, and a
/// region that goes dark twice on two different silences writes twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionWentDark {
    pub region: String,
    pub last_heard_from: String,
    pub last_heard_at: Timestamp,
    /// The operator's window — the one number the derivation was made
    /// under, restated so the record stands alone.
    pub window: Duration,
    pub dark_since: Timestamp,
    pub cycle: u64,
}

impl RegionWentDark {
    pub fn of(reading: &RegionDarkness, cycle: u64) -> Self {
        Self {
            region: reading.region.clone(),
            last_heard_from: reading.last_heard_from.clone(),
            last_heard_at: reading.last_heard_at,
            window: reading.window,
            dark_since: reading.dark_since(),
            cycle,
        }
    }
}

impl EventBody for RegionWentDark {
    const TOPIC: Topic = Topic::RegionDark;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "region-dark:{}:{}",
            self.region,
            self.last_heard_at.as_secs()
        ))
    }
}

/// The `region.lit` record: a report from `cell`, heard at `heard_at`,
/// cleared the region's darkness. Keyed on the region and that instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionSpokeAgain {
    pub region: String,
    pub cell: String,
    pub heard_at: Timestamp,
    pub cycle: u64,
}

impl EventBody for RegionSpokeAgain {
    const TOPIC: Topic = Topic::RegionLit;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "region-lit:{}:{}",
            self.region,
            self.heard_at.as_secs()
        ))
    }
}
