//! Event markets: binary, categorical and scalar.
//!
//! A prediction market is a set of claims on the same proposition that between
//! them cover every outcome exactly once. That property is what makes the
//! prices probabilities and what makes a complete set worth the payoff, so it
//! is enforced at construction rather than assumed at pricing: a categorical
//! market whose outcomes overlap is not a market with a modelling error in it,
//! it is a market where buying every outcome does not guarantee the payoff.

use qip_contracts::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::resolution::{Observations, Proposition, ResolutionCriteria, Verdict};

/// One outcome's identity within its market.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OutcomeId(String);

impl OutcomeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OutcomeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One claim on the proposition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: OutcomeId,
    /// Display label. Like the proposition's statement, it decides nothing.
    pub label: String,
    /// The tradable instrument this outcome is, so a position in it is a
    /// position like any other.
    pub object_id: ObjectId,
    /// What has to be observed for this outcome to win.
    pub criteria: ResolutionCriteria,
}

impl Outcome {
    pub fn new(
        id: OutcomeId,
        label: impl Into<String>,
        object_id: ObjectId,
        criteria: ResolutionCriteria,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            object_id,
            criteria,
        }
    }

    /// Content hash of this outcome's criteria, for matching the same outcome
    /// across venues.
    pub fn digest(&self) -> String {
        self.criteria.digest()
    }
}

/// One bucket of a scalar market.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScalarBucket {
    pub outcome: Outcome,
    /// Inclusive lower bound; `None` is unbounded below.
    pub lower: Option<Decimal>,
    /// Exclusive upper bound; `None` is unbounded above.
    pub upper: Option<Decimal>,
}

impl ScalarBucket {
    pub fn contains(&self, value: Decimal) -> bool {
        self.lower.is_none_or(|bound| value >= bound)
            && self.upper.is_none_or(|bound| value < bound)
    }
}

/// What shape the market is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MarketKind {
    /// Two outcomes, the second the complement of the first.
    ///
    /// Boxed so the binary case does not make every categorical market carry
    /// its footprint.
    Binary { yes: Box<Outcome>, no: Box<Outcome> },
    /// Mutually exclusive, jointly exhaustive outcomes.
    Categorical { outcomes: Vec<Outcome> },
    /// A continuous quantity, bucketed into ranges that partition it.
    Scalar {
        metric: String,
        buckets: Vec<ScalarBucket>,
    },
}

impl MarketKind {
    /// A yes/no market, with the no outcome derived as the exact complement.
    ///
    /// Derived rather than supplied: a hand-written "no" that is not the
    /// negation of the "yes" leaves a gap where neither side pays.
    pub fn binary(yes: Outcome, no_id: OutcomeId, no_object: ObjectId) -> Self {
        let no = Outcome::new(
            no_id,
            format!("not {}", yes.label),
            no_object,
            ResolutionCriteria::Not(Box::new(yes.criteria.clone())),
        );
        Self::Binary {
            yes: Box::new(yes),
            no: Box::new(no),
        }
    }

    /// Refuses fewer than two outcomes, a repeated identifier, and two
    /// outcomes resolving on identical criteria — the last because outcomes
    /// that can both win are not a market anyone can price.
    pub fn categorical(outcomes: Vec<Outcome>) -> Result<Self> {
        if outcomes.len() < 2 {
            return Err(Error::invalid(
                "a categorical market needs at least two outcomes",
            ));
        }
        for (position, outcome) in outcomes.iter().enumerate() {
            if outcomes
                .iter()
                .skip(position + 1)
                .any(|other| other.id == outcome.id)
            {
                return Err(Error::invalid(format!(
                    "outcome {} appears twice",
                    outcome.id
                )));
            }
            if outcomes
                .iter()
                .skip(position + 1)
                .any(|other| other.digest() == outcome.digest())
            {
                return Err(Error::invalid(format!(
                    "outcomes {} and another resolve on identical criteria, so they are not mutually exclusive",
                    outcome.id
                )));
            }
        }
        Ok(Self::Categorical { outcomes })
    }

    /// Bucket a scalar range at the given interior edges.
    ///
    /// The buckets are built here rather than supplied so they partition by
    /// construction: each is `[edge, next_edge)`, the first is open below and
    /// the last open above, and no value can fall in two or in none.
    pub fn scalar(
        metric: impl Into<String>,
        edges: Vec<Decimal>,
        object_for: impl Fn(usize) -> ObjectId,
    ) -> Result<Self> {
        let metric = metric.into();
        if edges.is_empty() {
            return Err(Error::invalid(
                "a scalar market needs at least one edge to have more than one bucket",
            ));
        }
        for pair in edges.windows(2) {
            if pair[1] <= pair[0] {
                return Err(Error::invalid(format!(
                    "scalar edges must strictly increase; {} does not follow {}",
                    pair[1], pair[0]
                )));
            }
        }
        let mut buckets = Vec::with_capacity(edges.len() + 1);
        for index in 0..=edges.len() {
            let lower = if index == 0 {
                None
            } else {
                Some(edges[index - 1])
            };
            let upper = edges.get(index).copied();
            let label = match (lower, upper) {
                (None, Some(bound)) => format!("below {bound}"),
                (Some(bound), None) => format!("{bound} and above"),
                (Some(low), Some(high)) => format!("{low} to {high}"),
                (None, None) => "any".to_string(),
            };
            buckets.push(ScalarBucket {
                outcome: Outcome::new(
                    OutcomeId::new(format!("bucket-{index}")),
                    label,
                    object_for(index),
                    ResolutionCriteria::Within {
                        metric: metric.clone(),
                        lower,
                        upper,
                    },
                ),
                lower,
                upper,
            });
        }
        Ok(Self::Scalar { metric, buckets })
    }

    pub fn outcomes(&self) -> Vec<&Outcome> {
        match self {
            Self::Binary { yes, no } => vec![yes.as_ref(), no.as_ref()],
            Self::Categorical { outcomes } => outcomes.iter().collect(),
            Self::Scalar { buckets, .. } => buckets.iter().map(|bucket| &bucket.outcome).collect(),
        }
    }

    /// The bucket a value falls in. Exactly one always does.
    pub fn bucket_for(&self, value: Decimal) -> Result<&ScalarBucket> {
        let Self::Scalar { buckets, .. } = self else {
            return Err(Error::invalid("only a scalar market has buckets"));
        };
        let mut found = buckets.iter().filter(|bucket| bucket.contains(value));
        let first = found
            .next()
            .ok_or_else(|| Error::invalid(format!("no bucket covers {value}")))?;
        if found.next().is_some() {
            return Err(Error::invalid(format!("{value} falls in two buckets")));
        }
        Ok(first)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Binary { .. } => "binary",
            Self::Categorical { .. } => "categorical",
            Self::Scalar { .. } => "scalar",
        }
    }
}

/// What the venue charges.
///
/// Settlement fees are separated from trading fees because they apply to the
/// payoff rather than to the price, and folding them together is the most
/// common way an implied probability comes out wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeSchedule {
    pub taker_bps: u32,
    pub maker_bps: u32,
    /// Charged on the payoff of a winning contract.
    pub settlement_bps: u32,
}

impl FeeSchedule {
    /// Refuses a fee of a whole turn or more on any leg.
    pub fn new(taker_bps: u32, maker_bps: u32, settlement_bps: u32) -> Result<Self> {
        for (label, bps) in [
            ("taker", taker_bps),
            ("maker", maker_bps),
            ("settlement", settlement_bps),
        ] {
            if bps >= 10_000 {
                return Err(Error::invalid(format!(
                    "a {label} fee of {bps}bp is a hundred percent or more"
                )));
            }
        }
        Ok(Self {
            taker_bps,
            maker_bps,
            settlement_bps,
        })
    }

    /// A venue that charges nothing, for isolating fee effects in tests.
    pub const FREE: Self = Self {
        taker_bps: 0,
        maker_bps: 0,
        settlement_bps: 0,
    };

    /// What crossing costs on top of the price.
    pub fn taker_cost(&self, notional: Decimal) -> Decimal {
        scale_bps(notional, self.taker_bps)
    }

    /// What the venue keeps from a winning payoff.
    pub fn settlement_cost(&self, payoff: Decimal) -> Decimal {
        scale_bps(payoff, self.settlement_bps)
    }

    /// One unit of notional including the taker fee.
    pub fn gross_up(&self, price: Decimal) -> Decimal {
        price + self.taker_cost(price)
    }

    /// One unit of proceeds after the taker fee.
    pub fn net_down(&self, price: Decimal) -> Decimal {
        price - self.taker_cost(price)
    }

    /// What a winning contract actually pays.
    pub fn net_payoff(&self, payoff: Decimal) -> Decimal {
        payoff - self.settlement_cost(payoff)
    }
}

fn scale_bps(amount: Decimal, bps: u32) -> Decimal {
    amount
        .checked_mul(Decimal::from_raw(i128::from(bps) * 100_000))
        .unwrap_or(Decimal::ZERO)
}

/// A market on a proposition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventMarket {
    pub market_id: ObjectId,
    pub venue: VenueId,
    class: VenueClass,
    pub proposition: Proposition,
    pub kind: MarketKind,
    pub fees: FeeSchedule,
}

impl EventMarket {
    /// The unit this market's contracts settle in.
    ///
    /// The venue does not publish a tradeable settlement instrument, so this
    /// names the venue's own collateral pool deterministically. That is not a
    /// dodge: collateral posted at two venues is genuinely not fungible while
    /// a position is open, so two markets on different venues *should* report
    /// residual exposure in two units. When reference data supplies the real
    /// settlement asset, it replaces this label — the shape stays the same.
    pub fn settlement_unit(&self) -> ObjectId {
        ObjectId::from_string(format!("collateral-{}", self.venue.as_str()))
    }

    /// Refuses a venue that is not a prediction market: the pricing here
    /// assumes contracts that settle to a fixed payoff on a stated date, and
    /// nothing else behaves that way.
    pub fn new(
        market_id: ObjectId,
        venue: VenueId,
        class: VenueClass,
        proposition: Proposition,
        kind: MarketKind,
        fees: FeeSchedule,
    ) -> Result<Self> {
        if class != VenueClass::PredictionMarket {
            return Err(Error::invalid(format!(
                "venue {venue} is a {}, not a prediction market",
                class.as_str()
            )));
        }
        let market = Self {
            market_id,
            venue,
            class,
            proposition,
            kind,
            fees,
        };
        market.validate()?;
        Ok(market)
    }

    /// Every outcome's criteria must be resolvable from the market's source,
    /// or the market has an outcome that can never be paid.
    fn validate(&self) -> Result<()> {
        for outcome in self.kind.outcomes() {
            let unavailable: Vec<String> = outcome
                .criteria
                .metrics()
                .into_iter()
                .filter(|metric| !self.proposition.source.publishes_metric(metric))
                .collect();
            if !unavailable.is_empty() {
                return Err(Error::invalid(format!(
                    "outcome {} needs {} which source {} does not publish",
                    outcome.id,
                    unavailable.join(", "),
                    self.proposition.source.name
                )));
            }
        }
        Ok(())
    }

    pub const fn class(&self) -> VenueClass {
        self.class
    }

    pub const fn payoff(&self) -> Decimal {
        self.proposition.settlement.payoff
    }

    pub fn outcomes(&self) -> Vec<&Outcome> {
        self.kind.outcomes()
    }

    pub fn outcome(&self, id: &OutcomeId) -> Result<&Outcome> {
        self.kind
            .outcomes()
            .into_iter()
            .find(|outcome| &outcome.id == id)
            .ok_or_else(|| {
                Error::not_found(format!("market {} has no outcome {id}", self.market_id))
            })
    }

    /// Evaluate every outcome against what the source published.
    pub fn evaluate(&self, observations: &Observations) -> MarketVerdict {
        let mut winners = Vec::new();
        let mut missing = Vec::new();
        for outcome in self.kind.outcomes() {
            match outcome.criteria.evaluate(observations) {
                Verdict::Holds => winners.push(outcome.id.clone()),
                Verdict::Undetermined { missing: absent } => missing.extend(absent),
                Verdict::Fails => {}
            }
        }
        match winners.len() {
            1 => MarketVerdict::Resolved(winners.remove(0)),
            0 if missing.is_empty() => MarketVerdict::NoOutcome,
            0 => {
                missing.sort();
                missing.dedup();
                MarketVerdict::Undetermined { missing }
            }
            // More than one outcome holding means the market's own criteria
            // overlap. Settling it would pay two claims from one payoff.
            _ => MarketVerdict::Ambiguous(winners),
        }
    }
}

/// What the observations say the market resolves to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketVerdict {
    Resolved(OutcomeId),
    /// Every outcome failed. The market's outcomes were not exhaustive.
    NoOutcome,
    /// More than one outcome holds. The outcomes were not exclusive.
    Ambiguous(Vec<OutcomeId>),
    Undetermined {
        missing: Vec<String>,
    },
}

impl MarketVerdict {
    /// Whether this verdict may be settled on at all.
    pub const fn is_settleable(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }
}
