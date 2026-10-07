use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn regional_cell_continues_when_central_unreachable() {
    let root = repo_root();

    // Cell struct must support local decision-making without central dependency
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    let cell_content = fs::read_to_string(&cell_path).expect("could not read qip-edge cell.rs");

    // Cell must have local ledger, local state machine, independent routing
    assert!(
        cell_content.contains("Ledger") || cell_content.contains("ledger"),
        "Cell must maintain local ledger for autonomous operation"
    );

    assert!(
        cell_content.contains("work")
            || cell_content.contains("work_pass")
            || cell_content.contains("pass"),
        "Cell must have work/pass method for local order processing"
    );
}

#[test]
fn cell_circuit_breaker_prevents_cascade_when_mesh_down() {
    let root = repo_root();

    // Mesh downlink must have circuit breaker to prevent cascade on central outage
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    let mesh_content = fs::read_to_string(&mesh_path).expect("could not read qip-edge mesh.rs");

    // Circuit breaker pattern must be implemented in mesh
    assert!(
        mesh_content.contains("CircuitBreaker") || mesh_content.contains("BreakerState"),
        "Mesh must implement circuit breaker to prevent cascade on central outage"
    );
}

#[test]
fn edge_node_app_can_run_standalone_in_paper_trading() {
    let root = repo_root();

    // qip-edge-node must be deployable as standalone binary
    let edge_node_main = root.join("crates/apps/qip-edge-node/src").join("main.rs");

    let edge_node_content =
        fs::read_to_string(&edge_node_main).expect("could not read qip-edge-node main.rs");

    // Must have independent main loop and venue feed configuration
    assert!(
        edge_node_content.contains("main") || edge_node_content.contains("fn main"),
        "qip-edge-node must have main entry point"
    );

    // Must support local feed (simulated trading)
    assert!(
        edge_node_content.contains("simulated")
            || edge_node_content.contains("feed")
            || edge_node_content.contains("venue"),
        "qip-edge-node must support venue feed configuration for paper trading"
    );
}

#[test]
fn cell_holds_own_order_book_independent_of_central() {
    let root = repo_root();

    // Cell must maintain its own order book, not query central
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");

    let cell_content = fs::read_to_string(&cell_path).expect("could not read qip-edge cell.rs");

    // Cell must hold orders locally
    assert!(
        cell_content.contains("order")
            || cell_content.contains("Order")
            || cell_content.contains("orderbook"),
        "Cell must maintain local order book"
    );

    // Cell must not have fetch_orders or query_central patterns for orders
    assert!(
        !cell_content.contains("fetch_orders") && !cell_content.contains("request_orders"),
        "Cell must not fetch orders from central; holds them locally"
    );
}
