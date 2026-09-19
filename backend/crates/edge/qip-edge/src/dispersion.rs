//! How long each venue takes to fill, and what the spread between them
//! forbids (§32.1).
//!
//! Fill-time dispersion is the dominant risk on any multi-venue execution: a
//! cycle whose legs arrive five milliseconds apart is a position for five
//! milliseconds, and the cost of unwinding it is paid at whatever the market
//! did in between. §32.1 names five mechanisms against it. This module holds
//! two of them. The pre-trade one is *dispersion-aware* admission, which
//! "bounds worst-case unwind cost rather than reducing its probability". The
//! other is latency-equalised dispatch, which this module doc said until ADR
//! 0084 "needs a timer wheel on a dispatch thread, and this process has
//! neither" — that was the wrong obstacle. What the cell computes is a
//! [`ReleaseSchedule`]: a release instant per venue, `max_median - median`,
//! as a pure function of the same fill-time window the admission gate reads,
//! so the slowest venue is released first and every leg is expected to arrive
//! together. The instant travels through `Placer::place`'s `at`, whose
//! meaning is "release no earlier than", and whatever holds the leg until
//! then — the simulated gateway, driven by the pass instant; a release thread
//! in the node, when a real gateway exists — lives outside this crate, so a
//! replay of the same passes computes the same schedule.
//!
//! **The measurement is the cell's own and nothing else's.** The interval is
//! from the instant the cell sent an order to the instant the venue's own
//! execution report for it was confirmed, taken at the seam in `Cell::confirm`
//! where both are known. No venue tells the cell its latency, no policy
//! payload carries one, and nothing here reads a clock: the two timestamps
//! arrive from the pass. So a replay of the same reports produces the same
//! verdicts.
//!
//! **An unmeasured venue does not refuse, and that is deliberate.** The cell's
//! standing discipline is that a figure it cannot evaluate refuses rather than
//! abstains (see the crate documentation and `tests/unevaluated.rs`), and this
//! is the one figure where that rule would close a loop on itself: a venue has
//! no fill times until it fills something, and it fills nothing while it is
//! refused for having no fill times. A cell that has never traded would then
//! never trade. What the discipline demands instead is that the silence be
//! *visible* — [`FillTimes::unmeasured`] counts the configured venues with too
//! few samples to judge, it is published on every pass including the idle
//! ones, and a cycle admitted under an unmeasured verdict says so in its
//! journal entry rather than looking like one that passed a check.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The spread between the slowest and fastest leg a cycle may carry.
///
/// Refused rather than clamped at construction, for the reason every other
/// bound in this crate is: these numbers decide when a control fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispersionPolicy {
    bound: Duration,
    min_samples: u32,
    window: u32,
}

/// The default spread a cycle may carry between its fastest and slowest venue.
pub const DEFAULT_DISPERSION_BOUND: Duration = Duration::from_millis(250);
/// The default number of fills a venue must have produced before its fill
/// time is judged at all.
pub const DEFAULT_MIN_SAMPLES: u32 = 3;
/// The default number of fill times kept per venue.
pub const DEFAULT_WINDOW: u32 = 64;

impl Default for DispersionPolicy {
    /// A policy that is always in force, sized as a ceiling.
    ///
    /// Like the quote budget's, these numbers are not a claim about any
    /// venue — nothing this cell holds states one — but a bound above every
    /// dispersion this cell has been measured to produce. A quarter of a
    /// second between two legs of one cycle is not a fast market; it is a
    /// venue that has stopped answering, and a cycle sent into it is an
    /// unwind. A deployment that has measured its venues sets its own with
    /// [`DispersionPolicy::new`].
    fn default() -> Self {
        Self {
            bound: DEFAULT_DISPERSION_BOUND,
            min_samples: DEFAULT_MIN_SAMPLES,
            window: DEFAULT_WINDOW,
        }
    }
}

impl DispersionPolicy {
    /// The policy, or the refusal naming what to set instead.
    pub fn new(bound: Duration, min_samples: u32, window: u32) -> Result<Self> {
        if bound.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a dispersion bound of {} nanoseconds refuses every cycle whose venues are not \
                 identically fast, which is every cycle; name the spread between the fastest and \
                 slowest leg the desk will carry",
                bound.as_nanos()
            )));
        }
        if min_samples == 0 {
            return Err(Error::invalid(
                "a dispersion policy that judges a venue on zero fills judges it on nothing, so \
                 the first cycle would be refused or admitted by a median of an empty window; \
                 name how many fills a venue must have produced first",
            ));
        }
        if window < min_samples {
            return Err(Error::invalid(format!(
                "a window of {window} fill times cannot hold the {min_samples} a venue must \
                 produce before it is judged, so no venue would ever become measured and the \
                 control could not fire"
            )));
        }
        Ok(Self {
            bound,
            min_samples,
            window,
        })
    }

    /// The spread a cycle may carry.
    pub const fn bound(self) -> Duration {
        self.bound
    }

    /// How many fills a venue must have produced before it is judged.
    pub const fn min_samples(self) -> u32 {
        self.min_samples
    }

    /// How many fill times are kept per venue.
    pub const fn window(self) -> u32 {
        self.window
    }
}

/// What the dispersion of a set of venues came to.
///
/// Three arms rather than a boolean, because "this cycle was not refused" has
/// two completely different meanings — the spread was measured and was inside
/// the bound, or there was nothing to measure — and an operator reading a
/// journal needs to know which one admitted the cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispersionVerdict {
    /// Fewer than two of the venues have produced enough fills to be judged,
    /// so there is no spread to compare with the bound. Admits, and says so.
    Unmeasured { measured: usize, venues: usize },
    /// The spread between the fastest and slowest measured venue, inside the
    /// bound.
    Within { spread: Duration, measured: usize },
    /// The spread is wider than the bound. Refuses.
    Exceeds {
        spread: Duration,
        bound: Duration,
        fastest: String,
        slowest: String,
    },
}

impl DispersionVerdict {
    /// Whether the cycle may be opened.
    pub const fn admits(&self) -> bool {
        !matches!(self, Self::Exceeds { .. })
    }
}

/// One venue's fill times.
#[derive(Clone, Debug)]
struct VenueFillTimes {
    /// Newest last, bounded by the policy's window. A `VecDeque` because the
    /// oldest sample is dropped on every observation past the bound, and an
    /// unbounded history of every fill a cell has ever had is exactly the
    /// working set the data rules forbid.
    samples: VecDeque<Duration>,
    /// Reports whose interval was negative — the venue's confirmation
    /// arriving before the cell's own send. Not recorded as a fill time and
    /// not discarded either: it is a disagreement about time between the cell
    /// and the channel, and it reaches the summary so that a venue producing
    /// them is not read as a venue with a fast median.
    anomalies: u64,
}

/// Fill times per venue, and the dispersion between them.
#[derive(Clone, Debug)]
pub struct FillTimes {
    policy: DispersionPolicy,
    /// One entry per configured venue, fixed at assembly. A `BTreeMap`
    /// because the order reaches the summary and the metric registry.
    venues: BTreeMap<String, VenueFillTimes>,
}

impl FillTimes {
    /// An empty history for each of `venues`, under `policy`.
    pub fn new(policy: DispersionPolicy, venues: &[VenueId]) -> Self {
        let mut history = BTreeMap::new();
        for venue in venues {
            history.insert(
                venue.as_str().to_string(),
                VenueFillTimes {
                    samples: VecDeque::new(),
                    anomalies: 0,
                },
            );
        }
        Self {
            policy,
            venues: history,
        }
    }

    /// The policy in force.
    pub const fn policy(&self) -> DispersionPolicy {
        self.policy
    }

    /// Record how long one order took from send to confirmed fill.
    ///
    /// A venue the cell was not configured for records nothing: the cell
    /// sends only to venues on its configured list, so a sample for another
    /// is a fill on an order this cell did not send, which is the
    /// reconciler's business and not a latency measurement.
    pub fn observe(&mut self, venue: &VenueId, taken: Duration) {
        let window = self.policy.window() as usize;
        let Some(history) = self.venues.get_mut(venue.as_str()) else {
            return;
        };
        if taken.as_nanos() < 0 {
            history.anomalies = history.anomalies.saturating_add(1);
            return;
        }
        history.samples.push_back(taken);
        while history.samples.len() > window {
            history.samples.pop_front();
        }
    }

    /// The median fill time at `venue`, or `None` while it has produced fewer
    /// than the policy's minimum.
    ///
    /// The median rather than the mean: one fill that took a second because
    /// the order rested is not a slow venue, and a mean over a short window
    /// lets that single sample refuse every cycle the venue is in.
    pub fn median(&self, venue: &str) -> Option<Duration> {
        let history = self.venues.get(venue)?;
        if u32::try_from(history.samples.len()).unwrap_or(u32::MAX) < self.policy.min_samples() {
            return None;
        }
        let mut ordered: Vec<i64> = history
            .samples
            .iter()
            .map(|sample| sample.as_nanos())
            .collect();
        ordered.sort_unstable();
        // The lower median on an even count. Taking the mean of the middle
        // two would invent a value no fill produced, and this number is
        // compared against a bound rather than summed.
        ordered
            .get((ordered.len().saturating_sub(1)) / 2)
            .copied()
            .map(Duration::from_nanos)
    }

    /// Configured venues that have produced too few fills to be judged.
    ///
    /// Published on every pass, including the idle ones. A cell whose venues
    /// are all unmeasured is admitting every cycle on no evidence, and the
    /// only thing separating that from a cell whose venues are all fast is
    /// this number.
    pub fn unmeasured(&self) -> usize {
        self.venues
            .keys()
            .filter(|venue| self.median(venue).is_none())
            .count()
    }

    /// What the spread across `venues` comes to.
    ///
    /// Duplicates are harmless: a cycle with two legs at one venue compares
    /// that venue's median with itself and contributes nothing to the spread,
    /// which is correct — two legs at one venue arrive together.
    pub fn assess(&self, venues: &[VenueId]) -> DispersionVerdict {
        let mut slowest: Option<(&str, Duration)> = None;
        let mut fastest: Option<(&str, Duration)> = None;
        let mut measured = 0;
        let mut seen: Vec<&str> = Vec::new();
        for venue in venues {
            if seen.contains(&venue.as_str()) {
                continue;
            }
            seen.push(venue.as_str());
            let Some(median) = self.median(venue.as_str()) else {
                continue;
            };
            measured += 1;
            if slowest.is_none_or(|(_, held)| median > held) {
                slowest = Some((venue.as_str(), median));
            }
            if fastest.is_none_or(|(_, held)| median < held) {
                fastest = Some((venue.as_str(), median));
            }
        }
        let (Some((slow_name, slow)), Some((fast_name, fast))) = (slowest, fastest) else {
            return DispersionVerdict::Unmeasured {
                measured,
                venues: seen.len(),
            };
        };
        if measured < 2 {
            return DispersionVerdict::Unmeasured {
                measured,
                venues: seen.len(),
            };
        }
        let spread = Duration::from_nanos(slow.as_nanos().saturating_sub(fast.as_nanos()));
        if spread > self.policy.bound() {
            return DispersionVerdict::Exceeds {
                spread,
                bound: self.policy.bound(),
                fastest: fast_name.to_string(),
                slowest: slow_name.to_string(),
            };
        }
        DispersionVerdict::Within { spread, measured }
    }

    /// When each leg of a multi-venue cycle is released, relative to the
    /// decision instant (§32.1, ADR 0084).
    ///
    /// Offsets are `max_median - median(venue)` over the distinct venues in
    /// `venues`, so the slowest venue is released first and every leg is
    /// expected to arrive together. A pure function of this window, which is
    /// a pure function of the reports the pass was handed — so a replay of
    /// the same passes computes the same schedule, and nothing here reads a
    /// clock.
    ///
    /// **A venue with no median gets offset zero and makes the schedule
    /// unequalised, and it does not refuse.** The reason is the module's
    /// standing one: a venue has no fill times until it fills something. What
    /// the schedule does instead is say so — [`ReleaseSchedule::unmeasured`]
    /// names the venues it could not place, and every offset is zero rather
    /// than equalised against the venues it could, because a schedule that
    /// held the measured legs back to meet a leg whose arrival nobody has
    /// measured would be holding a decision for a number nobody computed.
    ///
    /// A single-venue schedule has one offset of zero and is equalised: rule
    /// 22 says "wherever more than one venue is involved", and one venue is
    /// trivially equalised rather than exempt, whatever its measurement.
    pub fn release_schedule(&self, venues: &[VenueId]) -> ReleaseSchedule {
        let mut medians: BTreeMap<VenueId, Option<Duration>> = BTreeMap::new();
        for venue in venues {
            medians
                .entry(venue.clone())
                .or_insert_with(|| self.median(venue.as_str()));
        }
        let unmeasured: BTreeSet<VenueId> = medians
            .iter()
            .filter(|(_, median)| median.is_none())
            .map(|(venue, _)| venue.clone())
            .collect();
        let equalised = medians.len() <= 1 || unmeasured.is_empty();
        let slowest = medians
            .values()
            .flatten()
            .map(|median| median.as_nanos())
            .max()
            .unwrap_or(0);
        let offsets = medians
            .iter()
            .map(|(venue, median)| {
                let offset = match median {
                    Some(median) if unmeasured.is_empty() => {
                        Duration::from_nanos(slowest.saturating_sub(median.as_nanos()))
                    }
                    _ => Duration::ZERO,
                };
                (venue.clone(), offset)
            })
            .collect();
        ReleaseSchedule {
            offsets,
            equalised,
            unmeasured,
        }
    }

    /// What every venue's history holds, in venue order.
    pub fn summary(&self) -> Vec<VenueFillTimeState> {
        self.venues
            .iter()
            .map(|(venue, history)| VenueFillTimeState {
                venue: venue.clone(),
                samples: history.samples.len(),
                median: self.median(venue),
                anomalies: history.anomalies,
            })
            .collect()
    }
}

/// When each leg of a multi-venue cycle is released, relative to the pass
/// (ADR 0084). Built by [`FillTimes::release_schedule`] and read at the seam
/// where a leg's `release_at` is stamped.
///
/// Honest about what it could not equalise: a schedule with a non-empty
/// [`Self::unmeasured`] set carries every offset at zero and reads
/// `equalised: false`, so the journal entry for a leg sent under it says the
/// cycle went out unequalised rather than looking like one that was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSchedule {
    /// Venue → delay after the decision instant. A `BTreeMap` because the
    /// order reaches the journal.
    offsets: BTreeMap<VenueId, Duration>,
    /// Whether every offset is the one the measurements call for. False when
    /// two or more venues are involved and any of them is unmeasured.
    equalised: bool,
    /// The venues the schedule had no median for. Each has offset zero.
    unmeasured: BTreeSet<VenueId>,
}

impl ReleaseSchedule {
    /// How long after the decision instant `venue` is released. Zero for a
    /// venue the schedule was not built over, because a leg the schedule
    /// does not know cannot be held for a number it never computed.
    pub fn offset(&self, venue: &VenueId) -> Duration {
        self.offsets.get(venue).copied().unwrap_or(Duration::ZERO)
    }

    /// Every offset, in venue order.
    pub const fn offsets(&self) -> &BTreeMap<VenueId, Duration> {
        &self.offsets
    }

    /// Whether the legs were released so as to arrive together.
    pub const fn equalised(&self) -> bool {
        self.equalised
    }

    /// The venues no offset could be computed for.
    pub const fn unmeasured(&self) -> &BTreeSet<VenueId> {
        &self.unmeasured
    }
}

/// One venue's fill-time history as the pass found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueFillTimeState {
    pub venue: String,
    pub samples: usize,
    /// `None` while the venue has produced fewer fills than the policy's
    /// minimum. Absent rather than zero, because a median of zero is what a
    /// venue that answers instantly looks like and a venue that has never
    /// answered must not read as the fastest one on the chart.
    pub median: Option<Duration>,
    pub anomalies: u64,
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn fast() -> VenueId {
        VenueId::new("XPAR")
    }

    fn slow() -> VenueId {
        VenueId::new("XLON")
    }

    fn policy() -> Result<DispersionPolicy> {
        DispersionPolicy::new(Duration::from_millis(10), 3, 8)
    }

    fn history() -> Result<FillTimes> {
        Ok(FillTimes::new(policy()?, &[fast(), slow()]))
    }

    fn fill(history: &mut FillTimes, venue: &VenueId, millis: i64, times: u32) {
        for _ in 0..times {
            history.observe(venue, Duration::from_millis(millis));
        }
    }

    #[test]
    fn a_pair_of_venues_a_bound_apart_is_inside_it_and_one_nanosecond_further_is_not() -> Result<()>
    {
        // The boundary itself, because a bound tested only well inside and
        // well outside is satisfied by any threshold between the two.
        let mut history = history()?;
        fill(&mut history, &fast(), 1, 3);
        fill(&mut history, &slow(), 11, 3);
        assert_eq!(
            history.assess(&[fast(), slow()]),
            DispersionVerdict::Within {
                spread: Duration::from_millis(10),
                measured: 2
            },
            "a spread of exactly the bound was refused"
        );

        // A second history rather than more samples in the first: adding
        // slower samples beside three identical ones leaves the median where
        // it was, which would have tested nothing at all.
        let mut further = FillTimes::new(policy()?, &[fast(), slow()]);
        fill(&mut further, &fast(), 1, 3);
        for _ in 0..3 {
            further.observe(&slow(), Duration::from_nanos(11_000_001));
        }
        let verdict = further.assess(&[fast(), slow()]);
        assert!(
            !verdict.admits(),
            "a spread past the bound admitted the cycle: {verdict:?}"
        );
        Ok(())
    }

    #[test]
    fn a_venue_with_too_few_fills_leaves_the_spread_unmeasured_rather_than_fast() -> Result<()> {
        // The failure this prevents: treating an absent history as a median
        // of zero. That would make the venue nobody has ever filled at the
        // fastest leg in the cycle, and the spread against a real venue would
        // refuse — or, with the comparison the other way, admit — on a number
        // nobody computed.
        let mut history = history()?;
        fill(&mut history, &fast(), 1, 3);
        fill(&mut history, &slow(), 500, 2);
        assert_eq!(
            history.median(slow().as_str()),
            None,
            "the premise failed: two fills already counted as a measured venue"
        );
        assert_eq!(
            history.assess(&[fast(), slow()]),
            DispersionVerdict::Unmeasured {
                measured: 1,
                venues: 2
            },
            "a venue with one fill short of the minimum was judged anyway"
        );
        assert_eq!(
            history.unmeasured(),
            1,
            "the unmeasured venue was not counted, so the silence is invisible"
        );

        fill(&mut history, &slow(), 500, 1);
        let verdict = history.assess(&[fast(), slow()]);
        assert!(
            !verdict.admits(),
            "the third fill made the venue measured and the half-second spread still admitted: \
             {verdict:?}"
        );
        Ok(())
    }

    #[test]
    fn the_window_forgets_the_oldest_fill_and_the_median_follows_the_venue_that_recovered()
    -> Result<()> {
        // A venue that was slow and is not any more must stop refusing
        // cycles, or the first bad minute of a session refuses the rest of
        // the day.
        //
        // Five recovered fills after eight slow ones, deliberately not eight:
        // a window of eight then holds three slow samples and five fast, and
        // the median is the fast one — while a window that kept everything
        // holds thirteen samples whose median is still the slow one. Equal
        // counts would leave the two medians identical and the test would
        // pass against a window that never forgets anything, which is exactly
        // what it is here to catch.
        let mut history = history()?;
        fill(&mut history, &fast(), 1, 3);
        fill(&mut history, &slow(), 500, 8);
        assert!(
            !history.assess(&[fast(), slow()]).admits(),
            "the premise failed: a half-second venue was admitted beside a one-millisecond one"
        );
        fill(&mut history, &slow(), 2, 5);
        assert_eq!(
            history.summary()[0].samples,
            policy()?.window() as usize,
            "the window grew past the bound it was configured with"
        );
        assert_eq!(
            history.median(slow().as_str()),
            Some(Duration::from_millis(2)),
            "the window kept samples past its bound, so a recovered venue stays refused"
        );
        assert!(
            history.assess(&[fast(), slow()]).admits(),
            "a recovered venue was still refused"
        );
        Ok(())
    }

    #[test]
    fn one_slow_fill_among_fast_ones_does_not_refuse_the_venue() -> Result<()> {
        // The median is the reason. A mean over eight samples, one of which
        // rested for a second, reads as a venue 125 milliseconds slow.
        let mut history = history()?;
        fill(&mut history, &fast(), 1, 3);
        fill(&mut history, &slow(), 1, 7);
        history.observe(&slow(), Duration::from_millis(1_000));
        assert_eq!(
            history.median(slow().as_str()),
            Some(Duration::from_millis(1)),
            "one resting order moved the venue's typical fill time"
        );
        assert!(
            history.assess(&[fast(), slow()]).admits(),
            "a single slow fill refused every cycle the venue is in"
        );
        Ok(())
    }

    #[test]
    fn a_confirmation_that_predates_its_own_order_is_counted_and_never_averaged_in() -> Result<()> {
        let mut history = history()?;
        history.observe(&fast(), Duration::from_millis(-5));
        fill(&mut history, &fast(), 4, 3);
        // The sample count, not only the median: a negative sample among
        // three fast ones does not move the lower median, so a test that
        // asserted the median alone passed with the guard deleted — which is
        // what a mutation found. The count says it was never recorded at all.
        assert_eq!(
            history.summary()[1].samples,
            3,
            "the negative interval was recorded as a fill time"
        );
        assert_eq!(
            history.median(fast().as_str()),
            Some(Duration::from_millis(4)),
            "a negative interval reached the fill-time history"
        );
        assert_eq!(
            history.summary()[1].anomalies,
            1,
            "the anomaly was discarded rather than reported"
        );
        Ok(())
    }

    #[test]
    fn a_policy_that_could_not_fire_is_refused_at_configuration() {
        assert!(
            DispersionPolicy::new(Duration::ZERO, 3, 8).is_err(),
            "a bound of zero refuses every cycle"
        );
        assert!(
            DispersionPolicy::new(Duration::from_millis(10), 0, 8).is_err(),
            "a minimum of zero judges a venue on an empty window"
        );
        assert!(
            DispersionPolicy::new(Duration::from_millis(10), 8, 3).is_err(),
            "a window smaller than the minimum never makes a venue measured"
        );
    }
}
