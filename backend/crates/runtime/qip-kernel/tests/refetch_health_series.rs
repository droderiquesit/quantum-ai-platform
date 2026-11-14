//! `Platform::record_refetch_health` feeds the `data-refetch-health`
//! objective (R12-OBS-9163).
//!
//! The failure prevented: the objective was declared and the health gauge was
//! recorded, but a gauge carries no good/total split, so the objective read
//! "unmeasured" however many extents a source withdrew.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_observability::critical_paths::evaluate;
use qip_risk::limits::LimitSet;

const OBJECTIVE: &str = "data-refetch-health";

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(1_000_000));
    let (context, _clock) =
        Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

#[test]
fn a_refetch_pass_moves_the_refetch_objective_and_an_unrun_pass_reads_unmeasured() -> Result<()> {
    let platform = platform()?;
    let objective = |platform: &Platform| {
        evaluate(&platform.telemetry().metrics.snapshot())
            .into_iter()
            .find(|status| status.slo.name == OBJECTIVE)
            .ok_or_else(|| Error::not_found("data-refetch-health is not in the catalogue"))
    };

    let before = objective(&platform)?;
    assert!(!before.is_observed(), "premise: no pass has run");

    platform.record_refetch_health("alpha", 4, 1)?;
    let after = objective(&platform)?;
    assert!(after.is_observed());
    assert_eq!(after.observations, 4);
    assert!((after.achieved - 0.75).abs() < 1e-12, "{after:?}");
    assert!(!after.is_met, "a quarter withdrawn cannot meet 99%");

    platform.record_refetch_health("alpha", 0, 0)?;
    assert_eq!(objective(&platform)?.observations, 4, "an empty pass adds nothing");
    assert!(platform.record_refetch_health("alpha", 1, 2).is_err());
    assert_eq!(objective(&platform)?.observations, 4, "a refused pass adds nothing");
    Ok(())
}
