//! Questions asked of the causal graph: what an intervention reaches, what
//! must be adjusted for, what would have happened, and what best explains a
//! surprise (REASON-006, -007, -008, -019, -020).
//!
//! [`CausalGraph::propagate`] answers one question — how large is the
//! strongest route from a shock to each target, above a floor — and its answer
//! was being used for four others. A floor and a hop limit make the set it
//! returns smaller than the set an intervention affects; keeping only the
//! strongest route discards the paths a counterfactual has to sum; and it
//! names nodes, so where two claims join the same pair it cannot say which
//! one it followed or what evidence that one rests on.
//!
//! Every answer here cites edges by [`EdgeCitation`], which carries the edge's
//! position in [`CausalGraph::edges`] and the evidence ids it holds, and every
//! query takes the instant it is asked as of and reads only edges knowable and
//! unretired by then.
//!
//! Numbers here are `f64`: transmissions, moves and scores are statistics, and
//! nothing in this module computes money.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};

use crate::causal::{CausalEdge, CausalGraph, Mechanism};

/// The most paths [`CausalGraph::intervene`] will enumerate. Simple paths grow
/// exponentially in a dense graph; past this the query is refused, because a
/// truncated list would read as the whole of what the intervention reaches.
pub const MAX_PATHS: usize = 4096;

/// The most candidates an [`Abduction`] retains.
pub const MAX_CANDIDATES: usize = 32;

/// One edge an answer rests on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeCitation {
    /// Position in [`CausalGraph::edges`] — the edge's identity.
    pub index: usize,
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The evidence ids the edge held when it was cited.
    pub evidence: Vec<String>,
}

impl EdgeCitation {
    fn of(index: usize, edge: &CausalEdge) -> Self {
        Self {
            index,
            cause: edge.cause.clone(),
            effect: edge.effect.clone(),
            mechanism: edge.mechanism,
            evidence: edge.evidence.clone(),
        }
    }
}

/// A directed path from an intervened variable to `target`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CausalPath {
    pub target: String,
    pub edges: Vec<EdgeCitation>,
}

/// What an intervention on one variable reaches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Intervention {
    pub variable: String,
    /// Every variable downstream of the intervention — its descendants, all
    /// of them, with no floor and no hop limit.
    pub affected: BTreeSet<String>,
    /// Every simple directed path the effect travels along.
    pub paths: Vec<CausalPath>,
}

/// Whether a causal effect can be told apart from confounding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Identification {
    /// Adjusting for `adjustment_set` blocks every backdoor path.
    Identified {
        adjustment_set: BTreeSet<String>,
        /// The common causes of treatment and outcome the graph holds.
        confounders: BTreeSet<String>,
    },
    /// The graph holds no directed path from treatment to outcome, so any
    /// association between them is confounding and none of it is effect.
    NoCausalPath,
    /// No set of observed variables is known to block every backdoor path.
    /// An association measured here must not be reported as the effect.
    Unidentified { reason: String },
}

/// What the graph implies an outcome would have been under a different value
/// of one variable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counterfactual {
    pub variable: String,
    pub observed: f64,
    pub hypothetical: f64,
    pub outcome: String,
    pub observed_outcome: f64,
    pub counterfactual_outcome: f64,
    /// Signed transmission from the variable to the outcome, summed over
    /// every path.
    pub total_effect: f64,
    /// The paths that sum was taken over, with their evidence.
    pub paths: Vec<CausalPath>,
}

impl Counterfactual {
    /// The counterfactual as a sentence that names every edge it used.
    pub fn explain(&self) -> String {
        let routes: Vec<String> = self
            .paths
            .iter()
            .map(|path| {
                path.edges
                    .iter()
                    .map(|e| {
                        format!(
                            "{} -> {} ({}; evidence: {})",
                            e.cause,
                            e.effect,
                            e.mechanism.as_str(),
                            if e.evidence.is_empty() {
                                "none".to_string()
                            } else {
                                e.evidence.join(", ")
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", then ")
            })
            .collect();
        format!(
            "had {} been {:+.4} rather than {:+.4}, {} would have been {:+.4} rather than {:+.4}: \
             a transmission of {:+.4} over {} path(s) — {}",
            self.variable,
            self.hypothetical,
            self.observed,
            self.outcome,
            self.counterfactual_outcome,
            self.observed_outcome,
            self.total_effect,
            self.paths.len(),
            routes.join("; ")
        )
    }
}

/// An observation that contradicts what was expected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surprise {
    pub target: String,
    pub expected: f64,
    pub observed: f64,
}

/// One explanation of a surprise: a cause, the edge it would have acted
/// through, and what believing it requires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// The edge, with the evidence the claim rests on.
    pub edge: EdgeCitation,
    /// What the cause was seen to do, if anything was seen.
    pub observed_move: Option<f64>,
    /// The move at the cause that would account for the whole surprise.
    pub implied_move: f64,
    /// Confidence in the claim times how much of the surprise it accounts
    /// for, multiplied by any later evidence. Comparable within one
    /// abduction only.
    pub score: f64,
    /// What has to be true for this to be the explanation.
    pub assumptions: Vec<String>,
    /// Evidence that arrived after the abduction and moved the score.
    pub later_evidence: Vec<String>,
}

/// Every explanation considered for one surprise, best first.
///
/// This is the record, not a view of the graph: the alternatives that did not
/// rank first stay here with their scores and evidence, and
/// [`Abduction::favour`] promotes one of them from here when later evidence
/// supports it. Nothing is asked of the graph a second time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Abduction {
    pub surprise: Surprise,
    pub candidates: Vec<Candidate>,
    /// Candidates beyond [`MAX_CANDIDATES`], dropped from the bottom.
    pub dropped: usize,
}

impl Abduction {
    /// The best current explanation.
    pub fn best(&self) -> Option<&Candidate> {
        self.candidates.first()
    }

    /// Weigh later evidence for the candidate acting through edge
    /// `edge_index`, and re-rank.
    ///
    /// `likelihood_ratio` is how much likelier the new evidence is if this
    /// candidate is the explanation than if it is not. Takes no graph: the
    /// candidate is promoted from this record as it was retained.
    pub fn favour(
        &mut self,
        edge_index: usize,
        evidence: impl Into<String>,
        likelihood_ratio: f64,
    ) -> Result<()> {
        let evidence = evidence.into();
        if evidence.trim().is_empty() || !likelihood_ratio.is_finite() || likelihood_ratio <= 0.0 {
            return Err(Error::invalid(format!(
                "later evidence needs an id and a positive, finite likelihood ratio; got \
                 '{evidence}' and {likelihood_ratio}"
            )));
        }
        let Some(candidate) = self
            .candidates
            .iter_mut()
            .find(|c| c.edge.index == edge_index)
        else {
            return Err(Error::not_found(format!(
                "no retained explanation of the move at {} acts through edge {edge_index}; \
                 it was never a candidate, so abduce again rather than promoting it",
                self.surprise.target
            )));
        };
        candidate.score *= likelihood_ratio;
        candidate.later_evidence.push(evidence);
        rank(&mut self.candidates);
        Ok(())
    }
}

/// Best first; ties broken by names so a replay ranks identically.
fn rank(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.edge.cause.cmp(&b.edge.cause))
            .then_with(|| a.edge.mechanism.as_str().cmp(b.edge.mechanism.as_str()))
            .then_with(|| a.edge.index.cmp(&b.edge.index))
    });
}

impl CausalGraph {
    /// What an intervention on `variable` affects, and along which paths.
    ///
    /// The affected set is exactly the variable's descendants as of
    /// `known_at`. Refuses a graph in which the intervention travels along
    /// more than [`MAX_PATHS`] simple paths.
    pub fn intervene(&self, variable: &str, known_at: Timestamp) -> Result<Intervention> {
        let mut paths = Vec::new();
        let mut on_path = vec![variable.to_string()];
        self.walk(known_at, &mut on_path, &mut Vec::new(), &mut paths)?;
        Ok(Intervention {
            variable: variable.to_string(),
            affected: paths.iter().map(|p| p.target.clone()).collect(),
            paths,
        })
    }

    /// Depth-first over simple paths. Recursion is as deep as the longest
    /// simple path, which the path bound keeps far below the stack.
    fn walk(
        &self,
        known_at: Timestamp,
        on_path: &mut Vec<String>,
        trail: &mut Vec<EdgeCitation>,
        paths: &mut Vec<CausalPath>,
    ) -> Result<()> {
        let Some(node) = on_path.last().cloned() else {
            return Ok(());
        };
        for (index, edge) in self.outgoing_indexed(&node, known_at) {
            if on_path.contains(&edge.effect) {
                continue; // a simple path does not revisit a variable
            }
            if paths.len() >= MAX_PATHS {
                return Err(Error::invalid(format!(
                    "an intervention on {} travels along more than {MAX_PATHS} paths; ask about \
                     a variable nearer the outcome, or retire the claims that no longer hold",
                    on_path.first().map_or("", String::as_str)
                )));
            }
            trail.push(EdgeCitation::of(index, edge));
            paths.push(CausalPath {
                target: edge.effect.clone(),
                edges: trail.clone(),
            });
            on_path.push(edge.effect.clone());
            self.walk(known_at, on_path, trail, paths)?;
            on_path.pop();
            trail.pop();
        }
        Ok(())
    }

    /// The live graph as adjacency, and a variable on a cycle if there is one.
    fn structure(&self, known_at: Timestamp) -> Structure<'_> {
        let mut structure = Structure::default();
        for edge in self.edges() {
            if edge.recorded_at > known_at || edge.retired_by(known_at) {
                continue;
            }
            let (cause, effect) = (edge.cause.as_str(), edge.effect.as_str());
            structure.children.entry(cause).or_default().insert(effect);
            structure.children.entry(effect).or_default();
            structure.parents.entry(effect).or_default().insert(cause);
            structure.parents.entry(cause).or_default();
        }
        structure
    }

    /// Whether the effect of `treatment` on `outcome` is identified, and by
    /// adjusting for what.
    ///
    /// The adjustment set is the treatment's parents that connect to the
    /// outcome without passing through the treatment, plus whatever a direct
    /// edge records it was established under. Every backdoor path leaves the
    /// treatment through one of those parents, where it is blocked.
    ///
    /// Deliberately conservative in one place: an unobserved common cause
    /// recorded on any edge touching the treatment answers
    /// [`Identification::Unidentified`], though a cleverer criterion could
    /// sometimes still identify the effect. Saying "unidentified" too often
    /// costs an estimate; saying "identified" once too often reports a
    /// correlation as a cause.
    pub fn identify(
        &self,
        treatment: &str,
        outcome: &str,
        known_at: Timestamp,
    ) -> Result<Identification> {
        let structure = self.structure(known_at);
        for variable in [treatment, outcome] {
            if !structure.children.contains_key(variable) {
                return Err(Error::invalid(format!(
                    "{variable} is not a variable of the causal graph as of {known_at}; ask \
                     about variables the graph holds a claim on"
                )));
            }
        }
        if treatment == outcome {
            return Err(Error::invalid(format!(
                "the effect of {treatment} on itself was asked for; name two variables"
            )));
        }
        if let Some(looped) = structure.cycle() {
            return Ok(Identification::Unidentified {
                reason: format!(
                    "the graph has a feedback loop through {looped}, and the backdoor criterion \
                     is defined only where no variable causes itself"
                ),
            });
        }
        let touching = |e: &&CausalEdge| {
            e.recorded_at <= known_at
                && !e.retired_by(known_at)
                && (e.cause == treatment || e.effect == treatment)
        };
        let latent: BTreeSet<&str> = self
            .edges()
            .iter()
            .filter(touching)
            .flat_map(|e| e.suspected_confounders.iter().map(String::as_str))
            .collect();
        if !latent.is_empty() {
            return Ok(Identification::Unidentified {
                reason: format!(
                    "{treatment} shares the unobserved common cause(s) {} with a neighbour, and \
                     no observed variable blocks a path through one nobody measures",
                    latent.into_iter().collect::<Vec<_>>().join(", ")
                ),
            });
        }
        if !reach(&structure.children, treatment, None).contains(outcome) {
            return Ok(Identification::NoCausalPath);
        }

        // Named on a direct edge: common causes its own estimate was adjusted
        // for, which need not be variables of the graph.
        let declared: BTreeSet<String> = self
            .edges()
            .iter()
            .filter(touching)
            .filter(|e| e.cause == treatment && e.effect == outcome)
            .flat_map(|e| e.adjusted_for.iter().cloned())
            .collect();
        let ancestors = reach(&structure.parents, treatment, None);
        let causes_of_outcome = reach(&structure.parents, outcome, Some(treatment));
        let mut confounders: BTreeSet<String> = ancestors
            .intersection(&causes_of_outcome)
            .map(|v| (*v).to_string())
            .collect();
        confounders.extend(declared.iter().cloned());

        let beside = structure.connected(outcome, treatment);
        let mut adjustment_set: BTreeSet<String> = structure
            .parents
            .get(treatment)
            .into_iter()
            .flatten()
            .filter(|parent| beside.contains(*parent))
            .map(|parent| (*parent).to_string())
            .collect();
        adjustment_set.extend(declared);
        Ok(Identification::Identified {
            adjustment_set,
            confounders,
        })
    }

    /// What `outcome` would have been had `variable` been `hypothetical`
    /// rather than `observed`, everything else that happened held as it was.
    ///
    /// The graph is read as it is everywhere else: each edge passes on its
    /// [`CausalEdge::signed_transmission`] of its cause's move. The change at
    /// the outcome is the change at the variable times the sum of that over
    /// every path. Refuses a graph with a feedback loop, where that sum is not
    /// the model's answer, and a pair with no path between them: no recorded
    /// mechanism is not evidence of no effect.
    pub fn counterfactual(
        &self,
        variable: &str,
        observed: f64,
        hypothetical: f64,
        outcome: &str,
        observed_outcome: f64,
        known_at: Timestamp,
    ) -> Result<Counterfactual> {
        if ![observed, hypothetical, observed_outcome]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(Error::numeric(format!(
                "a counterfactual on {variable} was given a value that is not a number; supply \
                 the observed and hypothetical values and the observed outcome"
            )));
        }
        if let Some(looped) = self.structure(known_at).cycle() {
            return Err(Error::invalid(format!(
                "the causal graph has a feedback loop through {looped} as of {known_at}; a \
                 counterfactual needs a model in which no variable causes itself"
            )));
        }
        let mut paths = self.intervene(variable, known_at)?.paths;
        paths.retain(|path| path.target == outcome);
        if paths.is_empty() {
            return Err(Error::not_found(format!(
                "the causal graph holds no path from {variable} to {outcome} as of {known_at}, \
                 so it implies nothing about {outcome} under a different {variable}; record the \
                 mechanism first"
            )));
        }
        let total_effect: f64 = paths
            .iter()
            .map(|path| {
                path.edges
                    .iter()
                    .filter_map(|cited| self.edges().get(cited.index))
                    .map(CausalEdge::signed_transmission)
                    .product::<f64>()
            })
            .sum();
        Ok(Counterfactual {
            variable: variable.to_string(),
            observed,
            hypothetical,
            outcome: outcome.to_string(),
            observed_outcome,
            counterfactual_outcome: observed_outcome + (hypothetical - observed) * total_effect,
            total_effect,
            paths,
        })
    }

    /// The candidate explanations of a surprise, ranked by how well each
    /// accounts for the move that was actually seen.
    ///
    /// `moves` is what each possible cause was observed to do. A candidate is
    /// scored by confidence in its edge times its fit, `1 / (1 + miss)`, where
    /// `miss` is the share of the surprise its cause's move, passed along the
    /// edge, fails to account for. A cause nobody observed accounts for none
    /// of it yet — it is retained, with the move it would need stated as an
    /// assumption. Ranking on transmission alone, as
    /// [`CausalGraph::explanations`] does, puts the strongest link first
    /// whether or not its cause moved at all.
    pub fn abduce(
        &self,
        surprise: &Surprise,
        moves: &BTreeMap<String, f64>,
        known_at: Timestamp,
    ) -> Result<Abduction> {
        let residual = surprise.observed - surprise.expected;
        let finite = residual.is_finite() && moves.values().all(|m| m.is_finite());
        if !finite || residual.abs() <= 0.0 {
            return Err(Error::invalid(format!(
                "the observation at {} ({}) does not differ from what was expected ({}) by a \
                 finite amount; there is no surprise to explain",
                surprise.target, surprise.observed, surprise.expected
            )));
        }
        let mut candidates: Vec<Candidate> = self
            .incoming_indexed(&surprise.target, known_at)
            .into_iter()
            .filter_map(|(index, edge)| {
                let passed_on = edge.signed_transmission();
                // An edge that transmits nothing cannot have produced a move.
                (passed_on.abs() > 0.0).then(|| {
                    let observed_move = moves.get(&edge.cause).copied();
                    let accounted = observed_move.unwrap_or(0.0) * passed_on;
                    let miss = (residual - accounted).abs() / residual.abs();
                    let implied_move = residual / passed_on;
                    Candidate {
                        edge: EdgeCitation::of(index, edge),
                        observed_move,
                        implied_move,
                        score: edge.confidence / (1.0 + miss),
                        assumptions: assumptions(edge, observed_move, implied_move),
                        later_evidence: Vec::new(),
                    }
                })
            })
            .collect();
        rank(&mut candidates);
        let dropped = candidates.len().saturating_sub(MAX_CANDIDATES);
        candidates.truncate(MAX_CANDIDATES);
        Ok(Abduction {
            surprise: surprise.clone(),
            candidates,
            dropped,
        })
    }
}

/// What has to hold for `edge` to be the explanation.
fn assumptions(edge: &CausalEdge, observed_move: Option<f64>, implied_move: f64) -> Vec<String> {
    let mut held = vec![match observed_move {
        Some(_) => format!(
            "the transmission recorded for {} -> {} ({:+.4}) held on this occasion",
            edge.cause,
            edge.effect,
            edge.signed_transmission()
        ),
        None => format!(
            "{} moved by {implied_move:+.4}, which nobody observed",
            edge.cause
        ),
    }];
    held.extend(
        edge.suspected_confounders.iter().map(|confounder| {
            format!("the unobserved common cause {confounder} did not move both")
        }),
    );
    if !edge.is_evidenced() {
        held.push("the causal claim itself, which cites no evidence".to_string());
    }
    if let Some(at) = edge.decayed_at {
        held.push(format!(
            "the claim still holds, though nothing has supported it since {at}"
        ));
    }
    held
}

/// The live graph's adjacency. Every variable is a key of both maps.
#[derive(Default)]
struct Structure<'a> {
    children: BTreeMap<&'a str, BTreeSet<&'a str>>,
    parents: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> Structure<'a> {
    /// A variable on a directed cycle, if any: what is left when variables
    /// with no remaining parent are removed until none can be.
    fn cycle(&self) -> Option<&'a str> {
        let mut waiting: BTreeMap<&str, usize> =
            self.parents.iter().map(|(v, p)| (*v, p.len())).collect();
        let mut free: Vec<&str> = waiting
            .iter()
            .filter(|(_, parents)| **parents == 0)
            .map(|(v, _)| *v)
            .collect();
        while let Some(variable) = free.pop() {
            waiting.remove(variable);
            for child in self.children.get(variable).into_iter().flatten() {
                if let Some(parents) = waiting.get_mut(child) {
                    *parents -= 1;
                    if *parents == 0 {
                        free.push(*child);
                    }
                }
            }
        }
        waiting.into_keys().next()
    }

    /// Variables joined to `from` by edges in either direction, never
    /// passing through `avoiding`.
    fn connected(&self, from: &'a str, avoiding: &str) -> BTreeSet<&'a str> {
        let mut seen = BTreeSet::from([from]);
        let mut frontier = vec![from];
        while let Some(variable) = frontier.pop() {
            let neighbours = [&self.children, &self.parents]
                .into_iter()
                .filter_map(|side| side.get(variable))
                .flatten();
            for next in neighbours {
                if *next != avoiding && seen.insert(*next) {
                    frontier.push(*next);
                }
            }
        }
        seen
    }
}

/// Everything reachable from `from` along `edges`, excluding `from` itself
/// and never passing through `avoiding`.
fn reach<'a>(
    edges: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    from: &str,
    avoiding: Option<&str>,
) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    let mut frontier: Vec<&str> = vec![from];
    while let Some(variable) = frontier.pop() {
        for next in edges.get(variable).into_iter().flatten() {
            if Some(*next) != avoiding && seen.insert(*next) {
                frontier.push(*next);
            }
        }
    }
    seen
}
