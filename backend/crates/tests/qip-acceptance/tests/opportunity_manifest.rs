//! R12-REFLEX-5030, ADR 0149 decision 7: the opportunity manifest's two ends
//! digest one definition alike.
//!
//! The manifest is published by `qip-cli`'s grant fixture, which compiles the
//! plan's strategies itself, and read by `qip-edge-node`'s
//! `StrategyInstaller`, which compiles them on the node. The two apps cannot
//! depend on each other, so each builds the standard feature vocabulary on
//! its own; if the two ever differed, every definition would read as
//! unnamed and the node would activate nothing. This suite holds them to the
//! same digest over the committed slice plan.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_edge_node::strategies::{StrategyInstaller, StrategyPlan};
use std::collections::BTreeMap;

const PLAN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/qip-edge-node/fixtures/slice-strategy-plan.json"
);

#[test]
fn the_publisher_and_the_node_digest_every_entry_rule_of_the_committed_plan_alike() {
    let text = std::fs::read_to_string(PLAN).expect("the committed plan is readable");
    let published = qip_cli::event_fabric::grant::plan_opportunities(&text)
        .expect("the publisher digests the plan");
    let plan = StrategyPlan::from_bytes(text.as_bytes()).expect("the node parses the plan");
    let mut compiled_on_node = BTreeMap::new();
    for spec in &plan.strategies {
        let (compiled, program) = StrategyInstaller::compile(spec).expect("the node compiles it");
        compiled_on_node.extend(
            compiled
                .opportunities(&program)
                .expect("the node digests it"),
        );
    }
    // Premise: the plan has an entry rule, and it reads a computed feature,
    // so both vocabularies were exercised.
    assert!(
        !published.is_empty(),
        "the committed plan names no entry rule"
    );
    assert!(
        text.contains("\"feature\""),
        "premise: the plan reads a feature"
    );
    assert_eq!(published, compiled_on_node);
}
