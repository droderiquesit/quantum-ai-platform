//! Lane 2 of the time-lane architecture (blueprint v11.6 §4, R12-ARCH-1191):
//! the warm, seconds-to-minutes refresh the central cycle carries between the
//! fast lane's per-pass decisions and the slow lane's research.
//!
//! Before this module the pieces existed and nothing refreshed them on the
//! default path. The [`LiquidityTopology`] was fed by every quote and book
//! SENSE absorbed and read by nobody — `capacity_review::measure` has no
//! production caller — so a map of where depth lived was never computed. The
//! fill scores `score_filled` writes in LEARN were read per instrument by the
//! sizing review and per strategy by the replay-divergence hold, and never
//! per venue for the desk's own sizing, so a venue the twin had stopped
//! modelling went on receiving full-sized desk orders.
//!
//! # What one refresh does
//!
//! On a cadence of [`LANE_TWO_CADENCE`] — the minute the requirement names —
//! and on the first cycle a process runs, [`refresh`] builds a
//! [`TacticalRefresh`]:
//!
//! 1. **Liquidity maps.** One [`LiquidityMap`] per instrument the topology
//!    holds, as known at the refresh instant. An instrument whose every
//!    observation has aged out is listed as unmapped rather than dropped, so
//!    a basis that shrank is visible as shrunk.
//! 2. **Execution quality.** One [`VenueExecutionQuality`] per venue that has
//!    a scored fill, from [`FillScore`]s alone: how many fills, how many were
//!    partial, and the twin's mean absolute fill error measured by
//!    [`crate::replay_divergence::measure`] — the same measurement, sample
//!    floor and threshold the replay-divergence hold uses.
//! 3. **A narrowing-only intervention.** A venue whose measured error is past
//!    [`REPLAY_DIVERGENCE_THRESHOLD_BPS`] has its weight narrowed to
//!    [`REPLAY_DIVERGENCE_MULTIPLIER`], and every instrument filled there in
//!    the scored window has its DECIDE weight bound narrowed by the same
//!    factor ([`narrow`]), unless that instrument's own fill record has
//!    already narrowed it as far or further (next section).
//!
//! # One fill error is evidence once
//!
//! Lane 2 reads the same [`FillScore`]s ADR 0063's sizing review reads. When
//! both fire on one instrument they are two readings of one set of fills,
//! not two findings, and multiplying them would halve the instrument twice
//! for a single piece of evidence. This happened: the first version of this
//! module multiplied, and `valuation_seam.rs`'s compounding test saw AAA's
//! bound fall to 0.005 where ADR 0055 and ADR 0063 together put it at 0.01.
//! So [`narrow`] combines Lane 2's factor with the fill-record factor already
//! in DECIDE's caps map as **the stronger of the two — the minimum, never the
//! product** ([`beyond_fill_record`]). Lane 2 still narrows an instrument the
//! fill record left alone, and it still compounds with the regime,
//! federation and lattice narrowings, which read evidence other than fills.
//! The strategy hold `replay_divergence` raises on the same divergence
//! narrows a strategy's capital grant, not this weight bound, so the two do
//! not meet on any DECIDE bound.
//!
//! # No new number
//!
//! The intervention invents no limit. The threshold, the sample floor and the
//! multiplier are the replay-divergence hold's constants, read from that
//! module rather than restated, so the platform keeps one answer to "how far
//! from the venue is too far" and "by how much is size narrowed when it is".
//! The cadence is a schedule taken from the requirement text, not a bound on
//! risk or capital.
//!
//! # Why the intervention can only narrow
//!
//! A [`Narrowing`] is constructed only through [`Narrowing::new`], which
//! refuses a multiplier outside the open interval `(0, 1)` rather than
//! clamping it: a factor of one narrows nothing and would be an intervention
//! recorded against no effect, a factor above one would widen a bound past
//! what every other control sized, and a factor of zero is a veto, which is a
//! refusal and should be one in its own words. [`narrow`] only multiplies an
//! entry of DECIDE's caps map, every one of which is already in `(0, 1]`, so
//! the bound after it is never above the bound before it.
//!
//! A narrowing lasts as long as its evidence. Each refresh recomputes the set
//! from the bounded fill-score history, so a venue whose recent fills are
//! modelled again is released at the next refresh — returning the bound to
//! what the other controls set, which is not a widening past them.
//!
//! # Why the evidence is the error and not a venue withdrawal
//!
//! ADR 0062 withdraws a venue on feasibility evidence and nothing else, and
//! this module withdraws nothing: it neither writes the withdrawn set nor the
//! feasibility window (`a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue`
//! still holds). What a mean fill error past one percent says is that the
//! desk is sizing an instrument on execution costs the venue does not
//! deliver, by more than a typical edge — so the size, not the venue, is the
//! thing to narrow.
//!
//! # What this does not do
//!
//! It does not ship a map to a cell. The thirteen-plus policy slots have no
//! liquidity item, and adding one is a wire change a cell built before it
//! would refuse the whole payload over. The maps are published on the
//! platform ([`crate::platform::Platform::tactical_refresh`]) for the
//! composition roots and the console; the narrowings are journaled under
//! `learning.sizing_reviewed` when they change.

use crate::platform::FillScore;
use crate::replay_divergence::{
    REPLAY_DIVERGENCE_MIN_SAMPLE, REPLAY_DIVERGENCE_MULTIPLIER, REPLAY_DIVERGENCE_THRESHOLD_BPS,
    measure,
};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::liquidity::{LiquidityMap, LiquidityTopology};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How often Lane 2 refreshes: once a minute, the cadence R12-ARCH-1191's
/// integration criterion names. The fast brain cycles every hundred
/// milliseconds; recomputing every map and every venue's quality on each of
/// those passes would spend the decision core's budget on figures that cannot
/// move faster than fills are scored.
pub const LANE_TWO_CADENCE: Duration = Duration::from_mins(1);

/// The producer on every narrowing record.
pub const TACTICAL_LANE_ORIGIN: &str = "kernel/tactical-lane";

/// A narrowing came into force at the refresh.
pub const TACTICAL_NARROWED: &str = "narrowed";
/// A narrowing's evidence no longer clears the bar and it was released.
pub const TACTICAL_RELEASED: &str = "released";

/// A multiplier strictly inside `(0, 1)`: the only shape a Lane 2
/// intervention can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Narrowing(Decimal);

impl Narrowing {
    /// Refuses anything that is not strictly between zero and one. Refused,
    /// not clamped: a caller handing a widening factor here has a defect, and
    /// clamping it to one would let the defect survive as a silent no-op.
    pub fn new(multiplier: Decimal) -> Result<Self> {
        if !multiplier.is_positive() || multiplier >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "a Lane 2 narrowing of {multiplier} is outside (0, 1); the tactical lane may only \
                 narrow a bound, a factor of one narrows nothing, and a factor of zero is a veto \
                 that belongs to a control that says so"
            )));
        }
        Ok(Self(multiplier))
    }

    pub fn multiplier(&self) -> Decimal {
        self.0
    }
}

/// One venue's execution quality as its scored fills show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueExecutionQuality {
    pub venue: String,
    /// Scored fills at the venue in the bounded history.
    pub fills: usize,
    /// Of those, how many the venue filled only in part.
    pub partial: usize,
    /// Fills whose twin entered and so carry an error — the measured sample.
    /// Zero below [`REPLAY_DIVERGENCE_MIN_SAMPLE`], where no mean is stated.
    pub sample: usize,
    /// Statistic: the twin's mean absolute fill error in basis points, `None`
    /// below the sample floor — too few fills to say, which is not zero.
    pub mean_abs_error_bps: Option<f64>,
    /// The venue's weight narrowing, when its error is past the threshold.
    pub narrowing: Option<Narrowing>,
}

/// One Lane 2 refresh, as the cycle that ran it left it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TacticalRefresh {
    /// The cycle instant the refresh was computed at — the clock the cadence
    /// is measured on.
    pub refreshed_at: Timestamp,
    pub cycle: u64,
    /// Current maps by instrument, as known at `refreshed_at`.
    pub liquidity: BTreeMap<String, LiquidityMap>,
    /// Instruments the topology knows whose every observation has aged out.
    pub unmapped: BTreeSet<String>,
    /// Execution quality by venue.
    pub execution_quality: BTreeMap<String, VenueExecutionQuality>,
    /// DECIDE weight-bound narrowings by instrument: those filled in the
    /// scored window at a venue whose weight is narrowed.
    pub instrument_narrowings: BTreeMap<String, (Narrowing, String)>,
}

impl TacticalRefresh {
    /// Whether a refresh is due at `now`: none has run, or the last is at
    /// least [`LANE_TWO_CADENCE`] old.
    pub fn due(last: Option<&Self>, now: Timestamp) -> bool {
        last.is_none_or(|last| now >= last.refreshed_at.saturating_add(LANE_TWO_CADENCE))
    }

    /// One line for the cycle report.
    pub fn describe(&self) -> String {
        let narrowed_venues = self
            .execution_quality
            .values()
            .filter(|quality| quality.narrowing.is_some())
            .count();
        format!(
            "lane 2 refreshed: {} liquidity map(s), {} unmapped, execution quality for {} \
             venue(s), {narrowed_venues} venue weight(s) and {} instrument bound(s) narrowed",
            self.liquidity.len(),
            self.unmapped.len(),
            self.execution_quality.len(),
            self.instrument_narrowings.len()
        )
    }
}

/// Build one refresh. Problems are returned beside it rather than ending it:
/// a venue whose error cannot be measured is reported and left un-narrowed,
/// as the replay-divergence hold leaves it, and the rest of the refresh
/// stands.
pub fn refresh(
    liquidity: &LiquidityTopology,
    scores: &[FillScore],
    cycle: u64,
    now: Timestamp,
) -> (TacticalRefresh, Vec<String>) {
    let mut problems = Vec::new();

    let mut maps = BTreeMap::new();
    let mut unmapped = BTreeSet::new();
    for object in liquidity.instruments() {
        match liquidity.current_map(&object, now) {
            Some(map) => {
                maps.insert(object.as_str().to_string(), map);
            }
            None => {
                unmapped.insert(object.as_str().to_string());
            }
        }
    }

    let mut execution_quality: BTreeMap<String, VenueExecutionQuality> = BTreeMap::new();
    for score in scores {
        let quality = execution_quality
            .entry(score.venue.clone())
            .or_insert_with(|| VenueExecutionQuality {
                venue: score.venue.clone(),
                fills: 0,
                partial: 0,
                sample: 0,
                mean_abs_error_bps: None,
                narrowing: None,
            });
        quality.fills += 1;
        if score.is_partial() {
            quality.partial += 1;
        }
    }
    for quality in execution_quality.values_mut() {
        match measure(scores, &quality.venue, None) {
            Ok(Some(divergence)) => {
                quality.sample = divergence.sample;
                quality.mean_abs_error_bps = Some(divergence.mean_abs_error_bps);
                if divergence.exceeds_threshold() {
                    match Narrowing::new(REPLAY_DIVERGENCE_MULTIPLIER) {
                        Ok(narrowing) => quality.narrowing = Some(narrowing),
                        Err(error) => problems.push(format!(
                            "lane 2: {}'s weight was not narrowed: {}",
                            quality.venue,
                            error.message()
                        )),
                    }
                }
            }
            Ok(None) => {}
            Err(error) => problems.push(format!(
                "lane 2: execution quality at {} was not measured: {}",
                quality.venue,
                error.message()
            )),
        }
    }

    let mut instrument_narrowings = BTreeMap::new();
    for score in scores {
        let Some(narrowing) = execution_quality
            .get(&score.venue)
            .and_then(|quality| quality.narrowing)
        else {
            continue;
        };
        instrument_narrowings
            .entry(score.object_id.as_str().to_string())
            .or_insert((narrowing, score.venue.clone()));
    }

    (
        TacticalRefresh {
            refreshed_at: now,
            cycle,
            liquidity: maps,
            unmapped,
            execution_quality,
            instrument_narrowings,
        },
        problems,
    )
}

/// The further factor Lane 2 applies to an instrument whose bound its own
/// fill record has already narrowed to `fill_record`. It is the stronger of
/// the two divided by the one already applied, so the bound ends at
/// `min(lane, fill_record)` times every non-fill narrowing, never at their
/// product. `None` where the fill record is already as strong or stronger:
/// the same fills have been counted once and are not counted again.
///
/// Refuses a fill-record factor outside `(0, 1]` rather than guessing what it
/// meant; [`narrow`] then applies Lane 2 in full, the restrictive reading.
pub fn beyond_fill_record(lane: Narrowing, fill_record: Decimal) -> Result<Option<Decimal>> {
    if !fill_record.is_positive() || fill_record > Decimal::ONE {
        return Err(Error::invalid(format!(
            "a fill-record factor of {fill_record} is outside (0, 1]; the sizing review cannot \
             produce one, so Lane 2 does not net against it"
        )));
    }
    let stronger = lane.multiplier().min(fill_record);
    if stronger >= fill_record {
        return Ok(None);
    }
    stronger.checked_div(fill_record).map(Some).ok_or_else(|| {
        Error::numeric(format!(
            "Lane 2's {} over the fill record's {fill_record} cannot be represented",
            lane.multiplier()
        ))
    })
}

/// Write the refresh's instrument narrowings into DECIDE's `caps` map and
/// name each in `notes`. `fill_record` reads the instrument's ADR 0063
/// factor, which must be the one already folded into `caps`. Lane 2 and that
/// factor combine as the stronger of the two, never their product
/// ([`beyond_fill_record`]); every other entry already in `caps` is
/// multiplied, because it reads evidence other than fills.
///
/// Once per instrument however many theses name it, for the reason
/// `regime_allocation::narrow` gives: per thesis would square it.
pub fn narrow(
    caps: &mut BTreeMap<String, f64>,
    notes: &mut Vec<String>,
    objects: impl IntoIterator<Item = String>,
    refresh: Option<&TacticalRefresh>,
    fill_record: impl Fn(&str) -> Decimal,
) {
    let Some(refresh) = refresh else {
        return;
    };
    let distinct: BTreeSet<String> = objects.into_iter().collect();
    for object in distinct {
        let Some((narrowing, venue)) = refresh.instrument_narrowings.get(&object) else {
            continue;
        };
        let already = fill_record(&object);
        let further = match beyond_fill_record(*narrowing, already) {
            Ok(Some(further)) => further,
            Ok(None) => {
                notes.push(format!(
                    "{object}: filled at {venue}, whose fills the twin misses by more than \
                     {REPLAY_DIVERGENCE_THRESHOLD_BPS} bps on average; Lane 2's {} is not \
                     compounded, because the same fills already narrowed it to {already} on \
                     its own fill record",
                    narrowing.multiplier()
                ));
                continue;
            }
            Err(error) => {
                notes.push(format!(
                    "{object}: Lane 2 applied in full: {}",
                    error.message()
                ));
                narrowing.multiplier()
            }
        };
        // `Decimal → f64` here, once: the constructor's bounds are f64, and
        // the one multiplier this lane produces, 0.5, is exact in both.
        let multiplier = further.to_f64();
        *caps.entry(object.clone()).or_insert(1.0) *= multiplier;
        notes.push(format!(
            "{object}: filled at {venue}, whose fills the twin misses by more than \
             {REPLAY_DIVERGENCE_THRESHOLD_BPS} bps on average, narrowed to {multiplier} of the \
             bound by Lane 2"
        ));
    }
}

/// A change in one instrument's Lane 2 narrowing, journaled when it changes
/// and not at every refresh it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TacticalNarrowingEntry {
    pub object_id: String,
    /// The venue whose execution quality caused it.
    pub venue: String,
    /// [`TACTICAL_NARROWED`] or [`TACTICAL_RELEASED`].
    pub state: String,
    /// The multiplier now in force: the narrowing, or one on release.
    pub multiplier: Decimal,
    pub sample: usize,
    pub mean_abs_error_bps: Option<f64>,
    pub threshold_bps: f64,
    pub min_sample: usize,
    pub cycle: u64,
    pub at: Timestamp,
}

impl EventBody for TacticalNarrowingEntry {
    /// A bound on how much an instrument may be sized, beside the sizing
    /// review's own cap records.
    const TOPIC: Topic = Topic::SizingReviewed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "tactical-narrowing:{}:{}:{}",
            self.object_id, self.state, self.cycle
        ))
    }
}

/// The records that move the journal from `previous`'s narrowings to
/// `current`'s.
pub fn changes(
    previous: Option<&TacticalRefresh>,
    current: &TacticalRefresh,
) -> Vec<TacticalNarrowingEntry> {
    let empty = BTreeMap::new();
    let before = previous.map_or(&empty, |previous| &previous.instrument_narrowings);
    let mut entries = Vec::new();
    let quality = |venue: &str| current.execution_quality.get(venue);
    for (object, (narrowing, venue)) in &current.instrument_narrowings {
        if before.get(object) == Some(&(*narrowing, venue.clone())) {
            continue;
        }
        entries.push(TacticalNarrowingEntry {
            object_id: object.clone(),
            venue: venue.clone(),
            state: TACTICAL_NARROWED.to_string(),
            multiplier: narrowing.multiplier(),
            sample: quality(venue).map_or(0, |quality| quality.sample),
            mean_abs_error_bps: quality(venue).and_then(|quality| quality.mean_abs_error_bps),
            threshold_bps: REPLAY_DIVERGENCE_THRESHOLD_BPS,
            min_sample: REPLAY_DIVERGENCE_MIN_SAMPLE,
            cycle: current.cycle,
            at: current.refreshed_at,
        });
    }
    for (object, (_, venue)) in before {
        if current.instrument_narrowings.contains_key(object) {
            continue;
        }
        entries.push(TacticalNarrowingEntry {
            object_id: object.clone(),
            venue: venue.clone(),
            state: TACTICAL_RELEASED.to_string(),
            multiplier: Decimal::ONE,
            sample: quality(venue).map_or(0, |quality| quality.sample),
            mean_abs_error_bps: quality(venue).and_then(|quality| quality.mean_abs_error_bps),
            threshold_bps: REPLAY_DIVERGENCE_THRESHOLD_BPS,
            min_sample: REPLAY_DIVERGENCE_MIN_SAMPLE,
            cycle: current.cycle,
            at: current.refreshed_at,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrowing_refuses_every_factor_that_would_not_narrow() {
        for refused in [
            Decimal::ZERO,
            Decimal::ONE,
            Decimal::from_int(2),
            Decimal::from_int(-1),
        ] {
            assert!(
                Narrowing::new(refused).is_err(),
                "{refused} was admitted as a narrowing"
            );
        }
        assert!(Narrowing::new(REPLAY_DIVERGENCE_MULTIPLIER).is_ok());
    }

    fn refresh_narrowing(object: &str) -> Result<TacticalRefresh> {
        let mut instrument_narrowings = BTreeMap::new();
        instrument_narrowings.insert(
            object.to_string(),
            (
                Narrowing::new(REPLAY_DIVERGENCE_MULTIPLIER)?,
                "venue-a".to_string(),
            ),
        );
        Ok(TacticalRefresh {
            refreshed_at: Timestamp::from_secs(0),
            cycle: 1,
            liquidity: BTreeMap::new(),
            unmapped: BTreeSet::new(),
            execution_quality: BTreeMap::new(),
            instrument_narrowings,
        })
    }

    /// One fill error is evidence once. The first version of Lane 2
    /// multiplied its factor into a bound ADR 0063's fill record had already
    /// halved on the same fills, and `valuation_seam.rs` caught the
    /// instrument halved twice. With a non-fill narrowing of 0.5 (standing
    /// in for the regime's) and the fill record's 0.5 already in the map,
    /// Lane 2's 0.5 must leave the bound at 0.25, the stronger fill factor
    /// times the regime, and not at the product's 0.125.
    #[test]
    fn lane_two_and_a_fill_record_narrowing_combine_as_the_stronger_not_the_product() {
        let refresh = refresh_narrowing("AAA").expect("0.5 is a narrowing");
        let half = REPLAY_DIVERGENCE_MULTIPLIER;
        // Premise: both factors are real narrowings, so a product would move
        // the bound and the minimum would not.
        assert!(half.is_positive() && half < Decimal::ONE);

        let mut caps = BTreeMap::new();
        caps.insert("AAA".to_string(), half.to_f64() * 0.5);
        let mut notes = Vec::new();
        narrow(
            &mut caps,
            &mut notes,
            ["AAA".to_string()],
            Some(&refresh),
            |_| half,
        );
        assert_eq!(
            caps.get("AAA").copied(),
            Some(0.25),
            "Lane 2 and the fill record compounded on one set of fills: {notes:?}"
        );

        // On an instrument the fill record left alone Lane 2 still narrows:
        // the stronger of 0.5 and one is 0.5.
        let mut caps = BTreeMap::new();
        let mut notes = Vec::new();
        narrow(
            &mut caps,
            &mut notes,
            ["AAA".to_string()],
            Some(&refresh),
            |_| Decimal::ONE,
        );
        assert_eq!(
            caps.get("AAA").copied(),
            Some(half.to_f64()),
            "Lane 2 did not narrow an instrument its fill record left alone: {notes:?}"
        );
    }

    #[test]
    fn a_fill_record_factor_outside_the_unit_interval_is_refused() {
        let lane = Narrowing::new(REPLAY_DIVERGENCE_MULTIPLIER).expect("0.5 is a narrowing");
        for refused in [Decimal::ZERO, Decimal::from_int(2), Decimal::from_int(-1)] {
            assert!(
                beyond_fill_record(lane, refused).is_err(),
                "{refused} was taken as a fill-record factor"
            );
        }
    }
}
