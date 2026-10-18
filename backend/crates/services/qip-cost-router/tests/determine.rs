//! `Router::determine`: the entry point a pre-trade check routes through.
//!
//! R12-RISK-3022. `Router::select` leaves `Determinism` to a public field, so a
//! guard written against it is only as strong as the caller's spelling.
//! `determine` removes the field from the caller's hands.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_cost_router::{
    Conditions, Horizon, IntelligenceTier, MarketRegime, Region, Router, VolatilityRegime,
};
use qip_financial::asset_class::AssetClass;

fn conditions(regime: MarketRegime, horizon: Horizon) -> Conditions {
    Conditions::new(
        AssetClass::Equity,
        Region::new("home"),
        regime,
        VolatilityRegime::Extreme,
        horizon,
    )
}

#[test]
fn a_determined_routing_is_deterministic_code_in_every_regime_and_horizon() -> Result<()> {
    let router = Router::default();
    // Premise: the sweep is not empty.
    assert!(!MarketRegime::ALL.is_empty() && !Horizon::ALL.is_empty());
    for regime in MarketRegime::ALL {
        for horizon in Horizon::ALL {
            let routing =
                router.determine("pre-trade limits for obj-AAA", conditions(regime, horizon))?;
            let charge = routing.charge();
            assert_eq!(charge.tier, IntelligenceTier::DeterministicCode);
            assert_eq!(charge.tier.model_tier(), None, "no model rung may be named");
            assert_eq!(routing.conditions(), &conditions(regime, horizon));
            assert!(routing.rationale().contains("deterministic code"));
        }
    }
    Ok(())
}

#[test]
fn a_determined_routing_with_no_subject_is_refused_not_defaulted() {
    let router = Router::default();
    for blank in ["", "   "] {
        let error = router
            .determine(blank, conditions(MarketRegime::Quiet, Horizon::Microsecond))
            .expect_err("an unrecorded routing decision is not auditable");
        assert!(
            error.message().contains("no subject"),
            "{}",
            error.message()
        );
    }
}
