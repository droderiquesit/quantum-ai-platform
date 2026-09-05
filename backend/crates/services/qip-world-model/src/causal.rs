//! The causal layer.
//!
//! Separate from the relationship graph on purpose. That Kestrel supplies
//! Northwind is a fact. That a disruption at Kestrel moves Northwind's price by
//! roughly a certain amount, after roughly a certain delay, through a named
//! mechanism, is a *claim* — and it needs a mechanism, a lag, a strength and
//! evidence before anything should act on it.
//!
//! Propagation is bounded and attenuating: each hop multiplies the shock by the
//! edge's strength, and effects below a floor are dropped. Without that, a
//! shock reaches everything and the "third-order effect" the charter asks for
//! becomes a list of every instrument in the universe.

use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How one thing affects another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mechanism {
    /// Input cost or availability passes through to the buyer.
    SupplyChain,
    /// Lost sales at one firm are gained by a rival.
    CompetitiveSubstitution,
    /// Demand for a product moves demand for its inputs.
    DemandLinkage,
    /// A change in policy rates repricing discount rates.
    DiscountRate,
    /// A currency move altering translated revenue.
    CurrencyTranslation,
    /// A commodity price entering a cost base.
    InputCost,
    /// A commodity price entering revenue.
    OutputPrice,
    /// Credit conditions altering funding cost or availability.
    CreditConditions,
    /// Sentiment or positioning spilling across similar names.
    Sentiment,
    /// Index membership forcing mechanical flows.
    IndexFlow,
    /// Shared ownership forcing correlated liquidation.
    CommonOwnership,
    /// Regulatory action applying across an industry.
    Regulatory,
}

impl Mechanism {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SupplyChain => "supply_chain",
            Self::CompetitiveSubstitution => "competitive_substitution",
            Self::DemandLinkage => "demand_linkage",
            Self::DiscountRate => "discount_rate",
            Self::CurrencyTranslation => "currency_translation",
            Self::InputCost => "input_cost",
            Self::OutputPrice => "output_price",
            Self::CreditConditions => "credit_conditions",
            Self::Sentiment => "sentiment",
            Self::IndexFlow => "index_flow",
            Self::CommonOwnership => "common_ownership",
            Self::Regulatory => "regulatory",
        }
    }

    /// A readable description of the transmission, used in causal chains.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::SupplyChain => "input availability or cost passes through to the buyer",
            Self::CompetitiveSubstitution => "demand lost by one firm is captured by a rival",
            Self::DemandLinkage => "demand for the product moves demand for its inputs",
            Self::DiscountRate => "a change in rates reprices future cash flows",
            Self::CurrencyTranslation => "a currency move alters translated revenue and costs",
            Self::InputCost => "a commodity price moves the cost base",
            Self::OutputPrice => "a commodity price moves realised revenue",
            Self::CreditConditions => "funding cost or availability changes",
            Self::Sentiment => "positioning and sentiment spill across similar names",
            Self::IndexFlow => "index membership forces mechanical buying or selling",
            Self::CommonOwnership => "shared holders liquidate correlated positions",
            Self::Regulatory => "a regulatory action applies across the industry",
        }
    }

    /// Whether the effect moves in the same direction as its cause.
    ///
    /// Competitive substitution is the notable inversion: a rival's loss is a
    /// gain, and treating it as same-signed would produce theses that are
    /// exactly backwards.
    pub fn preserves_sign(&self) -> bool {
        !matches!(self, Self::CompetitiveSubstitution)
    }
}

/// A claimed causal link.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CausalEdge {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// Fraction of the cause's move transmitted, in `[0, 1]`.
    pub strength: f64,
    /// Typical delay before the effect is observable.
    pub lag: Duration,
    /// Confidence in the claim itself, in `[0, 1]`.
    pub confidence: f64,
    /// Ids of the evidence supporting the claim.
    pub evidence: Vec<String>,
    /// When the platform recorded the claim.
    pub recorded_at: Timestamp,
}

impl CausalEdge {
    pub fn new(
        cause: impl Into<String>,
        effect: impl Into<String>,
        mechanism: Mechanism,
        strength: f64,
        lag: Duration,
        recorded_at: Timestamp,
    ) -> Self {
        Self {
            cause: cause.into(),
            effect: effect.into(),
            mechanism,
            strength: strength.clamp(0.0, 1.0),
            lag,
            confidence: 0.7,
            evidence: Vec::new(),
            recorded_at,
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    pub fn with_evidence(mut self, evidence: Vec<String>) -> Self {
        self.evidence = evidence;
        self
    }

    /// Effective transmission: strength discounted by confidence in the claim.
    ///
    /// A strong mechanism nobody is sure of should not move a portfolio as much
    /// as a weaker one that is well established.
    pub fn transmission(&self) -> f64 {
        self.strength * self.confidence
    }

    /// Whether the claim rests on anything.
    pub fn is_evidenced(&self) -> bool {
        !self.evidence.is_empty()
    }
}

/// One node in a propagated shock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub target: String,
    /// Hops from the origin: 1 is a direct effect, 2 second-order, and so on.
    pub order: usize,
    /// Signed magnitude relative to the original shock.
    pub magnitude: f64,
    /// When the effect should be observable.
    pub expected_at: Timestamp,
    /// The chain of mechanisms that produced it, in order.
    pub chain: Vec<Mechanism>,
    /// Nodes traversed, starting at the origin.
    pub path: Vec<String>,
    /// Product of the confidences along the chain.
    pub confidence: f64,
}

impl Effect {
    /// A sentence describing the transmission, for a thesis.
    pub fn explain(&self) -> String {
        if self.chain.is_empty() {
            return format!("{} is the origin of the shock", self.target);
        }
        let steps: Vec<String> = self
            .chain
            .iter()
            .zip(self.path.windows(2))
            .map(|(mechanism, pair)| {
                format!("{} to {} ({})", pair[0], pair[1], mechanism.describe())
            })
            .collect();
        format!(
            "order {} effect on {} at {:+.1}% of the original move, via {}",
            self.order,
            self.target,
            self.magnitude * 100.0,
            steps.join("; then ")
        )
    }
}

/// The result of propagating a shock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropagationResult {
    pub origin: String,
    pub initial_shock: f64,
    pub effects: Vec<Effect>,
    /// Effects dropped for falling below the magnitude floor.
    pub truncated: usize,
}

impl PropagationResult {
    /// Effects at exactly one order.
    pub fn at_order(&self, order: usize) -> Vec<&Effect> {
        self.effects.iter().filter(|e| e.order == order).collect()
    }

    /// The largest effects, by absolute magnitude.
    pub fn strongest(&self, limit: usize) -> Vec<&Effect> {
        let mut ranked: Vec<&Effect> = self.effects.iter().collect();
        ranked.sort_by(|a, b| {
            b.magnitude
                .abs()
                .partial_cmp(&a.magnitude.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.target.cmp(&b.target))
        });
        ranked.truncate(limit);
        ranked
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

/// A directed graph of causal claims.
#[derive(Debug, Default)]
pub struct CausalGraph {
    edges: Vec<CausalEdge>,
    /// Cause to the indices of its outgoing edges.
    by_cause: BTreeMap<String, Vec<usize>>,
    by_effect: BTreeMap<String, Vec<usize>>,
    /// The newest instant at which the graph absorbed a claim, recorded in
    /// [`Self::add`] and nowhere else. `None` until the first claim.
    ///
    /// This is the fact §6.2 row 2 is judged on. It is recorded at the seam
    /// rather than derived from the edges on demand so that the answer the
    /// degradation table reads is the one the graph wrote when the claim
    /// landed, not a scan somebody could later change the rule of.
    last_updated: Option<Timestamp>,
}

impl CausalGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Record a claim, and the instant the platform recorded it as the
    /// graph's newest update.
    ///
    /// The instant is the edge's own `recorded_at` — the one fact the edge
    /// carries about when the platform absorbed it — and it only ever moves
    /// forward: a claim backfilled with an older `recorded_at` is still a
    /// claim the graph absorbed, but it does not make the graph *less*
    /// current than the newest thing it holds, and letting it rewind would
    /// turn a replay of history into a stale reading.
    pub fn add(&mut self, edge: CausalEdge) {
        let index = self.edges.len();
        self.by_cause
            .entry(edge.cause.clone())
            .or_default()
            .push(index);
        self.by_effect
            .entry(edge.effect.clone())
            .or_default()
            .push(index);
        self.last_updated = Some(match self.last_updated {
            Some(held) if held >= edge.recorded_at => held,
            _ => edge.recorded_at,
        });
        self.edges.push(edge);
    }

    /// The newest instant at which a claim was absorbed, or `None` if none
    /// ever was.
    ///
    /// What `qip_contracts::degradation::CausalGraphFreshness::assess` reads.
    /// Queries — propagation, explanation, the point-in-time views — never
    /// move it: reading the graph is not evidence about the world.
    pub fn last_updated(&self) -> Option<Timestamp> {
        self.last_updated
    }

    pub fn edges(&self) -> &[CausalEdge] {
        &self.edges
    }

    /// Edges leaving `cause`, known by `known_at`.
    pub fn outgoing(&self, cause: &str, known_at: Timestamp) -> Vec<&CausalEdge> {
        self.by_cause
            .get(cause)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|i| self.edges.get(*i))
                    .filter(|e| e.recorded_at <= known_at)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Edges arriving at `effect` — what could explain a move.
    pub fn incoming(&self, effect: &str, known_at: Timestamp) -> Vec<&CausalEdge> {
        self.by_effect
            .get(effect)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|i| self.edges.get(*i))
                    .filter(|e| e.recorded_at <= known_at)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Propagate a shock from `origin`, breadth-first with attenuation.
    ///
    /// Each hop multiplies by the edge's transmission and may flip the sign.
    /// Anything falling below `magnitude_floor` is dropped and counted, so the
    /// caller knows the chain was cut rather than exhausted.
    pub fn propagate(
        &self,
        origin: &str,
        initial_shock: f64,
        max_order: usize,
        magnitude_floor: f64,
        at: Timestamp,
        known_at: Timestamp,
    ) -> PropagationResult {
        let mut effects: Vec<Effect> = Vec::new();
        let mut truncated = 0usize;
        // The strongest path to each target wins; a weaker route to somewhere
        // already reached adds nothing but noise.
        let mut best: BTreeMap<String, f64> = BTreeMap::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        visited.insert(origin.to_string());

        let mut queue: VecDeque<Effect> = VecDeque::new();
        queue.push_back(Effect {
            target: origin.to_string(),
            order: 0,
            magnitude: initial_shock,
            expected_at: at,
            chain: Vec::new(),
            path: vec![origin.to_string()],
            confidence: 1.0,
        });

        while let Some(current) = queue.pop_front() {
            if current.order >= max_order {
                continue;
            }
            for edge in self.outgoing(&current.target, known_at) {
                if current.path.contains(&edge.effect) {
                    continue; // a cycle would amplify without limit
                }
                let sign = if edge.mechanism.preserves_sign() {
                    1.0
                } else {
                    -1.0
                };
                let magnitude = current.magnitude * edge.transmission() * sign;
                if magnitude.abs() < magnitude_floor {
                    truncated += 1;
                    continue;
                }

                let mut chain = current.chain.clone();
                chain.push(edge.mechanism);
                let mut path = current.path.clone();
                path.push(edge.effect.clone());

                let effect = Effect {
                    target: edge.effect.clone(),
                    order: current.order + 1,
                    magnitude,
                    expected_at: current.expected_at.saturating_add(edge.lag),
                    chain,
                    path,
                    confidence: current.confidence * edge.confidence,
                };

                let previous = best.get(&edge.effect).copied().unwrap_or(0.0);
                if magnitude.abs() > previous.abs() {
                    best.insert(edge.effect.clone(), magnitude);
                    effects.retain(|e| e.target != edge.effect);
                    effects.push(effect.clone());
                }
                if visited.insert(edge.effect.clone()) || magnitude.abs() > previous.abs() {
                    queue.push_back(effect);
                }
            }
        }

        effects.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| {
                    b.magnitude
                        .abs()
                        .partial_cmp(&a.magnitude.abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.target.cmp(&b.target))
        });

        PropagationResult {
            origin: origin.to_string(),
            initial_shock,
            effects,
            truncated,
        }
    }

    /// Plausible causes of a move at `target`, strongest first.
    ///
    /// The inverse question to propagation, and the one the reasoning engine
    /// asks when something moved and nobody knows why.
    pub fn explanations(&self, target: &str, known_at: Timestamp) -> Vec<&CausalEdge> {
        let mut candidates = self.incoming(target, known_at);
        candidates.sort_by(|a, b| {
            b.transmission()
                .partial_cmp(&a.transmission())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cause.cmp(&b.cause))
        });
        candidates
    }

    /// Claims with no supporting evidence.
    ///
    /// Surfaced so an unevidenced claim can be challenged rather than quietly
    /// accumulating influence over decisions.
    pub fn unevidenced(&self) -> Vec<&CausalEdge> {
        self.edges.iter().filter(|e| !e.is_evidenced()).collect()
    }
}
