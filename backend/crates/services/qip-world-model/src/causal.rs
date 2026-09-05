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

use qip_core::{Duration, Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A link, as re-estimation keys it: cause, effect and the mechanism claimed
/// between them.
///
/// The mechanism is part of the key because evidence about a cost pass-through
/// is not evidence about a sentiment spillover, even between the same pair.
/// Ordered, and carried in a [`BTreeMap`], because a re-estimation report
/// reaches an operator and a replay that reorders is not a replay.
type EdgeKey = (String, String, Mechanism);

/// How one thing affects another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// When a re-estimation last found no supporting claim inside its horizon,
    /// if one ever has.
    ///
    /// A mark, not an attenuation. [`CausalGraph::reestimate`] reports every
    /// decayed edge to its caller and leaves [`Self::transmission`] alone on
    /// purpose: silently shrinking a stale claim would move every propagation
    /// result in the platform with nothing in the record naming the number
    /// that changed, which is the shape of failure this crate exists to
    /// refuse. Cleared the moment a claim inside the horizon supports the edge
    /// again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decayed_at: Option<Timestamp>,
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
            decayed_at: None,
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

    /// Whether the last re-estimation found no claim inside its horizon for
    /// this link.
    ///
    /// A graph nobody has re-estimated answers `false` for every edge, which
    /// is honest: an unasked question is not a negative answer.
    pub fn is_decayed(&self) -> bool {
        self.decayed_at.is_some()
    }

    /// This edge's key, as [`CausalGraph::reestimate`] matches claims to it.
    fn key(&self) -> EdgeKey {
        (self.cause.clone(), self.effect.clone(), self.mechanism)
    }
}

/// Evidence bearing on a link the causal graph already holds.
///
/// Deliberately not a [`CausalEdge`]. An edge is the claim a shock is
/// propagated along; a supporting claim is one observation of what that
/// transmission measured, on a day, from named evidence. Absorbing support as
/// a second edge would leave two edges for one link, and
/// [`CausalGraph::propagate`] keeps the *strongest* path to a target rather
/// than the newest — so a link re-measured downwards would go on propagating
/// its old, larger number for ever. Re-estimation exists so that the newest
/// evidence inside the horizon changes the edge instead of accumulating
/// beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SupportingClaim {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The transmission this evidence measured, in `[0, 1]`.
    ///
    /// Not clamped here, unlike [`CausalEdge::new`]: a reading outside the
    /// range is a producer's bug, and [`CausalGraph::reestimate`] refuses it
    /// naming the link. A value silently corrected is a caller bug that
    /// survives into a strength the platform sizes against.
    pub strength: f64,
    /// When the platform recorded the evidence — the instant it became
    /// knowable here, not the instant the world produced it.
    pub recorded_at: Timestamp,
    /// Ids of the evidence, carried onto the edge when the claim is used, so
    /// that a strength which moved names what moved it.
    ///
    /// Empty is legitimate and stays empty. Manufacturing an id would silence
    /// [`CausalGraph::unevidenced`], which exists to surface a claim resting
    /// on nothing.
    pub evidence: Vec<String>,
}

impl SupportingClaim {
    pub fn new(
        cause: impl Into<String>,
        effect: impl Into<String>,
        mechanism: Mechanism,
        strength: f64,
        recorded_at: Timestamp,
    ) -> Self {
        Self {
            cause: cause.into(),
            effect: effect.into(),
            mechanism,
            strength,
            recorded_at,
            evidence: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence: Vec<String>) -> Self {
        self.evidence = evidence;
        self
    }

    fn key(&self) -> EdgeKey {
        (self.cause.clone(), self.effect.clone(), self.mechanism)
    }
}

/// One edge whose strength a re-estimation moved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrengthUpdate {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The strength the graph propagated along before.
    pub previous: f64,
    /// The mean of the in-horizon claims that supported the link.
    pub updated: f64,
    /// How many claims that mean is over. A caller reading a large move off
    /// one observation needs to see the one.
    pub claims: usize,
    /// The newest claim used — the instant the updated strength became
    /// knowable, and the edge's `recorded_at` from here on unless the edge was
    /// already recorded later than that.
    pub newest_claim: Timestamp,
}

/// One edge no claim inside the horizon supported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecayedEdge {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// When the edge itself was recorded — how old the newest evidence for
    /// this link is.
    pub recorded_at: Timestamp,
    /// The edge's effective transmission, so a caller can tell a decayed link
    /// that matters from one that never moved anything.
    pub transmission: f64,
    /// When an earlier re-estimation already marked it, if one did. `None` is
    /// the transition into decay — the one a caller records, so that repeating
    /// a re-estimation does not repeat the entry.
    pub previously_marked: Option<Timestamp>,
}

/// What one re-estimation did, in full.
///
/// Returned rather than logged: the decay of a link is a fact about the
/// platform's evidence, and a caller that must decide what to do about it
/// cannot read a log line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reestimation {
    /// The instant re-estimated at, and — when [`Self::refreshed`] — the
    /// graph's new `last_updated`.
    pub at: Timestamp,
    pub horizon: Duration,
    /// Every claim offered, in horizon or not.
    pub claims_considered: usize,
    /// Claims inside the horizon that matched an edge and moved a strength.
    /// The count [`Self::refreshed`] is decided on.
    pub claims_used: usize,
    /// Edges re-estimated, in key order.
    pub updated: Vec<StrengthUpdate>,
    /// Edges with no claim inside the horizon, in key order. Marked and
    /// reported; never dropped from the graph.
    pub decayed: Vec<DecayedEdge>,
    /// In-horizon claims naming a link the graph does not hold, in key order.
    /// Reported rather than turned into an edge: inventing a link from
    /// evidence about one is how a correlation becomes a thesis.
    pub unmatched: Vec<SupportingClaim>,
    /// Whether the graph's `last_updated` moved to [`Self::at`].
    pub refreshed: bool,
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
    /// The newest instant at which the graph absorbed evidence. `None` until
    /// the first claim.
    ///
    /// This is the fact §6.2 row 2 is judged on. It is recorded at the seams
    /// rather than derived from the edges on demand so that the answer the
    /// degradation table reads is the one the graph wrote when the evidence
    /// landed, not a scan somebody could later change the rule of.
    ///
    /// Exactly two writers, and no others: [`Self::add`], from the absorbed
    /// edge's own `recorded_at`, and [`Self::reestimate`], from the instant it
    /// re-estimated at — and that one only when at least one claim inside the
    /// horizon was actually used. A re-estimation that used nothing must leave
    /// the fact alone, or every stale graph would read fresh the moment
    /// somebody asked it a question.
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

    /// The newest instant at which a claim was absorbed or used, or `None` if
    /// none ever was.
    ///
    /// What `qip_contracts::degradation::CausalGraphFreshness::assess` reads.
    /// Queries — propagation, explanation, the point-in-time views — never
    /// move it: reading the graph is not evidence about the world. Neither
    /// does a re-estimation that found nothing inside its horizon, for the
    /// same reason.
    pub fn last_updated(&self) -> Option<Timestamp> {
        self.last_updated
    }

    /// Re-estimate every edge from the supporting claims absorbed inside
    /// `horizon`, and report what that did.
    ///
    /// The failure this closes: a graph carried whichever strength arrived
    /// first, for ever. Nothing re-estimated it from what the world model kept
    /// absorbing, so §6.2 row 2 read stale on the demo seed — correctly, and
    /// with no path to reading anything else.
    ///
    /// What it does, and what it deliberately does not:
    ///
    /// * An edge with at least one claim inside the horizon takes the **mean**
    ///   of those claims as its strength, and the evidence ids that produced
    ///   it are merged into its own. The mean rather than a blend with the
    ///   prior: a blend weight would be a policy number nobody here measured,
    ///   and the horizon already decides which evidence counts.
    /// * That edge's `recorded_at` moves forward to the newest claim used.
    ///   Without this the re-estimated strength would be readable by a
    ///   point-in-time query at an instant before the evidence for it existed
    ///   — leakage a backtest cannot see, because the record itself would
    ///   claim to have been available. It moves forward only; a claim older
    ///   than the edge does not make the edge knowable earlier than it was.
    /// * An edge with no claim inside the horizon is **marked** decayed and
    ///   reported, never dropped and never silently attenuated. Dropping it
    ///   would delete a relationship nobody disproved; attenuating it would
    ///   change every propagation with nothing naming the number that moved.
    /// * A claim naming a link the graph does not hold is reported unmatched.
    ///   Adding it would invent a causal edge from evidence about one, which
    ///   is what [`CausalGraph::add`] is for and what a mechanism, a lag and a
    ///   confidence exist to make deliberate.
    /// * `last_updated` moves to `now` only if at least one in-horizon claim
    ///   was used. A re-estimation over nothing is not evidence, and a graph
    ///   that refreshed itself by being asked would report a freshness it had
    ///   not earned.
    ///
    /// Deterministic: claims are grouped and reported through [`BTreeMap`]s
    /// keyed on the link, so the report and the resulting edges depend on the
    /// input and not on its order or on any hash seed.
    ///
    /// Refuses, before touching a single edge — so a refused re-estimation
    /// leaves the graph exactly as it was: a non-positive horizon, a strength
    /// outside `[0, 1]` or not finite, a claim recorded after `now`, and a
    /// `now` before the instant the graph has already absorbed. The last two
    /// are clock bugs, and reading either as new evidence would size against
    /// one.
    pub fn reestimate<I>(
        &mut self,
        claims: I,
        horizon: Duration,
        now: Timestamp,
    ) -> Result<Reestimation>
    where
        I: IntoIterator<Item = SupportingClaim>,
    {
        if horizon <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "a causal re-estimation horizon of {horizon:?} admits no claim at all; pass a \
                 positive horizon — the centre's is qip_contracts::degradation::\
                 CAUSAL_GRAPH_HORIZON"
            )));
        }
        if let Some(held) = self.last_updated
            && held > now
        {
            return Err(Error::invalid(format!(
                "the causal graph last absorbed evidence at {}, after the {} it is being \
                 re-estimated at; a re-estimation cannot rewind the freshness fact — fix the \
                 clock rather than the reading",
                held.to_rfc3339(),
                now.to_rfc3339()
            )));
        }

        let mut claims_considered = 0usize;
        let mut in_horizon: BTreeMap<EdgeKey, Vec<SupportingClaim>> = BTreeMap::new();
        for claim in claims {
            claims_considered += 1;
            if !claim.strength.is_finite() || !(0.0..=1.0).contains(&claim.strength) {
                return Err(Error::invalid(format!(
                    "a claim supporting {} -> {} measured a transmission of {}; a transmission is \
                     a fraction in [0, 1] — fix the reading at its source rather than clamping it \
                     into the graph",
                    claim.cause, claim.effect, claim.strength
                )));
            }
            if claim.recorded_at > now {
                return Err(Error::invalid(format!(
                    "a claim supporting {} -> {} is recorded at {}, after the {} it is being \
                     re-estimated at; a claim from the future is a clock bug, not new evidence",
                    claim.cause,
                    claim.effect,
                    claim.recorded_at.to_rfc3339(),
                    now.to_rfc3339()
                )));
            }
            // Counted and then deliberately unused: an old claim is what the
            // horizon exists to exclude, and its absence is what marks decay.
            if now.since(claim.recorded_at) > horizon {
                continue;
            }
            in_horizon.entry(claim.key()).or_default().push(claim);
        }

        // Keyed by link *and* edge index: two edges may claim the same link,
        // and both are re-estimated, so the key alone would collapse them.
        let mut updated: BTreeMap<(EdgeKey, usize), StrengthUpdate> = BTreeMap::new();
        let mut decayed: BTreeMap<(EdgeKey, usize), DecayedEdge> = BTreeMap::new();
        let mut matched: BTreeSet<EdgeKey> = BTreeSet::new();
        for (index, edge) in self.edges.iter_mut().enumerate() {
            let key = edge.key();
            let Some(support) = in_horizon.get(&key) else {
                let previously_marked = edge.decayed_at;
                let entry = DecayedEdge {
                    cause: edge.cause.clone(),
                    effect: edge.effect.clone(),
                    mechanism: edge.mechanism,
                    recorded_at: edge.recorded_at,
                    transmission: edge.transmission(),
                    previously_marked,
                };
                edge.decayed_at = Some(now);
                decayed.insert((key, index), entry);
                continue;
            };
            // At least one claim, because a group exists only where one was
            // pushed: the mean below cannot divide by zero.
            let mut total = 0.0f64;
            let mut newest: Option<Timestamp> = None;
            let mut fresh_evidence: BTreeSet<String> = BTreeSet::new();
            for claim in support {
                total += claim.strength;
                newest = Some(match newest {
                    Some(held) if held >= claim.recorded_at => held,
                    _ => claim.recorded_at,
                });
                for id in &claim.evidence {
                    if !edge.evidence.contains(id) {
                        fresh_evidence.insert(id.clone());
                    }
                }
            }
            // The group is never empty; the fallback is the edge's own instant
            // so that an empty one could only ever leave knowability where it
            // already was.
            let newest_claim = newest.unwrap_or(edge.recorded_at);
            let previous = edge.strength;
            edge.strength = total / support.len() as f64;
            if newest_claim > edge.recorded_at {
                edge.recorded_at = newest_claim;
            }
            edge.decayed_at = None;
            edge.evidence.extend(fresh_evidence);
            updated.insert(
                (key.clone(), index),
                StrengthUpdate {
                    cause: edge.cause.clone(),
                    effect: edge.effect.clone(),
                    mechanism: edge.mechanism,
                    previous,
                    updated: edge.strength,
                    claims: support.len(),
                    newest_claim,
                },
            );
            matched.insert(key);
        }

        let mut claims_used = 0usize;
        let mut unmatched: Vec<SupportingClaim> = Vec::new();
        for (key, support) in in_horizon {
            if matched.contains(&key) {
                claims_used += support.len();
            } else {
                unmatched.extend(support);
            }
        }
        let refreshed = claims_used > 0;
        if refreshed {
            self.last_updated = Some(now);
        }
        Ok(Reestimation {
            at: now,
            horizon,
            claims_considered,
            claims_used,
            updated: updated.into_values().collect(),
            decayed: decayed.into_values().collect(),
            unmatched,
            refreshed,
        })
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
