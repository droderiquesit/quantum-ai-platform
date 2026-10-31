//! The capital society (R12-CAPITAL-4085, ADR 0229): ten named agents that
//! each propose a capital plan every cycle, recorded before the kernel
//! chooses.
//!
//! The agents only propose. The plan acted on is the allocator's, with each
//! line narrowed to the minimum of every non-abstaining agent's line for that
//! strategy: [`settle`] can lower a line and can never raise one, so nothing
//! is sized here that the allocator did not size (decision 7 of
//! `docs/blueprint/owner-decisions-2026-10-31.md`).
//!
//! An agent with no input abstains and names what it lacks. Abstention
//! constrains nothing, deliberately: a missing figure is not evidence that a
//! line should be zero, and reading it as zero would let an unwired input
//! close the desk without anyone deciding to.

use crate::allocation::AllocationPlan;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The ten agents the requirement names, as a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CapitalAgent {
    Treasury,
    Liquidity,
    Funding,
    Leverage,
    Collateral,
    StrategyAllocation,
    RegionAllocation,
    VenueAllocation,
    TaxAware,
    Survival,
}

impl CapitalAgent {
    /// Every agent, in the order their plans are recorded.
    pub const ALL: [Self; 10] = [
        Self::Treasury,
        Self::Liquidity,
        Self::Funding,
        Self::Leverage,
        Self::Collateral,
        Self::StrategyAllocation,
        Self::RegionAllocation,
        Self::VenueAllocation,
        Self::TaxAware,
        Self::Survival,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Treasury => "treasury",
            Self::Liquidity => "liquidity",
            Self::Funding => "funding",
            Self::Leverage => "leverage",
            Self::Collateral => "collateral",
            Self::StrategyAllocation => "strategy-allocation",
            Self::RegionAllocation => "region-allocation",
            Self::VenueAllocation => "venue-allocation",
            Self::TaxAware => "tax-aware",
            Self::Survival => "survival",
        }
    }
}

/// What one agent proposes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Proposal {
    /// A notional per strategy the agent has an opinion on. A strategy it
    /// omits is one it is silent on, not one it would give nothing.
    Lines(BTreeMap<String, Decimal>),
    /// No opinion, and the input the agent would need to have one.
    Abstain { missing: String },
}

/// One agent's plan for one allocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalPlan {
    pub agent: CapitalAgent,
    pub proposal: Proposal,
}

/// The figures already on record that the central plane hands the society.
/// `None` or empty is "not on record", and the agent that would read it
/// abstains.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SocietyInputs {
    /// What the survival waterfall released to opportunity this cycle.
    pub survival_budget: Option<Decimal>,
    /// The capital ceiling on record per cell.
    pub cell_ceilings: BTreeMap<String, Decimal>,
}

/// Every agent's plan for `plan`, in [`CapitalAgent::ALL`] order.
pub fn propose_all(plan: &AllocationPlan, inputs: &SocietyInputs) -> Result<Vec<CapitalPlan>> {
    CapitalAgent::ALL
        .iter()
        .map(|agent| {
            Ok(CapitalPlan {
                agent: *agent,
                proposal: propose(*agent, plan, inputs)?,
            })
        })
        .collect()
}

fn abstain(missing: &str) -> Proposal {
    Proposal::Abstain {
        missing: missing.to_string(),
    }
}

fn propose(agent: CapitalAgent, plan: &AllocationPlan, inputs: &SocietyInputs) -> Result<Proposal> {
    Ok(match agent {
        // The allocator is the strategy-allocation agent.
        CapitalAgent::StrategyAllocation => Proposal::Lines(
            plan.allocations
                .iter()
                .map(|line| (line.strategy.as_str().to_string(), line.notional))
                .collect(),
        ),
        CapitalAgent::Survival => match inputs.survival_budget {
            None => abstain("the survival waterfall's released budget for this cycle"),
            Some(budget) => Proposal::Lines(scaled(
                plan.allocations
                    .iter()
                    .map(|line| (line.strategy.as_str().to_string(), line.notional)),
                budget,
                "the survival budget",
            )?),
        },
        CapitalAgent::RegionAllocation => {
            if inputs.cell_ceilings.is_empty() {
                abstain("a capital ceiling on record for each cell")
            } else {
                let mut lines = BTreeMap::new();
                for (cell, ceiling) in &inputs.cell_ceilings {
                    lines.extend(scaled(
                        plan.allocations
                            .iter()
                            .filter(|line| &line.cell == cell)
                            .map(|line| (line.strategy.as_str().to_string(), line.notional)),
                        *ceiling,
                        "the cell's ceiling",
                    )?);
                }
                Proposal::Lines(lines)
            }
        }
        CapitalAgent::Treasury => abstain("the treasury's per-desk funding position"),
        CapitalAgent::Liquidity => abstain("the liquidity ladder's per-strategy exit capacity"),
        CapitalAgent::Funding => abstain("the funding plan's per-strategy obligations"),
        CapitalAgent::Leverage => abstain("each strategy's leverage on record"),
        CapitalAgent::Collateral => abstain("the collateral graph's per-strategy pledges"),
        CapitalAgent::VenueAllocation => abstain("a capital ceiling on record for each venue"),
        CapitalAgent::TaxAware => {
            abstain("a tax input, which ADR 0153 gives the ledger none of yet")
        }
    })
}

/// `lines` scaled pro rata so their sum is at most `bound`, or unchanged
/// when it already is. A negative bound is refused rather than read as zero.
fn scaled(
    lines: impl Iterator<Item = (String, Decimal)>,
    bound: Decimal,
    what: &str,
) -> Result<BTreeMap<String, Decimal>> {
    if bound.is_negative() {
        return Err(Error::invalid(format!(
            "{what} is {bound}; a capital bound is zero or more, never negative"
        )));
    }
    let lines: BTreeMap<String, Decimal> = lines.collect();
    let total = lines.values().fold(Decimal::ZERO, |sum, line| sum + *line);
    if total <= bound || total.is_zero() {
        return Ok(lines);
    }
    lines
        .into_iter()
        .map(|(strategy, line)| {
            let share = line
                .checked_mul(bound)
                .and_then(|product| product.checked_div(total))
                .ok_or_else(|| Error::numeric(format!("scaling {strategy}'s line to {what}")))?;
            Ok((strategy, share.min(line)))
        })
        .collect()
}

/// `plan` with each line at the minimum of its own notional and every
/// non-abstaining agent's line for that strategy. A line an agent lowered
/// names the agent among its binding constraints. Never raises a line.
pub fn settle(plan: &AllocationPlan, plans: &[CapitalPlan]) -> AllocationPlan {
    let mut settled = plan.clone();
    for line in &mut settled.allocations {
        let strategy = line.strategy.as_str().to_string();
        for proposal in plans {
            if let Proposal::Lines(lines) = &proposal.proposal
                && let Some(proposed) = lines.get(&strategy)
                && *proposed < line.notional
            {
                line.notional = (*proposed).max(Decimal::ZERO);
                line.binding_constraints
                    .push(format!("capital-society:{}", proposal.agent.as_str()));
            }
        }
    }
    settled
}
