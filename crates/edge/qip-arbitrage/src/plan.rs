//! Turning a confirmed path into a plan someone could actually execute.
//!
//! A priced path says what the trade is worth. It says nothing about the order
//! the legs go in, which is the part that decides what happens when the plan
//! does not finish — and plans do not finish.
//!
//! Three decisions are made here, and each is about failure rather than profit:
//!
//! * **Order.** The least reversible leg goes first. The instinct is the
//!   opposite — take the easy leg, then the hard one — but that puts the leg
//!   you might not be able to undo at the moment you are most committed. Doing
//!   it first means the only thing at stake when it fails is the decision to
//!   try.
//! * **Prefunding.** A leg cannot rely on the previous leg's output if the
//!   previous leg is executed after it, or if the venue that produces it can
//!   revert. [`VenueClass::settles_atomically`] is the test: a decentralised
//!   venue can land one side of a trade and roll back the other, and a plan
//!   that assumed otherwise discovers it holding half a position.
//! * **Leg risk.** [`LegPlan::residual_after`] is what is left exposed if the
//!   plan stops partway. A plan whose residual exceeds its budget is refused.
//!   Not warned about, not attempted with a note in the log — refused, because
//!   the alternative is a position nobody sized.

use crate::arith::mul;
use crate::pricing::{PathLeg, PathPricing};
use qip_contracts::edge::{LegPlan, LegStep};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Smallest headroom factor a leg can be scored with.
///
/// A leg with nothing resting at the touch would otherwise score zero on
/// reversibility whatever venue it is on, and every such leg would tie — which
/// throws away the venue-class information that is the more reliable half of
/// the score.
const MIN_HEADROOM: f64 = 0.01;

/// How the planner is allowed to trade off convenience against exposure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanSettings {
    /// Most residual exposure tolerated once the plan is committed.
    ///
    /// Compared against [`LegPlan::residual_after`], which sums the notional of
    /// every mandatory leg still outstanding. That is a gross bound rather than
    /// a net one — a cycle strands the value of the path once, not once per
    /// remaining leg — so the check errs toward refusing, which is the
    /// direction to err in. Where a cycle quotes its legs in more than one
    /// currency the sum spans them, and [`PlannedTrade::residual_by_quote`]
    /// breaks it back out so the mixture is visible rather than implied.
    pub leg_risk_budget: Decimal,
    /// Share of a synthetic conversion's notional below which a component leg
    /// may be abandoned.
    ///
    /// Basket execution really does leave the dust behind: chasing a name worth
    /// a fraction of a percent of the trade costs more in spread and delay than
    /// the tracking error it removes. The threshold is explicit so that "we
    /// skipped a leg" is a decision with a number attached.
    pub optional_leg_fraction_f64: f64,
}

impl Default for PlanSettings {
    fn default() -> Self {
        Self {
            leg_risk_budget: Decimal::from_int(0),
            optional_leg_fraction_f64: 0.01,
        }
    }
}

impl PlanSettings {
    /// Settings with a stated leg-risk budget.
    pub fn with_budget(leg_risk_budget: Decimal) -> Self {
        Self {
            leg_risk_budget,
            ..Self::default()
        }
    }
}

/// Where one leg landed in the order, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegRanking {
    pub object: ObjectId,
    pub venue: VenueId,
    pub order: u16,
    /// How easily this leg could be undone. A statistic, and the sort key.
    pub reversibility_f64: f64,
    pub reason: String,
}

/// A plan, its leg risk, and the reasoning that produced both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedTrade {
    pub plan: LegPlan,
    /// Exposure stranded if the plan stops after its first leg.
    pub residual_risk: Decimal,
    /// The same exposure, split by the instrument each leg is priced in.
    ///
    /// A single-entry breakdown means `residual_risk` is a number with units.
    /// More than one entry means it is a sum across currencies, and the parts
    /// are where the meaning is.
    pub residual_by_quote: Vec<(ObjectId, Decimal)>,
    pub ordering: Vec<LegRanking>,
    /// Why the plan looks the way it does, in the order the decisions were
    /// made. A plan whose ordering has to be reverse-engineered from the leg
    /// list is a plan nobody will check.
    pub rationale: Vec<String>,
}

/// Builds executable plans from priced paths.
#[derive(Clone, Debug, PartialEq)]
pub struct LegPlanner {
    settings: PlanSettings,
}

impl LegPlanner {
    pub fn new(settings: PlanSettings) -> Self {
        Self { settings }
    }

    pub fn settings(&self) -> &PlanSettings {
        &self.settings
    }

    /// Order the legs, decide what must be prefunded, and bound the leg risk.
    pub fn plan(&self, pricing: &PathPricing) -> Result<PlannedTrade> {
        let legs: Vec<&PathLeg> = pricing.legs().collect();
        if legs.is_empty() {
            return Err(Error::invalid(
                "the path has no market legs, so there is nothing to plan",
            ));
        }

        let mut scored: Vec<(f64, &PathLeg)> = Vec::with_capacity(legs.len());
        for leg in legs {
            scored.push((reversibility_f64(leg)?, leg));
        }
        // Ascending: hardest to reverse first. Ties broken on identity so that
        // two runs over the same market produce the same plan.
        scored.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.venue.as_str().cmp(b.1.venue.as_str()))
                .then_with(|| a.1.object.as_str().cmp(b.1.object.as_str()))
                .then_with(|| a.1.conversion.cmp(&b.1.conversion))
        });

        let mut rationale: Vec<String> = Vec::new();
        let mut ordering: Vec<LegRanking> = Vec::with_capacity(scored.len());
        let mut steps: Vec<LegStep> = Vec::with_capacity(scored.len());
        let mut earliest: BTreeMap<usize, u16> = BTreeMap::new();
        let mut by_quote: BTreeMap<ObjectId, Decimal> = BTreeMap::new();

        for (position, (reversibility_f64, leg)) in scored.iter().enumerate() {
            let order = u16::try_from(position)
                .map_err(|_| Error::invalid("a plan with more than 65,535 legs is not a plan"))?;
            let optional = self.is_optional(pricing, leg)?;
            let entry = earliest.entry(leg.conversion).or_insert(order);
            *entry = (*entry).min(order);

            ordering.push(LegRanking {
                object: leg.object.clone(),
                venue: leg.venue.clone(),
                order,
                reversibility_f64: *reversibility_f64,
                reason: format!(
                    "{} at {} scores {reversibility_f64} on reversibility",
                    leg.object.as_str(),
                    leg.venue.as_str()
                ),
            });
            if !optional && order > 0 {
                *by_quote
                    .entry(leg.quote_object.clone())
                    .or_insert(Decimal::ZERO) += leg.notional()?;
            }
            steps.push(LegStep {
                object_id: leg.object.clone(),
                venue: leg.venue.clone(),
                side: leg.side,
                quantity: leg.quantity,
                reference_price: leg.executable_price,
                priced_in: leg.quote_object.clone(),
                order,
                optional,
            });
        }

        if let Some(first) = ordering.first() {
            rationale.push(format!(
                "{} at {} goes first: it is the hardest leg to undo, so it is done while nothing else is committed",
                first.object.as_str(),
                first.venue.as_str()
            ));
        }

        let mut plan = LegPlan::new(steps)?;
        for (object, quantity) in self.prefunding(pricing, &earliest, &mut rationale) {
            plan = plan.with_prefunded(object, quantity);
        }

        // Residual after one leg, not after none: before the first leg lands
        // nothing has been committed and there is nothing to strand. After it
        // lands, everything still outstanding is exposure, and this is the
        // largest that number ever gets.
        let residual_risk = plan.residual_after(1);
        if residual_risk > self.settings.leg_risk_budget {
            return Err(Error::guard(format!(
                "the plan leaves {residual_risk} exposed if it stops after its first leg, against a budget of {}; a plan that cannot be sized is not attempted hopefully",
                self.settings.leg_risk_budget
            )));
        }
        rationale.push(format!(
            "stopping after the first leg strands {residual_risk}, within the {} budget",
            self.settings.leg_risk_budget
        ));

        Ok(PlannedTrade {
            plan,
            residual_risk,
            residual_by_quote: by_quote.into_iter().collect(),
            ordering,
            rationale,
        })
    }

    /// Whether a leg can be abandoned without leaving the plan half-on.
    ///
    /// Only synthetic dust qualifies. Every trade leg of a cycle changes what is
    /// held, so skipping one leaves a position — which is the thing the plan
    /// exists to avoid, not a corner to cut.
    fn is_optional(&self, pricing: &PathPricing, leg: &PathLeg) -> Result<bool> {
        let Some(conversion) = pricing.conversions.get(leg.conversion) else {
            return Ok(false);
        };
        if conversion.kind != "synthetic" {
            return Ok(false);
        }
        let total = conversion.notional()?;
        if total <= Decimal::ZERO {
            return Ok(false);
        }
        let threshold = mul(
            total,
            crate::arith::from_statistic(
                self.settings.optional_leg_fraction_f64.max(0.0),
                "optional leg fraction",
            )?,
            "optional leg threshold",
        )?;
        Ok(leg.notional()? <= threshold)
    }

    /// Inventory that has to be in place before the first order goes out.
    fn prefunding(
        &self,
        pricing: &PathPricing,
        earliest: &BTreeMap<usize, u16>,
        rationale: &mut Vec<String>,
    ) -> Vec<(ObjectId, Decimal)> {
        let mut required: BTreeMap<ObjectId, Decimal> = BTreeMap::new();
        for (index, conversion) in pricing.conversions.iter().enumerate() {
            if conversion.legs.is_empty() {
                // A transfer is not an order. What it delivers has to be
                // standing at the destination before the plan starts, which is
                // exactly what prefunded inventory means.
                *required
                    .entry(conversion.to.object.clone())
                    .or_insert(Decimal::ZERO) += conversion.output;
                rationale.push(format!(
                    "{} is held at {} up front: moving it mid-flight is not something the plan can wait for",
                    conversion.to.object.as_str(),
                    conversion.to.venue.as_str()
                ));
                continue;
            }

            let reason = if index == 0 {
                Some("the path starts here, so its input is capital already in place".to_string())
            } else {
                let producer = &pricing.conversions[index - 1];
                if producer.legs.is_empty() {
                    // The producer is a transfer, whose output is already
                    // prefunded above. Counting it twice would double the
                    // inventory the plan asks for.
                    None
                } else if earliest
                    .get(&index)
                    .zip(earliest.get(&(index - 1)))
                    .is_some_and(|(own, produced)| own < produced)
                {
                    Some("this leg runs before the leg that would produce its input".to_string())
                } else if !producer.settles_atomically {
                    Some(format!(
                        "the leg that produces its input runs at {}, which can revert after this one has landed",
                        producer.to.venue.as_str()
                    ))
                } else {
                    None
                }
            };

            if let Some(reason) = reason {
                *required
                    .entry(conversion.from.object.clone())
                    .or_insert(Decimal::ZERO) += conversion.input;
                rationale.push(format!(
                    "{} must be prefunded: {reason}",
                    conversion.from.object.as_str()
                ));
            }
        }
        required.into_iter().collect()
    }
}

/// How easily a leg could be undone if the rest of the plan fails.
///
/// Three things make a leg hard to reverse, and they multiply rather than add
/// because any one of them is enough on its own: a venue whose quotes are not
/// firm or whose fills can revert, a size that dwarfs what is resting at the
/// touch, and a price that has already been walked a long way from it.
fn reversibility_f64(leg: &PathLeg) -> Result<f64> {
    let venue_f64 = match (
        leg.venue_class.quotes_are_firm(),
        leg.venue_class.settles_atomically(),
    ) {
        (true, true) => 1.0,
        (true, false) => 0.6,
        (false, true) => 0.4,
        (false, false) => 0.2,
    };

    let headroom_f64 = if leg.quantity > Decimal::ZERO {
        leg.touch_quantity
            .checked_div(leg.quantity)
            .map_or(MIN_HEADROOM, |ratio| ratio.to_f64())
            .clamp(MIN_HEADROOM, 1.0)
    } else {
        MIN_HEADROOM
    };

    // A hundred times the slippage fraction: a leg that gave up a full percent
    // walking the book is already half as reversible as one that gave up
    // nothing, which matches how much of it a round trip would hand back.
    let slippage_f64 = leg.slippage_fraction()?.to_f64().max(0.0);
    let depth_penalty_f64 = 1.0 / (1.0 + slippage_f64 * 100.0);

    Ok(venue_f64 * headroom_f64 * depth_penalty_f64)
}
