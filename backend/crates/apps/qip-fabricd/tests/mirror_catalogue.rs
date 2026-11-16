//! R12-FABRIC-7142: the mirror catalogue lists only outcome and control
//! streams. Arbitrage state is on `qip-mesh` links, not the durable fabric.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_events::event_fabric::policy::QosClass;
use qip_fabricd::mirror::MirrorConfig;

fn names() -> Vec<(String, QosClass)> {
    vec![
        ("control.halts".to_string(), QosClass::P0Control),
        ("outcomes.fills".to_string(), QosClass::P1Outcomes),
    ]
}

/// Mutation (run, failed, restored): widen the `matches!` in
/// `MirrorConfig::outcome_and_control` to admit every class — the loop below
/// stops refusing and its `expect_err` fires.
#[test]
fn the_mirror_catalogue_admits_outcome_and_control_streams_and_refuses_every_other_class() {
    let region = || ["us-east4".to_string()];
    assert!(
        MirrorConfig::outcome_and_control(region(), names()).is_ok(),
        "premise: P0 and P1 streams are admitted"
    );
    for class in [
        QosClass::P2MarketJournal,
        QosClass::P3Research,
        QosClass::P4Telemetry,
    ] {
        let mut streams = names();
        streams.push(("arbitrage.state".to_string(), class));
        let refusal = MirrorConfig::outcome_and_control(region(), streams)
            .expect_err("a non-outcome, non-control stream entered the mirror catalogue");
        assert!(
            refusal.message().contains("`arbitrage.state`"),
            "{}",
            refusal.message()
        );
    }
}
