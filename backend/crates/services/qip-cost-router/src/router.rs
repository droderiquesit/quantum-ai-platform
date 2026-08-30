//! The contextual model and agent router.
//!
//! Same idea as `qip-optimization-engine`'s compute router, one level up. That
//! one chooses a *solver* and always computes a classical baseline first; this
//! one chooses an *intelligence* and always starts from the cheapest rung that
//! could work. Both refuse to spend on the expensive path without a stated
//! reason, and both put the reason in the returned value rather than in a log
//! line, so a decision arrives with the evidence for how it was made or not at
//! all.
//!
//! The rule: **select the cheapest intelligence level capable of making the
//! decision.** Two of its consequences are structural rather than checked.
//!
//! * **A decision requiring determinism can never route above the deterministic
//!   rung.** [`Router::select`] matches on [`Determinism`], and the
//!   `Required` arm returns a [`DeterministicRouting`] — a type with no field a
//!   [`ModelTier`] fits in. There is no code that could be written in that arm
//!   that routes a risk check to a model, which is a stronger statement than
//!   "no code that does". Escalation compounds it: [`Router::escalate`] takes a
//!   [`JudgedRouting`], which is constructible only by the other arm, so a
//!   determinism-required decision has nothing to hand it.
//! * **A rung that costs more than the decision is worth is refused, by name.**
//!   Spending more on inference than the opportunity can earn is the specific
//!   failure this crate exists to prevent, and it is refused at the rung rather
//!   than netted out afterwards — by the time it is a deduction, the money is
//!   already gone.
//!
//! Nothing here reads a clock or draws a random number. The same context routes
//! the same way every time, which is what lets a replay reproduce not just the
//! decision but the reasoning that priced it.

use crate::context::{DecisionContext, Determinism};
use crate::ledger::TierCharge;
use crate::tier::{IntelligenceTier, ModelTier};
use qip_contracts::signal::Conviction;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};

/// How much of a decision's value the platform will spend reaching it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// The largest share of the value at stake that may be spent on reaching
    /// the answer, in `(0, 1]`.
    ///
    /// The absolute rule is that a rung costing more than the decision is worth
    /// is never used; this is the practical version of it. A fraction of one
    /// means the platform will spend a dollar to make a dollar, which is not a
    /// business, so the default is far below that and a deployment that raises
    /// it has to say so in a diff.
    pub cost_ceiling_fraction_f64: f64,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            cost_ceiling_fraction_f64: 0.05,
        }
    }
}

impl RoutingPolicy {
    pub fn validate(&self) -> Result<()> {
        if !self.cost_ceiling_fraction_f64.is_finite()
            || self.cost_ceiling_fraction_f64 <= 0.0
            || self.cost_ceiling_fraction_f64 > 1.0
        {
            return Err(Error::invalid(format!(
                "a cost ceiling of {} is not a share of the value at stake; above one the platform spends more than the decision can earn",
                self.cost_ceiling_fraction_f64
            )));
        }
        Ok(())
    }

    /// The most this decision may spend, exact.
    ///
    /// The fraction crosses from `f64` to [`Decimal`] here and only here. It is
    /// a policy statistic; everything downstream of this line is money.
    pub fn affordable(&self, value_at_stake: Decimal) -> Result<Decimal> {
        self.validate()?;
        let fraction = Decimal::from_f64(self.cost_ceiling_fraction_f64)
            .ok_or_else(|| Error::numeric("the cost ceiling is not a representable fraction"))?;
        value_at_stake
            .checked_mul(fraction)
            .ok_or_else(|| Error::numeric("the affordable spend on this decision overflowed"))
    }
}

/// Whether a rung may be used for a decision, and why not when it may not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierVerdict {
    /// Affordable, fast enough, and strong enough.
    Usable,
    /// Costs more than the decision can justify. Terminal: everything above
    /// this rung costs more.
    Unaffordable { cost: Decimal, affordable: Decimal },
    /// Cannot answer inside the latency budget. Terminal: everything above this
    /// rung is slower.
    TooSlow { needs: Duration, budget: Duration },
    /// Not strong enough for the confidence this decision needs. Not terminal —
    /// this is the one that makes the router climb.
    Incapable { reaches_f64: f64, required_f64: f64 },
}

impl TierVerdict {
    pub const fn is_usable(&self) -> bool {
        matches!(self, Self::Usable)
    }

    /// Whether a stronger rung could still rescue the decision.
    pub const fn can_climb(&self) -> bool {
        matches!(self, Self::Incapable { .. })
    }

    /// The refusal, as a sentence naming the rung and the reason.
    pub fn reason(&self, tier: IntelligenceTier) -> String {
        match self {
            Self::Usable => format!("the {} rung is usable", tier.as_str()),
            Self::Unaffordable { cost, affordable } => format!(
                "the {} rung costs {cost} to reach a decision that can justify {affordable}; spending more on the answer than the answer is worth is not an opportunity, and every rung above it costs more",
                tier.as_str()
            ),
            Self::TooSlow { needs, budget } => format!(
                "the {} rung needs {needs:?} and the decision is worthless after {budget:?}; every rung above it is slower",
                tier.as_str()
            ),
            Self::Incapable {
                reaches_f64,
                required_f64,
            } => format!(
                "the {} rung reaches {reaches_f64} confidence and the decision needs {required_f64}",
                tier.as_str()
            ),
        }
    }
}

/// A decision answered in deterministic code.
///
/// Carries no rung, and that absence is the guarantee: there is nowhere in this
/// type to record that a model was consulted, so a value of it is proof that
/// none was. It is what [`Router::select`] returns for a decision that requires
/// determinism, and also what it returns when deterministic code was simply the
/// cheapest rung that could answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeterministicRouting {
    rationale: String,
    charge: TierCharge,
}

impl DeterministicRouting {
    pub fn rationale(&self) -> &str {
        &self.rationale
    }

    pub fn charge(&self) -> TierCharge {
        self.charge
    }

    pub fn cost(&self) -> Decimal {
        self.charge.cost
    }
}

/// A decision that tolerated an estimate, and where on the ladder it landed.
///
/// The fields are private and there is no public constructor. The only way to
/// obtain one is [`Router::select`] on a context whose [`Determinism`] is
/// `NotRequired`, which is what stops a determinism-required decision from
/// reaching [`Router::escalate`] — it has no value of this type to pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgedRouting {
    tier: IntelligenceTier,
    rationale: String,
    charges: Vec<TierCharge>,
}

impl JudgedRouting {
    /// The rung that produced the answer.
    pub fn tier(&self) -> IntelligenceTier {
        self.tier
    }

    /// The rung as something that can be wrong, or `None` if deterministic code
    /// turned out to be the cheapest rung that could answer.
    pub fn model_tier(&self) -> Option<ModelTier> {
        self.tier.model_tier()
    }

    pub fn rationale(&self) -> &str {
        &self.rationale
    }

    /// Every rung used, including the ones that answered and were escalated
    /// past. All of them were paid for.
    pub fn charges(&self) -> &[TierCharge] {
        &self.charges
    }

    pub fn tiers(&self) -> Vec<IntelligenceTier> {
        self.charges.iter().map(|c| c.tier).collect()
    }

    /// Everything spent reaching this answer, exact.
    pub fn total_cost(&self) -> Decimal {
        self.charges
            .iter()
            .fold(Decimal::ZERO, |sum, charge| sum + charge.cost)
    }

    pub fn total_latency(&self) -> Duration {
        self.charges
            .iter()
            .fold(Duration::from_nanos(0), |sum, charge| sum + charge.latency)
    }

    /// How many rungs were climbed after the first.
    pub fn escalations(&self) -> usize {
        self.charges.len().saturating_sub(1)
    }
}

/// Where a decision was routed.
///
/// An enum rather than a struct with an optional tier, because the two cases
/// are not the same shape: one of them has no ladder above it. An exhaustive
/// match on this type is the assertion that a determinism-required decision
/// never reached a model, and it is checked by the compiler at every call site
/// rather than by a test at one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Routing {
    /// Answered in deterministic code.
    Deterministic(DeterministicRouting),
    /// Answered by something that could have been wrong.
    Judged(JudgedRouting),
}

impl Routing {
    /// The rung that answered.
    pub fn tier(&self) -> IntelligenceTier {
        match self {
            Self::Deterministic(_) => IntelligenceTier::DeterministicCode,
            Self::Judged(judged) => judged.tier(),
        }
    }

    /// The rung as something that can be wrong.
    ///
    /// `None` for the deterministic variant because there is no field to read
    /// it from, not because a check returned false.
    pub fn model_tier(&self) -> Option<ModelTier> {
        match self {
            Self::Deterministic(_) => None,
            Self::Judged(judged) => judged.model_tier(),
        }
    }

    pub fn rationale(&self) -> &str {
        match self {
            Self::Deterministic(routing) => routing.rationale(),
            Self::Judged(judged) => judged.rationale(),
        }
    }

    /// Every rung paid for.
    pub fn charges(&self) -> Vec<TierCharge> {
        match self {
            Self::Deterministic(routing) => vec![routing.charge()],
            Self::Judged(judged) => judged.charges().to_vec(),
        }
    }

    pub fn tiers(&self) -> Vec<IntelligenceTier> {
        self.charges().into_iter().map(|c| c.tier).collect()
    }

    pub fn total_cost(&self) -> Decimal {
        match self {
            Self::Deterministic(routing) => routing.cost(),
            Self::Judged(judged) => judged.total_cost(),
        }
    }
}

/// What bounds an escalation.
///
/// Both bounds are refused past rather than clipped to. A router that silently
/// stopped at the ceiling would return the ceiling rung's answer as though it
/// were the answer the decision asked for, and the caller would act on a
/// confidence it never got.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EscalationLimits {
    /// The highest rung this decision may reach.
    pub ceiling: IntelligenceTier,
    /// The most that may be spent across every rung used, exact.
    pub maximum_spend: Decimal,
}

impl EscalationLimits {
    pub fn new(ceiling: IntelligenceTier, maximum_spend: Decimal) -> Result<Self> {
        if maximum_spend <= Decimal::ZERO {
            return Err(Error::invalid(
                "an escalation budget of nothing cannot pay for the rung that is already answering",
            ));
        }
        Ok(Self {
            ceiling,
            maximum_spend,
        })
    }
}

/// What an escalation did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Escalation {
    /// The answer already cleared the bar. Nothing more was bought, which is
    /// the outcome the ladder is trying to reach.
    Settled(JudgedRouting),
    /// One rung higher. Both rungs are charged: the one that answered badly was
    /// still paid for, and pretending otherwise makes escalation look free.
    Climbed(JudgedRouting),
}

impl Escalation {
    pub fn routing(&self) -> &JudgedRouting {
        match self {
            Self::Settled(routing) | Self::Climbed(routing) => routing,
        }
    }

    pub const fn climbed(&self) -> bool {
        matches!(self, Self::Climbed(_))
    }
}

/// Places decisions on the ladder.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Router {
    policy: RoutingPolicy,
}

impl Router {
    pub fn new(policy: RoutingPolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self { policy })
    }

    pub fn policy(&self) -> RoutingPolicy {
        self.policy
    }

    /// Whether a rung may be used for this decision, and why not when it may
    /// not.
    ///
    /// Public so a caller can ask before committing to the cost, and so the
    /// reasoning is testable without routing anything.
    ///
    /// The order of the checks is load-bearing. Affordability and latency are
    /// terminal — nothing above a rung is cheaper or faster — so they are
    /// answered before capability, which is the only verdict that justifies
    /// looking higher. Asking about capability first would have the router
    /// climb past a rung it already could not afford and refuse several rungs
    /// later with the wrong reason.
    pub fn assess(&self, tier: IntelligenceTier, context: &DecisionContext) -> Result<TierVerdict> {
        let affordable = self.policy.affordable(context.value_at_stake)?;
        let cost = tier.cost();
        if cost > affordable {
            return Ok(TierVerdict::Unaffordable { cost, affordable });
        }
        let needs = tier.latency();
        if needs.as_nanos() > context.latency_budget.as_nanos() {
            return Ok(TierVerdict::TooSlow {
                needs,
                budget: context.latency_budget,
            });
        }
        let reaches_f64 = tier.resolving_power_f64();
        if reaches_f64 < context.required_confidence_f64 {
            return Ok(TierVerdict::Incapable {
                reaches_f64,
                required_f64: context.required_confidence_f64,
            });
        }
        Ok(TierVerdict::Usable)
    }

    /// Place a decision on the ladder.
    ///
    /// The match below is where the determinism rule lives. The `Required` arm
    /// returns a [`DeterministicRouting`], which cannot name a rung; the
    /// `NotRequired` arm is the only one that can produce a [`JudgedRouting`].
    /// Nothing about that is a runtime check, and nothing a later edit adds to
    /// the `Required` arm can route a risk check to a model without changing
    /// the return type of the function it is in.
    pub fn select(&self, context: &DecisionContext) -> Result<Routing> {
        context.validate()?;
        self.policy.validate()?;
        match context.determinism {
            Determinism::Required => Ok(Routing::Deterministic(self.determined(context))),
            Determinism::NotRequired => self.judged(context).map(Routing::Judged),
        }
    }

    /// The routing for a decision that may not be estimated.
    ///
    /// Takes no verdict and consults no ladder. The rung is not chosen — it is
    /// the only one the decision is allowed to occupy — so affordability and
    /// confidence do not apply: refusing a pre-trade risk check because the
    /// decision was small would leave the check unmade, which is worse than any
    /// cost it could have avoided.
    fn determined(&self, context: &DecisionContext) -> DeterministicRouting {
        DeterministicRouting {
            rationale: format!(
                "'{}' must be a function of its inputs, so it is answered in deterministic code under {}; no rung above it is reachable from here",
                context.subject,
                context.conditions.label()
            ),
            charge: TierCharge::of(IntelligenceTier::DeterministicCode),
        }
    }

    /// The cheapest rung that can answer a decision which tolerates an estimate.
    fn judged(&self, context: &DecisionContext) -> Result<JudgedRouting> {
        let mut last: Option<(IntelligenceTier, TierVerdict)> = None;
        for tier in IntelligenceTier::LADDER {
            let verdict = self.assess(tier, context)?;
            match verdict {
                TierVerdict::Usable => {
                    return Ok(JudgedRouting {
                        tier,
                        rationale: format!(
                            "'{}' routes to the {} rung: the cheapest that reaches {} confidence at {} a decision, inside a {:?} budget, under {}",
                            context.subject,
                            tier.as_str(),
                            context.required_confidence_f64,
                            tier.cost(),
                            context.latency_budget,
                            context.conditions.label()
                        ),
                        charges: vec![TierCharge::of(tier)],
                    });
                }
                other => {
                    if !other.can_climb() {
                        return Err(Error::denied(format!(
                            "'{}' cannot be routed: {}",
                            context.subject,
                            other.reason(tier)
                        )));
                    }
                    last = Some((tier, other));
                }
            }
        }
        let detail = match last {
            Some((tier, verdict)) => verdict.reason(tier),
            None => "the ladder is empty".to_string(),
        };
        Err(Error::denied(format!(
            "'{}' cannot be routed: no rung reaches {} confidence — {}",
            context.subject, context.required_confidence_f64, detail
        )))
    }

    /// Climb one rung after an answer that did not clear the bar.
    ///
    /// Takes a [`JudgedRouting`], which is the second half of the determinism
    /// rule: a decision that required determinism was routed to a
    /// [`DeterministicRouting`] and has no value of this type to escalate with.
    ///
    /// `achieved` is a [`qip_contracts::signal::Conviction`] rather than a bare
    /// probability, so it is shrunk toward a coin flip by how little evidence
    /// backs it. A rung that reports 0.95 from two observations has not cleared
    /// a 0.9 bar, and escalating on the shrunk figure is what stops a confident
    /// small sample from ending the climb early.
    ///
    /// Both bounds refuse rather than clip. Every rung climbed stays charged.
    pub fn escalate(
        &self,
        routing: &JudgedRouting,
        context: &DecisionContext,
        achieved: Conviction,
        limits: &EscalationLimits,
    ) -> Result<Escalation> {
        context.validate()?;
        if achieved.clears(context.required_confidence_f64) {
            return Ok(Escalation::Settled(routing.clone()));
        }

        let current = routing.tier();
        let Some(next) = current.next() else {
            return Err(Error::denied(format!(
                "'{}' answered at {} with {} confidence and the ladder has no rung above it; the decision has to be refused rather than escalated",
                context.subject,
                current.as_str(),
                achieved.shrunk()
            )));
        };
        if next > limits.ceiling {
            return Err(Error::denied(format!(
                "'{}' would escalate from {} to {}, above the {} ceiling this decision was granted",
                context.subject,
                current.as_str(),
                next.as_str(),
                limits.ceiling.as_str()
            )));
        }

        let verdict = self.assess(next, context)?;
        if matches!(
            verdict,
            TierVerdict::Unaffordable { .. } | TierVerdict::TooSlow { .. }
        ) {
            return Err(Error::denied(format!(
                "'{}' cannot escalate to {}: {}",
                context.subject,
                next.as_str(),
                verdict.reason(next)
            )));
        }

        let spent = routing.total_cost();
        let after = spent
            .checked_add(next.cost())
            .ok_or_else(|| Error::numeric("the escalated spend on this decision overflowed"))?;
        if after > limits.maximum_spend {
            return Err(Error::denied(format!(
                "'{}' has spent {spent} and escalating to {} would take it to {after}, past the {} this decision may spend in total",
                context.subject,
                next.as_str(),
                limits.maximum_spend
            )));
        }

        let mut charges = routing.charges.clone();
        charges.push(TierCharge::of(next));
        Ok(Escalation::Climbed(JudgedRouting {
            tier: next,
            rationale: format!(
                "'{}' answered at {} with {} confidence against a {} bar, so it climbed to {}; both rungs are charged",
                context.subject,
                current.as_str(),
                achieved.shrunk(),
                context.required_confidence_f64,
                next.as_str()
            ),
            charges,
        }))
    }
}
