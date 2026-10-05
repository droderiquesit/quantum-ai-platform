//! MESH-033: drift a cycle leaves behind becomes a rebalancing task.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used)]

use qip_core::{Decimal, Duration};
use qip_mesh::peer::{EpochDraft, EpochLeg, OpportunityEpoch};
use qip_mesh::rebalance::rebalancing_tasks;
use std::collections::BTreeMap;

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}
fn epoch() -> OpportunityEpoch {
    let leg = |id: &str, node: &str| EpochLeg {
        leg: id.into(),
        node: node.into(),
        size: d(100),
        unwind_loss: d(1),
    };
    EpochDraft {
        opportunity: Some("opp-9".into()),
        sequence: Some(4),
        cycle: Some("c".into()),
        legs: Some(vec![leg("a", "americas"), leg("b", "europe")]),
        size: Some(d(100)),
        min_edge_bps: Some(d(5)),
        model_versions: Some(vec!["m".into()]),
        opened_at_nanos: Some(1),
        ttl: Some(Duration::from_millis(100)),
        max_recovery_loss: Some(d(5)),
    }
    .build(Duration::from_millis(500))
    .unwrap()
}
fn book(rows: &[(&str, i64)]) -> BTreeMap<String, Decimal> {
    rows.iter().map(|(k, v)| (k.to_string(), d(*v))).collect()
}

#[test]
fn drift_beyond_tolerance_becomes_one_task_per_place_citing_the_epoch() {
    let target = book(&[("eu/x", 100), ("us/y", 100), ("ap/z", 50)]);
    let actual = book(&[("eu/x", 140), ("us/y", 98), ("us/new", 7)]);
    let tasks = rebalancing_tasks(&epoch(), &target, &actual, d(5)).unwrap();
    let got: Vec<(&str, Decimal)> = tasks.iter().map(|t| (t.place.as_str(), t.amount)).collect();
    // eu/x over by 40, ap/z wholly unfilled (-50), us/new unplanned (+7);
    // us/y at -2 is inside the tolerance and raises nothing.
    assert_eq!(
        got,
        vec![("ap/z", d(-50)), ("eu/x", d(40)), ("us/new", d(7))]
    );
    assert!(
        tasks
            .iter()
            .all(|t| t.opportunity == "opp-9" && t.epoch_sequence == 4)
    );
}

#[test]
fn a_cycle_that_leaves_no_drift_raises_nothing_and_a_negative_tolerance_is_refused() {
    let both = book(&[("eu/x", 100)]);
    assert!(
        rebalancing_tasks(&epoch(), &both, &both, d(0))
            .unwrap()
            .is_empty()
    );
    assert!(rebalancing_tasks(&epoch(), &both, &both, d(-1)).is_err());
}
