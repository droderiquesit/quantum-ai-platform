//! Resolution criteria a program can evaluate.
//!
//! Free text is where prediction markets go wrong. "Will the Fed cut rates by
//! March?" reads unambiguously until March arrives and two venues disagree
//! about intermeeting moves, about the announcement date versus the effective
//! date, and about what "cut" means for a range. Every dispute of that kind is
//! a proposition that was never actually specified, and no amount of care in
//! the trading logic recovers from it.
//!
//! So a criterion here is a structure: named metrics, comparisons, thresholds
//! and combinators, evaluated against observations published by a named
//! source. Two markets are the same proposition when their criteria have the
//! same digest, and not when their titles look alike — which is what makes
//! cross-venue arbitrage a structural question rather than a judgement call.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How an observed value is compared with a threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comparison {
    AtLeast,
    GreaterThan,
    AtMost,
    LessThan,
    EqualTo,
}

impl Comparison {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AtLeast => ">=",
            Self::GreaterThan => ">",
            Self::AtMost => "<=",
            Self::LessThan => "<",
            Self::EqualTo => "==",
        }
    }

    pub fn holds(&self, observed: Decimal, threshold: Decimal) -> bool {
        match self {
            Self::AtLeast => observed >= threshold,
            Self::GreaterThan => observed > threshold,
            Self::AtMost => observed <= threshold,
            Self::LessThan => observed < threshold,
            Self::EqualTo => observed == threshold,
        }
    }
}

/// One value the resolution source published.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Observation {
    Numeric(Decimal),
    Category(String),
    Flag(bool),
}

/// What the source published, and when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Observations {
    values: BTreeMap<String, Observation>,
    observed_at: Option<Timestamp>,
}

impl Observations {
    pub fn at(observed_at: Timestamp) -> Self {
        Self {
            values: BTreeMap::new(),
            observed_at: Some(observed_at),
        }
    }

    pub fn with(mut self, metric: impl Into<String>, observation: Observation) -> Self {
        self.values.insert(metric.into(), observation);
        self
    }

    pub fn get(&self, metric: &str) -> Option<&Observation> {
        self.values.get(metric)
    }

    pub const fn observed_at(&self) -> Option<Timestamp> {
        self.observed_at
    }
}

/// What a criterion says about the world, given what was observed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Holds,
    Fails,
    /// The source has not published what the criterion needs. Not the same as
    /// failing, and settling it as failure is how a market resolves against a
    /// position that was right.
    Undetermined {
        missing: Vec<String>,
    },
}

impl Verdict {
    pub const fn is_determined(&self) -> bool {
        !matches!(self, Self::Undetermined { .. })
    }

    pub const fn holds(&self) -> bool {
        matches!(self, Self::Holds)
    }
}

/// A machine-evaluable statement of what has to happen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResolutionCriteria {
    /// A published number stands in a stated relation to a threshold.
    Threshold {
        metric: String,
        comparison: Comparison,
        value: Decimal,
    },
    /// A published number falls in `[lower, upper)`. An open end is unbounded.
    ///
    /// Half-open by construction, so adjacent ranges cannot both claim a
    /// boundary value and cannot leave one uncovered.
    Within {
        metric: String,
        lower: Option<Decimal>,
        upper: Option<Decimal>,
    },
    /// A published category equals a stated value.
    Category {
        metric: String,
        equals: String,
    },
    /// A published flag has a stated value.
    Flag {
        metric: String,
        expected: bool,
    },
    All(Vec<ResolutionCriteria>),
    Any(Vec<ResolutionCriteria>),
    Not(Box<ResolutionCriteria>),
}

impl ResolutionCriteria {
    /// Evaluate against what the source published.
    pub fn evaluate(&self, observations: &Observations) -> Verdict {
        match self {
            Self::Threshold {
                metric,
                comparison,
                value,
            } => match observations.get(metric) {
                Some(Observation::Numeric(observed)) => {
                    if comparison.holds(*observed, *value) {
                        Verdict::Holds
                    } else {
                        Verdict::Fails
                    }
                }
                _ => Verdict::Undetermined {
                    missing: vec![metric.clone()],
                },
            },
            Self::Within {
                metric,
                lower,
                upper,
            } => match observations.get(metric) {
                Some(Observation::Numeric(observed)) => {
                    let above = lower.is_none_or(|bound| *observed >= bound);
                    let below = upper.is_none_or(|bound| *observed < bound);
                    if above && below {
                        Verdict::Holds
                    } else {
                        Verdict::Fails
                    }
                }
                _ => Verdict::Undetermined {
                    missing: vec![metric.clone()],
                },
            },
            Self::Category { metric, equals } => match observations.get(metric) {
                Some(Observation::Category(observed)) => {
                    if observed == equals {
                        Verdict::Holds
                    } else {
                        Verdict::Fails
                    }
                }
                _ => Verdict::Undetermined {
                    missing: vec![metric.clone()],
                },
            },
            Self::Flag { metric, expected } => match observations.get(metric) {
                Some(Observation::Flag(observed)) => {
                    if observed == expected {
                        Verdict::Holds
                    } else {
                        Verdict::Fails
                    }
                }
                _ => Verdict::Undetermined {
                    missing: vec![metric.clone()],
                },
            },
            // A conjunction fails as soon as one part fails, even if another
            // is unobserved: the answer is knowable and waiting for the rest
            // would be a delay with no information in it.
            Self::All(parts) => {
                let mut missing = Vec::new();
                for part in parts {
                    match part.evaluate(observations) {
                        Verdict::Fails => return Verdict::Fails,
                        Verdict::Undetermined { missing: absent } => missing.extend(absent),
                        Verdict::Holds => {}
                    }
                }
                if missing.is_empty() {
                    Verdict::Holds
                } else {
                    Verdict::Undetermined { missing }
                }
            }
            Self::Any(parts) => {
                let mut missing = Vec::new();
                for part in parts {
                    match part.evaluate(observations) {
                        Verdict::Holds => return Verdict::Holds,
                        Verdict::Undetermined { missing: absent } => missing.extend(absent),
                        Verdict::Fails => {}
                    }
                }
                if missing.is_empty() {
                    Verdict::Fails
                } else {
                    Verdict::Undetermined { missing }
                }
            }
            Self::Not(inner) => match inner.evaluate(observations) {
                Verdict::Holds => Verdict::Fails,
                Verdict::Fails => Verdict::Holds,
                undetermined => undetermined,
            },
        }
    }

    /// Every metric the source has to publish for this to be evaluable.
    pub fn metrics(&self) -> Vec<String> {
        let mut found = Vec::new();
        self.collect_metrics(&mut found);
        found.sort();
        found.dedup();
        found
    }

    fn collect_metrics(&self, into: &mut Vec<String>) {
        match self {
            Self::Threshold { metric, .. }
            | Self::Within { metric, .. }
            | Self::Category { metric, .. }
            | Self::Flag { metric, .. } => into.push(metric.clone()),
            Self::All(parts) | Self::Any(parts) => {
                for part in parts {
                    part.collect_metrics(into);
                }
            }
            Self::Not(inner) => inner.collect_metrics(into),
        }
    }

    /// A canonical rendering: the same criterion always produces the same
    /// string, and two criteria that differ anywhere produce different ones.
    ///
    /// Conjunctions and disjunctions are sorted, because `A and B` and `B and
    /// A` are the same proposition and must not look like different ones.
    pub fn canonical(&self) -> String {
        match self {
            Self::Threshold {
                metric,
                comparison,
                value,
            } => format!("threshold({metric},{},{value})", comparison.as_str()),
            Self::Within {
                metric,
                lower,
                upper,
            } => format!(
                "within({metric},{},{})",
                lower.map_or("-inf".to_string(), |bound| bound.to_string()),
                upper.map_or("+inf".to_string(), |bound| bound.to_string())
            ),
            Self::Category { metric, equals } => format!("category({metric},{equals})"),
            Self::Flag { metric, expected } => format!("flag({metric},{expected})"),
            Self::All(parts) => format!("all[{}]", canonical_parts(parts)),
            Self::Any(parts) => format!("any[{}]", canonical_parts(parts)),
            Self::Not(inner) => format!("not({})", inner.canonical()),
        }
    }

    /// Content hash of the canonical rendering. The identity of a proposition.
    pub fn digest(&self) -> String {
        qip_core::sha256_hex(self.canonical().as_bytes())
    }
}

fn canonical_parts(parts: &[ResolutionCriteria]) -> String {
    let mut rendered: Vec<String> = parts.iter().map(ResolutionCriteria::canonical).collect();
    rendered.sort();
    rendered.join(",")
}

/// What kind of authority resolves the market.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// The body that produces the number in the first place.
    Official,
    /// A committee that reads the official number and votes.
    Committee,
    /// A token-holder vote, or any other economically-secured attestation.
    Optimistic,
    /// A data provider aggregating others.
    Aggregator,
}

impl SourceKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Committee => "committee",
            Self::Optimistic => "optimistic",
            Self::Aggregator => "aggregator",
        }
    }
}

/// Who publishes the observations, and which ones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionSource {
    pub name: String,
    pub kind: SourceKind,
    /// Metrics this source publishes. A criterion referencing anything else
    /// cannot be resolved from this source, and the market is unsettleable
    /// before it is ever traded.
    pub publishes: Vec<String>,
}

impl ResolutionSource {
    pub fn new(name: impl Into<String>, kind: SourceKind, publishes: Vec<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            publishes,
        }
    }

    pub fn publishes_metric(&self, metric: &str) -> bool {
        self.publishes.iter().any(|known| known == metric)
    }
}

/// What happens when the criteria cannot be evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndeterminedRule {
    /// Everyone gets their stake back.
    VoidAndRefund,
    /// Wait for the next observation window.
    RollForward,
    /// Anything not proven true resolves false. The harshest rule, and the one
    /// most often left implicit.
    ResolveAsNo,
}

impl UndeterminedRule {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::VoidAndRefund => "void_and_refund",
            Self::RollForward => "roll_forward",
            Self::ResolveAsNo => "resolve_as_no",
        }
    }
}

/// What a winning contract pays and what happens if nothing wins.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettlementRule {
    /// Paid per winning contract.
    pub payoff: Decimal,
    pub on_undetermined: UndeterminedRule,
}

impl SettlementRule {
    /// Refuses a non-positive payoff: a contract that pays nothing when it
    /// wins is not a contract.
    pub fn new(payoff: Decimal, on_undetermined: UndeterminedRule) -> Result<Self> {
        if !payoff.is_positive() {
            return Err(Error::invalid("a settlement payoff must be positive"));
        }
        Ok(Self {
            payoff,
            on_undetermined,
        })
    }

    /// The conventional unit contract paying one unit of quote currency.
    pub fn unit(on_undetermined: UndeterminedRule) -> Self {
        Self {
            payoff: Decimal::ONE,
            on_undetermined,
        }
    }
}

/// The proposition a market is written on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposition {
    /// Human-readable, for display only. Nothing decides anything from this.
    pub statement: String,
    pub criteria: ResolutionCriteria,
    pub source: ResolutionSource,
    /// When the criteria are to be evaluated.
    pub resolves_at: Timestamp,
    pub settlement: SettlementRule,
    /// How long a proposed resolution may be challenged.
    pub dispute_window: qip_core::Duration,
}

impl Proposition {
    /// Refuses a proposition whose source does not publish what its criteria
    /// need, which is a market that cannot be settled at any price.
    pub fn new(
        statement: impl Into<String>,
        criteria: ResolutionCriteria,
        source: ResolutionSource,
        resolves_at: Timestamp,
        settlement: SettlementRule,
        dispute_window: qip_core::Duration,
    ) -> Result<Self> {
        let statement = statement.into();
        let unavailable: Vec<String> = criteria
            .metrics()
            .into_iter()
            .filter(|metric| !source.publishes_metric(metric))
            .collect();
        if !unavailable.is_empty() {
            return Err(Error::invalid(format!(
                "source {} does not publish {}, so \"{statement}\" cannot be resolved from it",
                source.name,
                unavailable.join(", ")
            )));
        }
        Ok(Self {
            statement,
            criteria,
            source,
            resolves_at,
            settlement,
            dispute_window,
        })
    }

    /// Identity of the question, ignoring who answers it.
    pub fn criteria_digest(&self) -> String {
        self.criteria.digest()
    }

    /// Everything that differs between two propositions.
    pub fn differences(&self, other: &Self) -> Vec<PropositionDifference> {
        let mut differences = Vec::new();
        if self.criteria_digest() != other.criteria_digest() {
            differences.push(PropositionDifference::Criteria);
        }
        if self.resolves_at != other.resolves_at {
            differences.push(PropositionDifference::ResolutionTime);
        }
        if self.settlement.payoff != other.settlement.payoff {
            differences.push(PropositionDifference::Payoff);
        }
        if self.settlement.on_undetermined != other.settlement.on_undetermined {
            differences.push(PropositionDifference::UndeterminedRule);
        }
        if self.source != other.source {
            differences.push(PropositionDifference::Source);
        }
        differences
    }
}

/// One way two propositions fail to be the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropositionDifference {
    /// Different questions. Nothing else matters once this is true.
    Criteria,
    ResolutionTime,
    Payoff,
    UndeterminedRule,
    /// The same question answered by different authorities. Tradable, but the
    /// two can disagree, and the position is short that disagreement.
    Source,
}

impl PropositionDifference {
    /// Whether the difference makes the two contracts different instruments
    /// rather than the same instrument carrying an extra risk.
    pub const fn is_structural(&self) -> bool {
        !matches!(self, Self::Source)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Criteria => "resolution criteria",
            Self::ResolutionTime => "resolution time",
            Self::Payoff => "payoff",
            Self::UndeterminedRule => "undetermined rule",
            Self::Source => "resolution source",
        }
    }
}
