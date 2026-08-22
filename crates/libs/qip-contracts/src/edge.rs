//! Net edge and the leg plan that realises it.
//!
//! An opportunity is not the difference between two prices. It is that
//! difference minus everything that stands between seeing it and banking it,
//! and the deductions are usually larger than the gross figure that motivated
//! the trade. Reporting a net number without its parts is how a strategy runs
//! for a month before anyone notices the fees.

use crate::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};

/// What is being taken off the gross edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DeductionKind {
    /// Crossing the bid-ask on every leg.
    Spread,
    /// Exchange, clearing and regulatory fees, net of rebates.
    Fees,
    /// Expected adverse price movement during the round trip.
    Latency,
    /// Expected market impact of the size being traded.
    Slippage,
    /// Cost of carry: borrow, funding rate, or gas.
    Funding,
    /// Margin or collateral that the position ties up and could earn elsewhere.
    Collateral,
    /// A haircut for the estimate itself being uncertain.
    ///
    /// Not a cost the market charges — a discount the platform charges itself
    /// for acting on a number it is not sure of.
    Uncertainty,
}

impl DeductionKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Spread => "spread",
            Self::Fees => "fees",
            Self::Latency => "latency",
            Self::Slippage => "slippage",
            Self::Funding => "funding",
            Self::Collateral => "collateral",
            Self::Uncertainty => "uncertainty",
        }
    }

    /// Every kind, so a caller can assert it has considered all of them.
    pub const fn all() -> [Self; 7] {
        [
            Self::Spread,
            Self::Fees,
            Self::Latency,
            Self::Slippage,
            Self::Funding,
            Self::Collateral,
            Self::Uncertainty,
        ]
    }
}

/// One deduction, with the reasoning that produced it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deduction {
    pub kind: DeductionKind,
    /// Always non-negative. A deduction that adds to edge is a rebate and
    /// belongs in `Fees` as a smaller number, not as a negative deduction.
    pub amount: Decimal,
    /// How the amount was arrived at, for the attribution that follows a fill.
    pub basis: String,
}

impl Deduction {
    pub fn new(kind: DeductionKind, amount: Decimal, basis: impl Into<String>) -> Result<Self> {
        if amount < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "a {} deduction cannot be negative; a rebate is a smaller fee",
                kind.as_str()
            )));
        }
        Ok(Self {
            kind,
            amount,
            basis: basis.into(),
        })
    }
}

/// Gross edge, its deductions, and the net figure they imply.
///
/// The net figure is computed, never supplied. There is no constructor that
/// accepts one, so a caller cannot report a net edge that its own deductions
/// contradict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetEdge {
    gross: Decimal,
    deductions: Vec<Deduction>,
    /// The size the figures are quoted for. Edge does not scale linearly, so a
    /// net edge without a size is not a number anyone can act on.
    quantity: Decimal,
}

impl NetEdge {
    /// Start from a gross edge for a stated quantity.
    pub fn gross(gross: Decimal, quantity: Decimal) -> Result<Self> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid("net edge requires a positive quantity"));
        }
        Ok(Self {
            gross,
            deductions: Vec::new(),
            quantity,
        })
    }

    pub fn deduct(mut self, deduction: Deduction) -> Self {
        self.deductions.push(deduction);
        self
    }

    pub fn gross_edge(&self) -> Decimal {
        self.gross
    }

    pub fn quantity(&self) -> Decimal {
        self.quantity
    }

    pub fn deductions(&self) -> &[Deduction] {
        &self.deductions
    }

    pub fn total_deducted(&self) -> Decimal {
        self.deductions
            .iter()
            .fold(Decimal::ZERO, |sum, d| sum + d.amount)
    }

    /// What is left. May be negative, which is the useful answer.
    pub fn net(&self) -> Decimal {
        self.gross - self.total_deducted()
    }

    /// Whether the trade is worth doing at all.
    pub fn is_positive(&self) -> bool {
        self.net() > Decimal::ZERO
    }

    /// Deduction kinds that were never considered.
    ///
    /// An empty result does not mean the numbers are right; a non-empty one
    /// means they are definitely incomplete. Cheap to check and the most
    /// common way a net edge flatters itself.
    pub fn unconsidered(&self) -> Vec<DeductionKind> {
        DeductionKind::all()
            .into_iter()
            .filter(|kind| !self.deductions.iter().any(|d| d.kind == *kind))
            .collect()
    }

    /// Refuse the edge unless every deduction kind has been considered.
    pub fn require_complete(&self) -> Result<()> {
        let missing = self.unconsidered();
        if missing.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = missing.iter().map(DeductionKind::as_str).collect();
        Err(Error::invalid(format!(
            "net edge did not consider: {}",
            names.join(", ")
        )))
    }

    pub fn summarise(&self) -> String {
        let parts: Vec<String> = self
            .deductions
            .iter()
            .map(|d| format!("{} {}", d.kind.as_str(), d.amount))
            .collect();
        format!(
            "gross {} less [{}] = net {} on {}",
            self.gross,
            parts.join(", "),
            self.net(),
            self.quantity
        )
    }
}

/// One leg of a planned trade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegStep {
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub side: crate::message::BookSide,
    pub quantity: Decimal,
    /// The price the plan assumes. The difference between this and the fill is
    /// what the latency and slippage deductions were estimating.
    pub reference_price: Decimal,
    /// The instrument `reference_price` is denominated in: one unit of
    /// `object_id` costs `reference_price` of this.
    ///
    /// Carried so residual exposure can be stated per unit of account.
    /// Without it a mixed-currency cycle's residual is a sum across
    /// incomparable units, which was found the hard way and is exactly the
    /// number a leg-risk budget must not be compared against.
    pub priced_in: ObjectId,
    /// Position in the sequence. Legs with the same order may go together.
    pub order: u16,
    /// Whether the plan can proceed if this leg fails.
    ///
    /// A leg that is not optional and does not fill leaves the position
    /// half-on, which is the risk the whole plan exists to manage.
    pub optional: bool,
}

/// An ordered set of legs that together realise an opportunity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegPlan {
    steps: Vec<LegStep>,
    /// Inventory that must be held before starting, because the plan cannot
    /// rely on acquiring it mid-flight.
    pub prefunded: Vec<(ObjectId, Decimal)>,
}

impl LegPlan {
    pub fn new(mut steps: Vec<LegStep>) -> Result<Self> {
        if steps.is_empty() {
            return Err(Error::invalid("a leg plan with no legs is not a plan"));
        }
        steps.sort_by_key(|step| step.order);
        Ok(Self {
            steps,
            prefunded: Vec::new(),
        })
    }

    pub fn with_prefunded(mut self, object_id: ObjectId, quantity: Decimal) -> Self {
        self.prefunded.push((object_id, quantity));
        self
    }

    pub fn steps(&self) -> &[LegStep] {
        &self.steps
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Legs that cannot be abandoned once the plan has started.
    pub fn mandatory(&self) -> Vec<&LegStep> {
        self.steps.iter().filter(|s| !s.optional).collect()
    }

    /// The exposure left behind if the plan stops after `completed` legs.
    ///
    /// What the leg-risk budget is sized against, and deliberately a
    /// conservative **gross** bound: a cycle strands the value of the path
    /// once, not once per remaining leg, so this errs toward refusing a plan.
    /// That is the direction to err in.
    ///
    /// It has a limitation worth stating rather than discovering. A [`LegStep`]
    /// carries no quote currency, so where a cycle prices its legs in more than
    /// one, this sum spans them and the total is unitless. It remains a usable
    /// bound for a single-currency plan and still errs toward refusal for a
    /// mixed one — but a caller whose legs can have different quotes must break
    /// the exposure out per currency itself and treat this figure as a screen
    /// rather than a measurement.
    pub fn residual_after(&self, completed: usize) -> Decimal {
        self.steps
            .iter()
            .skip(completed)
            .filter(|s| !s.optional)
            .fold(Decimal::ZERO, |sum, s| sum + s.quantity * s.reference_price)
    }

    /// The residual exposure after `completed` legs, per unit of account.
    ///
    /// This is the measurement [`LegPlan::residual_after`] screens for: each
    /// entry is a sum of like-denominated notionals, so a budget stated in one
    /// unit can be compared against the entry in that unit instead of against
    /// a total that quietly spans several.
    pub fn residual_by_unit(&self, completed: usize) -> Vec<(ObjectId, Decimal)> {
        let mut totals: std::collections::BTreeMap<ObjectId, Decimal> =
            std::collections::BTreeMap::new();
        for step in self.steps.iter().skip(completed).filter(|s| !s.optional) {
            let entry = totals
                .entry(step.priced_in.clone())
                .or_insert(Decimal::ZERO);
            *entry = *entry + step.quantity * step.reference_price;
        }
        totals.into_iter().collect()
    }

    /// Venues the plan touches, in first-use order.
    pub fn venues(&self) -> Vec<&VenueId> {
        let mut seen: Vec<&VenueId> = Vec::new();
        for step in &self.steps {
            if !seen.contains(&&step.venue) {
                seen.push(&step.venue);
            }
        }
        seen
    }
}
