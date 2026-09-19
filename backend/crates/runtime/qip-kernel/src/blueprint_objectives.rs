//! Blueprint §49.1's targets, evaluated against what this process actually
//! observed.
//!
//! [`qip_observability::slo::blueprint_slos`] writes §49.1's fourteen table
//! rows down as fifteen objectives. Until this module existed its only caller
//! was a test, which made §49.1 a declaration rather than a control: a set of
//! objectives nobody evaluates reads as measurement and is not.
//!
//! # The failure this is built to avoid, stated before the design
//!
//! A reader that reports every objective met because nothing fed it is worse
//! than no reader at all. `MaxExpectedShortfall` shipped in every default
//! limit set here and could never fire, because the state it consulted was
//! always empty; it read as protection for as long as nobody checked. The
//! same shape is available to an SLO reader for free, because
//! [`Slo::evaluate`] reports `is_met` for a window with no observations —
//! deliberately, since a quiet hour must not page.
//!
//! So this module never reports a count of "met" without reporting the count
//! of *unobserved* beside it, and [`ObjectiveReview`] keeps the two in
//! separate fields rather than in one number a caller might sum. An objective
//! nothing fed is [`ObjectiveStanding::Unobserved`], with the reason attached
//! as a value.
//!
//! # What this process can honestly feed, and what it cannot
//!
//! Two of the fifteen. That is the finding, and a bigger number would have
//! been achieved by inventing figures.
//!
//! * **`reconciliation-breaks-zero-unexplained`** and **`netting-ratio`** are
//!   fed from [`crate::Platform::ingest_cell_report`], which is the seam
//!   where a cell's report becomes the centre's knowledge and is reached in
//!   production by `qip_api::mesh`'s delta sink. Both are facts the report
//!   carries; neither is derived from a number this module chose.
//! * Four need a deployment, and they are not listed here — they are read
//!   from [`BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT`], so the list has one owner.
//! * Three are p99 latency targets in microseconds and low milliseconds. A
//!   debug-profile test on shared container hardware measures the container,
//!   not the platform, and a figure produced that way would be a target that
//!   passes or fails on who else is running.
//! * The remaining six have **no figure anywhere in this workspace** to
//!   compare against. Each says so in its own sentence in [`unmeasured`],
//!   because "unobserved" without a reason is the same silence the row
//!   already had.

use crate::cycle::StageOutcome;
use qip_core::decimal::Decimal;
use qip_core::time::{Duration, Timestamp};
use qip_mesh::delta::DeltaOrder;
use qip_observability::slo::{
    BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT, Slo, SloStatus, SloWindow, blueprint_slos,
};
use std::collections::{BTreeMap, VecDeque};

/// §49.1's "Reconciliation breaks — zero unexplained. Any is severity one".
///
/// A `&'static str` rather than a `String` on purpose: [`ObjectiveLedger`]
/// keys on `&'static str`, so the set of objectives this process can feed is
/// bounded by the constants declared in this file and cannot be widened by a
/// runtime value. The same discipline the edge's `gate` label holds.
pub const RECONCILIATION_BREAKS_ZERO: &str = "reconciliation-breaks-zero-unexplained";

/// §49.1's "Netting ratio — above 1.5".
pub const NETTING_RATIO: &str = "netting-ratio";

/// Every objective this process feeds, in the order it reports them.
pub const FED_OBJECTIVES: &[&str] = &[RECONCILIATION_BREAKS_ZERO, NETTING_RATIO];

/// Observations retained per objective.
///
/// Bounded, because an unbounded history is the failure mode this platform
/// refuses everywhere else and a process that ran for a year would otherwise
/// hold a year of them. At the bound the oldest is dropped, so a window wider
/// than the retained tail is evaluated over the tail — which says nothing
/// false about the entries it holds, but does mean a very busy month window
/// is a very busy *recent* window. Said here rather than discovered.
const RETAINED_PER_OBJECTIVE: usize = 4096;

/// Why an objective this process does not feed reads as unobserved.
///
/// A value rather than a sentence in a log line, so a caller can group on the
/// reason and an operator can tell "nothing is deployed" from "nobody has
/// built the figure".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unmeasured {
    /// The denominator is elapsed time on a node that has to exist. Nothing
    /// is deployed: `execution_nodes = {}` in every environment.
    NeedsDeployment,
    /// A percentile in microseconds or low milliseconds. Needs a release
    /// build on quiet hardware, not a debug test under a shared container.
    NeedsReleaseTiming,
    /// Nothing in this workspace computes the quantity, or the bound it would
    /// be compared against, so there is no observation to take.
    NoFigureExists,
}

impl Unmeasured {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NeedsDeployment => "needs_deployment",
            Self::NeedsReleaseTiming => "needs_release_timing",
            Self::NoFigureExists => "no_figure_exists",
        }
    }
}

/// Why one named objective is not fed here, or `None` where it is fed or
/// where nobody has classified it.
///
/// `None` for an unclassified name is deliberate and is the structural guard
/// this module turns on itself: [`review`] reports an objective that is
/// neither fed nor explained as a **problem**, so a sixteenth objective added
/// to `blueprint_slos` cannot quietly join the unobserved pile and be read
/// later as "nothing feeds it yet, like the others".
pub fn unmeasured(name: &str) -> Option<(Unmeasured, &'static str)> {
    if BLUEPRINT_SLOS_NEEDING_A_DEPLOYMENT.contains(&name) {
        return Some((
            Unmeasured::NeedsDeployment,
            "the denominator is elapsed wall-clock time on a running node, and nothing is \
             deployed",
        ));
    }
    match name {
        "strategy-evaluation-p99" | "wire-to-first-order-p99" | "arrival-dispersion-p99" => Some((
            Unmeasured::NeedsReleaseTiming,
            "a p99 in microseconds or low milliseconds measures the container it is taken on \
             unless the build is a release one and the hardware is quiet",
        )),
        "belief-calibration-within-tolerance" => Some((
            Unmeasured::NoFigureExists,
            "the learning engine reports a per-class hit rate and a window-wide Brier score; \
             §49.1 asks for a per-class calibration *error* against a tolerance, and neither \
             the per-class error nor the tolerance exists as a value. Deriving one here would \
             put the only copy of a target in the kernel",
        )),
        "quote-message-to-trade-within-venue-requirement" => Some((
            Unmeasured::NoFigureExists,
            "the requirement is the venue's and is held by the edge quoting lane; no cell \
             report carries the message-to-trade count the centre would compare against it",
        )),
        "live-versus-holdout-consistency" => Some((
            Unmeasured::NoFigureExists,
            "the holdout series is deflated at admission and never compared against what the \
             funded strategy realised afterwards, and §49.1's band is stated nowhere",
        )),
        "mark-staleness-zero-past-limit" => Some((
            Unmeasured::NoFigureExists,
            "no position carries the age of its mark and no staleness limit is configured, so \
             there is nothing to count past",
        )),
        "effective-breadth-above-floor" => Some((
            Unmeasured::NoFigureExists,
            "effective breadth is computed and recorded on every sized proposal, and the \
             allocator declares no floor for it to be above",
        )),
        "unauthorised-transfers-reaching-execution-zero" => Some((
            Unmeasured::NoFigureExists,
            "ADR 0021: this process instructs no transfer, so there is no transfer attempt to \
             authorise or refuse. A zero reported here would be the absence of a path rather \
             than a control holding",
        )),
        _ => None,
    }
}

/// Where one objective stands after a review.
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectiveStanding {
    /// Observed within its window and reached its target.
    Met(SloStatus),
    /// Observed within its window and did not.
    Missed(SloStatus),
    /// Fed by this process, and its window holds nothing. A quiet window, not
    /// an achievement.
    Quiet(SloStatus),
    /// Not fed by this process at all, with the reason.
    Unobserved(Unmeasured, &'static str),
    /// Neither fed nor explained. A defect in this module, reported as one.
    Unclassified,
}

impl ObjectiveStanding {
    /// Whether this standing rests on an observation. `false` for everything
    /// but [`Self::Met`] and [`Self::Missed`].
    pub const fn is_observed(&self) -> bool {
        matches!(self, Self::Met(_) | Self::Missed(_))
    }
}

/// What one pass of [`assess`] found, per objective.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveReview {
    /// Every §49.1 objective by name, in `blueprint_slos` order.
    pub standings: Vec<(String, ObjectiveStanding)>,
}

impl ObjectiveReview {
    pub fn met(&self) -> usize {
        self.count(|standing| matches!(standing, ObjectiveStanding::Met(_)))
    }

    pub fn missed(&self) -> usize {
        self.count(|standing| matches!(standing, ObjectiveStanding::Missed(_)))
    }

    /// Objectives resting on no observation at all — quiet, unfed, or
    /// unclassified.
    ///
    /// Reported separately from [`Self::met`] and never folded into it. The
    /// whole point of this module is that a reader cannot present the second
    /// as the first.
    pub fn unobserved(&self) -> usize {
        self.count(|standing| !standing.is_observed())
    }

    /// The standing of one objective by name.
    pub fn standing(&self, name: &str) -> Option<&ObjectiveStanding> {
        self.standings
            .iter()
            .find(|(objective, _)| objective == name)
            .map(|(_, standing)| standing)
    }

    fn count(&self, predicate: impl Fn(&ObjectiveStanding) -> bool) -> usize {
        self.standings
            .iter()
            .filter(|(_, standing)| predicate(standing))
            .count()
    }
}

/// Observations one process has taken against §49.1's objectives.
///
/// Keyed on `&'static str`, which is what bounds the key set to the constants
/// this module declares. A caller cannot widen it with a formatted string.
#[derive(Clone, Debug, Default)]
pub struct ObjectiveLedger {
    observations: BTreeMap<&'static str, VecDeque<(Timestamp, bool)>>,
}

impl ObjectiveLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observation of `objective`, stamped at the instant the fact
    /// became known.
    ///
    /// `good` means the observation satisfied the objective. There is no
    /// third value: an observation nobody can grade must not be recorded at
    /// all, because an ungradeable one recorded as good is precisely the
    /// reading this module exists to stop.
    pub fn observe(&mut self, objective: &'static str, good: bool, at: Timestamp) {
        let entries = self.observations.entry(objective).or_default();
        entries.push_back((at, good));
        while entries.len() > RETAINED_PER_OBJECTIVE {
            entries.pop_front();
        }
    }

    /// Whether this ledger holds a series for `name` at all.
    ///
    /// Distinct from holding observations *in a window*: a series that exists
    /// and has aged out is a fed objective with a quiet window, and a series
    /// that does not exist is an objective nothing feeds. Reporting both as
    /// "no data" is how the two become indistinguishable.
    pub fn feeds(&self, name: &str) -> bool {
        self.observations.contains_key(name)
    }

    /// Good and total observations of `name` inside `window` as at `now`.
    pub fn counts(&self, name: &str, window: SloWindow, now: Timestamp) -> (u64, u64) {
        let span = window_span(window);
        let Some(entries) = self.observations.get(name) else {
            return (0, 0);
        };
        let mut good = 0u64;
        let mut total = 0u64;
        for (at, ok) in entries {
            // `since` is signed, so an observation stamped ahead of `now` — a
            // report whose clock ran fast — counts as inside the window
            // rather than being dropped. Dropping it would make a clock skew
            // look like a quiet window, which is the one reading this module
            // must never produce.
            if now.since(*at) <= span {
                total += 1;
                if *ok {
                    good += 1;
                }
            }
        }
        (good, total)
    }
}

/// An objective's evaluation window as a duration.
///
/// [`SloWindow::hours`] is an `f64` because a burn-rate divides by it. Every
/// variant is a whole number of hours and a whole number of seconds, so the
/// product below is exact for each of them; the cast saturates rather than
/// wrapping.
fn window_span(window: SloWindow) -> Duration {
    Duration::from_secs((window.hours() * 3600.0) as i64)
}

/// Evaluate every §49.1 objective against `ledger`.
pub fn assess(ledger: &ObjectiveLedger, now: Timestamp) -> ObjectiveReview {
    let standings = blueprint_slos()
        .into_iter()
        .map(|slo| {
            let standing = stand(ledger, &slo, now);
            (slo.name, standing)
        })
        .collect();
    ObjectiveReview { standings }
}

fn stand(ledger: &ObjectiveLedger, slo: &Slo, now: Timestamp) -> ObjectiveStanding {
    if ledger.feeds(&slo.name) {
        let (good, total) = ledger.counts(&slo.name, slo.window, now);
        let status = slo.evaluate(good, total);
        // `SloStatus::is_observed` and not `is_met`. `evaluate(0, 0)` reports
        // met, on purpose, and a caller that read that as an achievement is
        // the whole reason this module was written.
        return if status.is_observed() {
            if status.is_met {
                ObjectiveStanding::Met(status)
            } else {
                ObjectiveStanding::Missed(status)
            }
        } else {
            ObjectiveStanding::Quiet(status)
        };
    }
    match unmeasured(&slo.name) {
        Some((reason, detail)) => ObjectiveStanding::Unobserved(reason, detail),
        None => ObjectiveStanding::Unclassified,
    }
}

/// Review §49.1 in the `(summary, problems)` shape every other LEARN review
/// uses.
///
/// What is a problem and what is merely a sentence is the judgement here, and
/// it was made once already in [`crate::venue_admission`]: a review that
/// pushes a problem on every cycle of a deployment that is working as
/// configured teaches an operator that problems are noise. So:
///
/// * A **missed** objective is a problem. Something was measured and the
///   platform did not reach the target it wrote down.
/// * An **unobserved** objective is not. Nothing is deployed and six of the
///   figures do not exist; that is today's honest state and it belongs in the
///   summary, where it is visible without being an alarm.
/// * An **unclassified** objective is a problem, and a problem about this
///   module rather than about the platform.
pub fn review(ledger: &ObjectiveLedger, now: Timestamp) -> (Option<String>, Vec<String>) {
    let reviewed = assess(ledger, now);
    let mut problems = Vec::new();
    for (name, standing) in &reviewed.standings {
        match standing {
            ObjectiveStanding::Missed(status) => problems.push(format!(
                "blueprint §49.1 objective `{name}` was missed: {:.4} achieved against a target \
                 of {:.4} over {} observation(s); the objective is the platform's own and is \
                 not to be relaxed to clear this",
                status.achieved, status.slo.target, status.observations
            )),
            ObjectiveStanding::Unclassified => problems.push(format!(
                "blueprint §49.1 objective `{name}` is neither fed by this process nor listed \
                 in `blueprint_objectives::unmeasured`, so it would be reported as unobserved \
                 with no reason. Feed it, or say why it cannot be fed"
            )),
            _ => {}
        }
    }
    // Never `None`, and the three counts are always all three. A summary that
    // reported only what was met would be the sentence this module exists to
    // refuse.
    let summary = format!(
        "§49.1: {} of {} objective(s) met, {} missed, {} unobserved ({} fed by this process)",
        reviewed.met(),
        reviewed.standings.len(),
        reviewed.missed(),
        reviewed.unobserved(),
        FED_OBJECTIVES.len(),
    );
    (Some(summary), problems)
}

/// Fold a review onto a stage outcome, the way every other LEARN review is
/// folded.
pub fn fold(outcome: StageOutcome, ledger: &ObjectiveLedger, now: Timestamp) -> StageOutcome {
    let (summary, problems) = review(ledger, now);
    let mut outcome = match summary {
        Some(summary) => StageOutcome {
            detail: format!("{}; {summary}", outcome.detail),
            ..outcome
        },
        None => outcome,
    };
    for problem in problems {
        outcome = outcome.with_problem(problem);
    }
    outcome
}

/// The netting ratio one cell report evidences, or `None` where it evidences
/// none.
///
/// §27.2's ratio, and the same quantity the edge's own
/// `qip_edge_netting_ratio` histogram holds: gross intent size over the net
/// size actually sent. The centre can compute it because `DeltaOrder` carries
/// the contributor vector the order was netted from, each contributor's share
/// signed so the vector sums to the net.
///
/// `None` in three cases, and each matters:
///
/// * the report sent no orders — nothing was netted, so nothing is evidence
///   about netting;
/// * no order carried a contributor vector — `contributors` is
///   `#[serde(default)]`, so a delta written before the field decodes as
///   naming nobody, and an absent vector is an absence rather than a ratio
///   of one;
/// * the net quantity summed to zero — the ratio's denominator.
///
/// Returning `Some(1.0)` in any of those would have been the cheap option and
/// would have fed a target with a number nobody measured.
///
/// The crossing point from `Decimal` to `f64` is the last line, and it is
/// there rather than earlier because the sizes are money-adjacent quantities
/// whose arithmetic stays exact until the ratio itself is formed.
pub fn netting_ratio_of(orders: &[DeltaOrder]) -> Option<f64> {
    let netted: Vec<&DeltaOrder> = orders
        .iter()
        .filter(|order| !order.contributors.is_empty())
        .collect();
    if netted.is_empty() {
        return None;
    }
    let gross = netted
        .iter()
        .flat_map(|order| order.contributors.iter())
        .map(|contributor| contributor.signed_size.abs())
        .fold(Decimal::ZERO, |a, b| a + b);
    let net = netted
        .iter()
        .map(|order| order.quantity.abs())
        .fold(Decimal::ZERO, |a, b| a + b);
    if net.is_zero() {
        return None;
    }
    Some(gross.to_f64() / net.to_f64())
}
