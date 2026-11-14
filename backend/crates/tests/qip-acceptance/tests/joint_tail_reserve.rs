//! Owner decision 36 (2026-11-14): the generated joint tail world sets the
//! survival reserve in every deployed process that runs a cycle.
//!
//! `Platform::set_joint_tail_reserve_applied` defaults to off, and for weeks
//! no composition root turned it on: the joint world was computed and charted
//! every cycle while the reserve DECIDE withheld ignored it, a stress test
//! that reads as protection and is not.

use std::fs;

#[test]
fn every_cycle_running_composition_root_applies_the_joint_tail_reserve() {
    let root = qip_acceptance::repository_root().join("backend/crates/apps");
    let binaries = ["qip-api", "qip-fastbrain", "qip-deepbrain"];
    assert_eq!(binaries.len(), 3, "premise: three roots run cycles");
    for binary in binaries {
        let path = root.join(binary).join("src/main.rs");
        let source = fs::read_to_string(&path).expect("a composition root is readable");
        assert!(
            source.contains("Platform::new") || source.contains("Platform::with_model_provider"),
            "premise: {binary} constructs a Platform"
        );
        let applied = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .any(|line| line.trim() == "platform.set_joint_tail_reserve_applied(true);");
        assert!(
            applied,
            "{binary} must call platform.set_joint_tail_reserve_applied(true) (owner decision 36)"
        );
    }
}
