//! R12-QUANT-4028 at the composition seam: the router the kernel builds is
//! holding quantum answers to the Foundry's registered promotion threshold.
//!
//! The engine's own tests prove the arithmetic with a router assembled by
//! hand. This one boots a `Platform` the way the composition roots do
//! (`PlatformConfig::with_quantum()`, so `Platform::new` runs
//! `ComputeRouter::classical(..).with_policy(..)?.with_quantum(SimulatedProvider)`
//! with no foundry supplied) and asks the live router which margin it reads.
//! The routing policy margin is set to zero, which `RoutingPolicy::validate`
//! admits, so the only source of a non-zero bar is the Foundry.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Context, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_optimization_engine::router::RoutingPolicy;
use qip_quantum::foundry::{FamilyKind, FamilyStatus, Foundry};
use qip_risk::limits::LimitSet;

/// Mutation: in `ComputeRouter::margin_for`, return
/// `Some(self.policy.quantum_margin)` for every solver (stop reading the
/// foundry) — the router reports `Some(0.0)` and the equality fires. In
/// `ComputeRouter::with_quantum`, drop `self.foundry = Some(foundry)` — the
/// router reads no foundry, reports `None`, and the premise fires.
#[test]
fn the_kernels_router_reads_the_foundrys_registered_threshold_not_the_policy_zero() -> Result<()> {
    let registered = match Foundry::platform()?.status(FamilyKind::PortfolioAllocation) {
        Some(FamilyStatus::Registered(family)) => family.promotion_threshold(),
        other => panic!("the platform foundry does not register portfolio allocation: {other:?}"),
    };
    // Premise: a positive threshold exists to be read.
    assert!(registered > 0.0);

    let mut config = PlatformConfig::default().with_quantum();
    config.routing = RoutingPolicy {
        quantum_margin: 0.0,
        ..config.routing
    };
    let (context, _clock) =
        Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let router = platform.compute_router();

    // Premises: the arm is attached and the policy really is zero.
    assert!(router.quantum_gateway().is_some());
    assert_eq!(router.policy().quantum_margin.to_bits(), 0.0f64.to_bits());

    assert_eq!(router.quantum_promotion_margin()?, Some(registered));
    Ok(())
}
