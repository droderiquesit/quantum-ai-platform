//! R12-OBS-7115: the deployment entity key is built from configuration.

use qip_core::Timestamp;
use qip_deepbrain::operations::{Operations, deployment_entity_key};
use qip_observability::aiops::EntityKey;

#[test]
fn the_deployment_entity_key_carries_service_region_and_cell_and_a_blank_value_is_refused() {
    let key = deployment_entity_key("qip-deepbrain", "europe-west2", "cell-a")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(key.kind, "deployment");
    assert_eq!(key.id, "qip-deepbrain/europe-west2/cell-a");
    // Two regions of one service must not share a node.
    let other = deployment_entity_key("qip-deepbrain", "us-east4", "cell-a")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_ne!(key, other);
    // Each part blank on its own, and a separator inside a part, is refused.
    for (s, r, c) in [
        ("", "r", "c"),
        ("s", "  ", "c"),
        ("s", "r", ""),
        ("s/x", "r", "c"),
    ] {
        assert!(deployment_entity_key(s, r, c).is_err(), "{s:?} {r:?} {c:?}");
    }
}

#[test]
fn an_operations_loop_given_a_deployment_key_attaches_its_signals_to_that_key() {
    let at = Timestamp::from_nanos(1_000_000_000);
    let key = deployment_entity_key("qip-deepbrain", "europe-west2", "cell-a")
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ops = Operations::new(at)
        .unwrap_or_else(|e| panic!("{e}"))
        .with_deployment(key.clone());
    ops.observe_fills(&[("XNYS".to_string(), 40.0, at)], at)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!ops.graph().classes_for(&key).is_empty());
    let literal = EntityKey::new("deployment", "qip-deepbrain");
    assert!(ops.graph().classes_for(&literal).is_empty());
}
