//! Cross-domain synthesis over the knowledge graph (R12-EXPAND-6031).
//!
//! The graph has held typed relationships that cross domains — a company
//! supplies a company, belongs to an industry, is domiciled in a country, a
//! country issues a currency, an industry consumes a commodity — since the
//! relationship vocabulary was written, and nothing walked them as chains.
//! The requirement's words are a synthesis layer that searches links across
//! domains and produces a `CrossDomainHypothesis`; this module is the search
//! and the record is `qip_contracts::expansion::CrossDomainHypothesis`.
//!
//! What a chain is here, stated so a reader can recompute one: a forward walk
//! over facts that hold at `now` and were known by `now`, without revisiting
//! a node, of at most [`MAX_HOPS`] hops, through nodes that belong to a world
//! domain ([`domain_of`]); it is emitted at each hop that raises the count of
//! distinct domains crossed to [`CrossDomainHypothesis::MIN_DOMAINS`] or
//! beyond. Platform-internal nodes — theses, evidence, portfolios, resolving
//! authorities — belong to no domain and are not walked through: a chain
//! "through the platform's own thesis" is not a fact about the world.
//!
//! What it refuses to do: claim anything. A hypothesis says the graph records
//! the chain and how many of its hops the causal graph already holds a live
//! claim for; raising, researching and eventually claiming an edge are the
//! deep brain's and the evidence's business. The output is bounded by
//! [`MAX_HYPOTHESES`], strongest first, because the walk is exponential in
//! the graph's degree and a sweep that produced ten thousand chains would
//! bury the one worth reading.

use crate::graph::{EntityKind, Node, NodeKind};
use crate::world::WorldModel;
use qip_contracts::expansion::CrossDomainHypothesis;
use qip_core::Timestamp;
use std::collections::BTreeSet;

/// The longest chain walked. Four hops cross at most five domains, which is
/// already past what a reviewer can hold as one mechanism.
pub const MAX_HOPS: usize = 4;

/// The most chains one synthesis returns, strongest first.
pub const MAX_HYPOTHESES: usize = 64;

/// The world domain a node belongs to, or `None` for a node that is the
/// platform's own bookkeeping rather than a thing in the world.
///
/// The vocabulary is the one the requirement names (weather, shipping, FX,
/// equities and so on) folded onto the entity kinds the graph actually
/// types, so a chain's domains read as the markets it crosses rather than as
/// node kinds.
pub fn domain_of(node: &Node) -> Option<&'static str> {
    match node.kind {
        NodeKind::Entity => node.entity_kind.map(|kind| match kind {
            EntityKind::Company
            | EntityKind::Person
            | EntityKind::Fund
            | EntityKind::Product
            | EntityKind::Contract => "companies",
            EntityKind::Sector | EntityKind::Industry => "industry",
            EntityKind::Commodity => "commodities",
            EntityKind::Currency => "fx",
            EntityKind::Country => "economy",
            EntityKind::CentralBank | EntityKind::Regulator => "policy",
            EntityKind::Index | EntityKind::Market | EntityKind::Venue => "markets",
            EntityKind::SupplyChain | EntityKind::Logistics => "shipping",
            EntityKind::Weather => "weather",
        }),
        NodeKind::FinancialObject => Some("markets"),
        NodeKind::Event => Some("events"),
        NodeKind::Factor => Some("factors"),
        NodeKind::Portfolio
        | NodeKind::Thesis
        | NodeKind::Evidence
        | NodeKind::ResolutionSource => None,
    }
}

struct Walk {
    nodes: Vec<String>,
    edges: Vec<String>,
    domains: Vec<&'static str>,
    sources: BTreeSet<String>,
    strength: f64,
    claimed: usize,
}

/// Every chain across [`CrossDomainHypothesis::MIN_DOMAINS`] or more domains
/// the graph holds at `now`, as validated hypotheses, strongest first and at
/// most [`MAX_HYPOTHESES`] of them. A chain the contract refuses is dropped
/// rather than returned invalid, and cannot arise from this walk: every
/// emitted chain has at least two hops, one edge per hop, distinct domains
/// and a positive strength, because a fact with zero weight or confidence
/// is not walked.
pub fn cross_domain_hypotheses(world: &WorldModel, now: Timestamp) -> Vec<CrossDomainHypothesis> {
    let graph = world.graph();
    let causal = world.causal();
    let mut found: Vec<Walk> = Vec::new();
    // Nodes in id order, so two platforms holding the same graph emit the
    // same list in the same order.
    for start in graph.nodes() {
        let Some(domain) = domain_of(start) else {
            continue;
        };
        let mut stack = vec![Walk {
            nodes: vec![start.id.clone()],
            edges: Vec::new(),
            domains: vec![domain],
            sources: BTreeSet::new(),
            strength: 1.0,
            claimed: 0,
        }];
        while let Some(walk) = stack.pop() {
            if walk.edges.len() >= MAX_HOPS {
                continue;
            }
            let Some(current) = walk.nodes.last() else {
                continue;
            };
            let mut facts = graph.neighbours(current, None, now, now);
            // Deterministic order: by target then kind, so the walk does not
            // depend on how the graph happened to index its keys.
            facts.sort_by(|a, b| {
                a.relationship.to.cmp(&b.relationship.to).then_with(|| {
                    a.relationship
                        .kind
                        .as_str()
                        .cmp(b.relationship.kind.as_str())
                })
            });
            for fact in facts {
                let next_id = &fact.relationship.to;
                if walk.nodes.contains(next_id) {
                    continue;
                }
                let Some(next) = graph.node(next_id) else {
                    continue;
                };
                let Some(next_domain) = domain_of(next) else {
                    continue;
                };
                let hop_strength = fact.relationship.weight * fact.confidence;
                if hop_strength <= 0.0 || !hop_strength.is_finite() {
                    continue;
                }
                let claimed = causal
                    .outgoing(current, now)
                    .iter()
                    .any(|edge| &edge.effect == next_id);
                let mut extended = Walk {
                    nodes: walk.nodes.clone(),
                    edges: walk.edges.clone(),
                    domains: walk.domains.clone(),
                    sources: walk.sources.clone(),
                    strength: walk.strength * hop_strength,
                    claimed: walk.claimed + usize::from(claimed),
                };
                extended.nodes.push(next_id.clone());
                extended
                    .edges
                    .push(fact.relationship.kind.as_str().to_string());
                extended.sources.insert(fact.relationship.source.clone());
                let new_domain = !extended.domains.contains(&next_domain);
                if new_domain {
                    extended.domains.push(next_domain);
                }
                if new_domain && extended.domains.len() >= CrossDomainHypothesis::MIN_DOMAINS {
                    found.push(Walk {
                        nodes: extended.nodes.clone(),
                        edges: extended.edges.clone(),
                        domains: extended.domains.clone(),
                        sources: extended.sources.clone(),
                        strength: extended.strength,
                        claimed: extended.claimed,
                    });
                }
                stack.push(extended);
            }
        }
    }
    found.sort_by(|a, b| {
        b.strength
            .partial_cmp(&a.strength)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.nodes.cmp(&b.nodes))
    });
    found
        .into_iter()
        .take(MAX_HYPOTHESES)
        .filter_map(|walk| {
            let hypothesis = CrossDomainHypothesis {
                subject: walk.nodes[0].clone(),
                path: walk.nodes,
                edges: walk.edges,
                domains: walk.domains.iter().map(|d| (*d).to_string()).collect(),
                strength: walk.strength,
                sources: walk.sources.into_iter().collect(),
                causally_claimed_hops: walk.claimed,
                synthesised_at: now,
            };
            hypothesis.validate().ok().map(|()| hypothesis)
        })
        .collect()
}
