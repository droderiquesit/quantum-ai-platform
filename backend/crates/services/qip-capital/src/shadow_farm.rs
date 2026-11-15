//! The shadow-portfolio farm: a plan's counterfactuals, scored exactly into a
//! compact outcome pack (R12-CAPITAL-9051, ADR 0653).
//!
//! [`crate::shadow`] defines what a shadow is. This module is the batch that
//! uses it: build the universe a plan implies, score every shadow once the
//! realised result is known, and seal the scores as a pack whose digest a
//! reader can recompute. It is deliberately separate from
//! `ShadowPortfolioUniverse::score_shadow`, which converts money through
//! `f64` and clamps regret at 10,000 basis points; a clamped regret reads a
//! miss of ten times the position the same as a miss of one, which is the
//! silent correction principle 2 forbids. Here every figure is a `Decimal`
//! and an overflow or an undefined ratio is refused.

use crate::shadow::{AllocationVariant, ShadowPortfolioUniverse};
use qip_contracts::allocation::AllocationPlan;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp, sha256_hex};
use std::collections::BTreeSet;

const BASIS_POINTS: i64 = 10_000;

/// The shadows a plan implies: for each allocation with a positive notional,
/// one alternate-size shadow per `size_factors` entry, and one refused shadow
/// per proposal the plan gave nothing.
///
/// The factors are the caller's; this module chooses none. Each must be
/// positive and not one (a factor of one is the actual choice, not an
/// alternative), the list must be non-empty and free of duplicates, and an
/// alternate notional that overflows is refused rather than wrapped. An
/// allocation of zero has no alternate size and gets none.
pub fn universe_from_plan(
    plan: &AllocationPlan,
    size_factors: &[Decimal],
) -> Result<ShadowPortfolioUniverse> {
    if size_factors.is_empty() {
        return Err(Error::invalid(
            "a shadow farm needs at least one alternate size factor; none was given and the \
             farm chooses none",
        ));
    }
    let mut seen = BTreeSet::new();
    for factor in size_factors {
        if !factor.is_positive() || *factor == Decimal::from_int(1) {
            return Err(Error::invalid(format!(
                "alternate size factor {factor} must be positive and not one"
            )));
        }
        if !seen.insert(factor.raw()) {
            return Err(Error::invalid(format!(
                "alternate size factor {factor} is listed twice"
            )));
        }
    }
    let mut universe = ShadowPortfolioUniverse::new(plan.at);
    for allocation in &plan.allocations {
        if !allocation.notional.is_positive() {
            continue;
        }
        for factor in size_factors {
            let alternate = allocation.notional.checked_mul(*factor).ok_or_else(|| {
                Error::numeric(format!(
                    "{} x {factor} overflows for {}; the shadow is refused, not wrapped",
                    allocation.notional, allocation.strategy
                ))
            })?;
            universe = universe.with_alternate_size(
                allocation.clone(),
                alternate,
                format!("size x{factor}"),
                plan.at,
            )?;
        }
    }
    for (strategy, reason) in &plan.refusals {
        universe = universe.with_refused(strategy.to_string(), reason.clone(), plan.at)?;
    }
    Ok(universe)
}

/// The hypothetical P&L of a different size, from the realised fills' own
/// P&L scaled by notional: `actual_pnl * alternate / actual`.
///
/// **Provisional model:** linear in size, so it carries no market impact. A
/// larger size would in fact have moved the price against itself, so this
/// flatters bigger shadows; ADR 0653 records that and the pack says which
/// model scored it. Refused when the actual notional is not positive.
pub fn linear_pnl(
    actual_pnl: Decimal,
    actual_notional: Decimal,
    alternate_notional: Decimal,
) -> Result<Decimal> {
    if !actual_notional.is_positive() {
        return Err(Error::invalid(
            "a size shadow is scored against a positive actual notional; a refused proposal \
             has none and needs its own hypothetical P&L",
        ));
    }
    actual_pnl
        .checked_mul(alternate_notional)
        .and_then(|scaled| scaled.checked_div(actual_notional))
        .ok_or_else(|| Error::numeric("the linear shadow P&L overflows; nothing was scored"))
}

/// What the resolved outcome says about one shadow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub strategy: String,
    /// The shadow's index under its strategy, as the universe holds it.
    pub index: usize,
    /// What the shadow would have made.
    pub simulated_pnl: Decimal,
    /// What the realised allocation made.
    pub actual_pnl: Decimal,
}

/// One scored shadow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutcomeRow {
    pub strategy: String,
    pub index: usize,
    /// `alternate_size`, `alternate_venue`, `alternate_hedge` or `refused`.
    pub kind: &'static str,
    /// The alternate notional, for an alternate size.
    pub alternate_notional: Option<Decimal>,
    pub actual_notional: Decimal,
    pub simulated_pnl: Decimal,
    pub actual_pnl: Decimal,
    /// `simulated - actual`: positive is what the declined choice would have
    /// added, negative is what the realised choice saved. Never clamped.
    pub regret: Decimal,
    /// `regret` as basis points of the actual notional, exact and unclamped;
    /// absent where there is no actual notional to state it against.
    pub regret_bp: Option<Decimal>,
}

/// The compact, sealed result of scoring one plan's shadows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutcomePack {
    pub plan_at: Timestamp,
    pub resolved_at: Timestamp,
    /// The model that priced the alternates, so a reader knows what the
    /// regret assumes.
    pub model: String,
    pub rows: Vec<OutcomeRow>,
    /// SHA-256 over the pack's canonical text, so an edited row is found.
    pub digest: String,
}

impl OutcomePack {
    fn canonical(
        plan_at: Timestamp,
        resolved_at: Timestamp,
        model: &str,
        rows: &[OutcomeRow],
    ) -> String {
        let mut text = format!("qip.outcome-pack.v1|{plan_at:?}|{resolved_at:?}|{model}\n");
        for row in rows {
            text.push_str(&format!(
                "{}|{}|{}|{:?}|{}|{}|{}|{}|{:?}\n",
                row.strategy,
                row.index,
                row.kind,
                row.alternate_notional,
                row.actual_notional,
                row.simulated_pnl,
                row.actual_pnl,
                row.regret,
                row.regret_bp
            ));
        }
        text
    }

    /// Whether the stored digest is the digest of the stored rows.
    pub fn verify(&self) -> bool {
        self.digest
            == sha256_hex(
                Self::canonical(self.plan_at, self.resolved_at, &self.model, &self.rows).as_bytes(),
            )
    }

    /// The canonical text the digest covers, for durable storage.
    pub fn to_canonical_text(&self) -> String {
        Self::canonical(self.plan_at, self.resolved_at, &self.model, &self.rows)
    }
}

/// Score every shadow of `universe` from `resolutions` and seal the pack.
///
/// Refuses unless every shadow has exactly one resolution and no resolution
/// names a shadow that does not exist: "each allocation has shadow
/// alternatives scored" is the requirement, and a pack that silently omitted
/// the shadows nobody resolved would read as complete. Arithmetic that
/// overflows is refused, not saturated.
pub fn resolve(
    universe: &ShadowPortfolioUniverse,
    resolutions: &[Resolution],
    model: &str,
    resolved_at: Timestamp,
) -> Result<OutcomePack> {
    if model.trim().is_empty() {
        return Err(Error::invalid(
            "an outcome pack names the model that priced its alternates",
        ));
    }
    let mut used = BTreeSet::new();
    for r in resolutions {
        let exists = universe
            .shadows
            .get(&r.strategy)
            .is_some_and(|shadows| r.index < shadows.len());
        if !exists {
            return Err(Error::not_found(format!(
                "resolution names shadow {} of {}, which the universe does not hold",
                r.index, r.strategy
            )));
        }
        if !used.insert((r.strategy.clone(), r.index)) {
            return Err(Error::invalid(format!(
                "shadow {} of {} is resolved twice",
                r.index, r.strategy
            )));
        }
    }
    let mut rows = Vec::new();
    for (strategy, shadows) in &universe.shadows {
        for (index, shadow) in shadows.iter().enumerate() {
            let resolution = resolutions
                .iter()
                .find(|r| &r.strategy == strategy && r.index == index)
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "shadow {index} of {strategy} has no resolution; a pack is sealed only \
                         when every alternative is scored"
                    ))
                })?;
            let regret = resolution
                .simulated_pnl
                .checked_sub(resolution.actual_pnl)
                .ok_or_else(|| Error::numeric("regret overflows; nothing was scored"))?;
            let actual_notional = shadow.actual.notional;
            let regret_bp = if actual_notional.is_positive() {
                Some(
                    regret
                        .checked_mul(Decimal::from_int(BASIS_POINTS))
                        .and_then(|scaled| scaled.checked_div(actual_notional))
                        .ok_or_else(|| Error::numeric("regret in basis points overflows"))?,
                )
            } else {
                None
            };
            let (kind, alternate_notional) = match &shadow.alternate {
                AllocationVariant::AlternateSize { notional, .. } => {
                    ("alternate_size", Some(*notional))
                }
                AllocationVariant::AlternateVenue { .. } => ("alternate_venue", None),
                AllocationVariant::AlternateHedge { .. } => ("alternate_hedge", None),
                AllocationVariant::Refused { .. } => ("refused", None),
            };
            rows.push(OutcomeRow {
                strategy: strategy.clone(),
                index,
                kind,
                alternate_notional,
                actual_notional,
                simulated_pnl: resolution.simulated_pnl,
                actual_pnl: resolution.actual_pnl,
                regret,
                regret_bp,
            });
        }
    }
    let digest =
        sha256_hex(OutcomePack::canonical(universe.plan_at, resolved_at, model, &rows).as_bytes());
    Ok(OutcomePack {
        plan_at: universe.plan_at,
        resolved_at,
        model: model.to_string(),
        rows,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::allocation::Allocation;
    use qip_contracts::signal::StrategyId;
    use qip_contracts::venue::VenueId;

    fn plan() -> AllocationPlan {
        AllocationPlan {
            at: Timestamp::from_secs(1_000),
            total_budget: Decimal::from_int(200_000),
            drawdown: 0.0,
            drawdown_multiplier: Decimal::from_int(1),
            budget: Decimal::from_int(200_000),
            allocations: vec![Allocation {
                strategy: StrategyId::new("momentum"),
                cell: "CELL-A".to_string(),
                venue: VenueId::new("VENUE-1"),
                notional: Decimal::from_int(100_000),
                indicated: Decimal::from_int(120_000),
                risk_adjusted_edge: 0.01,
                binding_constraints: vec![],
            }],
            refusals: vec![(StrategyId::new("carry"), "no capacity".to_string())],
        }
    }

    fn factors() -> Vec<Decimal> {
        vec![Decimal::parse("0.5").unwrap(), Decimal::from_int(2)]
    }

    fn resolutions(universe: &ShadowPortfolioUniverse) -> Vec<Resolution> {
        let mut out = Vec::new();
        for (strategy, shadows) in &universe.shadows {
            for (index, shadow) in shadows.iter().enumerate() {
                let actual = Decimal::from_int(4_000);
                let simulated = match &shadow.alternate {
                    AllocationVariant::AlternateSize { notional, .. } => {
                        linear_pnl(actual, shadow.actual.notional, *notional).unwrap()
                    }
                    _ => Decimal::from_int(300),
                };
                out.push(Resolution {
                    strategy: strategy.clone(),
                    index,
                    simulated_pnl: simulated,
                    actual_pnl: if shadow.actual.notional.is_positive() {
                        actual
                    } else {
                        Decimal::ZERO
                    },
                });
            }
        }
        out
    }

    #[test]
    fn a_plan_yields_one_size_shadow_per_factor_and_one_refused_shadow_per_refusal() {
        let universe = universe_from_plan(&plan(), &factors()).unwrap();
        assert_eq!(universe.alternate_size_shadows(), 2);
        assert_eq!(universe.refused_shadows(), 1);
        assert!(universe.has_required_shadows());
    }

    #[test]
    fn a_farm_with_no_factor_a_factor_of_one_or_a_repeat_is_refused() {
        assert!(universe_from_plan(&plan(), &[]).is_err());
        assert!(universe_from_plan(&plan(), &[Decimal::from_int(1)]).is_err());
        assert!(universe_from_plan(&plan(), &[Decimal::ZERO]).is_err());
        let two = Decimal::from_int(2);
        assert!(universe_from_plan(&plan(), &[two, two]).is_err());
    }

    #[test]
    fn regret_far_past_ten_thousand_basis_points_is_stated_not_clamped() {
        // The failure this prevents: score_shadow's `.min(10_000.0)` reading a
        // miss of ten times the position the same as a miss of the whole of it.
        let universe = universe_from_plan(&plan(), &factors()).unwrap();
        let mut all = resolutions(&universe);
        let momentum = all
            .iter_mut()
            .find(|r| r.strategy == "momentum" && r.index == 0)
            .unwrap();
        momentum.simulated_pnl = Decimal::from_int(1_004_000);
        momentum.actual_pnl = Decimal::from_int(4_000);
        let pack = resolve(
            &universe,
            &all,
            "linear-no-impact",
            Timestamp::from_secs(2_000),
        )
        .unwrap();
        let row = pack
            .rows
            .iter()
            .find(|r| r.strategy == "momentum" && r.index == 0)
            .unwrap();
        assert_eq!(row.regret, Decimal::from_int(1_000_000));
        assert_eq!(
            row.regret_bp,
            Some(Decimal::from_int(100_000)),
            "ten thousand percent of the position, not 10,000 bp"
        );
    }

    #[test]
    fn a_pack_is_sealed_only_when_every_shadow_is_resolved_and_names_only_real_shadows() {
        let universe = universe_from_plan(&plan(), &factors()).unwrap();
        let mut all = resolutions(&universe);
        let spare = all.pop().unwrap();
        assert!(resolve(&universe, &all, "m", Timestamp::from_secs(2)).is_err());
        all.push(spare.clone());
        all.push(spare);
        assert!(resolve(&universe, &all, "m", Timestamp::from_secs(2)).is_err());
        all.pop();
        all.push(Resolution {
            strategy: "ghost".to_string(),
            index: 0,
            simulated_pnl: Decimal::ZERO,
            actual_pnl: Decimal::ZERO,
        });
        assert!(resolve(&universe, &all, "m", Timestamp::from_secs(2)).is_err());
    }

    #[test]
    fn an_outcome_pack_digest_verifies_and_an_edited_row_is_found() {
        let universe = universe_from_plan(&plan(), &factors()).unwrap();
        let all = resolutions(&universe);
        let mut pack = resolve(
            &universe,
            &all,
            "linear-no-impact",
            Timestamp::from_secs(2_000),
        )
        .unwrap();
        assert_eq!(pack.rows.len(), 3, "premise: two sizes and one refusal");
        assert!(pack.verify());
        pack.rows[0].regret += Decimal::from_int(1);
        assert!(!pack.verify());
    }

    #[test]
    fn linear_pnl_scales_by_notional_and_refuses_a_zero_base() {
        let scaled = linear_pnl(
            Decimal::from_int(4_000),
            Decimal::from_int(100_000),
            Decimal::from_int(50_000),
        )
        .unwrap();
        assert_eq!(scaled, Decimal::from_int(2_000));
        assert!(linear_pnl(Decimal::from_int(1), Decimal::ZERO, Decimal::from_int(1)).is_err());
    }
}
