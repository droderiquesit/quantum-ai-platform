//! Blueprint §9.2's qualifier, applied to the graph the platform has already
//! written, and §8.2's concentration query over the same graph.
//!
//! # What was wrong, and it was not a gap
//!
//! ADR 0054 gave `WorldModel::claim_causal` its first real production writer:
//! `Platform::discover_temporal_precedence` scans pairs of instruments in
//! `price_history` every UNDERSTAND stage and writes an edge for each pair
//! clearing a strict F-test. That writer exists, runs, and writes edges built
//! from ingested bars.
//!
//! It also runs the test **uncontrolled**, and §9.2 does not name an
//! uncontrolled test. The method the blueprint names is "Granger-style
//! lead-lag *with controls* — temporal precedence with confounders explicitly
//! adjusted", and the qualifier is not decoration. Every instrument in one
//! book moves partly with the book. Regress any instrument's future on
//! another's past and the shared component shows through, because the
//! effect's own lag — the only control an uncontrolled test has — absorbs it
//! imperfectly. The result is that one common driver manufactures an edge
//! between very many of the pairs it touches. Each edge is significant. Each
//! is spurious. And they are spurious *together*, which is the precise
//! failure the whole of blueprint §9 was written to answer: "when a regime
//! breaks, models that learned the same spurious structure break together,
//! and nothing in the system can say which relationships should have
//! survived."
//!
//! So a graph fed by an uncontrolled scan is not a sparse graph that needs
//! filling. It is a graph whose density *is* the artefact, and adding
//! traversal queries on top of it without saying so would be building a risk
//! surface over manufactured edges.
//!
//! # What this module does about it
//!
//! [`audit_controls`] re-runs each precedence edge's own test with a
//! measured common driver held constant, and reports the edges that do not
//! survive. It writes nothing: it is a read over the graph producing a
//! finding, and retracting an edge is a decision with a writer of its own
//! (see [`ControlAudit`]'s note on what is deliberately not done here).
//!
//! [`concentration`] answers §8.2's fourth query — "which of my current
//! positions share an underlying exposure I have not counted?" — over the
//! same graph, and carries each finding's dependence on an unaudited edge
//! with it rather than presenting all findings alike.
//!
//! # The common driver is measured, not assumed
//!
//! The control is the equal-weighted cross-sectional mean log return of the
//! tracked universe, computed from the same `price_history` the edges were
//! built from. It is a real series the platform holds, not a coefficient
//! anybody chose, and the audit says exactly how many instruments went into
//! it so that a reader can judge it.
//!
//! **The pair under test is excluded from its own control.** A factor
//! containing the cause and the effect is regressing each series partly on
//! itself, and would refuse edges for a reason that has nothing to do with
//! confounding. That exclusion is why the audit reports pairs it could not
//! judge instead of quietly judging them badly.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::{Duration, Timestamp};
use qip_world_model::causal::{CausalGraph, EdgeStanding, Mechanism};
use qip_world_model::confounder::{Confounder, ConfounderSet};
use qip_world_model::exposure::{self, ConcentrationReport};
use qip_world_model::granger;
use serde::{Deserialize, Serialize};

/// The fewest instruments *besides* the pair under test that the
/// cross-sectional factor must average before it is used as a control.
///
/// One would be a control, arithmetically. It would also be a single other
/// instrument's return wearing the word "market", and refusing an edge on
/// that basis would be worse than not auditing it: the finding would read as
/// "this edge is confounded" when what happened is "one unrelated name moved".
/// Three is the smallest number at which the average is an average.
pub const MIN_FACTOR_CONSTITUENTS: usize = 3;

/// The most edges one audit pass examines.
///
/// A bounded working set, matching the discipline of the writer it audits:
/// `Platform::discover_temporal_precedence` caps pairs per cycle, and an
/// audit with no cap would be the unbounded pass in a system that took care
/// not to have one. The graph is walked in its own stable order, so
/// successive cycles cover it rather than one cycle covering all of it.
pub const MAX_EDGES_AUDITED_PER_PASS: usize = 200;

/// The id the cross-sectional control is recorded under on any edge
/// established with it.
///
/// A constant rather than a formatted string: it becomes a label on an
/// edge's `adjusted_for`, it is compared across cycles, and a name that
/// varied with the universe size would make two edges adjusted for the same
/// thing look adjusted for different things.
pub const CROSS_SECTIONAL_FACTOR: &str = "cross_sectional_mean_return";

/// One edge the control removed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnsupportedEdge {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The confidence the edge carries in the graph, established without the
    /// control. Carried so the finding can say what is being questioned
    /// rather than only that something is.
    pub uncontrolled_confidence: f64,
    /// How many instruments the control averaged, so a reader can weigh the
    /// finding rather than take it.
    pub factor_constituents: usize,
}

impl UnsupportedEdge {
    /// A line an operator can read without holding the struct.
    pub fn explain(&self) -> String {
        format!(
            "{} -> {} ({}) held at confidence {:.3} without controls and does not clear its own \
             bar once the mean return of {} other tracked instrument(s) is held constant",
            self.cause,
            self.effect,
            self.mechanism.as_str(),
            self.uncontrolled_confidence,
            self.factor_constituents
        )
    }
}

/// What [`audit_controls`] found.
///
/// # What this deliberately does not do
///
/// It does not retract, attenuate, or re-mark an edge. Three reasons, and
/// the first is the one that matters.
///
/// A control that both finds a problem and silently fixes it leaves nothing
/// in the record naming the number that changed — the same argument
/// `CausalEdge::decayed_at` already makes for being a mark rather than an
/// attenuation. Second, the audit's own power depends on the universe size,
/// so an edge unsupported in a thin universe may be supported in a fuller
/// one a week later, and a retraction would be irreversible on evidence that
/// is not. Third, a writer belongs where writers are: the graph has exactly
/// two today, and a third that fires from a read path is the kind of seam
/// nobody finds again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlAudit {
    /// Precedence edges in the graph at `known_at`.
    pub edges_examined: usize,
    /// Of those, the ones a control could actually be built for and a test
    /// actually run on.
    pub edges_audited: usize,
    /// Edges skipped because the universe was too thin to build a factor
    /// excluding the pair, or because a series was too short.
    ///
    /// Reported rather than folded into "supported". An edge nobody could
    /// test is not an edge that passed a test, and a report that conflated
    /// the two would be a control that cannot fire reading as one that did.
    pub unauditable: usize,
    /// Instruments in the tracked universe the factor was drawn from.
    pub universe_size: usize,
    /// Findings, least-supported first.
    pub unsupported: Vec<UnsupportedEdge>,
}

impl ControlAudit {
    /// Whether this audit is evidence of anything.
    ///
    /// An empty `unsupported` means "every audited edge survived its
    /// control" only when something was audited. With `edges_audited == 0`
    /// the same empty list means "nothing was asked", and a caller reporting
    /// the first when it has the second has built a control that cannot fire
    /// and reads as protection — the `MaxExpectedShortfall` shape this
    /// repository already records one example of and does not need a second.
    pub fn was_answerable(&self) -> bool {
        self.edges_audited > 0
    }

    /// A one-line summary for a stage outcome, or `None` when the pass had
    /// nothing to say.
    ///
    /// `None` rather than a cheerful string, so that a stage report stays
    /// silent about a pass that could not run instead of implying it ran
    /// clean.
    pub fn summary(&self) -> Option<String> {
        if !self.was_answerable() {
            return None;
        }
        Some(format!(
            "; causal control audit: {} of {} precedence edge(s) did not survive a \
             cross-sectional control ({} unauditable, universe {})",
            self.unsupported.len(),
            self.edges_audited,
            self.unauditable,
            self.universe_size
        ))
    }
}

/// Re-test every temporal-precedence edge in `causal` with a measured common
/// driver held constant, and report the ones that do not survive.
///
/// # What makes this return a finding
///
/// A graph holding at least one [`Mechanism::TemporalPrecedence`] or
/// [`Mechanism::InverseTemporalPrecedence`] edge recorded at or before
/// `known_at`; `returns` holding series for both its ends and for at least
/// [`MIN_FACTOR_CONSTITUENTS`] other instruments; and the pair's own test
/// failing to clear `qip_world_model::granger`'s bar once the mean of those
/// others is a regressor in both fits. That is exactly the shape a shared
/// driver produces, and exactly what `Platform::discover_temporal_precedence`
/// writes today without the control — so this is reachable from ingested
/// data and not only from a constructed graph.
///
/// # Point in time
///
/// `known_at` filters the edges through [`CausalGraph::outgoing`]; an edge
/// recorded after the instant being asked about is not audited at that
/// instant, because it was not knowable then. `returns` is the caller's
/// history as of the same instant, and passing a longer history than the
/// caller held would be look-ahead this function cannot detect — which is
/// why it takes the history rather than reaching for it.
///
/// Mechanism-backed edges are left alone. A supply-chain claim asserted by a
/// person with evidence behind it is not a lead-lag test and there is
/// nothing here to re-run: auditing it against a return factor would be
/// answering a question nobody asked with statistics that do not bear on it.
pub fn audit_controls(
    causal: &CausalGraph,
    returns: &BTreeMap<String, Vec<f64>>,
    bar_interval: Duration,
    known_at: Timestamp,
) -> ControlAudit {
    let mut audit = ControlAudit {
        universe_size: returns.len(),
        ..ControlAudit::default()
    };

    for edge in causal.edges() {
        if audit.edges_examined >= MAX_EDGES_AUDITED_PER_PASS {
            break;
        }
        // Point in time: an edge not yet knowable is not audited, for the
        // same reason it is not readable.
        if edge.recorded_at > known_at {
            continue;
        }
        if !matches!(
            edge.mechanism,
            Mechanism::TemporalPrecedence | Mechanism::InverseTemporalPrecedence
        ) {
            continue;
        }
        audit.edges_examined += 1;

        let (Some(cause_returns), Some(effect_returns)) =
            (returns.get(&edge.cause), returns.get(&edge.effect))
        else {
            audit.unauditable += 1;
            continue;
        };

        let Some((factor, constituents)) =
            cross_sectional_factor(returns, &edge.cause, &edge.effect)
        else {
            audit.unauditable += 1;
            continue;
        };

        let Ok(confounder) = Confounder::observed(
            CROSS_SECTIONAL_FACTOR,
            "the equal-weighted mean return of the tracked universe excluding this pair — the \
             shared component an uncontrolled lead-lag test would otherwise read as a link",
            factor,
        ) else {
            audit.unauditable += 1;
            continue;
        };
        let Ok(controls) = ConfounderSet::new().with(confounder) else {
            audit.unauditable += 1;
            continue;
        };

        match granger::establish_temporal_precedence_controlling_for(
            &edge.cause,
            cause_returns,
            &edge.effect,
            effect_returns,
            &controls,
            bar_interval,
            known_at,
        ) {
            // Cleared its bar with the driver held constant: the edge says
            // something the common component does not.
            Ok(Some(_)) => audit.edges_audited += 1,
            // Ran, and did not clear it. This is the finding.
            Ok(None) => {
                audit.edges_audited += 1;
                audit.unsupported.push(UnsupportedEdge {
                    cause: edge.cause.clone(),
                    effect: edge.effect.clone(),
                    mechanism: edge.mechanism,
                    uncontrolled_confidence: edge.confidence,
                    factor_constituents: constituents,
                });
            }
            // A malformed pair — a non-finite return, a length mismatch. Not
            // a finding about the edge, and counted as unauditable rather
            // than allowed to look like one.
            Err(_) => audit.unauditable += 1,
        }
    }

    // Highest uncontrolled confidence first: the edge the platform trusted
    // most and can support least is the one to read first.
    audit.unsupported.sort_by(|a, b| {
        b.uncontrolled_confidence
            .partial_cmp(&a.uncontrolled_confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cause.cmp(&b.cause))
            .then_with(|| a.effect.cmp(&b.effect))
    });
    audit
}

/// The equal-weighted mean return across the universe, excluding the pair
/// under test, or `None` if too few instruments remain.
///
/// # Why the pair is excluded, and why `None` rather than a fallback
///
/// A factor containing the cause and the effect regresses each series partly
/// on itself. Every edge would fail such a control, and the audit would read
/// as "the graph is entirely confounded" when what happened is that the
/// control was built wrong. There is no honest fallback — a factor that
/// includes the pair is not a weaker control, it is a different and wrong
/// one — so this refuses and the caller counts the edge unauditable.
///
/// Series shorter than the longest are padded at neither end: the mean at
/// each index is taken over the constituents that *have* that index,
/// counting from the most recent observation backwards, because bar `i` from
/// the end is the same bar across instruments and bar `i` from the start is
/// not.
fn cross_sectional_factor(
    returns: &BTreeMap<String, Vec<f64>>,
    cause: &str,
    effect: &str,
) -> Option<(Vec<f64>, usize)> {
    // BTreeMap iteration: the constituents enter the mean in a stable order,
    // so the factor is bit-identical across replays. Floating-point addition
    // is not associative, and a factor summed in hash order would give a
    // different last bit and, at a bar's worth of bad luck, a different side
    // of the significance bar.
    let constituents: Vec<&Vec<f64>> = returns
        .iter()
        .filter(|(id, _)| id.as_str() != cause && id.as_str() != effect)
        .map(|(_, series)| series)
        .collect();
    if constituents.len() < MIN_FACTOR_CONSTITUENTS {
        return None;
    }

    // The control must be sampled on the same bars as the pair, so it is as
    // long as the pair's own series and aligned to the most recent bar.
    let length = returns.get(cause)?.len().min(returns.get(effect)?.len());
    if length == 0 {
        return None;
    }

    let mut factor = vec![0.0; length];
    for (offset, slot) in factor.iter_mut().enumerate() {
        // Counting back from the newest bar, which is the instant the
        // instruments share. Aligning from the front would pair last
        // Tuesday's bar for one name with last year's for another.
        let from_end = length - offset;
        let mut sum = 0.0;
        let mut count = 0usize;
        for series in &constituents {
            if series.len() >= from_end {
                let value = series[series.len() - from_end];
                if value.is_finite() {
                    sum += value;
                    count += 1;
                }
            }
        }
        if count == 0 {
            // No constituent covers this bar. Zero is the honest value for
            // "the rest of the universe contributed nothing measurable
            // here", and it is not a guess about the market: it is the mean
            // of an empty set of returns, used as a regressor that moves the
            // fit not at all on that row.
            *slot = 0.0;
        } else {
            *slot = sum / count as f64;
        }
    }
    Some((factor, constituents.len()))
}

/// §8.2's fourth query over the platform's own graph: which held positions
/// share a driver the book does not hold?
///
/// A thin pass-through to [`exposure::hidden_concentration`], and it exists
/// as a kernel entry point rather than a direct call for one reason: the
/// audit above changes how a finding should be read. A concentration resting
/// on edges a control would remove is a different claim from one resting on
/// edges that survived, and this is the seam where both facts are in scope.
///
/// # What makes this return something
///
/// Two or more members of `held` with a causal edge arriving from one cause
/// that is not itself in `held`, recorded at or before `known_at`.
pub fn concentration(
    causal: &CausalGraph,
    held: &BTreeSet<String>,
    known_at: Timestamp,
) -> ConcentrationReport {
    exposure::hidden_concentration(causal, held, known_at)
}

/// Whether any finding in `report` rests on an edge the audit could not
/// support.
///
/// Read together rather than separately on purpose: a concentration report
/// and a control audit that never meet let an operator act on a
/// concentration whose every leg the audit had already questioned.
pub fn concentration_rests_on_unaudited_edges(report: &ConcentrationReport) -> bool {
    report.drivers.iter().any(|d| d.rests_on_suggestive_edge)
}

/// Whether `standing` permits a finding to be reported as established.
///
/// A single place for the question so that two callers cannot answer it
/// differently.
pub fn is_established(standing: EdgeStanding) -> bool {
    standing == EdgeStanding::Established
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_world_model::causal::CausalEdge;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    /// A universe in which one **persistent** factor drives every instrument
    /// contemporaneously, and no instrument drives any other at any lag.
    ///
    /// Every edge an uncontrolled lead-lag test finds here is by
    /// construction spurious, because there is no lagged path between any
    /// two names in the generating process — only a shared driver.
    ///
    /// # Why the driver has to be autocorrelated, and it was not at first
    ///
    /// The first version of this fixture used a white-noise driver, and the
    /// test's own premise assertion refused it: with no persistence in the
    /// driver, one name's past carries nothing about another's future and
    /// the uncontrolled test correctly found no edge, so there was nothing
    /// for the control to remove. That is worth recording, because it is
    /// also the reason the confounding is real rather than an artefact of
    /// the fixture. With an AR(1) driver, `AAA[t-1]` and `BBB[t-1]` are two
    /// *noisy* proxies for `driver[t-1]`, which predicts `driver[t]` and so
    /// `BBB[t]`. `BBB`'s own lag — the only control an uncontrolled test has
    /// — absorbs the driver imperfectly because of its idiosyncratic
    /// component, so adding `AAA`'s lag genuinely improves the fit. That is
    /// confounding by measurement error, it is the ordinary condition of
    /// every equity book, and it is exactly what an uncontrolled pairwise
    /// scan reads as a causal link.
    ///
    /// Deterministic rather than sampled: a linear congruential sequence, so
    /// the verdict does not depend on which run it was.
    fn common_driver_universe(names: &[&str], bars: usize) -> BTreeMap<String, Vec<f64>> {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            ((state >> 33) as f64 / (1u64 << 31) as f64) - 0.5
        };
        // A persistent common driver. The persistence is what gives one
        // name's past information about another's future.
        let mut driver = Vec::with_capacity(bars);
        let mut level = 0.0;
        for _ in 0..bars {
            level = 0.8 * level + next();
            driver.push(level);
        }
        let mut universe = BTreeMap::new();
        for name in names {
            // Contemporaneous loading only — no lag of the driver and no
            // reference to any other name, so nothing in this process is a
            // lead-lag relationship between two instruments.
            let series: Vec<f64> = (0..bars).map(|t| driver[t] + 0.6 * next()).collect();
            universe.insert((*name).to_string(), series);
        }
        universe
    }

    fn precedence_edge(cause: &str, effect: &str) -> CausalEdge {
        CausalEdge::new(
            cause,
            effect,
            Mechanism::TemporalPrecedence,
            0.3,
            Duration::from_days(1),
            at(1_000),
        )
        .with_confidence(0.45)
    }

    #[test]
    fn an_edge_that_is_only_a_shared_driver_does_not_survive_the_control() {
        let names = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"];
        let universe = common_driver_universe(&names, 200);
        // The premise: the uncontrolled test really does write this edge, so
        // the refusal below is the control working rather than a pair that
        // was never significant.
        let uncontrolled = granger::establish_temporal_precedence(
            "AAA",
            &universe["AAA"],
            "BBB",
            &universe["BBB"],
            Duration::from_days(1),
            at(1_000),
        )
        .expect("the pair is well formed");
        assert!(
            uncontrolled.is_some(),
            "the premise: an uncontrolled test writes an edge for this pair — without this the \
             audit below would be refusing something nothing produced"
        );

        let mut causal = CausalGraph::new();
        causal.add(precedence_edge("AAA", "BBB"));
        let audit = audit_controls(&causal, &universe, Duration::from_days(1), at(2_000));

        assert!(audit.was_answerable(), "the audit ran on a real edge");
        assert_eq!(audit.edges_audited, 1);
        assert_eq!(
            audit.unsupported.len(),
            1,
            "an edge that is nothing but the shared driver must not survive the control"
        );
        assert_eq!(audit.unsupported[0].cause, "AAA");
        assert!(audit.unsupported[0].factor_constituents >= MIN_FACTOR_CONSTITUENTS);
    }

    #[test]
    fn an_edge_the_control_cannot_be_built_for_is_counted_unauditable_and_never_supported() {
        // Two instruments only: excluding the pair leaves nothing to average.
        let universe = common_driver_universe(&["AAA", "BBB"], 200);
        let mut causal = CausalGraph::new();
        causal.add(precedence_edge("AAA", "BBB"));

        let audit = audit_controls(&causal, &universe, Duration::from_days(1), at(2_000));
        assert_eq!(
            audit.edges_examined, 1,
            "the premise: the edge was in scope"
        );
        assert_eq!(audit.unauditable, 1, "and could not be judged");
        assert_eq!(audit.edges_audited, 0);
        assert!(
            audit.unsupported.is_empty(),
            "an edge nobody could test is not a finding against it"
        );
        // The distinction the whole type exists for: this empty finding list
        // is not evidence of a healthy graph.
        assert!(
            !audit.was_answerable(),
            "an audit that judged nothing must not read as an audit that found nothing"
        );
        assert!(
            audit.summary().is_none(),
            "and it must not write a reassuring line into a stage report"
        );
    }

    #[test]
    fn a_mechanism_backed_edge_is_left_alone_by_the_control_audit() {
        let universe = common_driver_universe(&["AAA", "BBB", "CCC", "DDD", "EEE"], 200);
        let mut causal = CausalGraph::new();
        causal.add(CausalEdge::new(
            "AAA",
            "BBB",
            Mechanism::SupplyChain,
            0.6,
            Duration::from_days(1),
            at(1_000),
        ));
        let audit = audit_controls(&causal, &universe, Duration::from_days(1), at(2_000));
        assert_eq!(
            audit.edges_examined, 0,
            "a person's mechanism claim is not a lead-lag test and there is nothing to re-run"
        );
        assert!(audit.unsupported.is_empty());
    }

    #[test]
    fn an_edge_not_yet_knowable_is_not_audited_at_an_earlier_instant() {
        let universe = common_driver_universe(&["AAA", "BBB", "CCC", "DDD", "EEE"], 200);
        let mut causal = CausalGraph::new();
        causal.add(precedence_edge("AAA", "BBB"));

        // The premise: at a later instant the edge is examined.
        let later = audit_controls(&causal, &universe, Duration::from_days(1), at(2_000));
        assert_eq!(
            later.edges_examined, 1,
            "the premise: the edge is auditable later"
        );

        let earlier = audit_controls(&causal, &universe, Duration::from_days(1), at(500));
        assert_eq!(
            earlier.edges_examined, 0,
            "an edge recorded at 1,000 is not knowable at 500, and auditing it there would be \
             reading a fact before the instant it became knowable"
        );
    }

    #[test]
    fn the_cross_sectional_factor_never_contains_the_pair_it_controls() {
        let mut returns = BTreeMap::new();
        // The pair carries a value no other name has, so its presence in the
        // factor would be arithmetically visible.
        returns.insert("AAA".to_string(), vec![100.0, 100.0, 100.0]);
        returns.insert("BBB".to_string(), vec![100.0, 100.0, 100.0]);
        for name in ["CCC", "DDD", "EEE"] {
            returns.insert(name.to_string(), vec![1.0, 1.0, 1.0]);
        }
        let (factor, constituents) =
            cross_sectional_factor(&returns, "AAA", "BBB").expect("three others is enough");
        assert_eq!(constituents, 3, "the premise: three constituents, not five");
        // 1.0 exactly: the mean of the three others. Any contribution from
        // the pair's 100.0 would move this by tens.
        assert!(
            factor.iter().all(|v| (*v - 1.0).abs() < 1e-12),
            "the pair must not appear in the factor that controls it; got {factor:?}"
        );
    }

    #[test]
    fn a_universe_too_thin_to_average_yields_no_factor_rather_than_a_thin_one() {
        let mut returns = BTreeMap::new();
        for name in ["AAA", "BBB", "CCC", "DDD"] {
            returns.insert(name.to_string(), vec![0.1, 0.2, 0.3]);
        }
        // Four names minus the pair leaves two, one short of the floor.
        assert!(
            cross_sectional_factor(&returns, "AAA", "BBB").is_none(),
            "two constituents is not an average and must not be used as one"
        );
        returns.insert("EEE".to_string(), vec![0.1, 0.2, 0.3]);
        assert!(
            cross_sectional_factor(&returns, "AAA", "BBB").is_some(),
            "and the floor admits the smallest real universe — a gate that refuses everything \
             is not a working gate"
        );
    }
}
