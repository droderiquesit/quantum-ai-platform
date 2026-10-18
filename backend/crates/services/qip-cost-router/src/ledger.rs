//! What a decision cost to reach, accumulated as it is reached.
//!
//! A decision is not one call. It is a rule that fired, then a model that was
//! unsure, then two agents that disagreed, then a simulation that settled it —
//! and the number that matters is the sum, charged to the decision that caused
//! it rather than to a monthly infrastructure line nobody nets against edge.
//!
//! Agent accounting is not restated here. [`qip_agents::BudgetLedger`] already
//! charges a model call before it happens and refuses the one that would breach
//! the allowance, and it is composed rather than copied: every rung that calls a
//! model is charged through it, so a decision that escalates past what its agent
//! was authorised for fails at the rung that would have breached it. What this
//! type adds is the money in the currency the edge is quoted in, which the agent
//! budget deliberately does not carry (it works in micro-units of a single run,
//! not in the unit a [`qip_contracts::NetEdge`] is denominated in).

use crate::tier::IntelligenceTier;
use qip_agents::budget::{Budget, BudgetLedger, Spend};
use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One rung, charged once.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TierCharge {
    pub tier: IntelligenceTier,
    /// Money, exact.
    pub cost: Decimal,
    /// Time the rung is expected to take. Summed rather than measured, because
    /// this crate reads no clock: a replay must price the same decision the
    /// same way.
    pub latency: Duration,
}

impl TierCharge {
    pub fn of(tier: IntelligenceTier) -> Self {
        Self {
            tier,
            cost: tier.cost(),
            latency: tier.latency(),
        }
    }
}

/// Converts an exact amount into the agent budget's micro-units.
///
/// Truncates, and truncating downward is the wrong direction for a cost, so
/// anything with a fractional micro-unit is rounded *up*. Under-reporting a
/// charge to the component whose job is to refuse the next one is how a budget
/// stops binding.
fn to_micros(amount: Decimal) -> u64 {
    let raw = amount.raw().max(0);
    let micros = raw / 1_000 + i128::from(raw % 1_000 != 0);
    u64::try_from(micros).unwrap_or(u64::MAX)
}

/// The key a charge is reported under when the decision it served cannot
/// name one.
///
/// Spelled out rather than left blank, so a report shows how much spend could
/// not be attributed instead of folding it silently into whichever key sorts
/// first. A cycle's compute serves every strategy the cycle looked at; billing
/// it to one of them would be a number nobody computed.
pub const UNATTRIBUTED: &str = "unattributed";

/// The key a charge for a rung that calls no model is reported under on the
/// model dimension.
pub const NO_MODEL: &str = "no-model";

/// Who a decision's spend is attributed to (R12-FINOPS-10136).
///
/// Set once per ledger, because one ledger is one decision: a decision made
/// for a strategy, in a region, on a data source. Each part is optional
/// because each is known only where it is known — a cycle-wide ledger knows
/// its region and not a strategy — and a guessed key is worse than an
/// honest [`UNATTRIBUTED`]. The model is not here: it is a fact about each
/// rung, read off the rung's tier.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Attribution {
    pub strategy: Option<String>,
    pub data_source: Option<String>,
    pub region: Option<String>,
}

impl Attribution {
    /// Attributed to a region and nothing narrower.
    pub fn region(region: impl Into<String>) -> Self {
        Self {
            region: Some(region.into()),
            ..Self::default()
        }
    }
}

/// The dimensions a cost report can be cut by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CostDimension {
    Strategy,
    Model,
    DataSource,
    Region,
}

/// One charge with all four of its keys, as a cost report reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostEntry {
    pub decision: String,
    pub tier: IntelligenceTier,
    pub cost: Decimal,
    pub strategy: String,
    pub model: String,
    pub data_source: String,
    pub region: String,
}

impl CostEntry {
    /// The key this entry is reported under on one dimension.
    pub fn key(&self, dimension: CostDimension) -> &str {
        match dimension {
            CostDimension::Strategy => &self.strategy,
            CostDimension::Model => &self.model,
            CostDimension::DataSource => &self.data_source,
            CostDimension::Region => &self.region,
        }
    }
}

/// Spend per key on one dimension, summed exactly across ledgers.
///
/// Read off [`ComputeLedger::entries`], so the report and the ledger cannot
/// disagree: a second computation of what a decision cost is a second claim
/// about it, and the two drift the first time a budget truncates a bill.
pub fn report_by<'a>(
    ledgers: impl IntoIterator<Item = &'a ComputeLedger>,
    dimension: CostDimension,
) -> BTreeMap<String, Decimal> {
    let mut report = BTreeMap::new();
    for ledger in ledgers {
        for entry in ledger.entries() {
            *report
                .entry(entry.key(dimension).to_string())
                .or_insert(Decimal::ZERO) += entry.cost;
        }
    }
    report
}

/// The running cost of one decision.
#[derive(Clone, Debug)]
pub struct ComputeLedger {
    decision: String,
    charges: Vec<TierCharge>,
    agents: BudgetLedger,
    attribution: Attribution,
}

impl ComputeLedger {
    /// Open a ledger for one decision, bounded by an agent budget.
    ///
    /// The budget is validated here rather than at first use. A ledger opened
    /// against an incoherent budget — model calls permitted with no tokens to
    /// spend them on — would accept every rung until the first one that mattered.
    pub fn new(decision: impl Into<String>, budget: Budget) -> Result<Self> {
        budget.validate()?;
        let decision = decision.into();
        let agents = BudgetLedger::new(budget, decision.clone());
        Ok(Self {
            decision,
            charges: Vec::new(),
            agents,
            attribution: Attribution::default(),
        })
    }

    /// Attribute this decision's spend.
    pub fn with_attribution(mut self, attribution: Attribution) -> Self {
        self.attribution = attribution;
        self
    }

    pub fn attribution(&self) -> &Attribution {
        &self.attribution
    }

    /// Every charge with its strategy, model, data-source and region keys.
    pub fn entries(&self) -> Vec<CostEntry> {
        let key = |part: &Option<String>| part.clone().unwrap_or_else(|| UNATTRIBUTED.to_string());
        self.charges
            .iter()
            .map(|charge| CostEntry {
                decision: self.decision.clone(),
                tier: charge.tier,
                cost: charge.cost,
                strategy: key(&self.attribution.strategy),
                model: charge
                    .tier
                    .model_tier()
                    .map_or(NO_MODEL, |model| model.as_str())
                    .to_string(),
                data_source: key(&self.attribution.data_source),
                region: key(&self.attribution.region),
            })
            .collect()
    }

    /// A ledger for a decision made entirely in deterministic code.
    ///
    /// [`qip_agents::Budget::fast_path`] permits no model calls at all, so a
    /// ledger opened this way refuses the first rung above deterministic code
    /// rather than quietly paying for it. That is the point: the fast path must
    /// never wait on a model, and the budget is what makes that true of the
    /// accounting as well as of the intent.
    pub fn fast_path(decision: impl Into<String>) -> Result<Self> {
        Self::new(decision, Budget::fast_path())
    }

    pub fn decision(&self) -> &str {
        &self.decision
    }

    /// Every rung that answered, in the order it was asked.
    pub fn charges(&self) -> &[TierCharge] {
        &self.charges
    }

    /// What the agent budget has recorded. The composed view, for an operator
    /// looking at a run rather than at a decision.
    pub fn spend(&self) -> Spend {
        self.agents.spend()
    }

    /// Fraction of the tightest agent budget line consumed.
    pub fn utilisation(&self) -> f64 {
        self.agents.utilisation()
    }

    /// Whether another rung could be afforded at all.
    pub fn has_headroom(&self) -> bool {
        self.agents.has_headroom()
    }

    /// Total money spent reaching the decision so far. Exact.
    pub fn total_cost(&self) -> Decimal {
        self.charges
            .iter()
            .fold(Decimal::ZERO, |sum, charge| sum + charge.cost)
    }

    /// Total time the rungs are expected to have taken.
    pub fn total_latency(&self) -> Duration {
        self.charges
            .iter()
            .fold(Duration::from_nanos(0), |sum, charge| sum + charge.latency)
    }

    /// How many times a rung has answered.
    pub fn count(&self, tier: IntelligenceTier) -> usize {
        self.charges.iter().filter(|c| c.tier == tier).count()
    }

    /// Charge one invocation of a rung.
    ///
    /// The agent budget is charged first and the rung is recorded only if every
    /// seat it needed was authorised. A refusal therefore leaves the agent's
    /// allowance charged for the calls that were made and this ledger without
    /// the rung — which is correct, because a rung that ran out of budget
    /// part-way through did not produce an answer, and charging a decision for
    /// an answer it never got would flatter the next decision's budget.
    pub fn charge(&mut self, tier: IntelligenceTier) -> Result<TierCharge> {
        // Every rung is one capability invocation, deterministic code included.
        // A rule that runs a billion times is not free, and a ledger that only
        // counts model calls cannot see that.
        self.agents.charge_tool_call()?;

        let calls = tier.model_calls();
        if calls > 0 {
            let mut remaining_micros = to_micros(tier.cost());
            let mut remaining_tokens = tier.tokens();
            let per_micros = remaining_micros / u64::from(calls);
            let per_tokens = remaining_tokens / calls;
            for seat in 1..=calls {
                // The last seat carries the remainder, so the seats sum to the
                // rung's stated cost exactly rather than to the nearest
                // multiple of the fan-out.
                let (micros, tokens) = if seat == calls {
                    (remaining_micros, remaining_tokens)
                } else {
                    (per_micros, per_tokens)
                };
                self.agents.charge_language_model(tokens, micros)?;
                remaining_micros = remaining_micros.saturating_sub(micros);
                remaining_tokens = remaining_tokens.saturating_sub(tokens);
            }
        }

        let charge = TierCharge::of(tier);
        self.charges.push(charge);
        Ok(charge)
    }

    /// Charge every rung a routing used, in order.
    ///
    /// Takes the rungs rather than the routing so the ledger stays ignorant of
    /// why they were chosen: a ledger that could read a routing decision would
    /// eventually be asked to second-guess one.
    pub fn charge_all(&mut self, tiers: &[IntelligenceTier]) -> Result<Decimal> {
        for tier in tiers {
            self.charge(*tier)?;
        }
        Ok(self.total_cost())
    }

    /// A sentence naming what was spent and on what, for the deduction basis.
    pub fn basis(&self) -> String {
        if self.charges.is_empty() {
            return format!("no compute was charged to '{}'", self.decision);
        }
        let mut parts: Vec<String> = Vec::new();
        for tier in IntelligenceTier::LADDER {
            let used = self.count(tier);
            if used > 0 {
                parts.push(format!("{}×{}", used, tier.as_str()));
            }
        }
        format!(
            "{} reaching '{}' over {:?}",
            parts.join(" + "),
            self.decision,
            self.total_latency()
        )
    }
}
