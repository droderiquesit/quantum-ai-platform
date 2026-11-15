//! R12-RISK-5044: the node arms the net-edge gate at the decided margin.
//!
//! `cell_configuration` lives in the binary and needs a whole `NodeConfig`, so
//! this reads the source the way `arbitrage.rs` does for the desk's region. An
//! armed gate that no root builds is a control that cannot fire, which is the
//! gap this row was graded on.

#[test]
fn the_node_arms_the_net_edge_gate_at_the_decided_margin() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .unwrap_or_else(|error| panic!("main.rs: {error}"));
    let body = main
        .split("fn cell_configuration")
        .nth(1)
        .unwrap_or_else(|| panic!("cell_configuration moved"));
    let body = body.split("\nfn ").next().unwrap_or(body);
    assert!(
        body.contains(".with_net_edge_margin_bp(qip_edge::cell::NET_EDGE_MARGIN_BP)"),
        "cell_configuration no longer arms the net-edge gate"
    );
    assert_eq!(qip_edge::cell::NET_EDGE_MARGIN_BP, qip_core::dec!("25"));
}
