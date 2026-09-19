//! Regime transition markers — blueprint §9.4's first handling.
//!
//! §9.4 answers "causal discovery from observational data is unreliable" with
//! "edges carry a confidence; low-confidence edges inform exploration, not
//! sizing". The confidence half has been built for a long while; the
//! exploration half had nothing feeding it, and
//! [`qip_capital::exploration::ProbeKind::RegimeBoundary`] — "enter a
//! regime-boundary trade, to learn how the causal edges behave at the
//! transition" — sat declared and unfed.
//!
//! # Why a marker was the missing piece, and not a probe kind
//!
//! [`crate::exploration`]'s module doc named the gap precisely: "nothing
//! marks a regime *transition* — `Platform::regime_label` names the regime in
//! force and not the boundary", so a `RegimeBoundary` candidate would have
//! been "a probe sized against a figure nobody computed". This module
//! computes that figure and nothing else. It stores the regime key each
//! subject was last seen in; a pass that sees a different key has found a
//! boundary, and that is the whole of what it knows.
//!
//! # A first sighting is not a transition, and that is the load-bearing rule
//!
//! A subject observed for the first time has not crossed anything. Recording
//! a cold start as a boundary would make every restart of the process look
//! like a market-wide regime change and would put the platform's largest
//! exploration slate immediately after its least informed moment. So the
//! first observation seats the mark and reports no crossing, and the same
//! convention covers an eviction: a subject dropped from the bounded set and
//! seen again later re-enters as a first sighting, which fails towards "no
//! transition" rather than towards a boundary nobody observed.
//!
//! # What reads it
//!
//! `Platform::regime_boundary_uncertainties`, in the DECIDE stage, over the
//! subjects the causal graph actually names. The uncertainty it pairs with a
//! crossing is the share of that subject's incoming edges whose conditions
//! are [`qip_world_model::causal::ConditionStanding::Untested`] in the regime
//! now in force — one measure taken the same way for every edge, which is
//! what `1 - confidence` could not be: a hand-asserted mechanism claim, a
//! precedence edge and a confounded edge take their confidence from three
//! different ceilings, so ranking probes on it would rank them by how an edge
//! was established rather than by how little is known about it here.

use qip_core::{Duration, Timestamp};
use qip_world_model::causal::ConditionStanding;
use std::collections::BTreeMap;

/// How long a crossing stays readable after it happened.
///
/// Longer than [`crate::exploration::PROBE_VALIDITY`] on purpose, and the
/// margin is the point rather than the number: a probe opened at the instant
/// of a crossing runs for `PROBE_VALIDITY`, and if the marker expired first
/// the subject would vanish from the measured set before its own probe came
/// due. The exploration desk settles a probe whose subject can still be
/// measured and *abandons* one whose subject cannot, so an expiry shorter
/// than the probe's life would abandon every regime-boundary probe ever
/// opened — a whole probe kind that could never produce an observation,
/// which is the `MaxExpectedShortfall` shape in a new place.
pub const MARKER_WINDOW: Duration = Duration::from_hours(48);

/// The prefix an exploration subject built from a regime boundary carries.
///
/// One constant rather than a literal at each end, because the exploration
/// desk keys a probe's history on the subject string: if the writer and the
/// settler spelled it differently the probe would be opened against one key
/// and looked up under another, and every regime-boundary probe would be
/// abandoned as "no longer measured" without anything reading as broken.
pub const SUBJECT_PREFIX: &str = "regime-boundary:";

/// The most subjects whose regime is remembered at once.
///
/// A bounded working set, like every other buffer here. Eviction drops the
/// least recently changed mark, and losing a mark costs at most one missed
/// crossing, because the subject re-enters as a first sighting.
pub const MAX_SUBJECTS: usize = 1024;

/// The share of a subject's edges whose conditions are untested in the regime
/// it has just entered — the uncertainty a regime-boundary probe is ranked on.
///
/// `None` for a subject with no edges at all: a subject the graph makes no
/// claim about has nothing to learn *about its edges*, and scoring it zero
/// would put a row in the plan that reads as a choice nobody could take.
///
/// [`ConditionStanding::KnownToFail`] counts as tested, not as untested, and
/// that is the load-bearing arm. A refuted edge is the one case where the
/// platform knows exactly how the edge behaves here — it does not — so a
/// probe buys nothing. Counting a refutation as ignorance would aim the
/// budget at the edges the platform has already finished learning about,
/// which is the opposite of what §9.4 asks the budget to do.
///
/// usize → f64 at the quotient: a share of edges is a statistic, and this is
/// where the counts stop being counts.
pub fn untested_share<I>(standings: I) -> Option<f64>
where
    I: IntoIterator<Item = ConditionStanding>,
{
    let (untested, total) = standings.into_iter().fold((0usize, 0usize), |(u, t), s| {
        (u + usize::from(s == ConditionStanding::Untested), t + 1)
    });
    if total == 0 {
        return None;
    }
    Some(untested as f64 / total as f64)
}

/// One subject's regime as this module last saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Mark {
    /// The regime key in force at the last observation.
    current: String,
    /// The key it came from, or `None` while the subject has only ever been
    /// seen in one regime — the first-sighting rule above.
    previous: Option<String>,
    /// When `current` was first observed. For a subject that has never
    /// changed, the instant it was first seen at all.
    entered_at: Timestamp,
}

/// Which subjects have crossed a regime boundary, and when.
#[derive(Debug, Default)]
pub struct RegimeTransitions {
    marks: BTreeMap<String, Mark>,
}

/// A boundary one subject crossed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crossing {
    pub subject: String,
    pub from: String,
    pub to: String,
    pub at: Timestamp,
}

impl RegimeTransitions {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many subjects are remembered. For the bound's own test.
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Record the regime `subject` is in at `now`, returning the crossing if
    /// this observation is one.
    ///
    /// Idempotent within a regime: observing the same key again moves
    /// nothing, so a subject whose regime has been stable for a month still
    /// reports the instant it entered that regime rather than the instant of
    /// the last pass. Without that, `entered_at` would track the clock and
    /// every crossing would look brand new forever.
    pub fn observe(&mut self, subject: &str, regime: &str, now: Timestamp) -> Option<Crossing> {
        if let Some(mark) = self.marks.get_mut(subject) {
            if mark.current == regime {
                return None;
            }
            let from = std::mem::replace(&mut mark.current, regime.to_string());
            mark.previous = Some(from.clone());
            mark.entered_at = now;
            return Some(Crossing {
                subject: subject.to_string(),
                from,
                to: regime.to_string(),
                at: now,
            });
        }
        self.evict_if_full();
        self.marks.insert(
            subject.to_string(),
            Mark {
                current: regime.to_string(),
                previous: None,
                entered_at: now,
            },
        );
        None
    }

    /// Whether `subject` crossed a boundary within [`MARKER_WINDOW`] of
    /// `now`.
    ///
    /// `previous.is_some()` is checked as well as the age, because a subject
    /// first seen a minute ago has a fresh `entered_at` and has crossed
    /// nothing.
    pub fn crossed_recently(&self, subject: &str, now: Timestamp) -> bool {
        self.marks.get(subject).is_some_and(|mark| {
            mark.previous.is_some() && now.since(mark.entered_at) <= MARKER_WINDOW
        })
    }

    /// The regime `subject` was last observed in, for a report.
    pub fn regime_of(&self, subject: &str) -> Option<&str> {
        self.marks.get(subject).map(|mark| mark.current.as_str())
    }

    /// Every subject that crossed within the window, in key order.
    pub fn crossed_within(&self, now: Timestamp) -> Vec<&str> {
        self.marks
            .iter()
            .filter(|(_, mark)| {
                mark.previous.is_some() && now.since(mark.entered_at) <= MARKER_WINDOW
            })
            .map(|(subject, _)| subject.as_str())
            .collect()
    }

    /// Drop the least recently changed mark once the set is full.
    ///
    /// Ties broken on the key so the choice is deterministic: a replay that
    /// evicted a different subject would diverge from here on.
    fn evict_if_full(&mut self) {
        if self.marks.len() < MAX_SUBJECTS {
            return;
        }
        let victim = self
            .marks
            .iter()
            .min_by(|a, b| {
                a.1.entered_at
                    .cmp(&b.1.entered_at)
                    .then_with(|| a.0.cmp(b.0))
            })
            .map(|(subject, _)| subject.clone());
        if let Some(victim) = victim {
            self.marks.remove(&victim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: i64) -> Timestamp {
        Timestamp::from_secs(hours * 3_600)
    }

    #[test]
    fn a_subject_with_no_edges_has_no_boundary_uncertainty_rather_than_a_zero() {
        // Zero would be a row in the plan reading as a choice nobody could
        // take; `None` is the graph saying it makes no claim about the
        // subject at all.
        assert_eq!(untested_share(std::iter::empty()), None);
    }

    #[test]
    fn the_boundary_uncertainty_is_the_share_of_edges_untested_in_the_regime_entered() {
        assert_eq!(
            untested_share([
                ConditionStanding::Untested,
                ConditionStanding::Holds,
                ConditionStanding::Holds,
                ConditionStanding::Holds,
            ]),
            Some(0.25),
            "the share counted something other than the untested edges"
        );
        assert_eq!(
            untested_share([ConditionStanding::Untested, ConditionStanding::Untested]),
            Some(1.0)
        );
        assert_eq!(
            untested_share([ConditionStanding::Holds, ConditionStanding::Holds]),
            Some(0.0),
            "a subject whose every edge was tested here has nothing left to probe"
        );
    }

    #[test]
    fn an_edge_known_to_fail_in_the_regime_entered_counts_as_tested_and_not_as_ignorance() {
        // The arm most easily got backwards. A refuted edge is the one case
        // where the platform knows exactly how it behaves here - it does not
        // - so probing buys nothing. Counting it as ignorance would aim the
        // budget at the edges the platform has already finished learning
        // about.
        assert_eq!(
            untested_share([ConditionStanding::KnownToFail, ConditionStanding::Holds]),
            Some(0.0),
            "a refutation was charged to the budget as something still unknown"
        );
        assert_eq!(
            untested_share([ConditionStanding::KnownToFail, ConditionStanding::Untested]),
            Some(0.5)
        );
    }

    #[test]
    fn a_subject_seen_for_the_first_time_has_not_crossed_a_boundary() {
        let mut transitions = RegimeTransitions::new();
        assert_eq!(
            transitions.observe("btc-usd", "trending/normal", at(0)),
            None
        );
        assert!(!transitions.crossed_recently("btc-usd", at(0)));
        // The mark is seated even though nothing crossed — otherwise the
        // second observation would also read as a first sighting.
        assert_eq!(transitions.len(), 1);
    }

    #[test]
    fn a_subject_whose_regime_key_changes_reports_the_boundary_it_crossed() {
        let mut transitions = RegimeTransitions::new();
        transitions.observe("btc-usd", "trending/normal", at(0));
        let crossing = transitions
            .observe("btc-usd", "crisis/extreme", at(5))
            .expect("a changed regime key is a crossing");
        assert_eq!(crossing.from, "trending/normal");
        assert_eq!(crossing.to, "crisis/extreme");
        assert_eq!(crossing.at, at(5));
        assert!(transitions.crossed_recently("btc-usd", at(5)));
    }

    #[test]
    fn observing_the_same_regime_again_does_not_refresh_when_it_was_entered() {
        let mut transitions = RegimeTransitions::new();
        transitions.observe("btc-usd", "quiet/low", at(0));
        transitions.observe("btc-usd", "crisis/extreme", at(1));
        // Forty-nine passes later, still in crisis. The crossing is an hour
        // old plus forty-nine, which is past the window: if a repeat
        // observation moved `entered_at` the boundary would read as fresh
        // forever and the probe would never stop being a candidate.
        for hour in 2..=50 {
            assert_eq!(
                transitions.observe("btc-usd", "crisis/extreme", at(hour)),
                None
            );
        }
        assert!(!transitions.crossed_recently("btc-usd", at(50)));
    }

    #[test]
    fn a_crossing_stops_being_recent_once_the_window_has_passed() {
        let mut transitions = RegimeTransitions::new();
        transitions.observe("eth-usd", "quiet/low", at(0));
        transitions.observe("eth-usd", "trending/high", at(1));
        assert!(transitions.crossed_recently("eth-usd", at(1 + 48)));
        assert!(!transitions.crossed_recently("eth-usd", at(1 + 49)));
    }

    #[test]
    fn the_remembered_set_is_bounded_and_evicts_the_least_recently_changed() {
        let mut transitions = RegimeTransitions::new();
        for n in 0..MAX_SUBJECTS {
            // i64 from usize: a loop index, not a measurement.
            transitions.observe(&format!("s{n:05}"), "quiet/low", at(n as i64));
        }
        assert_eq!(transitions.len(), MAX_SUBJECTS);
        transitions.observe("newcomer", "quiet/low", at(100_000));
        assert_eq!(transitions.len(), MAX_SUBJECTS);
        assert_eq!(transitions.regime_of("s00000"), None);
        assert_eq!(transitions.regime_of("newcomer"), Some("quiet/low"));
    }

    #[test]
    fn an_evicted_subject_seen_again_re_enters_as_a_first_sighting_rather_than_a_crossing() {
        let mut transitions = RegimeTransitions::new();
        transitions.observe("btc-usd", "quiet/low", at(0));
        for n in 0..MAX_SUBJECTS {
            transitions.observe(&format!("s{n:05}"), "quiet/low", at(n as i64 + 1));
        }
        assert_eq!(transitions.regime_of("btc-usd"), None);
        // Fails towards "no transition": the platform forgot, so it claims
        // nothing rather than claiming a boundary it never observed.
        assert_eq!(
            transitions.observe("btc-usd", "crisis/extreme", at(200_000)),
            None
        );
        assert!(!transitions.crossed_recently("btc-usd", at(200_000)));
    }

    #[test]
    fn crossed_within_lists_only_the_subjects_that_actually_crossed() {
        let mut transitions = RegimeTransitions::new();
        transitions.observe("a", "quiet/low", at(0));
        transitions.observe("b", "quiet/low", at(0));
        transitions.observe("c", "quiet/low", at(0));
        transitions.observe("b", "crisis/extreme", at(1));
        transitions.observe("c", "trending/high", at(1));
        assert_eq!(transitions.crossed_within(at(2)), vec!["b", "c"]);
    }
}
