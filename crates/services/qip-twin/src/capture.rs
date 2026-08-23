//! What the platform did, what it declined to do, and what came of both.
//!
//! This is the central-plane counterpart to the edge cell's journal in
//! `qip_edge::journal`. The cell hash-chains what it decided at speed; this
//! chains what the whole platform decided and what the world then did about it.
//! The shape is deliberately the same — an append-only chain whose digests make
//! a dropped entry visible — because a record that can lose an entry silently
//! is not a record.
//!
//! Two things the audit specifically asked for shape the vocabulary here.
//!
//! **A refusal is an outcome.** [`Action::MissedOpportunity`] and
//! [`Action::StaleOpportunity`] sit alongside [`Action::Filled`] rather than in
//! a separate log, because a platform that records only its trades can say what
//! it earned and not what it declined. The second number is the one that says
//! whether the gates are calibrated or merely tight.
//!
//! **What a refusal would have earned is not money.** It is carried as a
//! [`Simulated<Decimal>`], so the total of everything the platform passed on
//! can be computed, reported, and never added to the total of what it made.
//! [`OutcomeCapture::realised_pnl`] and [`OutcomeCapture::forgone`] return
//! different types on purpose.

use crate::value::Simulated;
use qip_contracts::message::BookSide;
use qip_contracts::signal::{Conviction, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::{DecisionId, EventId, ObjectId, OpportunityId, OrderId};
use qip_core::lineage::{CausationId, CorrelationId, Lineage, TraceId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One thing the platform did, or declined to do.
///
/// Twelve variants, covering the whole surface the audit named. The ones that
/// describe an absence are not second-class: a stale opportunity and a filled
/// order are recorded by the same call, into the same chain, and are counted by
/// the same tally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// An order reached a venue.
    OrderPlaced {
        order_id: OrderId,
        venue: VenueId,
        side: BookSide,
        quantity: Decimal,
        /// The execution method the router chose, e.g. `twap`, `participation`.
        method: String,
    },
    /// An order completed.
    Filled {
        order_id: OrderId,
        venue: VenueId,
        quantity: Decimal,
        price: Decimal,
    },
    /// An order filled in part and is still working, or stopped short.
    PartiallyFilled {
        order_id: OrderId,
        venue: VenueId,
        filled: Decimal,
        remaining: Decimal,
        price: Decimal,
    },
    /// An order was withdrawn before completing.
    Cancelled { order_id: OrderId, reason: String },
    /// An order was refused, by a gate here or by the venue.
    Rejected {
        order_id: OrderId,
        /// Which control refused it, so refusals can be counted by cause.
        gate: String,
        reason: String,
    },
    /// An opportunity was seen and not taken.
    ///
    /// The variant the audit flagged as missing. `would_have_earned` is
    /// simulated by construction — it is what an alternative world produced —
    /// and the type is what keeps it out of the P&L it is reported next to.
    MissedOpportunity {
        opportunity: OpportunityId,
        object_id: ObjectId,
        /// Why it was passed on: the gate, the budget, the absent conviction.
        reason: String,
        would_have_earned: Simulated<Decimal>,
    },
    /// An opportunity was acted on after it had stopped being one.
    ///
    /// Distinct from a miss: the platform did decide, just too late, and the
    /// interesting number is the age rather than the reason.
    StaleOpportunity {
        opportunity: OpportunityId,
        object_id: ObjectId,
        seen_at: Timestamp,
        /// How long the opportunity had been visible before it was acted on.
        age: Duration,
        would_have_earned: Simulated<Decimal>,
    },
    /// The gap between the price a decision assumed and the price it got.
    Slippage {
        order_id: OrderId,
        expected: Decimal,
        achieved: Decimal,
        bps_f64: f64,
    },
    /// A stage took longer than its budget, or is being recorded because it
    /// did not.
    Latency {
        stage: String,
        elapsed: Duration,
        budget: Duration,
    },
    /// A model proposed something, whether or not anyone acted on it.
    ModelRecommendation {
        model: String,
        version: String,
        recommendation: String,
        confidence: Conviction,
    },
    /// Capital was asked for and some amount was granted.
    CapitalDecision {
        strategy: StrategyId,
        requested: Decimal,
        granted: Decimal,
        reason: String,
    },
    /// A risk control ruled on something.
    RiskDecision {
        control: String,
        allowed: bool,
        reason: String,
    },
}

impl Action {
    /// A short label for the kind, for counting without matching.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::OrderPlaced { .. } => "order_placed",
            Self::Filled { .. } => "filled",
            Self::PartiallyFilled { .. } => "partially_filled",
            Self::Cancelled { .. } => "cancelled",
            Self::Rejected { .. } => "rejected",
            Self::MissedOpportunity { .. } => "missed_opportunity",
            Self::StaleOpportunity { .. } => "stale_opportunity",
            Self::Slippage { .. } => "slippage",
            Self::Latency { .. } => "latency",
            Self::ModelRecommendation { .. } => "model_recommendation",
            Self::CapitalDecision { .. } => "capital_decision",
            Self::RiskDecision { .. } => "risk_decision",
        }
    }

    /// Whether the platform actually put capital at risk here.
    ///
    /// The predicate that separates "what did we do" from "what did we decline
    /// to do". A missed opportunity carrying a large `would_have_earned` is not
    /// a trade, and any report that treats it as one is wrong in the direction
    /// that flatters.
    pub const fn is_taken(&self) -> bool {
        matches!(
            self,
            Self::OrderPlaced { .. } | Self::Filled { .. } | Self::PartiallyFilled { .. }
        )
    }

    /// Whether this records the platform declining.
    pub const fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::Rejected { .. }
                | Self::MissedOpportunity { .. }
                | Self::StaleOpportunity { .. }
                | Self::RiskDecision { allowed: false, .. }
        )
    }

    /// What an untaken action would have earned, when it is one.
    pub const fn forgone(&self) -> Option<Simulated<Decimal>> {
        match self {
            Self::MissedOpportunity {
                would_have_earned, ..
            }
            | Self::StaleOpportunity {
                would_have_earned, ..
            } => Some(*would_have_earned),
            _ => None,
        }
    }
}

/// What actually happened, in money that actually moved.
///
/// The counterpart type to [`crate::counterfactual::SimulatedOutcome`], and
/// deliberately unrelated to it: there is no conversion in either direction, so
/// nothing that came out of a simulation can be recorded here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealisedOutcome {
    /// Profit and loss that settled. Exact.
    realised_pnl: Decimal,
    /// Commissions, fees and financing that were actually paid.
    costs: Decimal,
    /// Quantity that actually traded. Zero for a refusal.
    filled_quantity: Decimal,
    /// Difference between the decision price and the achieved price.
    slippage_bps_f64: f64,
    /// Wall time from the source event to this outcome.
    latency: Duration,
    /// When the outcome was known.
    at: Timestamp,
}

impl RealisedOutcome {
    /// An outcome where money moved.
    pub const fn realised(
        at: Timestamp,
        realised_pnl: Decimal,
        costs: Decimal,
        filled_quantity: Decimal,
    ) -> Self {
        Self {
            realised_pnl,
            costs,
            filled_quantity,
            slippage_bps_f64: 0.0,
            latency: Duration::ZERO,
            at,
        }
    }

    /// The outcome of a refusal: nothing traded, nothing earned, nothing lost.
    ///
    /// Recorded rather than omitted. A refusal with no outcome row is
    /// indistinguishable from a decision nobody made, and the difference is the
    /// whole point of capturing refusals.
    pub const fn nothing_happened(at: Timestamp) -> Self {
        Self::realised(at, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
    }

    pub const fn with_slippage_bps(mut self, bps: f64) -> Self {
        self.slippage_bps_f64 = bps;
        self
    }

    pub const fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    pub const fn realised_pnl(&self) -> Decimal {
        self.realised_pnl
    }

    pub const fn costs(&self) -> Decimal {
        self.costs
    }

    pub const fn filled_quantity(&self) -> Decimal {
        self.filled_quantity
    }

    pub const fn slippage_bps_f64(&self) -> f64 {
        self.slippage_bps_f64
    }

    pub const fn latency(&self) -> Duration {
        self.latency
    }

    pub const fn at(&self) -> Timestamp {
        self.at
    }
}

/// One decision, with everything needed to find where it came from.
///
/// The trace id is a required constructor argument rather than an optional
/// field. An outcome that cannot be traced is one nobody can explain after the
/// fact, so the type does not offer the option of omitting it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub decision_id: DecisionId,
    /// The distributed trace this decision belongs to. Required.
    pub trace: TraceId,
    /// Groups every record produced while working one originating observation.
    pub correlation: CorrelationId,
    /// The decision or event that directly produced this one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub caused_by: Option<CausationId>,
    /// The market observation the chain started from.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_event: Option<EventId>,
    /// When the decision was made. Every counterfactual for it is evaluated as
    /// of this instant and no later.
    pub at: Timestamp,
    pub object_id: ObjectId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub strategy: Option<StrategyId>,
    pub action: Action,
    /// One sentence on why, for the human reading this after a bad quarter.
    pub rationale: String,
}

impl Decision {
    /// Record a decision. The trace is not optional.
    pub fn new(
        decision_id: DecisionId,
        trace: TraceId,
        correlation: CorrelationId,
        at: Timestamp,
        object_id: ObjectId,
        action: Action,
    ) -> Self {
        Self {
            decision_id,
            trace,
            correlation,
            caused_by: None,
            source_event: None,
            at,
            object_id,
            strategy: None,
            action,
            rationale: String::new(),
        }
    }

    /// Name the originating market observation.
    pub fn from_event(mut self, event: EventId) -> Self {
        self.caused_by = Some(CausationId::from_event(&event));
        self.source_event = Some(event);
        self
    }

    /// Name the decision this one followed from.
    pub fn after(mut self, parent: &Decision) -> Self {
        self.caused_by = Some(CausationId(parent.decision_id.as_str().to_string()));
        self.source_event = parent.source_event.clone();
        self
    }

    pub fn by(mut self, strategy: StrategyId) -> Self {
        self.strategy = Some(strategy);
        self
    }

    pub fn because(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = rationale.into();
        self
    }

    /// Provenance in the platform-wide shape, with the trace always present.
    pub fn lineage(&self, producer: impl Into<String>) -> Lineage {
        Lineage {
            correlation_id: self.correlation.clone(),
            causation_id: self.caused_by.clone(),
            trace_id: Some(self.trace.clone()),
            producer: producer.into(),
        }
    }

    pub const fn kind(&self) -> &'static str {
        self.action.kind()
    }
}

/// A decision, its outcome, and its place in the chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapturedOutcome {
    pub sequence: u64,
    pub decision: Decision,
    pub outcome: RealisedOutcome,
    /// `sha256(previous_digest | sequence | decision | outcome)`.
    pub digest: String,
}

impl CapturedOutcome {
    /// The trace this record belongs to. Always present.
    pub const fn trace(&self) -> &TraceId {
        &self.decision.trace
    }
}

/// One link in a reconstructed chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceLink {
    pub decision_id: DecisionId,
    pub kind: String,
    pub at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub caused_by: Option<CausationId>,
}

/// The chain from a source event through to an outcome.
///
/// What "reconstructable end to end" means concretely: the links in order, the
/// event they descend from, and the realised total for the whole trace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceChain {
    pub trace: TraceId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_event: Option<EventId>,
    pub links: Vec<TraceLink>,
    /// Realised P&L across the trace. Exact, and containing nothing simulated.
    pub realised_pnl: Decimal,
    /// What the trace declined, kept separately and in the type that cannot be
    /// added to the line above.
    pub forgone: Simulated<Decimal>,
}

impl TraceChain {
    /// Whether the chain starts at a named observation rather than in mid-air.
    pub const fn is_rooted(&self) -> bool {
        self.source_event.is_some()
    }

    pub fn summarise(&self) -> String {
        let steps: Vec<&str> = self.links.iter().map(|link| link.kind.as_str()).collect();
        format!(
            "trace {} from {}: {} -> realised {} (declined {})",
            self.trace.as_str(),
            self.source_event
                .as_ref()
                .map_or_else(|| "an unnamed observation".to_string(), ToString::to_string),
            steps.join(" -> "),
            self.realised_pnl,
            self.forgone
        )
    }
}

/// Everything the platform decided, and what came of it.
///
/// Append-only and hash-chained. The chain is not decoration: an outcome
/// capture that can lose a refusal without anyone noticing answers "what did we
/// earn" and not "what did we decline", which is the question the audit found
/// unanswerable.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OutcomeCapture {
    entries: Vec<CapturedOutcome>,
}

impl OutcomeCapture {
    /// The digest an empty chain starts from.
    ///
    /// Named rather than empty, so a capture that claims to start at the
    /// beginning is distinguishable from one whose head went missing.
    pub const GENESIS: &'static str = "genesis";

    pub fn new() -> Self {
        Self::default()
    }

    /// Record a decision and its outcome.
    ///
    /// Refuses an outcome dated before its decision. That combination has no
    /// physical meaning and always means a clock bug or a mis-stitched record,
    /// and letting it into the chain would put a realised figure earlier in
    /// time than the decision that earned it.
    pub fn record(
        &mut self,
        decision: Decision,
        outcome: RealisedOutcome,
    ) -> Result<&CapturedOutcome> {
        if decision.trace.as_str().trim().is_empty() {
            return Err(Error::invalid(
                "a captured outcome needs a trace id; an outcome nobody can trace is one nobody can explain",
            ));
        }
        if outcome.at() < decision.at {
            return Err(Error::invalid(format!(
                "the outcome of decision {} is dated {} but the decision was made at {}",
                decision.decision_id,
                outcome.at(),
                decision.at
            )));
        }
        let sequence = self.entries.len() as u64;
        let previous = self
            .entries
            .last()
            .map_or_else(|| Self::GENESIS.to_string(), |entry| entry.digest.clone());
        let digest = chain_digest(&previous, sequence, &decision, &outcome);
        self.entries.push(CapturedOutcome {
            sequence,
            decision,
            outcome,
            digest,
        });
        self.entries
            .last()
            .ok_or_else(|| Error::invalid("an entry was just pushed and is not there"))
    }

    pub fn entries(&self) -> &[CapturedOutcome] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Verify the chain, naming the sequence where it first breaks.
    pub fn verify(&self) -> Result<()> {
        let mut previous = Self::GENESIS.to_string();
        for entry in &self.entries {
            let expected = chain_digest(&previous, entry.sequence, &entry.decision, &entry.outcome);
            if expected != entry.digest {
                return Err(Error::invalid(format!(
                    "the outcome capture breaks its chain at sequence {}",
                    entry.sequence
                )));
            }
            previous = entry.digest.clone();
        }
        Ok(())
    }

    /// How many outcomes of each kind were captured.
    pub fn tally(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for entry in &self.entries {
            *counts.entry(entry.decision.kind()).or_insert(0) += 1;
        }
        counts
    }

    /// Realised P&L. Exact, and containing nothing that did not happen.
    pub fn realised_pnl(&self) -> Decimal {
        self.entries
            .iter()
            .map(|entry| entry.outcome.realised_pnl())
            .fold(Decimal::ZERO, |a, b| a + b)
    }

    /// What the platform declined, in the type that cannot join the line above.
    ///
    /// Reported next to [`OutcomeCapture::realised_pnl`] and never summed with
    /// it. The pair is the answer to "what did we decline and what would it
    /// have earned"; a single combined number would be a fabrication.
    pub fn forgone(&self) -> Simulated<Decimal> {
        self.entries
            .iter()
            .filter_map(|entry| entry.decision.action.forgone())
            .sum()
    }

    /// Everything the platform passed on.
    pub fn missed(&self) -> Vec<&CapturedOutcome> {
        self.entries
            .iter()
            .filter(|entry| entry.decision.action.forgone().is_some())
            .collect()
    }

    /// Everything the platform actually put capital behind.
    pub fn taken(&self) -> Vec<&CapturedOutcome> {
        self.entries
            .iter()
            .filter(|entry| entry.decision.action.is_taken())
            .collect()
    }

    /// Every refusal, whatever gate produced it.
    pub fn refusals(&self) -> Vec<&CapturedOutcome> {
        self.entries
            .iter()
            .filter(|entry| entry.decision.action.is_refusal())
            .collect()
    }

    /// Everything recorded under one trace, in capture order.
    pub fn trace(&self, trace: &TraceId) -> Vec<&CapturedOutcome> {
        self.entries
            .iter()
            .filter(|entry| entry.trace() == trace)
            .collect()
    }

    /// Rebuild the source-event -> decision -> execution -> outcome chain.
    ///
    /// Refuses a chain with a dangling parent rather than returning the links
    /// it happens to hold. A chain reconstructed from an incomplete record is
    /// worse than no chain: it reads like the whole story.
    pub fn reconstruct(&self, trace: &TraceId) -> Result<TraceChain> {
        let entries = self.trace(trace);
        if entries.is_empty() {
            return Err(Error::not_found(format!(
                "nothing was captured under trace {}",
                trace.as_str()
            )));
        }
        let source_event = entries
            .iter()
            .find_map(|entry| entry.decision.source_event.clone());
        let mut seen: Vec<String> = Vec::new();
        if let Some(event) = &source_event {
            seen.push(event.as_str().to_string());
        }
        let mut links = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let decision = &entry.decision;
            if index > 0 {
                let parent = decision.caused_by.as_ref().ok_or_else(|| {
                    Error::invalid(format!(
                        "the chain from trace {} breaks at {}: it names no parent",
                        trace.as_str(),
                        decision.decision_id
                    ))
                })?;
                if !seen.iter().any(|id| id == parent.as_str()) {
                    return Err(Error::invalid(format!(
                        "the chain from trace {} breaks at {}: its parent {} was never captured",
                        trace.as_str(),
                        decision.decision_id,
                        parent.as_str()
                    )));
                }
            }
            seen.push(decision.decision_id.as_str().to_string());
            links.push(TraceLink {
                decision_id: decision.decision_id.clone(),
                kind: decision.kind().to_string(),
                at: decision.at,
                caused_by: decision.caused_by.clone(),
            });
        }
        Ok(TraceChain {
            trace: trace.clone(),
            source_event,
            links,
            realised_pnl: entries
                .iter()
                .map(|entry| entry.outcome.realised_pnl())
                .fold(Decimal::ZERO, |a, b| a + b),
            forgone: entries
                .iter()
                .filter_map(|entry| entry.decision.action.forgone())
                .sum(),
        })
    }
}

/// The chain link. Hashes the serialized records so every field is covered —
/// hashing a summary would let a price change without the digest noticing.
fn chain_digest(
    previous: &str,
    sequence: u64,
    decision: &Decision,
    outcome: &RealisedOutcome,
) -> String {
    let decision_body =
        serde_json::to_string(decision).unwrap_or_else(|_| decision.kind().to_string());
    let outcome_body = serde_json::to_string(outcome).unwrap_or_default();
    sha256_hex(format!("{previous}|{sequence}|{decision_body}|{outcome_body}").as_bytes())
}
