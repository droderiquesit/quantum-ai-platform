//! Finding candidate cycles in log space, and refusing them in exact arithmetic.
//!
//! A cycle pays when the product of its rates exceeds one. Taking logarithms
//! turns that product into a sum, and a sum that comes out negative is a
//! negative cycle — which Bellman-Ford finds in `O(V·E)` without enumerating
//! anything. That is the whole reason for the change of representation.
//!
//! It is also the reason the answer cannot be trusted. `ln` and the additions
//! that follow it are `f64`, and a cycle whose true product is exactly one can
//! come out of the search looking like a tenth of a basis point of free money.
//! So the two jobs are split and never merged: **the search proposes, exact
//! arithmetic disposes.** [`search_candidates`] hands back things worth looking
//! at; [`confirm_exact`] recomputes the product in [`Decimal`] and throws away
//! the ones that were rounding. Nothing downstream ever sees a candidate that
//! only the floating point liked.
//!
//! Confirmation here is still only about the *quoted* rates. A cycle that
//! survives this stage has proved it is not an artefact of arithmetic; it has
//! not yet proved anything about the book, which is [`crate::pricing`]'s job.

use crate::graph::{ArbitrageGraph, PathKind};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The fewest legs a cycle has. One conversion cannot close (MESH-010).
pub const MIN_CYCLE_EDGES: usize = 2;

/// The most legs any cycle may have, whatever is configured (MESH-010).
///
/// The blueprint's ceiling on the product, not a tuning value: the maximum in
/// force is [`SearchSettings::max_cycle_edges`], which a deployment sets and
/// [`SearchSettings::validate`] refuses above this. A leg here is a
/// conversion, one edge of the cycle, which is what the blueprint's graph
/// counts; a synthetic conversion that fans out into several component orders
/// is still one leg of the cycle.
pub const MAX_CYCLE_EDGES: usize = 20;

/// How hard to look, and how much rounding noise to tolerate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchSettings {
    /// Smallest log-space gain worth confirming.
    ///
    /// Set low on purpose. Its job is to keep the search from returning every
    /// cycle in a perfectly consistent market, not to filter for profitability
    /// — that decision belongs to exact arithmetic and then to the book, and a
    /// threshold here that looked like a profitability filter would quietly
    /// become one.
    pub min_log_gain_f64: f64,
    /// Longest cycle accepted: the maximum leg count in force.
    ///
    /// Long cycles are found by the same algorithm and are almost never
    /// executable: every extra leg is another chance to be left half-on. A
    /// cycle longer than this is not dropped and not shortened: [`search`]
    /// hands it back as over-length so the scan can refuse it by name.
    pub max_cycle_edges: usize,
    /// Most candidates returned from one scan.
    pub max_candidates: usize,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            min_log_gain_f64: 1e-12,
            max_cycle_edges: 4,
            max_candidates: 32,
        }
    }
}

impl SearchSettings {
    /// Refuse a maximum in force outside the two to twenty legs a cycle has.
    ///
    /// Called where the maximum is read from configuration. A value above the
    /// ceiling is refused rather than lowered to it: a deployment that asked
    /// for thirty legs and silently got twenty would be running a limit
    /// nobody chose.
    pub fn validate(&self) -> Result<()> {
        if !(MIN_CYCLE_EDGES..=MAX_CYCLE_EDGES).contains(&self.max_cycle_edges) {
            return Err(Error::invalid(format!(
                "a maximum of {} legs per cycle is outside the {MIN_CYCLE_EDGES} to \
                 {MAX_CYCLE_EDGES} a cycle may have; set it within that range",
                self.max_cycle_edges
            )));
        }
        Ok(())
    }

    /// Whether a cycle of `legs` legs may be accepted under the maximum in
    /// force, with a refusal that names the limit it broke.
    ///
    /// The one place the length rule is stated, so the scan's refusal and a
    /// unit check of the boundary cannot disagree about where it sits.
    pub fn admit_length(&self, legs: usize) -> Result<()> {
        if legs < MIN_CYCLE_EDGES {
            return Err(Error::invalid(format!(
                "a cycle of {legs} leg(s) cannot close; a cycle has at least {MIN_CYCLE_EDGES}"
            )));
        }
        if legs > self.max_cycle_edges {
            return Err(Error::invalid(format!(
                "a cycle of {legs} legs exceeds the maximum of {} in force; it is refused whole \
                 and never shortened to fit, so raise the configured maximum (to at most \
                 {MAX_CYCLE_EDGES}) or leave it refused",
                self.max_cycle_edges
            )));
        }
        Ok(())
    }
}

/// A cycle the log-space search thinks is worth a closer look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathCandidate {
    /// Edge indices in traversal order. The last edge arrives where the first
    /// departs.
    pub edges: Vec<usize>,
    pub kind: PathKind,
    /// The log-space gain that motivated the candidate.
    ///
    /// A statistic, and named so. It decides what gets examined and never what
    /// gets traded.
    pub log_gain_f64: f64,
}

impl PathCandidate {
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// A readable rendering of the cycle, for rejection messages.
    pub fn describe(&self, graph: &ArbitrageGraph) -> String {
        let hops: Vec<String> = self
            .edges
            .iter()
            .filter_map(|index| graph.edge(*index))
            .map(|edge| edge.from.label())
            .collect();
        format!("{} cycle {}", self.kind.as_str(), hops.join(" -> "))
    }
}

/// The exact recomputation of a candidate's payoff multiple.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExactConfirmation {
    /// Product of the cycle's cost-adjusted rates, in exact arithmetic.
    pub multiple: Decimal,
}

impl ExactConfirmation {
    /// Whether a unit of the starting instrument comes back as more than a unit.
    ///
    /// A strict comparison. A multiple of exactly one is a consistent market,
    /// and treating it as an opportunity is how a strategy pays fees to stand
    /// still.
    pub fn is_profitable(&self) -> bool {
        self.multiple > Decimal::ONE
    }

    /// The surplus per unit committed. Negative when the cycle loses.
    pub fn surplus(&self) -> Decimal {
        self.multiple - Decimal::ONE
    }
}

/// Relaxation tolerance.
///
/// Strictly-less would let two paths of identical cost keep swapping places on
/// the last bits of their mantissas, and Bellman-Ford would report a negative
/// cycle where there is only a tie.
const RELAX_TOLERANCE: f64 = 1e-15;

/// What one search found: the cycles worth pricing, and the ones it will
/// not hand on because they are longer than the maximum in force.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchOutcome {
    /// Cycles within the maximum, in descending order of log gain.
    pub candidates: Vec<PathCandidate>,
    /// Cycles the search found that have more legs than
    /// [`SearchSettings::max_cycle_edges`], whole and unshortened.
    ///
    /// Kept so the scan can refuse each one by name (MESH-010). Until this
    /// existed the search dropped them, and a market whose only cycle was one
    /// leg too long read exactly like a market with no cycle in it. Bounded
    /// at [`SearchSettings::max_candidates`] like the candidates are.
    pub over_length: Vec<PathCandidate>,
}

/// Search the graph for cycles whose rates multiply to more than one.
///
/// [`search`] with the over-length cycles left out, for a caller that only
/// prices. A caller that has to account for what was refused wants
/// [`search`].
pub fn search_candidates(graph: &ArbitrageGraph, settings: &SearchSettings) -> Vec<PathCandidate> {
    search(graph, settings).candidates
}

/// Search the graph for cycles whose rates multiply to more than one.
///
/// Bellman-Ford over `-ln(rate)`, run repeatedly: each round extracts the
/// cycles it can reach, then excludes their edges so the next round finds a
/// different one. Returned in descending order of log gain, which is an
/// examination order and not a ranking anyone should act on.
pub fn search(graph: &ArbitrageGraph, settings: &SearchSettings) -> SearchOutcome {
    let node_count = graph.node_count();
    if node_count == 0 || graph.edge_count() == 0 {
        return SearchOutcome::default();
    }

    // Endpoints and weights, resolved once. An edge whose venue is shut, or
    // whose rate cannot be turned into a logarithm, never enters the search:
    // proposing a path through a halted venue wastes the confirmation stage's
    // time and reads, in a log, like an opportunity that was missed.
    let mut endpoints: Vec<Option<(usize, usize)>> = Vec::with_capacity(graph.edge_count());
    let mut weights: Vec<f64> = Vec::with_capacity(graph.edge_count());
    for edge in graph.edges() {
        let rate = edge.effective_rate().unwrap_or(Decimal::ZERO);
        let ends = match (
            rate > Decimal::ZERO && graph.edge_is_tradable(edge),
            graph.node_index(&edge.from),
            graph.node_index(&edge.to),
        ) {
            (true, Some(from), Some(to)) => Some((from, to)),
            _ => None,
        };
        // An unusable edge is weighted zero rather than infinite. It is never
        // relaxed across, and an infinity left lying in the array would
        // propagate through any future change that forgot why it was there.
        weights.push(if ends.is_some() {
            -rate.to_f64().ln()
        } else {
            0.0
        });
        endpoints.push(ends);
    }

    let mut excluded = vec![false; graph.edge_count()];
    let mut found: Vec<PathCandidate> = Vec::new();
    let mut over_length: Vec<PathCandidate> = Vec::new();
    let mut seen: Vec<Vec<usize>> = Vec::new();

    for _ in 0..settings.max_candidates {
        let cycles = negative_cycles(node_count, &endpoints, &weights, &excluded);
        if cycles.is_empty() {
            break;
        }
        let mut progressed = false;
        for cycle in cycles {
            if cycle.len() > settings.max_cycle_edges {
                // Still exclude it: leaving it in would make every later round
                // rediscover the same too-long cycle and find nothing else.
                for edge in &cycle {
                    excluded[*edge] = true;
                }
                progressed = true;
                // And keep it, whole, for the scan to refuse by name. It is
                // held to the same gain floor a candidate is, so a consistent
                // market does not report every long loop in it as refused.
                let canonical = canonicalise(&cycle);
                let log_gain_f64: f64 = canonical.iter().map(|edge| -weights[*edge]).sum();
                if over_length.len() < settings.max_candidates
                    && !log_gain_f64.is_nan()
                    && log_gain_f64 > settings.min_log_gain_f64
                    && !over_length.iter().any(|kept| kept.edges == canonical)
                {
                    over_length.push(PathCandidate {
                        kind: graph.classify(&canonical),
                        edges: canonical,
                        log_gain_f64,
                    });
                }
                continue;
            }
            let canonical = canonicalise(&cycle);
            for edge in &canonical {
                excluded[*edge] = true;
            }
            progressed = true;
            if seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical.clone());
            let log_gain_f64: f64 = canonical.iter().map(|edge| -weights[*edge]).sum();
            if log_gain_f64.is_nan() || log_gain_f64 <= settings.min_log_gain_f64 {
                continue;
            }
            found.push(PathCandidate {
                kind: graph.classify(&canonical),
                edges: canonical,
                log_gain_f64,
            });
        }
        if !progressed || found.len() >= settings.max_candidates {
            break;
        }
    }

    // Descending gain, then by edge list, so a replay orders identical gains
    // the same way every time.
    found.sort_by(|a, b| {
        b.log_gain_f64
            .partial_cmp(&a.log_gain_f64)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.edges.cmp(&b.edges))
    });
    found.truncate(settings.max_candidates);
    over_length.sort_by(|a, b| a.edges.cmp(&b.edges));
    SearchOutcome {
        candidates: found,
        over_length,
    }
}

/// Recompute a candidate's payoff multiple in exact arithmetic.
///
/// The gate every candidate passes before anything else looks at it. The search
/// works in `f64`; this works in [`Decimal`]; where they disagree, this is
/// right and the candidate is discarded.
pub fn confirm_exact(
    graph: &ArbitrageGraph,
    candidate: &PathCandidate,
) -> Result<ExactConfirmation> {
    if candidate.edges.is_empty() {
        return Err(Error::invalid("an empty cycle cannot be confirmed"));
    }
    let mut multiple = Decimal::ONE;
    for index in &candidate.edges {
        let edge = graph
            .edge(*index)
            .ok_or_else(|| Error::not_found(format!("no conversion at index {index}")))?;
        multiple = crate::arith::mul(multiple, edge.effective_rate()?, "cycle multiple")?;
    }
    Ok(ExactConfirmation { multiple })
}

/// Rotate a cycle so it starts at its lowest edge index.
///
/// Two runs that enter the same cycle at different nodes must produce the same
/// candidate, or a scan's output depends on iteration order.
fn canonicalise(cycle: &[usize]) -> Vec<usize> {
    let Some(pivot) = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, edge)| **edge)
        .map(|(position, _)| position)
    else {
        return Vec::new();
    };
    let mut rotated = Vec::with_capacity(cycle.len());
    rotated.extend_from_slice(&cycle[pivot..]);
    rotated.extend_from_slice(&cycle[..pivot]);
    rotated
}

/// One Bellman-Ford pass, returning every negative cycle it can reach.
///
/// A virtual source sits at distance zero from every node, so the search does
/// not need a start and cannot miss a cycle in a disconnected component.
fn negative_cycles(
    node_count: usize,
    endpoints: &[Option<(usize, usize)>],
    weights: &[f64],
    excluded: &[bool],
) -> Vec<Vec<usize>> {
    let mut distance = vec![0.0f64; node_count];
    let mut predecessor: Vec<Option<usize>> = vec![None; node_count];

    let live: Vec<(usize, usize, usize)> = endpoints
        .iter()
        .enumerate()
        .filter(|(index, _)| !excluded[*index])
        .filter_map(|(index, ends)| ends.map(|(from, to)| (index, from, to)))
        .collect();
    if live.is_empty() {
        return Vec::new();
    }

    for _ in 0..node_count {
        let mut relaxed = false;
        for (edge, from, to) in &live {
            let candidate = distance[*from] + weights[*edge];
            if candidate < distance[*to] - RELAX_TOLERANCE {
                distance[*to] = candidate;
                predecessor[*to] = Some(*edge);
                relaxed = true;
            }
        }
        if !relaxed {
            return Vec::new();
        }
    }

    // Anything still improving after `node_count` rounds is downstream of a
    // negative cycle. Walking back that many predecessors is guaranteed to land
    // inside the cycle rather than on the tail that leads to it.
    let mut affected: Vec<usize> = Vec::new();
    for (edge, from, to) in &live {
        if distance[*from] + weights[*edge] < distance[*to] - RELAX_TOLERANCE {
            predecessor[*to] = Some(*edge);
            if !affected.contains(to) {
                affected.push(*to);
            }
        }
    }

    let mut cycles: Vec<Vec<usize>> = Vec::new();
    for node in affected {
        if let Some(cycle) = walk_back(&predecessor, endpoints, node, node_count)
            && !cycles.contains(&cycle)
        {
            cycles.push(cycle);
        }
    }
    cycles
}

/// Follow predecessors from `start` into the cycle it is downstream of.
fn walk_back(
    predecessor: &[Option<usize>],
    endpoints: &[Option<(usize, usize)>],
    start: usize,
    node_count: usize,
) -> Option<Vec<usize>> {
    let mut node = start;
    for _ in 0..node_count {
        let edge = predecessor[node]?;
        node = endpoints.get(edge).copied().flatten()?.0;
    }

    let entry = node;
    let mut edges: Vec<usize> = Vec::new();
    loop {
        let edge = predecessor[node]?;
        edges.push(edge);
        node = endpoints.get(edge).copied().flatten()?.0;
        if node == entry {
            break;
        }
        if edges.len() > node_count {
            return None;
        }
    }
    edges.reverse();
    Some(edges)
}
