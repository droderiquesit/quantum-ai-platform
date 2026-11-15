//! R12-WORLD-9023 (ADR 0613): the node refuses a directional signal that
//! states no edge. Without this the net-edge gate is armed but judges only
//! the signals that volunteer an edge, and every committed strategy declares
//! none, so the gate would fire on nothing. Reads the source the way
//! `net_edge_armed.rs` does, because `cell_configuration` needs a `NodeConfig`.

#[test]
fn the_node_refuses_a_directional_signal_that_states_no_edge() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .unwrap_or_else(|error| panic!("main.rs: {error}"));
    let body = main
        .split("fn cell_configuration")
        .nth(1)
        .unwrap_or_else(|| panic!("cell_configuration moved"));
    let body = body.split("\nfn ").next().unwrap_or(body);
    // Premise: the margin is armed in the same body, so the switch has
    // something to act on (it has no effect without a margin).
    assert!(
        body.contains(".with_net_edge_margin_bp("),
        "the net-edge margin is not armed, so the edgeless switch would do nothing"
    );
    assert!(
        body.contains(".with_edgeless_signals_refused()"),
        "cell_configuration no longer refuses edgeless directional signals"
    );
}
