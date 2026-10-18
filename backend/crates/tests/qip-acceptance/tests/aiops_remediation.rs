//! The deep brain's AIOps loop from anomaly to effect (R12-COVERAGE-7166):
//! a level shift in the node's own iteration time opens an incident, the one
//! committed runbook names a step, and the `CapabilitySwitch` effector flips
//! that capability and nothing else.
//!
//! The library pieces were each tested alone. What nothing asserted was the
//! composition — that an anomaly reaches a runbook step through the loop a
//! binary runs, and that the step cannot reach past the node's own switch.

use qip_core::{Context, Duration, Timestamp};
use qip_deepbrain::operations::{
    CAPABILITIES, CapabilitySwitch, IterationFacts, Operations, runbooks,
};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_observability::aiops::{Remediator, Runbook};
use qip_risk::limits::LimitSet;
use std::collections::BTreeSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("the platform assembles: {e}"))
}

fn facts(at: Timestamp, ms: f64) -> IterationFacts<'static> {
    IterationFacts {
        at,
        cycle: 1,
        cycle_started_at: at,
        iteration_ms: ms,
        problems: 0,
        anomalies: None,
        drifted: &[],
        package_promoted: false,
    }
}

#[test]
fn an_iteration_time_anomaly_reaches_a_runbook_step_that_flips_only_this_nodes_research_switch() {
    let platform = platform();
    let mut ops = Operations::new(start()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        CAPABILITIES,
        ["research"],
        "premise: research is the node's only switchable capability"
    );
    assert!(!ops.is_disabled("research"), "premise: research starts on");

    let mut incident = None;
    for i in 0..24i64 {
        let ms = if i < 12 {
            10.0 + (i % 2) as f64
        } else {
            1_000.0
        };
        let at = start().saturating_add(Duration::from_secs(i));
        let round = ops
            .observe(&platform, &facts(at, ms))
            .unwrap_or_else(|e| panic!("{e}"));
        if round.incident.is_some() {
            incident = round.incident;
        }
    }
    let incident = incident.unwrap_or_else(|| panic!("the shift must open an incident"));
    assert!(incident.remediation.is_ok(), "{incident:?}");

    assert!(ops.is_disabled("research"), "the runbook step took effect");
    for other in ["execution-engine", "qip-api", "venue", "ledger"] {
        assert!(!ops.is_disabled(other), "{other} is not the node's switch");
    }
    let audit = ops.remediator().audit().records();
    assert_eq!(audit.len(), 1, "one step, one record: {audit:?}");
    assert_eq!(audit[0].action, "disable_capability");
    assert_eq!(audit[0].target, "research");
    assert_eq!(audit[0].runbook, "shed-research-on-iteration-shift");
}

/// Mutation: let `CapabilitySwitch::apply` accept any target (or add the
/// foreign name to `CAPABILITIES`) and the effector flips it, failing this.
#[test]
fn a_runbook_that_names_a_foreign_target_is_audited_as_failed_and_flips_nothing() {
    let mut with_foreign = runbooks();
    with_foreign.push(Runbook {
        name: "restart-the-execution-engine".to_string(),
        action: "disable_capability".to_string(),
        targets: BTreeSet::from(["execution-engine".to_string()]),
    });
    let mut remediator = Remediator::new(with_foreign, CapabilitySwitch::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !remediator.effector().is_disabled("research"),
        "premise: nothing is switched off"
    );

    let refusal = remediator
        .remediate("INC-foreign", "disable_capability", "execution-engine")
        .err()
        .unwrap_or_else(|| panic!("the effector must refuse a foreign target"));

    assert!(
        refusal
            .message()
            .contains("not a capability this node can switch off"),
        "{}",
        refusal.message()
    );
    assert!(!remediator.effector().is_disabled("research"));
    assert!(!remediator.effector().is_disabled("execution-engine"));
    let audit = remediator.audit().records();
    assert_eq!(audit.len(), 1);
    assert!(audit[0].result.starts_with("failed:"), "{:?}", audit[0]);
    assert!(remediator.audit().verify());
}
