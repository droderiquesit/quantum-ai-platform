//! ADR 0145: an ambient draft has no identified effect, so the unidentified
//! effect is recorded and acts on nothing.
//!
//! The deep brain's ambient door files the subject's follow-up reading less
//! its drafting-sweep reading as `Identification::NotIdentified`. That number
//! is a correlation, and each consumer that could act on it is closed by a
//! test beside the consumer: the federation refuses it
//! (`qip-world-model/tests/federation.rs`) and the memory will not recommend
//! on it (`qip-agency/tests/agency.rs`). Those tests prove the refusals
//! exist. This suite proves the other half, which no crate can see from
//! inside: that no running path *reaches* the consumers that have no refusal,
//! the causal graph and the engine, and that the one consumer with a refusal
//! has no production caller to refuse.
//!
//! Source pins, because the boundary is an absence: a call that is not there.

#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, production_text, read};

const AMBIENT_DOOR: &str = "backend/crates/apps/qip-deepbrain/src/ambient_agency.rs";
const AFFORDANCE_GRAPH: &str = "backend/crates/services/qip-agency/src/affordance.rs";
const FEDERATION: &str = "backend/crates/services/qip-world-model/src/federation.rs";

/// The calls that would carry an attribution out of the ambient door to
/// something that decides: the federation journal, the engine's ranking and
/// act stages, and an authority widening.
const FORBIDDEN_ON_THE_AMBIENT_DOOR: [&str; 6] = [
    "update_from_attribution",
    "run_governed",
    "engine::run",
    ".widen(",
    "Federation",
    "AffordanceGraph",
];

#[test]
fn a_boundary_suite_pins_that_no_production_code_feeds_an_unidentified_effect_to_the_federation_the_graph_or_the_engine()
 {
    let door = production_text(&read(AMBIENT_DOOR));
    // Premise: the door still files the unidentified effect through the
    // learner. A door that no longer did would make every absence below true
    // of a file that does nothing.
    assert!(
        door.contains("Identification::NotIdentified"),
        "premise: the ambient door files its effect as not identified"
    );
    assert!(
        door.contains("engine::learn("),
        "premise: the ambient door learns through `engine::learn`"
    );
    for forbidden in FORBIDDEN_ON_THE_AMBIENT_DOOR {
        assert!(
            !door.contains(forbidden),
            "the ambient door names `{forbidden}`; an unidentified effect must reach no \
             decision, size or order (ADR 0145 decision 2)"
        );
    }
}

#[test]
fn nothing_outside_the_world_model_calls_the_federations_attribution_update() {
    // The refusal in `Federation::update_from_attribution` is the last line;
    // the first is that nothing calls it. A production caller would be a
    // reviewed decision, and this test is where that review is forced.
    let definition = production_text(&read(FEDERATION));
    assert!(
        definition.contains("pub fn update_from_attribution"),
        "premise: the federation still defines the method this pins"
    );
    let sources = files_with_extension("backend/crates", "rs");
    assert!(!sources.is_empty(), "premise: the scan found source files");
    let callers: Vec<String> = sources
        .iter()
        .filter(|path| {
            let p = path.to_string_lossy();
            !p.contains("/tests/") && !p.ends_with("qip-world-model/src/federation.rs")
        })
        .filter(|path| {
            std::fs::read_to_string(path)
                .map(|text| production_text(&text).contains("update_from_attribution("))
                .unwrap_or(false)
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        callers.is_empty(),
        "production code calls `update_from_attribution`: {callers:?} (ADR 0145 decision 2)"
    );
}

#[test]
fn the_affordance_graph_has_no_operation_that_takes_an_attribution() {
    // A tool edge carries declared reversibility, latency, cost and
    // authority, not a learned strength. A field to receive an unidentified
    // number is the failure, not the closure (ADR 0145 decision 2).
    let graph = production_text(&read(AFFORDANCE_GRAPH));
    assert!(
        graph.contains("pub fn add_tool_edge"),
        "premise: the graph still has the operations this pins"
    );
    for forbidden in ["Attribution", "attribution", "EffectAttribution"] {
        assert!(
            !graph.contains(forbidden),
            "the affordance graph names `{forbidden}`; it must take no learned effect"
        );
    }
}
