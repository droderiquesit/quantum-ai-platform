//! The escalation ladder, and what each rung costs to stand on.
//!
//! Nine ways to answer a question, ordered by what they cost. The order is the
//! whole point: the platform's rule is to use the *cheapest* rung capable of
//! making the decision, and a ladder whose rungs are not ordered by cost cannot
//! express that rule at all.
//!
//! Every figure here is a stated assumption, not a measurement. They are
//! constants rather than configuration because a deployment that tunes them
//! per-call can make any rung look affordable, and because a routing decision
//! that cannot be reproduced from the code is not auditable. A deployment whose
//! own outcome record disagrees with a number here should change the number in
//! a reviewed diff, which is what makes the disagreement visible.

use qip_core::Decimal;
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};

/// How the platform can answer a question, cheapest first.
///
/// The derived ordering *is* the ladder. Variants are declared in cost order
/// and `Ord` is derived from that declaration order, so `a < b` reads as "a is
/// cheaper than b" everywhere in this crate. Reordering the variants is
/// therefore a behaviour change and not a tidy-up: the router walks this order
/// looking for the first rung it can justify.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntelligenceTier {
    /// A rule, a lookup, an arithmetic identity. The answer is a function of
    /// the inputs and nothing else, which is the only rung that can be said of.
    DeterministicCode,
    /// A fitted estimator — a regression, a filter, a scorecard — evaluated in
    /// process. Cheap enough to run on every message; wrong in ways a residual
    /// plot shows.
    StatisticalModel,
    /// A small language model. Classification, extraction, routing of text.
    TinyModel,
    /// A model fine-tuned for one domain. Better than the tiny model where it
    /// was trained and no better anywhere else.
    SpecialistModel,
    /// One agent: a model with tools, a budget and a capability grant.
    Agent,
    /// Several agents reasoning against each other, including an adversarial
    /// one. Buys disagreement, which is the only thing that catches a
    /// confidently wrong single agent.
    MultiAgentReasoning,
    /// A large model given room to think. The most expensive way to get a
    /// sentence back, and sometimes the only one that gets the right sentence.
    DeepModel,
    /// A counterfactual run: the decision priced against a simulated world
    /// rather than reasoned about.
    Simulation,
    /// An optimiser: annealing, quadratic programming, a solver on a device.
    /// Answers what no amount of reasoning can, at the cost of a machine.
    Solver,
}

impl IntelligenceTier {
    /// Every rung, cheapest first. What the router walks.
    pub const LADDER: [Self; 9] = [
        Self::DeterministicCode,
        Self::StatisticalModel,
        Self::TinyModel,
        Self::SpecialistModel,
        Self::Agent,
        Self::MultiAgentReasoning,
        Self::DeepModel,
        Self::Simulation,
        Self::Solver,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::DeterministicCode => "deterministic_code",
            Self::StatisticalModel => "statistical_model",
            Self::TinyModel => "tiny_model",
            Self::SpecialistModel => "specialist_model",
            Self::Agent => "agent",
            Self::MultiAgentReasoning => "multi_agent_reasoning",
            Self::DeepModel => "deep_model",
            Self::Simulation => "simulation",
            Self::Solver => "solver",
        }
    }

    /// Position on the ladder, zero-based.
    pub const fn rung(&self) -> u8 {
        match self {
            Self::DeterministicCode => 0,
            Self::StatisticalModel => 1,
            Self::TinyModel => 2,
            Self::SpecialistModel => 3,
            Self::Agent => 4,
            Self::MultiAgentReasoning => 5,
            Self::DeepModel => 6,
            Self::Simulation => 7,
            Self::Solver => 8,
        }
    }

    /// Typical cost of one decision at this rung, in the currency the caller
    /// prices its edge in.
    ///
    /// Exact, because it is money and it ends up inside a
    /// [`qip_contracts::NetEdge`]. The deterministic rung is not zero: a
    /// microsecond of CPU is a real cost, and rounding it to nothing is how a
    /// system convinces itself that running a rule a billion times is free.
    pub const fn cost(&self) -> Decimal {
        // Raw units are 1e-9, so 1_000 is one micro-unit of currency.
        Decimal::from_raw(match self {
            Self::DeterministicCode => 1_000,         // 0.000001
            Self::StatisticalModel => 20_000,         // 0.00002
            Self::TinyModel => 400_000,               // 0.0004
            Self::SpecialistModel => 4_000_000,       // 0.004
            Self::Agent => 50_000_000,                // 0.05
            Self::MultiAgentReasoning => 300_000_000, // 0.30
            Self::DeepModel => 900_000_000,           // 0.90
            Self::Simulation => 4_000_000_000,        // 4.00
            Self::Solver => 12_000_000_000,           // 12.00
        })
    }

    /// How long this rung takes to answer.
    ///
    /// Monotone up the ladder, and deliberately so: the router refuses a rung
    /// that does not fit the latency budget without looking higher, which is
    /// only sound because nothing above is faster.
    pub const fn latency(&self) -> Duration {
        match self {
            Self::DeterministicCode => Duration::from_micros(10),
            Self::StatisticalModel => Duration::from_micros(500),
            Self::TinyModel => Duration::from_millis(120),
            Self::SpecialistModel => Duration::from_millis(600),
            Self::Agent => Duration::from_secs(5),
            Self::MultiAgentReasoning => Duration::from_secs(25),
            Self::DeepModel => Duration::from_secs(60),
            Self::Simulation => Duration::from_secs(300),
            Self::Solver => Duration::from_secs(900),
        }
    }

    /// The confidence this rung typically reaches on a decision that needs
    /// judgement, as a statistic in `(0, 1)`.
    ///
    /// The deterministic rung sits at a coin flip *for judgement questions*,
    /// which is not a claim that rules are unreliable. A rule is certain about
    /// what it computes; it simply cannot compute an answer to a question its
    /// inputs do not already decide. A decision that requires determinism never
    /// consults this figure — see [`crate::Determinism`].
    pub const fn resolving_power_f64(&self) -> f64 {
        match self {
            Self::DeterministicCode => 0.50,
            Self::StatisticalModel => 0.60,
            Self::TinyModel => 0.68,
            Self::SpecialistModel => 0.78,
            Self::Agent => 0.85,
            Self::MultiAgentReasoning => 0.90,
            Self::DeepModel => 0.93,
            Self::Simulation => 0.96,
            Self::Solver => 0.99,
        }
    }

    /// Language-model completions one invocation of this rung makes.
    ///
    /// Charged against an agent's own [`qip_agents::Budget`], which is what
    /// stops a rung from being invoked more times than the agent was
    /// authorised for. Zero for the rungs that consume machines rather than
    /// models.
    pub const fn model_calls(&self) -> u32 {
        match self {
            Self::DeterministicCode | Self::StatisticalModel | Self::Simulation | Self::Solver => 0,
            Self::TinyModel | Self::SpecialistModel | Self::Agent | Self::DeepModel => 1,
            // An adversarial panel is three seats. Fewer is a conversation.
            Self::MultiAgentReasoning => 3,
        }
    }

    /// Tokens one invocation of this rung consumes across its completions.
    pub const fn tokens(&self) -> u32 {
        match self {
            Self::DeterministicCode | Self::StatisticalModel | Self::Simulation | Self::Solver => 0,
            Self::TinyModel => 1_200,
            Self::SpecialistModel => 5_000,
            Self::Agent => 14_000,
            Self::MultiAgentReasoning => 45_000,
            Self::DeepModel => 60_000,
        }
    }

    /// Whether the answer is a function of the inputs and nothing else.
    ///
    /// True for exactly one rung, and that is the fact the determinism rule is
    /// built on rather than a coincidence of the current ladder.
    pub const fn is_deterministic(&self) -> bool {
        matches!(self, Self::DeterministicCode)
    }

    /// The next rung up, or `None` at the top.
    ///
    /// `None` is what makes escalation terminate. A ladder whose top rung
    /// escalated to itself would spend without bound and look like progress.
    pub const fn next(&self) -> Option<Self> {
        match self {
            Self::DeterministicCode => Some(Self::StatisticalModel),
            Self::StatisticalModel => Some(Self::TinyModel),
            Self::TinyModel => Some(Self::SpecialistModel),
            Self::SpecialistModel => Some(Self::Agent),
            Self::Agent => Some(Self::MultiAgentReasoning),
            Self::MultiAgentReasoning => Some(Self::DeepModel),
            Self::DeepModel => Some(Self::Simulation),
            Self::Simulation => Some(Self::Solver),
            Self::Solver => None,
        }
    }

    /// This rung as something that can be wrong, or `None` for the one that
    /// cannot.
    pub const fn model_tier(&self) -> Option<ModelTier> {
        match self {
            Self::DeterministicCode => None,
            Self::StatisticalModel => Some(ModelTier::StatisticalModel),
            Self::TinyModel => Some(ModelTier::TinyModel),
            Self::SpecialistModel => Some(ModelTier::SpecialistModel),
            Self::Agent => Some(ModelTier::Agent),
            Self::MultiAgentReasoning => Some(ModelTier::MultiAgentReasoning),
            Self::DeepModel => Some(ModelTier::DeepModel),
            Self::Simulation => Some(ModelTier::Simulation),
            Self::Solver => Some(ModelTier::Solver),
        }
    }
}

/// A rung above the deterministic one: a rung whose answer is an estimate.
///
/// A separate type rather than a predicate on [`IntelligenceTier`], because a
/// predicate can be forgotten and a type cannot. Anything that returns a
/// `ModelTier` is saying "this answer came from something that can be wrong",
/// and the only way to obtain one is [`IntelligenceTier::model_tier`], which
/// has nothing to hand back for the deterministic rung.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    StatisticalModel,
    TinyModel,
    SpecialistModel,
    Agent,
    MultiAgentReasoning,
    DeepModel,
    Simulation,
    Solver,
}

impl ModelTier {
    /// Every rung that can be wrong, cheapest first.
    pub const ALL: [Self; 8] = [
        Self::StatisticalModel,
        Self::TinyModel,
        Self::SpecialistModel,
        Self::Agent,
        Self::MultiAgentReasoning,
        Self::DeepModel,
        Self::Simulation,
        Self::Solver,
    ];

    /// The same rung on the full ladder.
    pub const fn tier(&self) -> IntelligenceTier {
        match self {
            Self::StatisticalModel => IntelligenceTier::StatisticalModel,
            Self::TinyModel => IntelligenceTier::TinyModel,
            Self::SpecialistModel => IntelligenceTier::SpecialistModel,
            Self::Agent => IntelligenceTier::Agent,
            Self::MultiAgentReasoning => IntelligenceTier::MultiAgentReasoning,
            Self::DeepModel => IntelligenceTier::DeepModel,
            Self::Simulation => IntelligenceTier::Simulation,
            Self::Solver => IntelligenceTier::Solver,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        self.tier().as_str()
    }
}

impl From<ModelTier> for IntelligenceTier {
    fn from(tier: ModelTier) -> Self {
        tier.tier()
    }
}
