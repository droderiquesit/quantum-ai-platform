//! R12-OBS-9074 (ADR 0683): the deep brain refuses a runbook set whose
//! digest is not the reviewed one, and the shipped set is the reviewed one.
//!
//! Every test here: mutation not run (nothing was compiled).

use qip_core::Timestamp;
use qip_deepbrain::operations::{
    Operations, REVIEWED_RUNBOOK_SET_DIGEST, runbook_set_digest, runbooks, verified_runbooks,
};
use qip_observability::aiops::Runbook;
use std::collections::BTreeSet;

/// The pin. A commit that changes `runbooks()` without the reviewed digest
/// fails here, in the pipeline, before an image is built and attested.
/// Mutation: add a target to the shipped runbook and this fails with both
/// digests in the message.
#[test]
fn the_shipped_runbook_set_is_the_reviewed_one() {
    let shipped = runbooks();
    assert!(!shipped.is_empty(), "premise: the node ships a runbook");
    assert_eq!(
        runbook_set_digest(&shipped),
        REVIEWED_RUNBOOK_SET_DIGEST,
        "runbooks() changed; review it and update REVIEWED_RUNBOOK_SET_DIGEST in the same commit"
    );
    assert!(
        verified_runbooks(shipped, REVIEWED_RUNBOOK_SET_DIGEST).is_ok(),
        "the reviewed set is admitted"
    );
}

/// A widened target set, an added runbook and a removed one each refuse, and
/// the refusal names what to do. The tampered sets are built from the shipped
/// one, so the test cannot pass on a digest that ignores a field.
/// Mutation: make `runbook_set_digest` skip the targets and the widened case
/// is admitted.
#[test]
fn a_widened_added_or_removed_runbook_is_refused_by_the_reviewed_digest() {
    let shipped = runbooks();

    let mut widened = shipped.clone();
    widened[0].targets.insert("ledger".to_string());

    let mut added = shipped.clone();
    added.push(Runbook {
        name: "restart-the-api".to_string(),
        action: "restart".to_string(),
        targets: BTreeSet::from(["qip-api".to_string()]),
    });

    let mut renamed_action = shipped.clone();
    renamed_action[0].action = "restart".to_string();

    for (label, tampered) in [
        ("widened", widened),
        ("added", added),
        ("action changed", renamed_action),
        ("removed", Vec::new()),
    ] {
        let refused = verified_runbooks(tampered, REVIEWED_RUNBOOK_SET_DIGEST)
            .expect_err(&format!("a {label} runbook set must be refused"));
        assert!(
            refused.message().contains("REVIEWED_RUNBOOK_SET_DIGEST"),
            "{label}: {}",
            refused.message()
        );
    }
}

/// The digest does not depend on the order a set is listed in, and a name
/// containing the separator cannot collide with two runbooks.
/// Mutation: drop the sort in `runbook_set_digest` and the order check fails.
#[test]
fn the_digest_ignores_listing_order_and_is_not_fooled_by_a_separator_in_a_name() {
    let a = Runbook {
        name: "a".to_string(),
        action: "restart".to_string(),
        targets: BTreeSet::from(["x".to_string()]),
    };
    let b = Runbook {
        name: "b".to_string(),
        action: "drain".to_string(),
        targets: BTreeSet::from(["y".to_string()]),
    };
    assert_eq!(
        runbook_set_digest(&[a.clone(), b.clone()]),
        runbook_set_digest(&[b.clone(), a.clone()])
    );
    let tricky = Runbook {
        name: "a|runbook|1:b".to_string(),
        action: "restart".to_string(),
        targets: BTreeSet::from(["x".to_string()]),
    };
    assert_ne!(runbook_set_digest(&[tricky]), runbook_set_digest(&[a, b]));
}

/// The node builds only on the reviewed set: `Operations::new` succeeds on
/// the shipped one. This is the half that fails if the check in `build` refuses
/// the set it ships with; the refusal half is the test above, because the
/// shipped set cannot be swapped from outside the crate, which is the point.
#[test]
fn the_node_builds_on_the_reviewed_set() {
    assert!(Operations::new(Timestamp::from_secs(1_760_000_000)).is_ok());
}
