//! Blueprint §8.2's traversal queries, and §9.3's "hidden concentration".
//!
//! §8.2 argues that the graph earns its place by turning one observation
//! into several positions, and names five queries it enables. The traversal
//! *primitives* have existed in [`crate::graph`] since this crate did —
//! `neighbours`, `paths_between`, `reachable`. What has not existed is the
//! queries themselves, which is a different thing: a primitive is a way to
//! walk, and a query is a question with a bounded answer somebody can act
//! on.
//!
//! # Point-in-time is the whole discipline here
//!
//! Every function in this module takes `valid_at` and `known_at` and passes
//! them down. A two-hop exposure assembled from a fact the platform learned
//! *after* the instant being asked about is a look-ahead that no backtest
//! result will reveal, because the answer looks better rather than
//! anomalous. There is no convenience overload taking a single timestamp,
//! deliberately: the only way to conflate the two dimensions here is to pass
//! the same value twice, which is visible at the call site.
//!
//! # What is bounded, and why every answer is
//!
//! Hops are capped, results are capped, and the frontier is capped. §8.2
//! says "directly or through two hops" and means it: a six-hop exposure is
//! not an exposure, it is a statement that the graph is connected. An
//! unbounded traversal over a graph in the low gigabytes (§8.3) is an
//! outage, and one that answers with every instrument in the universe is
//! worse than no answer because it reads as a finding.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use qip_core::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};

use crate::causal::{CausalEdge, CausalGraph};
use crate::graph::{KnowledgeGraph, NodeKind};

/// The hop ceiling §8.2 names: "directly or through two hops".
pub const MAX_EXPOSURE_HOPS: usize = 2;

/// The most instruments one exposure answer may name.
///
/// A bounded working set, and also a judgement: an answer naming more than
/// this is not a propagation candidate list, it is an observation that the
/// entity sits at a hub. The traversal reports that it truncated rather than
/// silently returning a prefix — a caller that cannot tell a complete answer
/// from a cut one will size against the cut one.
pub const MAX_EXPOSURE_RESULTS: usize = 256;

/// The most positions one shared-driver finding may name.
pub const MAX_CONCENTRATION_POSITIONS: usize = 256;

/// The fewest held positions a driver must reach before it is a
/// concentration at all.
///
/// Two. One position exposed to one driver is a position, not a
/// concentration, and reporting it as one would bury the real findings in
/// the trivial ones — which is the failure mode of every risk report nobody
/// reads.
pub const MIN_CONCENTRATION_POSITIONS: usize = 2;

/// One instrument reachable from the entity asked about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    /// The instrument node's id.
    pub instrument: String,
    /// Hops from the entity. `1` is direct.
    pub hops: usize,
    /// The node ids walked, entity first and instrument last, so the answer
    /// can be explained rather than only asserted — §9.3's "expressible as a
    /// path through the graph rather than as a model weight".
    pub path: Vec<String>,
    /// The product of the confidences of the facts walked.
    ///
    /// Multiplied rather than averaged or minimised: two independently
    /// uncertain links in series are less trustworthy than either alone, and
    /// an average would let a certain first hop launder an uncertain second.
    /// `f64` rather than `Decimal` because this is a statistic and never
    /// money — nothing downstream multiplies it by a position size without
    /// crossing back to `Decimal` at that seam.
    pub confidence: f64,
}

/// What [`instruments_exposed_to`] found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExposureSet {
    pub entity: String,
    pub hops_searched: usize,
    /// Ordered by hop count, then instrument id. A [`Vec`] built from a
    /// [`BTreeMap`] rather than a hash map: this reaches an operator and a
    /// replay that reorders is not a replay.
    pub exposures: Vec<Exposure>,
    /// Instruments found beyond [`MAX_EXPOSURE_RESULTS`] and dropped.
    ///
    /// Non-zero means the answer is a prefix. Named rather than silent for
    /// the reason [`crate::causal::PropagationResult::truncated`] is: a
    /// caller must be able to tell a complete answer from a cut one.
    pub truncated: usize,
}

impl ExposureSet {
    pub fn is_empty(&self) -> bool {
        self.exposures.is_empty()
    }

    pub fn instruments(&self) -> BTreeSet<String> {
        self.exposures
            .iter()
            .map(|e| e.instrument.clone())
            .collect()
    }
}

/// §8.2, query one: "Which instruments are exposed to this entity, directly
/// or through two hops?"
///
/// Breadth-first over the *relationship* graph — not the causal one. That a
/// company issues a security, or supplies another company that does, is a
/// fact; whether a shock travels along it is a claim, and
/// [`CausalGraph::propagate`] is where claims live. Conflating the two is
/// how a corporate structure becomes a thesis.
///
/// The first path found to an instrument wins, and it is the shortest,
/// because the frontier is explored in hop order. A longer route to
/// somewhere already reached adds a weaker path to the same name and nothing
/// else.
///
/// # What makes this return something
///
/// A graph holding a node for `entity`, at least one [`crate::Fact`] leaving
/// it that `holds(valid_at, known_at)`, and a
/// [`NodeKind::FinancialObject`] node within `max_hops` of it. The ordinary
/// production shape is a company that `Issues` a security — one hop — or a
/// supplier of a company that does — two.
///
/// Refuses rather than guessing on `max_hops == 0` (a zero-hop exposure
/// query is asking whether the entity is its own instrument) and on
/// `max_hops` beyond [`MAX_EXPOSURE_HOPS`] (§8.2's own ceiling; a caller
/// wanting five hops wants a different question).
pub fn instruments_exposed_to(
    graph: &KnowledgeGraph,
    entity: &str,
    max_hops: usize,
    valid_at: Timestamp,
    known_at: Timestamp,
) -> Result<ExposureSet> {
    if max_hops == 0 {
        return Err(Error::invalid(
            "an exposure query needs at least one hop; a zero-hop query asks whether the entity \
             is its own instrument",
        ));
    }
    if max_hops > MAX_EXPOSURE_HOPS {
        return Err(Error::invalid(format!(
            "{max_hops} hops exceeds the {MAX_EXPOSURE_HOPS} blueprint §8.2 names; a longer \
             chain is a statement that the graph is connected, not an exposure"
        )));
    }

    // Ordered, not hashed: `found` becomes the answer and the answer reaches
    // an operator.
    let mut found: BTreeMap<String, Exposure> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(entity.to_string());
    let mut truncated = 0usize;

    let mut frontier: VecDeque<(String, usize, Vec<String>, f64)> = VecDeque::new();
    frontier.push_back((entity.to_string(), 0, vec![entity.to_string()], 1.0));

    while let Some((node, hops, path, confidence)) = frontier.pop_front() {
        if hops >= max_hops {
            continue;
        }
        // `neighbours` already filters on `holds(valid_at, known_at)`, which
        // is where the point-in-time guarantee actually lives; passing both
        // instants down unchanged is the whole of this function's part in it.
        for fact in graph.neighbours(&node, None, valid_at, known_at) {
            let next = fact.relationship.to.clone();
            if seen.contains(&next) {
                continue;
            }
            let mut next_path = path.clone();
            next_path.push(next.clone());
            let next_confidence = confidence * fact.confidence;
            let next_hops = hops + 1;

            if graph
                .node(&next)
                .is_some_and(|n| n.kind == NodeKind::FinancialObject)
            {
                if found.len() >= MAX_EXPOSURE_RESULTS && !found.contains_key(&next) {
                    truncated += 1;
                } else {
                    found.entry(next.clone()).or_insert(Exposure {
                        instrument: next.clone(),
                        hops: next_hops,
                        path: next_path.clone(),
                        confidence: next_confidence,
                    });
                }
            }

            seen.insert(next.clone());
            frontier.push_back((next, next_hops, next_path, next_confidence));
        }
    }

    let mut exposures: Vec<Exposure> = found.into_values().collect();
    // Hop count first so the nearest exposures lead, then id so two
    // instruments at the same distance never swap places between runs.
    exposures.sort_by(|a, b| {
        a.hops
            .cmp(&b.hops)
            .then_with(|| a.instrument.cmp(&b.instrument))
    });

    Ok(ExposureSet {
        entity: entity.to_string(),
        hops_searched: max_hops,
        exposures,
        truncated,
    })
}

/// A driver two or more held positions depend on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedDriver {
    /// The common cause.
    pub driver: String,
    /// The held positions it reaches, in id order.
    pub positions: BTreeSet<String>,
    /// The weakest transmission among the edges to those positions.
    ///
    /// The *weakest*, not the strongest and not the mean. This number is
    /// read as "at least this much of the driver's move reaches every
    /// position named", and a mean would let one strong link advertise a
    /// concentration the other legs cannot carry.
    pub weakest_transmission: f64,
    /// Whether any edge underlying this finding is
    /// [`crate::causal::EdgeStanding::Suggestive`].
    ///
    /// Carried through rather than dropped, because a concentration resting
    /// on an edge with an unadjusted confounder is a weaker finding than one
    /// resting on adjusted edges, and a reader who cannot see the difference
    /// will act on both alike.
    pub rests_on_suggestive_edge: bool,
}

/// What [`hidden_concentration`] found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConcentrationReport {
    /// How many held positions were examined. The premise of every finding
    /// below, carried so that a caller can tell "no concentration" from
    /// "nothing was held".
    pub positions_examined: usize,
    /// How many causal edges were in scope at `known_at`. Zero means the
    /// graph was empty, which is a different fact from a clean book and must
    /// never read as one.
    pub edges_considered: usize,
    /// Findings, strongest first.
    pub drivers: Vec<SharedDriver>,
}

impl ConcentrationReport {
    pub fn is_clean(&self) -> bool {
        self.drivers.is_empty()
    }

    /// Whether this report is evidence of anything at all.
    ///
    /// **The distinction this method exists for is the one that makes a
    /// control real.** An empty `drivers` list means "no shared driver
    /// found" only when there were positions to examine *and* edges to
    /// examine them against. With either at zero the same empty list means
    /// "nothing was asked", and a caller that reports the first when it has
    /// the second has built a control that cannot fire and reads as
    /// protection — the shape this repository already records one example of
    /// in `MaxExpectedShortfall`.
    pub fn was_answerable(&self) -> bool {
        self.positions_examined >= MIN_CONCENTRATION_POSITIONS && self.edges_considered > 0
    }
}

/// §8.2, query four: "Which of my current positions share an underlying
/// exposure I have not counted?" — and §9.3's "positions that appear
/// diversified but share a causal driver are surfaced as concentration".
///
/// # Why a driver the book *holds* is not a finding
///
/// A driver that is itself one of `held` is counted concentration: the desk
/// can see it owns the thing. §8.2's question is about the exposure it has
/// *not* counted, so a held driver is excluded by name. Reporting it would
/// be technically true and would bury the findings that are not.
///
/// # What makes this return something
///
/// A causal graph holding two or more edges recorded at or before
/// `known_at`, leaving one cause, arriving at two different members of
/// `held`, where that cause is not itself in `held`. That is exactly the
/// shape [`crate::granger::establish_temporal_precedence_controlling_for`]
/// writes when one instrument leads two others — so this query is reachable
/// from data the platform actually ingests, not only from a hand-seeded
/// graph.
///
/// # Point in time
///
/// `known_at` filters the edges through [`CausalGraph::outgoing`], so a
/// concentration is only visible from the instant the edges establishing it
/// became knowable. An edge recorded later cannot make a past decision look
/// concentrated, which would be look-ahead pointed at the risk report.
pub fn hidden_concentration(
    causal: &CausalGraph,
    held: &BTreeSet<String>,
    known_at: Timestamp,
) -> ConcentrationReport {
    let mut report = ConcentrationReport {
        positions_examined: held.len(),
        ..ConcentrationReport::default()
    };

    // Driver to the held positions it reaches. BTreeMap because the findings
    // are ordered output.
    let mut reached: BTreeMap<String, Vec<&CausalEdge>> = BTreeMap::new();
    let mut counted: BTreeSet<(String, String)> = BTreeSet::new();
    for position in held {
        for edge in causal.incoming(position, known_at) {
            report.edges_considered += 1;
            // A driver the book holds is visible concentration, not hidden.
            if held.contains(&edge.cause) {
                continue;
            }
            // One edge per (driver, position). Two mechanisms between the
            // same pair are two claims about one dependency, and counting
            // both would let a pair masquerade as a concentration.
            if !counted.insert((edge.cause.clone(), position.clone())) {
                continue;
            }
            reached.entry(edge.cause.clone()).or_default().push(edge);
        }
    }

    for (driver, edges) in reached {
        if edges.len() < MIN_CONCENTRATION_POSITIONS {
            continue;
        }
        let positions: BTreeSet<String> = edges
            .iter()
            .map(|e| e.effect.clone())
            .take(MAX_CONCENTRATION_POSITIONS)
            .collect();
        let weakest_transmission = edges
            .iter()
            .map(|e| e.transmission())
            .fold(f64::INFINITY, f64::min);
        let rests_on_suggestive_edge = edges
            .iter()
            .any(|e| e.standing() == crate::causal::EdgeStanding::Suggestive);
        report.drivers.push(SharedDriver {
            driver,
            positions,
            weakest_transmission,
            rests_on_suggestive_edge,
        });
    }

    // Widest concentration first, then strongest, then by driver id so the
    // order is total and a replay reproduces it exactly.
    report.drivers.sort_by(|a, b| {
        b.positions
            .len()
            .cmp(&a.positions.len())
            .then_with(|| {
                b.weakest_transmission
                    .partial_cmp(&a.weakest_transmission)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.driver.cmp(&b.driver))
    });
    report
}

/// §8.2, query five: "Which entities does my portfolio depend on that I do
/// not hold?"
///
/// The same traversal as [`hidden_concentration`] with the threshold at one
/// rather than two, and that is the honest description of the difference —
/// second-order risk is the question "what am I exposed to", concentration
/// is "what am I exposed to more than once". Kept as a separate function
/// because the two answers are read by different people for different
/// reasons, and a single function with a threshold parameter invites a
/// caller to pass the wrong one.
///
/// # What makes this return something
///
/// One causal edge recorded at or before `known_at`, arriving at a member of
/// `held`, from a cause that is not in `held`.
pub fn unheld_dependencies(
    causal: &CausalGraph,
    held: &BTreeSet<String>,
    known_at: Timestamp,
) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    for position in held {
        for edge in causal.incoming(position, known_at) {
            if !held.contains(&edge.cause) {
                dependencies.insert(edge.cause.clone());
            }
        }
    }
    dependencies
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::causal::Mechanism;
    use qip_core::Duration;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn edge(cause: &str, effect: &str, recorded: i64) -> CausalEdge {
        CausalEdge::new(
            cause,
            effect,
            Mechanism::TemporalPrecedence,
            0.5,
            Duration::from_days(1),
            at(recorded),
        )
    }

    #[test]
    fn a_driver_reaching_two_held_positions_is_surfaced_as_hidden_concentration() {
        let mut causal = CausalGraph::new();
        causal.add(edge("FACTOR", "AAA", 1_000));
        causal.add(edge("FACTOR", "BBB", 1_000));
        // The premise: both positions are held and the driver is not.
        let held: BTreeSet<String> = ["AAA".to_string(), "BBB".to_string()].into_iter().collect();
        assert_eq!(held.len(), 2, "the premise: two positions are held");
        assert!(
            !held.contains("FACTOR"),
            "the premise: the driver is unheld"
        );

        let report = hidden_concentration(&causal, &held, at(2_000));
        assert!(
            report.was_answerable(),
            "the premise: there was a question to answer"
        );
        assert_eq!(report.drivers.len(), 1, "one shared driver");
        assert_eq!(report.drivers[0].driver, "FACTOR");
        assert_eq!(report.drivers[0].positions.len(), 2);
    }

    #[test]
    fn a_driver_the_book_already_holds_is_not_reported_as_hidden() {
        let mut causal = CausalGraph::new();
        causal.add(edge("FACTOR", "AAA", 1_000));
        causal.add(edge("FACTOR", "BBB", 1_000));
        // The premise: the same graph that produced a finding above.
        let unheld: BTreeSet<String> = ["AAA".to_string(), "BBB".to_string()].into_iter().collect();
        assert_eq!(
            hidden_concentration(&causal, &unheld, at(2_000))
                .drivers
                .len(),
            1,
            "the premise: this graph and these positions do produce a finding"
        );

        let held: BTreeSet<String> = ["AAA".to_string(), "BBB".to_string(), "FACTOR".to_string()]
            .into_iter()
            .collect();
        let report = hidden_concentration(&causal, &held, at(2_000));
        assert!(
            report.is_clean(),
            "a driver the desk can see it owns is counted exposure, not hidden concentration"
        );
    }

    #[test]
    fn a_concentration_is_invisible_before_the_instant_its_edges_became_knowable() {
        let mut causal = CausalGraph::new();
        causal.add(edge("FACTOR", "AAA", 5_000));
        causal.add(edge("FACTOR", "BBB", 5_000));
        let held: BTreeSet<String> = ["AAA".to_string(), "BBB".to_string()].into_iter().collect();

        // The premise: after the edges were recorded, the concentration is
        // there to be found.
        assert_eq!(
            hidden_concentration(&causal, &held, at(6_000))
                .drivers
                .len(),
            1,
            "the premise: the finding exists once the edges are knowable"
        );

        // The defect this prevents: a decision made at 4,000 reading a
        // concentration established at 5,000 is look-ahead that makes a
        // backtest look better rather than anomalous.
        let earlier = hidden_concentration(&causal, &held, at(4_000));
        assert!(
            earlier.is_clean(),
            "an edge recorded later must not make an earlier decision look concentrated"
        );
        assert_eq!(
            earlier.edges_considered, 0,
            "and the report says it examined nothing, so the empty answer is not read as clean"
        );
        assert!(
            !earlier.was_answerable(),
            "an empty finding over zero edges is not evidence of a clean book"
        );
    }

    #[test]
    fn a_single_position_on_a_driver_is_not_a_concentration() {
        let mut causal = CausalGraph::new();
        causal.add(edge("FACTOR", "AAA", 1_000));
        let held: BTreeSet<String> = ["AAA".to_string()].into_iter().collect();
        let report = hidden_concentration(&causal, &held, at(2_000));
        assert!(
            report.is_clean(),
            "one position on one driver is a position, not a concentration"
        );
        // But it is still a dependency the book does not hold.
        let dependencies = unheld_dependencies(&causal, &held, at(2_000));
        assert!(
            dependencies.contains("FACTOR"),
            "second-order risk asks a different question and must still answer it"
        );
    }

    #[test]
    fn an_exposure_query_refuses_a_hop_count_beyond_the_two_the_blueprint_names() {
        let graph = KnowledgeGraph::new();
        assert!(
            instruments_exposed_to(&graph, "E", MAX_EXPOSURE_HOPS + 1, at(1), at(1)).is_err(),
            "a deeper walk is refused rather than quietly capped"
        );
        assert!(
            instruments_exposed_to(&graph, "E", 0, at(1), at(1)).is_err(),
            "a zero-hop query is refused rather than answered with nothing"
        );
        assert!(
            instruments_exposed_to(&graph, "E", MAX_EXPOSURE_HOPS, at(1), at(1)).is_ok(),
            "and the gate admits the value the blueprint names — a gate that refuses \
             everything is not a working gate"
        );
    }
}
