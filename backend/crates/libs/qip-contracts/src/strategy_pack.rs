//! `StrategyPack` (blueprint v12.0 §27, R12-CONTRACT-7065): the strategies a
//! reflex cell is allowed to run, delivered ahead of time by the slow brain.
//!
//! A cell decides alone (ADR 0008), so whatever it needs to know about when a
//! strategy stops being valid has to arrive *with* the strategy. A pack that
//! carried compiled plans without their capacity, performance envelope or
//! retirement rules would leave a disconnected cell running a strategy past
//! the point the centre would have retired it, with nothing in hand to notice.

use crate::signal::StrategyId;
use crate::validated::{
    Contract, Validated, after, basis_points, digest, each_named, listed, named, names, positive,
    refuse,
};
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One compiled strategy, identified by the digest of its compiled plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledStrategy {
    pub strategy: StrategyId,
    /// SHA-256 of the compiled plan the cell will execute.
    pub plan_digest: String,
    /// Ids of the hypotheses the strategy depends on being true; at least one.
    /// A strategy with no stated hypothesis cannot be retired when one fails.
    pub hypotheses: Vec<String>,
    /// Instruments or universes the strategy may trade; at least one.
    pub universes: Vec<String>,
    /// Models, features or feeds the strategy needs present; may be empty.
    pub dependencies: Vec<String>,
    /// The most notional the strategy can deploy before its own impact eats
    /// its edge, in the account currency.
    pub capacity: Decimal,
    pub envelope: PerformanceEnvelope,
    /// Conditions under which the cell retires the strategy on its own
    /// authority; at least one.
    pub retirement_rules: Vec<RetirementRule>,
}

/// The band the strategy's live performance is expected to stay inside.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceEnvelope {
    /// The lowest hit rate, in basis points, consistent with the hypothesis.
    pub min_hit_rate_bp: u32,
    /// The deepest drawdown, as a fraction of capacity in basis points.
    pub max_drawdown_bp: u32,
    /// The lowest net edge per trade, in basis points of notional. Signed: a
    /// hedging strategy may legitimately expect to pay.
    pub min_net_edge_bp: i32,
}

/// When a cell must stop running a strategy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RetirementRule {
    /// Performance left the envelope for this many consecutive passes.
    EnvelopeBreached { consecutive_passes: u32 },
    /// A named hypothesis was refuted.
    HypothesisRefuted { hypothesis: String },
    /// The pack's expiry passed with no successor.
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyPackFields {
    pub schema_version: u32,
    pub pack_id: String,
    pub issued_at: Timestamp,
    pub expires_at: Timestamp,
    pub strategies: Vec<CompiledStrategy>,
}

/// The §27 strategy pack. Built only through [`Validated::new`].
pub type StrategyPack = Validated<StrategyPackFields>;

impl Contract for StrategyPackFields {
    const NAME: &'static str = "StrategyPack";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses an empty pack, a strategy listed twice, a malformed plan
    /// digest, no hypothesis or universe, a non-positive capacity, an
    /// envelope bound above 100%, no retirement rule, a retirement on a
    /// hypothesis the strategy does not hold, a zero-pass breach rule (which
    /// would retire on the first pass), and an expiry not after issue.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "pack_id", &self.pack_id)?;
        after(
            c,
            "expires_at",
            self.expires_at,
            "issued_at",
            self.issued_at,
        )?;
        listed(c, "strategies", &self.strategies)?;
        let mut seen = BTreeSet::new();
        for s in &self.strategies {
            named(c, "strategy", s.strategy.as_str())?;
            if !seen.insert(s.strategy.as_str()) {
                return refuse(
                    c,
                    format!(
                        "lists strategy {} twice; one pack holds one plan per strategy",
                        s.strategy
                    ),
                );
            }
            digest(c, "plan_digest", &s.plan_digest)?;
            names(c, "hypotheses", &s.hypotheses)?;
            names(c, "universes", &s.universes)?;
            each_named(c, "dependencies", &s.dependencies)?;
            positive(c, "capacity", s.capacity)?;
            basis_points(c, "envelope.min_hit_rate_bp", s.envelope.min_hit_rate_bp)?;
            basis_points(c, "envelope.max_drawdown_bp", s.envelope.max_drawdown_bp)?;
            listed(c, "retirement_rules", &s.retirement_rules)?;
            for rule in &s.retirement_rules {
                match rule {
                    RetirementRule::EnvelopeBreached {
                        consecutive_passes: 0,
                    } => {
                        return refuse(
                            c,
                            "has an envelope-breach rule of zero passes; state at least one",
                        );
                    }
                    RetirementRule::HypothesisRefuted { hypothesis }
                        if !s.hypotheses.contains(hypothesis) =>
                    {
                        return refuse(
                            c,
                            format!(
                                "retires {} on hypothesis `{hypothesis}`, which the strategy \
                                 does not depend on; a rule that can never fire is not a rule",
                                s.strategy
                            ),
                        );
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
