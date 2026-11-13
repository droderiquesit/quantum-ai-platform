//! The survival limit: the Risk Brain's `LimitKind::MaxSurvivalGross`,
//! installed on the ceiling the survival kernel computes.
//!
//! It lived in the survival kernel's own module until ADR 0464. The survival
//! kernel is the Capital Brain's (`qip-survival`), the limit is the Risk
//! Brain's (`qip-risk`), and ADR 0127 decision 5 lets only a composition —
//! this kernel, or an app — hold two brains. So the kernel, which meets
//! them, installs it; `qip_kernel::survival::install_survival_limit` still
//! names it, re-exported beside the survival kernel's own items.

use qip_core::error::{Error, Result};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_survival::survival::{SURVIVAL_LIMIT_BOUND, SURVIVAL_LIMIT_NAME};

/// Put `LimitKind::MaxSurvivalGross` into `limits` unless a survival limit is
/// already there.
///
/// The limit existed, evaluated deterministically, and was in no limit set
/// any process ran: `LimitSet::conservative_default` does not carry it, so
/// the survival ceiling the kernel computed every construction was a figure
/// in a `RiskState` that nothing compared an order against (R12-RISK-7205).
/// It is installed here — by the kernel, on whatever set the root hands in —
/// rather than in the shipped default, because the ceiling it reads is a
/// figure only this kernel produces: an edge gate or a library test holding
/// the shipped set has no survival kernel behind it, and would refuse every
/// order on the absent ceiling.
///
/// A bound of one: gross may not exceed the ceiling. Refuses a set that
/// already carries a *different* limit under [`SURVIVAL_LIMIT_NAME`], because
/// adding a second limit of that name would leave a rule review and a breach
/// report unable to tell the two apart.
pub fn install_survival_limit(limits: LimitSet) -> Result<LimitSet> {
    // A survival limit already in the set is kept only if it is the one this
    // function would install. This used to return early on *any*
    // `MaxSurvivalGross`, whatever its bound: a handed-in set carrying one
    // with a bound of 1000 left gross free to reach a thousand times the
    // ceiling the survival kernel computed, and the kernel installed nothing
    // because a limit of the right kind was "already there" — a ceiling that
    // cannot fire, wearing the name of one that can. Refused rather than
    // replaced, so the operator who wrote that bound learns it was not used.
    let mut carried = false;
    for limit in &limits.limits {
        if let LimitKind::MaxSurvivalGross { limit: bound } = limit.kind {
            if bound != SURVIVAL_LIMIT_BOUND {
                return Err(Error::invalid(format!(
                    "the limit set {} carries a survival-gross limit `{}` with a bound of \
                     {bound}; the kernel's survival ceiling is enforced at \
                     {SURVIVAL_LIMIT_BOUND} of what the survival kernel released, and a set \
                     that states another bound is refused rather than trusted or silently \
                     replaced — remove it, and the kernel installs its own",
                    limits.name, limit.name
                )));
            }
            carried = true;
        }
    }
    if carried {
        return Ok(limits);
    }
    if limits
        .limits
        .iter()
        .any(|limit| limit.name == SURVIVAL_LIMIT_NAME)
    {
        return Err(Error::invalid(format!(
            "the limit set {} already names a limit `{SURVIVAL_LIMIT_NAME}` that is not the \
             survival ceiling; rename it, because the kernel installs its survival limit under \
             that name",
            limits.name
        )));
    }
    Ok(limits.with(
        Limit::new(
            SURVIVAL_LIMIT_NAME,
            LimitKind::MaxSurvivalGross {
                limit: SURVIVAL_LIMIT_BOUND,
            },
        )
        .with_rationale(
            "the capital survival kernel stands behind no more gross than the book holds, \
                 what is already held for approved proposals, the hedge capacity it withheld and \
                 what its last construction released; an order past that is one the waterfall \
                 never funded",
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::regime::MarketRegime;
    use qip_core::{Decimal, Timestamp, dec};
    use qip_survival::confidence::ThesisView;
    use qip_survival::survival::{LedgerFigures, SurvivalGovernor};

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn figures(deployable: Decimal) -> LedgerFigures {
        LedgerFigures {
            equity: dec!("1000000"),
            held: dec!("100000"),
            obligations: dec!("50000"),
            deployable,
            gross: dec!("400000"),
            ..LedgerFigures::default()
        }
    }

    fn views() -> Vec<ThesisView> {
        vec![ThesisView {
            hypothesis_id: "h".into(),
            object_id: "obj-AAA".into(),
            regime: MarketRegime::Trending,
            reliability: None,
        }]
    }

    /// R12-RISK-7205: the survival limit is installed once, on any set, and a
    /// set that already names something else `survival-gross` is refused.
    /// Mutation: return `Ok(limits)` unchanged from `install_survival_limit`
    /// and the first assertion fails.
    #[test]
    fn the_survival_limit_is_installed_once_and_a_name_collision_is_refused() {
        let shipped = LimitSet::conservative_default();
        assert!(
            !shipped
                .limits
                .iter()
                .any(|limit| matches!(limit.kind, LimitKind::MaxSurvivalGross { .. })),
            "premise: the shipped set does not carry the survival limit"
        );
        let installed = install_survival_limit(shipped.clone()).unwrap();
        assert_eq!(installed.len(), shipped.len() + 1);
        let again = install_survival_limit(installed.clone()).unwrap();
        assert_eq!(again.len(), installed.len(), "installed twice");
        let clash = LimitSet::new("clash").with(Limit::new(
            SURVIVAL_LIMIT_NAME,
            LimitKind::MaxLeverage { limit: 1.0 },
        ));
        assert!(install_survival_limit(clash).is_err());
    }

    /// A handed-in survival limit with any bound but the kernel's own is
    /// refused, naming it. Until round 3 any `MaxSurvivalGross` short-
    /// circuited installation, so a set carrying one bound at 1000 made the
    /// ceiling unable to fire.
    /// Mutation: restore the early return on any `MaxSurvivalGross` and the
    /// loose set is admitted.
    #[test]
    fn a_handed_in_survival_limit_with_another_bound_is_refused_rather_than_trusted() {
        let loose = LimitSet::new("handed-in").with(Limit::new(
            "operator-survival",
            LimitKind::MaxSurvivalGross { limit: 1000.0 },
        ));
        let error = install_survival_limit(loose).expect_err("a bound of 1000 cannot fire");
        assert!(
            error
                .message()
                .contains("`operator-survival` with a bound of 1000"),
            "the refusal names the limit and its bound: {}",
            error.message()
        );
        // Premise and contrast: the kernel's own bound under another name is
        // kept, and installed only once.
        let own = LimitSet::new("handed-in").with(Limit::new(
            "operator-survival",
            LimitKind::MaxSurvivalGross {
                limit: SURVIVAL_LIMIT_BOUND,
            },
        ));
        let kept = install_survival_limit(own.clone()).unwrap();
        assert_eq!(kept.len(), own.len());
    }

    /// The limit the kernel installs vetoes an order that carries gross past
    /// the ceiling the governor computed, and admits one inside it — so a
    /// survival kernel that releases less is a gate that admits less.
    /// Mutation: write `None` for the ceiling and both orders are refused;
    /// remove the installed limit and the large one passes.
    #[test]
    fn an_order_past_the_survival_ceiling_is_vetoed_by_the_installed_limit() {
        let mut governor = SurvivalGovernor::default();
        governor
            .size(figures(dec!("850000")), None, 0.20, &views(), at())
            .unwrap();
        let ceiling = governor.gross_ceiling().unwrap();
        let limits = install_survival_limit(LimitSet::new("survival-only")).unwrap();
        let projected = |gross: Decimal| qip_risk::limits::RiskState {
            equity: dec!("1000000"),
            gross_exposure: gross,
            gross_before_order: Some(dec!("400000")),
            survival_gross_ceiling: Some(ceiling),
            ..qip_risk::limits::RiskState::default()
        };
        assert!(
            !limits.check(&projected(ceiling)).is_blocked(),
            "an order to the ceiling is admitted"
        );
        assert!(
            limits.check(&projected(ceiling + dec!("1"))).is_blocked(),
            "an order past the ceiling is vetoed"
        );
    }
}
