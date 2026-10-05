//! Questions asked of the causal graph: interventions (REASON-006),
//! adjustment sets (REASON-007), counterfactuals (REASON-008) and abduction
//! (REASON-019, -020).
//!
//! The generated-graph properties are checked against references written
//! here over plain index pairs — a transitive closure, a path count, a
//! d-separation walk — that share no code with the graph they judge.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

use qip_core::testing::approx_eq;
use qip_core::{Duration, Timestamp};
use qip_world_model::causal::{CausalEdge, CausalGraph, Mechanism};
use qip_world_model::inference::{Identification, MAX_PATHS, Surprise};

fn recorded() -> Timestamp {
    Timestamp::from_civil(2026, 8, 1)
}

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn edge(
    cause: &str,
    effect: &str,
    mechanism: Mechanism,
    strength: f64,
    confidence: f64,
    evidence: &[&str],
) -> CausalEdge {
    CausalEdge::new(
        cause,
        effect,
        mechanism,
        strength,
        Duration::from_days(1),
        recorded(),
    )
    .unwrap()
    .with_confidence(confidence)
    .unwrap()
    .with_evidence(evidence.iter().map(|id| (*id).to_string()).collect())
}

/// Deterministic generator so a failing case is reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn name(node: usize) -> String {
    format!("v{node}")
}

/// A generated acyclic graph: edges run from a lower index to a higher one.
struct Dag {
    graph: CausalGraph,
    nodes: usize,
    /// `(cause, effect)` for every edge, in the order added — so position
    /// here is the edge's position in `graph.edges()`.
    edges: Vec<(usize, usize)>,
    /// Edges carrying an unobserved common cause.
    latent: BTreeSet<usize>,
}

fn generate(g: &mut Lcg, latent_one_in: u64) -> Dag {
    let nodes = 3 + g.next(5) as usize;
    let mut dag = Dag {
        graph: CausalGraph::new(),
        nodes,
        edges: Vec::new(),
        latent: BTreeSet::new(),
    };
    let mechanisms = [Mechanism::SupplyChain, Mechanism::DemandLinkage];
    for cause in 0..nodes {
        for effect in (cause + 1)..nodes {
            if g.next(100) >= 40 {
                continue;
            }
            // Sometimes two claims join the same pair, under two mechanisms.
            let copies = if g.next(6) == 0 { 2 } else { 1 };
            for mechanism in mechanisms.iter().take(copies) {
                let index = dag.edges.len();
                let evidence: Vec<String> =
                    (0..g.next(3)).map(|k| format!("ev-{index}-{k}")).collect();
                let mut claim = CausalEdge::new(
                    name(cause),
                    name(effect),
                    *mechanism,
                    0.5,
                    Duration::from_days(1),
                    recorded(),
                )
                .unwrap()
                .with_evidence(evidence);
                if latent_one_in > 0 && g.next(latent_one_in) == 0 {
                    claim = claim.with_confounders(
                        BTreeSet::new(),
                        BTreeSet::from([format!("hidden-{index}")]),
                    );
                    dag.latent.insert(index);
                }
                dag.graph.add(claim);
                dag.edges.push((cause, effect));
            }
        }
    }
    dag
}

impl Dag {
    /// `reach[a]` is every node downstream of `a`, by transitive closure.
    fn closure(&self, without: Option<usize>) -> Vec<BTreeSet<usize>> {
        let mut reach = vec![BTreeSet::new(); self.nodes];
        // Effects have higher indices, so walking causes downwards sees each
        // effect's closure already complete.
        for cause in (0..self.nodes).rev() {
            for (from, to) in &self.edges {
                if *from == cause && Some(*from) != without && Some(*to) != without {
                    let below = reach[*to].clone();
                    reach[cause].insert(*to);
                    reach[cause].extend(below);
                }
            }
        }
        reach
    }

    /// How many directed paths run from `from` to `to`, parallel edges
    /// counted separately.
    fn path_count(&self, from: usize, to: usize) -> usize {
        self.edges
            .iter()
            .filter(|(cause, _)| *cause == from)
            .map(|(_, effect)| {
                if *effect == to {
                    1
                } else {
                    self.path_count(*effect, to)
                }
            })
            .sum()
    }

    fn points(&self, from: usize, to: usize) -> bool {
        self.edges.contains(&(from, to))
    }
}

// ---- REASON-006 -----------------------------------------------------------

#[test]
fn an_intervention_reaches_exactly_the_descendants_along_directed_paths_that_cite_their_edges_and_evidence()
 {
    // The failure this prevents: an "affected" set cut short by a floor or a
    // hop limit and read as complete, and a path that names nodes where two
    // claims join the same pair — so nobody can say which claim, or what
    // evidence, the conclusion rests on.
    let mut g = Lcg(7);
    let (mut sinks, mut spreading, mut parallel, mut evidenced, mut deep) = (0, 0, 0, 0, 0);
    for _ in 0..150 {
        let dag = generate(&mut g, 0);
        let reach = dag.closure(None);
        // The premise of the loop: the closure covers every node.
        assert_eq!(reach.len(), dag.nodes);
        for (variable, downstream) in reach.iter().enumerate() {
            let intervention = dag.graph.intervene(&name(variable), now()).unwrap();
            assert_eq!(intervention.variable, name(variable));

            // Affected is exactly the descendants.
            let descendants: BTreeSet<String> = downstream.iter().map(|n| name(*n)).collect();
            assert_eq!(intervention.affected, descendants, "do({variable})");

            let mut seen: BTreeSet<Vec<usize>> = BTreeSet::new();
            let mut arriving: BTreeMap<String, usize> = BTreeMap::new();
            for path in &intervention.paths {
                // Every returned path is a directed path in the graph, from
                // the intervened variable to the target it names.
                assert_eq!(path.edges.first().unwrap().cause, name(variable));
                assert_eq!(path.edges.last().unwrap().effect, path.target);
                for pair in path.edges.windows(2) {
                    assert_eq!(pair[0].effect, pair[1].cause, "a broken path: {path:?}");
                }
                for cited in &path.edges {
                    // The citation is the graph's own edge, with its evidence.
                    let (from, to) = dag.edges[cited.index];
                    assert_eq!((&cited.cause, &cited.effect), (&name(from), &name(to)));
                    let held = &dag.graph.edges()[cited.index];
                    assert_eq!(cited.mechanism, held.mechanism);
                    assert_eq!(cited.evidence, held.evidence);
                    evidenced += usize::from(!cited.evidence.is_empty());
                    parallel += usize::from(cited.mechanism == Mechanism::DemandLinkage);
                }
                deep += usize::from(path.edges.len() > 2);
                assert!(seen.insert(path.edges.iter().map(|e| e.index).collect()));
                *arriving.entry(path.target.clone()).or_default() += 1;
            }
            // Every path, not one per target: the count matches the graph's.
            for target in downstream {
                assert_eq!(
                    arriving.get(&name(*target)).copied().unwrap_or(0),
                    dag.path_count(variable, *target),
                    "paths from {variable} to {target}"
                );
            }

            if dag.edges.iter().any(|(cause, _)| *cause == variable) {
                spreading += 1;
            } else {
                // An intervention on a sink affects nothing else.
                assert!(intervention.affected.is_empty() && intervention.paths.is_empty());
                sinks += 1;
            }
            // Before any claim was recorded, nothing was knowable.
            let before = recorded().saturating_sub(Duration::from_days(1));
            assert!(
                dag.graph
                    .intervene(&name(variable), before)
                    .unwrap()
                    .affected
                    .is_empty()
            );
        }
    }
    // The premise: sinks and sources both occurred, paths longer than two
    // hops were followed, parallel claims were cited, and evidence was held.
    assert!(
        sinks > 100 && spreading > 100 && parallel > 100 && evidenced > 100 && deep > 100,
        "{sinks} {spreading} {parallel} {evidenced} {deep}"
    );

    // Past the bound the query is refused, never truncated: fourteen nodes
    // each pointing at every later one hold 2^13 - 1 paths from the first.
    let mut dense = CausalGraph::new();
    for cause in 0..14 {
        for effect in (cause + 1)..14 {
            dense.add(edge(
                &name(cause),
                &name(effect),
                Mechanism::SupplyChain,
                0.5,
                0.7,
                &[],
            ));
        }
    }
    const { assert!((1usize << 13) - 1 > MAX_PATHS) };
    let error = dense.intervene("v0", now()).unwrap_err();
    assert!(
        error.message().contains("more than 4096 paths"),
        "{error:?}"
    );
}

#[test]
fn a_propagated_effect_cites_the_edge_it_travelled_where_two_claims_join_the_same_pair() {
    // `Effect::path` names nodes. With two claims between the same pair it
    // cannot say which was followed, and a reader checking the evidence would
    // find both — including the one the number did not come from.
    let mut graph = CausalGraph::new();
    graph.add(edge(
        "kestrel",
        "northwind",
        Mechanism::Sentiment,
        0.2,
        0.7,
        &[],
    ));
    graph.add(edge(
        "kestrel",
        "northwind",
        Mechanism::SupplyChain,
        0.9,
        0.7,
        &["filing-12"],
    ));
    graph.add(edge(
        "northwind",
        "vantage",
        Mechanism::InputCost,
        0.8,
        0.7,
        &["note-3"],
    ));
    let result = graph.propagate("kestrel", -0.10, 3, 0.001, now(), now());
    // The premise: both hops were reached.
    assert_eq!(result.effects.len(), 2, "{result:?}");
    let direct = &result.effects[0];
    assert_eq!(
        (direct.target.as_str(), &direct.edges),
        ("northwind", &vec![1])
    );
    let second = &result.effects[1];
    assert_eq!(
        (second.target.as_str(), &second.edges),
        ("vantage", &vec![1, 2])
    );
    let cited: Vec<&String> = second
        .edges
        .iter()
        .flat_map(|index| &graph.edges()[*index].evidence)
        .collect();
    assert_eq!(cited, ["filing-12", "note-3"]);
}

// ---- REASON-007 -----------------------------------------------------------

/// Every simple path from `x` to `y`, ignoring direction, that leaves `x`
/// against an arrow — the backdoor paths.
fn backdoor_paths(dag: &Dag, x: usize, y: usize) -> Vec<Vec<usize>> {
    fn extend(dag: &Dag, y: usize, path: &mut Vec<usize>, found: &mut Vec<Vec<usize>>) {
        let here = *path.last().unwrap();
        if here == y {
            found.push(path.clone());
            return;
        }
        for next in 0..dag.nodes {
            let adjacent = dag.points(here, next) || dag.points(next, here);
            // The first step must be into a parent of x.
            let backdoor = path.len() > 1 || dag.points(next, here);
            if adjacent && backdoor && !path.contains(&next) {
                path.push(next);
                extend(dag, y, path, found);
                path.pop();
            }
        }
    }
    let mut found = Vec::new();
    extend(dag, y, &mut vec![x], &mut found);
    found
}

/// Whether conditioning on `given` blocks `path`, by the d-separation rules:
/// a chain or fork is blocked when its middle is conditioned on, a collider
/// when neither it nor anything downstream of it is.
fn blocked(dag: &Dag, path: &[usize], given: &BTreeSet<usize>, reach: &[BTreeSet<usize>]) -> bool {
    path.windows(3).any(|triple| {
        let (before, middle, after) = (triple[0], triple[1], triple[2]);
        if dag.points(before, middle) && dag.points(after, middle) {
            !given.contains(&middle) && reach[middle].is_disjoint(given)
        } else {
            given.contains(&middle)
        }
    })
}

#[test]
fn the_adjustment_set_blocks_every_backdoor_path_and_an_unobserved_confounder_is_unidentified() {
    // The failure this prevents: a correlation reported as an effect because
    // nobody asked what else moves both ends, or because the thing that does
    // was never measured and the estimate was produced anyway.
    let mut g = Lcg(19);
    let (mut adjusted, mut open_doors, mut no_path, mut unidentified, mut confounded) =
        (0, 0, 0, 0, 0);
    for _ in 0..250 {
        let dag = generate(&mut g, 9);
        let reach = dag.closure(None);
        let touched: BTreeSet<usize> = dag.edges.iter().flat_map(|(a, b)| [*a, *b]).collect();
        for x in 0..dag.nodes {
            for y in 0..dag.nodes {
                if x == y || !touched.contains(&x) || !touched.contains(&y) {
                    continue;
                }
                let latent_at_x = dag
                    .latent
                    .iter()
                    .any(|index| dag.edges[*index].0 == x || dag.edges[*index].1 == x);
                match dag.graph.identify(&name(x), &name(y), now()).unwrap() {
                    Identification::Unidentified { reason } => {
                        // Only ever for the reason given.
                        assert!(latent_at_x, "{x} -> {y}: {reason}");
                        assert!(reason.contains("hidden-"), "{reason}");
                        unidentified += 1;
                    }
                    Identification::NoCausalPath => {
                        assert!(!latent_at_x && !reach[x].contains(&y), "{x} -> {y}");
                        no_path += 1;
                    }
                    Identification::Identified {
                        adjustment_set,
                        confounders,
                    } => {
                        assert!(!latent_at_x && reach[x].contains(&y), "{x} -> {y}");
                        let given: BTreeSet<usize> = (0..dag.nodes)
                            .filter(|n| adjustment_set.contains(&name(*n)))
                            .collect();
                        assert_eq!(given.len(), adjustment_set.len(), "{adjustment_set:?}");
                        // Nothing downstream of the treatment is adjusted for.
                        assert!(given.is_disjoint(&reach[x]), "{x} -> {y}: {given:?}");
                        // Every backdoor path is blocked.
                        let doors = backdoor_paths(&dag, x, y);
                        for door in &doors {
                            assert!(
                                blocked(&dag, door, &given, &reach),
                                "{x} -> {y}: {door:?} is open given {given:?}"
                            );
                        }
                        open_doors += usize::from(
                            doors
                                .iter()
                                .any(|d| !blocked(&dag, d, &BTreeSet::new(), &reach)),
                        );
                        adjusted += usize::from(!given.is_empty());

                        // The confounders are the common causes: upstream of
                        // the treatment, and of the outcome by a route that
                        // does not run through the treatment.
                        let beside = dag.closure(Some(x));
                        let common: BTreeSet<String> = (0..dag.nodes)
                            .filter(|c| reach[*c].contains(&x) && beside[*c].contains(&y))
                            .map(name)
                            .collect();
                        assert_eq!(confounders, common, "{x} -> {y}");
                        confounded += usize::from(!common.is_empty());
                    }
                }
            }
        }
    }
    // The premise: all three verdicts occurred, and among the identified
    // effects were ones with a backdoor path that is open until adjusted for
    // — otherwise an empty adjustment set would have passed everything above.
    assert!(
        adjusted > 100
            && open_doors > 100
            && confounded > 100
            && no_path > 100
            && unidentified > 100,
        "{adjusted} {open_doors} {confounded} {no_path} {unidentified}"
    );

    // The textbook case, by hand: rates move both funding costs and the
    // equity, and funding costs move the equity.
    let mut graph = CausalGraph::new();
    graph.add(edge(
        "rates",
        "funding",
        Mechanism::DiscountRate,
        0.6,
        0.7,
        &[],
    ));
    graph.add(edge(
        "rates",
        "acme",
        Mechanism::DiscountRate,
        0.4,
        0.7,
        &[],
    ));
    graph.add(edge("funding", "acme", Mechanism::InputCost, 0.5, 0.7, &[]));
    assert_eq!(
        graph.identify("funding", "acme", now()).unwrap(),
        Identification::Identified {
            adjustment_set: BTreeSet::from(["rates".to_string()]),
            confounders: BTreeSet::from(["rates".to_string()]),
        }
    );
    // Asked the other way round there is nothing to estimate.
    assert_eq!(
        graph.identify("acme", "funding", now()).unwrap(),
        Identification::NoCausalPath
    );

    // The same link with a common cause nobody measures: unidentified, and
    // the answer names it rather than offering an adjustment set.
    let mut hidden = CausalGraph::new();
    hidden.add(
        edge("funding", "acme", Mechanism::InputCost, 0.5, 0.7, &[]).with_confounders(
            BTreeSet::from(["market".to_string()]),
            BTreeSet::from(["risk-appetite".to_string()]),
        ),
    );
    match hidden.identify("funding", "acme", now()).unwrap() {
        Identification::Unidentified { reason } => {
            assert!(reason.contains("risk-appetite"), "{reason}");
        }
        other => panic!("an unobserved confounder was answered with {other:?}"),
    }

    // A common cause the edge itself was established under is stated, though
    // the graph holds no variable for it.
    let mut controlled = CausalGraph::new();
    controlled.add(
        edge("funding", "acme", Mechanism::InputCost, 0.5, 0.7, &[])
            .with_confounders(BTreeSet::from(["market".to_string()]), BTreeSet::new()),
    );
    assert_eq!(
        controlled.identify("funding", "acme", now()).unwrap(),
        Identification::Identified {
            adjustment_set: BTreeSet::from(["market".to_string()]),
            confounders: BTreeSet::from(["market".to_string()]),
        }
    );

    // A feedback loop is outside the criterion, and a variable the graph does
    // not hold is refused rather than answered.
    graph.add(edge("acme", "rates", Mechanism::Sentiment, 0.1, 0.7, &[]));
    match graph.identify("funding", "acme", now()).unwrap() {
        Identification::Unidentified { reason } => {
            assert!(reason.contains("feedback loop"), "{reason}");
        }
        other => panic!("a cyclic graph was answered with {other:?}"),
    }
    let error = graph.identify("funding", "unheard-of", now()).unwrap_err();
    assert!(error.message().contains("not a variable"), "{error:?}");
}

// ---- REASON-008 -----------------------------------------------------------

/// rates -> funding -> acme, and rates -> acme directly with the sign
/// flipped. Confidence 1, so each edge passes on exactly its strength:
///
///   funding = 0.5 * rates
///   acme    = 0.4 * funding - 0.1 * rates   (= 0.1 * rates)
fn structural_model() -> CausalGraph {
    let mut graph = CausalGraph::new();
    graph.add(edge(
        "rates",
        "funding",
        Mechanism::CreditConditions,
        0.5,
        1.0,
        &["ev-credit"],
    ));
    graph.add(edge(
        "funding",
        "acme",
        Mechanism::InputCost,
        0.4,
        1.0,
        &["ev-cost"],
    ));
    graph.add(edge(
        "rates",
        "acme",
        Mechanism::CompetitiveSubstitution,
        0.1,
        1.0,
        &["ev-substitution"],
    ));
    graph
}

#[test]
fn a_counterfactual_outcome_equals_the_hand_computed_value_and_cites_the_edges_it_used() {
    // The failure this prevents: answering "what if rates had moved less"
    // with the strongest single route, which here is +0.2 per unit when the
    // model's answer over both routes is +0.1 — double the true effect.
    let graph = structural_model();
    // Rates rose 3% and acme rose 0.7%. Had rates risen 1%:
    //   0.007 + (0.01 - 0.03) * (0.5 * 0.4 - 0.1) = 0.007 - 0.002 = 0.005
    let what_if = graph
        .counterfactual("rates", 0.03, 0.01, "acme", 0.007, now())
        .unwrap();
    assert!(approx_eq(what_if.total_effect, 0.1, 1e-12), "{what_if:?}");
    assert!(
        approx_eq(what_if.counterfactual_outcome, 0.005, 1e-12),
        "{what_if:?}"
    );
    assert_eq!(
        (what_if.variable.as_str(), what_if.outcome.as_str()),
        ("rates", "acme")
    );

    // It cites the variables and edges it used: both routes, all three
    // edges, each with the evidence the claim rests on.
    assert_eq!(what_if.paths.len(), 2);
    let cited: BTreeMap<usize, (&str, &str, &[String])> = what_if
        .paths
        .iter()
        .flat_map(|path| &path.edges)
        .map(|e| {
            (
                e.index,
                (e.cause.as_str(), e.effect.as_str(), e.evidence.as_slice()),
            )
        })
        .collect();
    assert_eq!(
        cited,
        BTreeMap::from([
            (0, ("rates", "funding", &["ev-credit".to_string()][..])),
            (1, ("funding", "acme", &["ev-cost".to_string()][..])),
            (2, ("rates", "acme", &["ev-substitution".to_string()][..])),
        ])
    );
    let told = what_if.explain();
    for part in [
        "rates -> funding",
        "funding -> acme",
        "rates -> acme",
        "ev-substitution",
    ] {
        assert!(told.contains(part), "{told}");
    }

    // Changing nothing changes nothing.
    let same = graph
        .counterfactual("rates", 0.03, 0.03, "acme", 0.007, now())
        .unwrap();
    assert!(approx_eq(same.counterfactual_outcome, 0.007, 1e-15));

    // Point in time: before the claims were recorded the graph implied
    // nothing, and says so rather than answering "no change".
    let before = recorded().saturating_sub(Duration::from_days(1));
    let error = graph
        .counterfactual("rates", 0.03, 0.01, "acme", 0.007, before)
        .unwrap_err();
    assert!(error.message().contains("holds no path"), "{error:?}");
    // The same for a pair with no route between them.
    let error = graph
        .counterfactual("acme", 0.007, 0.0, "rates", 0.03, now())
        .unwrap_err();
    assert!(error.message().contains("holds no path"), "{error:?}");

    // A value that is not a number, and a model with feedback, are refused.
    let error = graph
        .counterfactual("rates", f64::NAN, 0.01, "acme", 0.007, now())
        .unwrap_err();
    assert!(error.message().contains("not a number"), "{error:?}");
    let mut looped = structural_model();
    looped.add(edge("acme", "rates", Mechanism::Sentiment, 0.1, 1.0, &[]));
    let error = looped
        .counterfactual("rates", 0.03, 0.01, "acme", 0.007, now())
        .unwrap_err();
    assert!(error.message().contains("feedback loop"), "{error:?}");
}

// ---- REASON-019, REASON-020 -----------------------------------------------

/// Three claims into northwind. The rates link transmits the most, the
/// supplier link is the one whose cause actually moved.
fn northwind_claims() -> Vec<CausalEdge> {
    vec![
        edge(
            "rates",
            "northwind",
            Mechanism::DiscountRate,
            1.0,
            0.9,
            &["study-rates"],
        ),
        edge(
            "kestrel",
            "northwind",
            Mechanism::SupplyChain,
            0.8,
            0.9,
            &["filing-kestrel-outage"],
        ),
        edge(
            "sector",
            "northwind",
            Mechanism::Sentiment,
            0.5,
            0.9,
            &["note-sector"],
        ),
    ]
}

fn graph_of(claims: impl IntoIterator<Item = CausalEdge>) -> CausalGraph {
    let mut graph = CausalGraph::new();
    for claim in claims {
        graph.add(claim);
    }
    graph
}

/// Northwind was expected flat and fell 4%.
fn the_surprise() -> Surprise {
    Surprise {
        target: "northwind".into(),
        expected: 0.0,
        observed: -0.04,
    }
}

/// Kestrel fell 5%, the sector rose 1%, and nobody has a reading for rates.
fn the_moves() -> BTreeMap<String, f64> {
    BTreeMap::from([("kestrel".to_string(), -0.05), ("sector".to_string(), 0.01)])
}

#[test]
fn abduction_ranks_the_planted_cause_of_a_surprise_first_with_its_evidence_and_assumptions() {
    // The failure this prevents: explaining a move by the strongest link into
    // the instrument, whether or not anything happened at the other end of it.
    let graph = graph_of(northwind_claims());

    // The premise: ranked on transmission alone, the planted cause is second.
    let by_strength: Vec<&str> = graph
        .explanations("northwind", now())
        .iter()
        .map(|e| e.cause.as_str())
        .collect();
    assert_eq!(by_strength, ["rates", "kestrel", "sector"]);

    let abduction = graph.abduce(&the_surprise(), &the_moves(), now()).unwrap();
    let ranked: Vec<&str> = abduction
        .candidates
        .iter()
        .map(|c| c.edge.cause.as_str())
        .collect();
    assert_eq!(ranked, ["kestrel", "rates", "sector"]);

    // The best explanation is named, with the evidence it rests on.
    let best = abduction.best().unwrap();
    assert_eq!(best.edge.cause, "kestrel");
    assert_eq!(best.edge.evidence, ["filing-kestrel-outage"]);
    assert_eq!(best.observed_move, Some(-0.05));
    // -0.05 * (0.8 * 0.9) = -0.036 of the -0.04: a miss of a tenth.
    assert!(approx_eq(best.score, 0.9 / 1.1, 1e-12), "{best:?}");
    assert!(
        approx_eq(best.implied_move, -0.04 / 0.72, 1e-12),
        "{best:?}"
    );

    // Every candidate states what believing it requires. The unobserved one
    // says its cause would have had to move, and by how much.
    assert!(
        abduction
            .candidates
            .iter()
            .all(|c| !c.assumptions.is_empty())
    );
    assert!(
        best.assumptions[0].contains("held on this occasion"),
        "{best:?}"
    );
    let rates = &abduction.candidates[1];
    assert_eq!(rates.observed_move, None);
    assert!(
        rates.assumptions[0].contains("which nobody observed"),
        "{rates:?}"
    );
    assert!(rates.assumptions[0].contains("-0.0444"), "{rates:?}");

    // Reproducible: the same question gives the same record, and the ranking
    // does not depend on the order the claims were recorded in.
    assert_eq!(
        graph.abduce(&the_surprise(), &the_moves(), now()).unwrap(),
        abduction
    );
    let reversed = graph_of(northwind_claims().into_iter().rev())
        .abduce(&the_surprise(), &the_moves(), now())
        .unwrap();
    let scored = |a: &qip_world_model::inference::Abduction| -> Vec<(String, f64)> {
        a.candidates
            .iter()
            .map(|c| (c.edge.cause.clone(), c.score))
            .collect()
    };
    assert_eq!(scored(&reversed), scored(&abduction));

    // An unevidenced claim says so among its assumptions.
    let mut bare = graph_of(northwind_claims());
    bare.add(edge(
        "weather",
        "northwind",
        Mechanism::DemandLinkage,
        0.3,
        0.9,
        &[],
    ));
    let with_bare = bare.abduce(&the_surprise(), &the_moves(), now()).unwrap();
    let weather = with_bare
        .candidates
        .iter()
        .find(|c| c.edge.cause == "weather")
        .unwrap();
    assert!(
        weather
            .assumptions
            .iter()
            .any(|a| a.contains("cites no evidence")),
        "{weather:?}"
    );

    // An observation that matches expectation is not a surprise.
    let calm = Surprise {
        target: "northwind".into(),
        expected: 0.01,
        observed: 0.01,
    };
    let error = graph.abduce(&calm, &the_moves(), now()).unwrap_err();
    assert!(
        error.message().contains("no surprise to explain"),
        "{error:?}"
    );
}

#[test]
fn the_alternatives_are_kept_with_scores_and_evidence_and_later_evidence_promotes_one_from_the_record()
 {
    // The failure this prevents: keeping only the winner, so that when the
    // evidence turns the runner-up has to be derived again from a graph that
    // may no longer hold what it held then.
    let graph = graph_of(northwind_claims());
    let mut record = graph.abduce(&the_surprise(), &the_moves(), now()).unwrap();
    // The graph is gone. Everything below reads the record alone.
    drop(graph);

    // All three are recorded, each with a score and its evidence.
    let kept: Vec<(&str, &[String])> = record
        .candidates
        .iter()
        .map(|c| (c.edge.cause.as_str(), c.edge.evidence.as_slice()))
        .collect();
    assert_eq!(
        kept,
        [
            ("kestrel", &["filing-kestrel-outage".to_string()][..]),
            ("rates", &["study-rates".to_string()][..]),
            ("sector", &["note-sector".to_string()][..]),
        ]
    );
    assert!(record.candidates.iter().all(|c| c.score > 0.0));
    assert_eq!(record.dropped, 0);
    let before = record.clone();
    let runner_up = before.candidates[1].clone();
    // The premise: the runner-up really is behind before the evidence lands.
    assert!(runner_up.score < before.candidates[0].score);

    // A rate decision surfaces: evidence two and a half times likelier if
    // rates were the cause.
    record
        .favour(runner_up.edge.index, "decision-rates-0819", 2.5)
        .unwrap();
    let promoted = record.best().unwrap();
    assert_eq!(promoted.edge.cause, "rates");
    // Promoted from what was retained: the same citation and assumptions,
    // the retained score times the ratio, and the new evidence beside it.
    assert_eq!(promoted.edge, runner_up.edge);
    assert_eq!(promoted.assumptions, runner_up.assumptions);
    assert!(approx_eq(promoted.score, runner_up.score * 2.5, 1e-12));
    assert_eq!(promoted.later_evidence, ["decision-rates-0819"]);

    // The former best is second, untouched, and nothing was discarded.
    assert_eq!(record.candidates.len(), 3);
    assert_eq!(record.candidates[1], before.candidates[0]);
    assert_eq!(record.candidates[2], before.candidates[2]);

    // Evidence for something that was never a candidate is refused, as is a
    // ratio that is not a positive number.
    let error = record.favour(99, "stray", 2.0).unwrap_err();
    assert!(error.message().contains("never a candidate"), "{error:?}");
    let error = record.favour(runner_up.edge.index, "bad", 0.0).unwrap_err();
    assert!(error.message().contains("positive, finite"), "{error:?}");
}
