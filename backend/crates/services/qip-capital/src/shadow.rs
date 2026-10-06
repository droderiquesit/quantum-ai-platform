//! Counterfactual shadow portfolios for capital allocations.
//!
//! For every allocation decision, shadow portfolios hold alternate sizes,
//! hedges, timing, venues and rejected opportunities, so the realised choice
//! is scored against what it declined.
//!
//! A shadow is not a second allocation — it is a what-if: if the Capital Brain
//! had made a different decision at that instant, on the information it had then,
//! what would the outcome have been? Shadows are scored after resolution, when
//! actual fills are known, so a counterfactual size can be priced using the
//! actual market impact from the real fills.

use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::allocation::Allocation;

/// One counterfactual allocation scenario — an alternate size, hedge, timing or venue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowAllocation {
    /// The allocation that was actually made.
    pub actual: Allocation,
    /// The alternate scenario.
    pub alternate: AllocationVariant,
    /// When this shadow was created (at allocation time, not at scoring time).
    pub created_at: Timestamp,
    /// Scored outcome after resolution, if available.
    pub score: Option<ShadowScore>,
}

/// An allocation scenario that did not happen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationVariant {
    /// Same venue, different size.
    AlternateSize { notional: Decimal, reason: String },
    /// Same size, alternate venue (if permitted).
    AlternateVenue { venue_id: String, reason: String },
    /// Same size and venue, alternate hedge ratio.
    AlternateHedge { hedge_ratio: f64, reason: String },
    /// Allocation that was refused — a proposal given nothing.
    Refused { reason: String },
}

/// Score of a shadow allocation after actual fills are known.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowScore {
    /// When the shadow was scored (after actual fills happened).
    pub scored_at: Timestamp,
    /// Simulated outcome if the shadow had been taken.
    pub simulated_pnl: Decimal,
    /// The actual allocation's realized P&L (for comparison).
    pub actual_pnl: Decimal,
    /// Opportunity cost: what was foregone by not taking the shadow.
    pub regret_bp: u32,
}

/// The universe of shadow portfolios for one allocation decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShadowPortfolioUniverse {
    /// The allocation plan this shadow universe tracks.
    pub plan_at: Timestamp,
    /// Shadows organized by strategy.
    pub shadows: BTreeMap<String, Vec<ShadowAllocation>>,
}

impl ShadowPortfolioUniverse {
    /// Create a new shadow universe for an allocation plan.
    pub fn new(plan_at: Timestamp) -> Self {
        Self {
            plan_at,
            shadows: BTreeMap::new(),
        }
    }

    /// Add an alternate-size shadow for an allocation.
    pub fn with_alternate_size(
        mut self,
        actual: Allocation,
        alternate_notional: Decimal,
        reason: String,
        created_at: Timestamp,
    ) -> Result<Self> {
        if alternate_notional.is_zero() || alternate_notional.is_negative() {
            return Err(Error::invalid("alternate allocation size must be positive"));
        }

        let strategy_key = actual.strategy.to_string();
        let shadow = ShadowAllocation {
            actual,
            alternate: AllocationVariant::AlternateSize {
                notional: alternate_notional,
                reason,
            },
            created_at,
            score: None,
        };

        self.shadows.entry(strategy_key).or_default().push(shadow);

        Ok(self)
    }

    /// Add a rejected-opportunity shadow for a proposal that got nothing.
    pub fn with_refused(
        mut self,
        strategy_id: String,
        reason: String,
        created_at: Timestamp,
    ) -> Result<Self> {
        if strategy_id.is_empty() {
            return Err(Error::invalid("strategy id must not be empty"));
        }
        if reason.is_empty() {
            return Err(Error::invalid("refusal reason must not be empty"));
        }

        // For refused shadows, we create a minimal allocation record.
        let refused_allocation = Allocation {
            strategy: StrategyId::new(strategy_id.clone()),
            cell: "unknown".to_string(),
            venue: VenueId::new("unallocated"),
            notional: Decimal::ZERO,
            indicated: Decimal::ZERO,
            risk_adjusted_edge: 0.0,
            binding_constraints: vec![reason.clone()],
        };

        let shadow = ShadowAllocation {
            actual: refused_allocation,
            alternate: AllocationVariant::Refused { reason },
            created_at,
            score: None,
        };

        self.shadows.entry(strategy_id).or_default().push(shadow);

        Ok(self)
    }

    /// Score a shadow after actual fills are known.
    pub fn score_shadow(
        &mut self,
        strategy_key: &str,
        shadow_idx: usize,
        simulated_pnl: Decimal,
        actual_pnl: Decimal,
        scored_at: Timestamp,
    ) -> Result<()> {
        let shadows = self
            .shadows
            .get_mut(strategy_key)
            .ok_or_else(|| Error::not_found(format!("no shadows for strategy {strategy_key}")))?;

        let shadow = shadows
            .get_mut(shadow_idx)
            .ok_or_else(|| Error::not_found(format!("shadow index {shadow_idx} not found")))?;

        if shadow.score.is_some() {
            return Err(Error::invalid("shadow already scored"));
        }

        // Calculate regret in basis points (10,000 bp = 1.0)
        let regret_decimal = simulated_pnl - actual_pnl;
        let regret_bp = if regret_decimal.is_zero() {
            0
        } else if regret_decimal.is_positive() {
            (regret_decimal.to_f64() * 10_000.0).abs().min(10_000.0) as u32
        } else {
            0
        };

        shadow.score = Some(ShadowScore {
            scored_at,
            simulated_pnl,
            actual_pnl,
            regret_bp,
        });

        Ok(())
    }

    /// Get all shadows for a strategy.
    pub fn shadows_for_strategy(&self, strategy_key: &str) -> Vec<&ShadowAllocation> {
        self.shadows
            .get(strategy_key)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Count alternate-size shadows.
    pub fn alternate_size_shadows(&self) -> usize {
        self.shadows
            .values()
            .flat_map(|shadows| {
                shadows
                    .iter()
                    .filter(|s| matches!(s.alternate, AllocationVariant::AlternateSize { .. }))
            })
            .count()
    }

    /// Count refused-opportunity shadows.
    pub fn refused_shadows(&self) -> usize {
        self.shadows
            .values()
            .flat_map(|shadows| {
                shadows
                    .iter()
                    .filter(|s| matches!(s.alternate, AllocationVariant::Refused { .. }))
            })
            .count()
    }

    /// Whether this universe has at least one alternate-size shadow and one refused shadow.
    pub fn has_required_shadows(&self) -> bool {
        self.alternate_size_shadows() > 0 && self.refused_shadows() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_allocation(strategy_id: &str) -> Allocation {
        Allocation {
            strategy: StrategyId::new(strategy_id),
            cell: "CELL-A".to_string(),
            venue: VenueId::new("VENUE-1"),
            notional: Decimal::from_int(100_000),
            indicated: Decimal::from_int(120_000),
            risk_adjusted_edge: 0.015,
            binding_constraints: vec!["risk_limit".to_string()],
        }
    }

    #[test]
    fn a_shadow_universe_starts_empty() {
        let now = Timestamp::from_secs(1_000);
        let universe = ShadowPortfolioUniverse::new(now);
        assert_eq!(universe.plan_at, now);
        assert!(universe.shadows.is_empty());
    }

    #[test]
    fn a_shadow_universe_accepts_an_alternate_size_shadow() {
        let now = Timestamp::from_secs(1_000);
        let actual = make_allocation("STRATEGY-A");
        let universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual,
                Decimal::from_int(80_000),
                "smaller due to liquidity".to_string(),
                now,
            )
            .expect("add alternate size");

        assert_eq!(universe.alternate_size_shadows(), 1);
    }

    #[test]
    fn a_shadow_universe_refuses_zero_or_negative_alternate_size() {
        let now = Timestamp::from_secs(1_000);
        let actual = make_allocation("STRATEGY-A");

        let result = ShadowPortfolioUniverse::new(now).with_alternate_size(
            actual,
            Decimal::ZERO,
            "impossible".to_string(),
            now,
        );

        assert!(result.is_err(), "zero size should be refused");
    }

    #[test]
    fn a_shadow_universe_accepts_a_refused_shadow() {
        let now = Timestamp::from_secs(1_000);
        let universe = ShadowPortfolioUniverse::new(now)
            .with_refused("STRATEGY-B".to_string(), "no capacity".to_string(), now)
            .expect("add refused shadow");

        assert_eq!(universe.refused_shadows(), 1);
    }

    #[test]
    fn a_shadow_universe_refuses_empty_strategy_or_reason() {
        let now = Timestamp::from_secs(1_000);

        let result = ShadowPortfolioUniverse::new(now).with_refused(
            "".to_string(),
            "reason".to_string(),
            now,
        );
        assert!(result.is_err(), "empty strategy should be refused");

        let result = ShadowPortfolioUniverse::new(now).with_refused(
            "STRATEGY".to_string(),
            "".to_string(),
            now,
        );
        assert!(result.is_err(), "empty reason should be refused");
    }

    #[test]
    fn a_shadow_universe_can_score_a_shadow_after_resolution() {
        let now = Timestamp::from_secs(1_000);
        let later = Timestamp::from_secs(2_000);
        let actual = make_allocation("STRATEGY-A");

        let mut universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual,
                Decimal::from_int(80_000),
                "smaller".to_string(),
                now,
            )
            .expect("add alternate size");

        universe
            .score_shadow(
                "STRATEGY-A",
                0,
                Decimal::from_int(5_000), // simulated: 5k
                Decimal::from_int(4_000), // actual: 4k
                later,
            )
            .expect("score shadow");

        let shadows = universe.shadows_for_strategy("STRATEGY-A");
        assert_eq!(shadows.len(), 1);
        let score = shadows[0].score.as_ref().expect("score exists");
        assert_eq!(score.simulated_pnl, Decimal::from_int(5_000));
        assert_eq!(score.actual_pnl, Decimal::from_int(4_000));
        assert!(
            score.regret_bp > 0,
            "regret should be positive when alternative is better"
        );
    }

    #[test]
    fn a_shadow_universe_requires_both_alternate_size_and_refused_for_completeness() {
        let now = Timestamp::from_secs(1_000);
        let actual = make_allocation("STRATEGY-A");

        // Only alternate-size shadows
        let universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual.clone(),
                Decimal::from_int(80_000),
                "smaller".to_string(),
                now,
            )
            .expect("add alternate size");

        assert!(
            !universe.has_required_shadows(),
            "only one type is not enough"
        );

        // Add a refused shadow
        let universe = universe
            .with_refused("STRATEGY-B".to_string(), "no capacity".to_string(), now)
            .expect("add refused");

        assert!(
            universe.has_required_shadows(),
            "both types satisfy requirement"
        );
    }

    #[test]
    fn a_shadow_universe_refuses_to_score_the_same_shadow_twice() {
        let now = Timestamp::from_secs(1_000);
        let later = Timestamp::from_secs(2_000);
        let actual = make_allocation("STRATEGY-A");

        let mut universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual,
                Decimal::from_int(80_000),
                "smaller".to_string(),
                now,
            )
            .expect("add alternate size");

        universe
            .score_shadow(
                "STRATEGY-A",
                0,
                Decimal::from_int(5_000),
                Decimal::from_int(4_000),
                later,
            )
            .expect("first score succeeds");

        let result = universe.score_shadow(
            "STRATEGY-A",
            0,
            Decimal::from_int(3_000),
            Decimal::from_int(2_000),
            later,
        );
        assert!(result.is_err(), "second score should be refused");
    }

    #[test]
    fn a_shadow_universe_calculates_regret_correctly() {
        let now = Timestamp::from_secs(1_000);
        let later = Timestamp::from_secs(2_000);
        let actual = make_allocation("STRATEGY-A");

        let mut universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual,
                Decimal::from_int(80_000),
                "smaller".to_string(),
                now,
            )
            .expect("add alternate size");

        // Scenario: shadow earned more (positive regret)
        universe
            .score_shadow(
                "STRATEGY-A",
                0,
                Decimal::from_int(10_000), // shadow would have earned 10k
                Decimal::from_int(6_000),  // actual earned 6k
                later,
            )
            .expect("score shadow");

        let shadows = universe.shadows_for_strategy("STRATEGY-A");
        let score = shadows[0].score.as_ref().expect("score exists");
        assert!(score.regret_bp > 0, "positive regret when shadow is better");
        assert!(score.regret_bp <= 10_000, "regret capped at 10,000 bp");
    }

    #[test]
    fn a_shadow_universe_accepts_zero_regret_when_both_outcomes_equal() {
        let now = Timestamp::from_secs(1_000);
        let later = Timestamp::from_secs(2_000);
        let actual = make_allocation("STRATEGY-A");

        let mut universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual,
                Decimal::from_int(80_000),
                "smaller".to_string(),
                now,
            )
            .expect("add alternate size");

        universe
            .score_shadow(
                "STRATEGY-A",
                0,
                Decimal::from_int(5_000),
                Decimal::from_int(5_000),
                later,
            )
            .expect("score shadow");

        let shadows = universe.shadows_for_strategy("STRATEGY-A");
        let score = shadows[0].score.as_ref().expect("score exists");
        assert_eq!(score.regret_bp, 0, "no regret when outcomes are equal");
    }

    #[test]
    fn a_shadow_universe_shows_no_regret_when_actual_was_better() {
        let now = Timestamp::from_secs(1_000);
        let later = Timestamp::from_secs(2_000);
        let actual = make_allocation("STRATEGY-A");

        let mut universe = ShadowPortfolioUniverse::new(now)
            .with_alternate_size(
                actual,
                Decimal::from_int(80_000),
                "smaller".to_string(),
                now,
            )
            .expect("add alternate size");

        universe
            .score_shadow(
                "STRATEGY-A",
                0,
                Decimal::from_int(2_000), // shadow would have earned 2k
                Decimal::from_int(6_000), // actual earned 6k
                later,
            )
            .expect("score shadow");

        let shadows = universe.shadows_for_strategy("STRATEGY-A");
        let score = shadows[0].score.as_ref().expect("score exists");
        assert_eq!(
            score.regret_bp, 0,
            "no regret when actual choice was better"
        );
    }
}
