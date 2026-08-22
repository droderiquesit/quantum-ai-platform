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
use qip_core::error::{Error, Result};
use qip_core::Decimal;
use serde::{Deserialize, Serialize};

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
    /// Longest cycle considered.
    ///
    /// Long cycles are found by the same algorithm and are almost never
    /// executable: every extra leg is another chance to be left half-on.
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

/// Search the graph for cycles whose rates multiply to more than one.
///
/// Bellman-Ford over `-ln(rate)`, run repeatedly: each round extracts the
/// cycles it can reach, then excludes their edges so the next round finds a
/// different one. Returned in descending order of log gain, which is an
/// examination order and not a ranking anyone should act on.
pub fn search_candidates(graph: &ArbitrageGraph, settings: &SearchSettings) -> Vec<PathCandidate> {
    let node_count = graph.node_count();
    if node_count == 0 || graph.edge_count() == 0 {
        return Vec::new();
    }

    // Endpoints and weights, resolved once. An edge whose venue is shut, or
    // whose rate cannot be turned into a logarithm, never enters the search:
    // proposing a path through a halted venue wastes the confirmation stage's
    // time and reads, in a log, like an opportunity that was missed.
    let mut endpoints: Vec<Option<(usize, usize)>> = Vec::with_capacity(graph.edge_count());
    let mut weights: Vec<f64> = Vec::with_capacity(graph.edge_count());
    for edge in graph.edges() {
        let usable = graph.edge_is_tradable(edge);
        let rate = edge.effective_rate().unwrap_or(Decimal::ZERO);
        let ends = match (
            usable && rate > Decimal::ZERO,
            graph.node_index(&edge.from),
            graph.node_index(&edge.to),
        ) {
            (true, Some(from), Some(to)) => Some((from, to)),
            _ => None,
        };
        endpoints.push(ends);
        weights.push(-rate.to_f64().ln());
    }

    let mut excluded = vec![false; graph.edge_count()];
    let mut found: Vec<PathCandidate> = Vec::new();
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
    found
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
