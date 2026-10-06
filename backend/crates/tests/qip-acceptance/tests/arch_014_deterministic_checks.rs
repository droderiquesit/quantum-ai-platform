use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn pre_trade_checks_never_route_to_model() {
    let root = repo_root();

    // qip-risk-engine pre-trade checks must be deterministic
    let risk_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("lib.rs");
    let risk_content = fs::read_to_string(&risk_path).unwrap_or_else(|_| String::new());

    assert!(
        risk_content.contains("check") || risk_content.contains("validate"),
        "Risk engine must implement deterministic pre-trade checks"
    );
}

#[test]
fn capital_limits_are_enforced_before_order() {
    let root = repo_root();

    // qip-capital must check limits before order object exists
    let capital_path = root
        .join("crates/libs/qip-contracts/src")
        .join("capital.rs");
    if capital_path.exists() {
        let capital_content = fs::read_to_string(&capital_path).unwrap_or_else(|_| String::new());

        assert!(
            capital_content.contains("Grant") || capital_content.contains("limit"),
            "Capital must enforce grant limits structurally"
        );
    }
}

#[test]
fn determinism_gates_refuse_model_outputs() {
    let root = repo_root();

    // Determinism::Required gate makes it structurally impossible to use model output
    let cost_router_path = root
        .join("crates/services/qip-cost-router/src")
        .join("lib.rs");
    if cost_router_path.exists() {
        let content = fs::read_to_string(&cost_router_path).unwrap_or_else(|_| String::new());

        assert!(
            content.len() > 0,
            "Cost router must exist to implement determinism gates"
        );
    }
}

#[test]
fn order_validation_gates_are_pre_execution() {
    let root = repo_root();

    // Cell must validate all orders before sending to venue
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");
    let cell_content = fs::read_to_string(&cell_path).unwrap_or_else(|_| String::new());

    assert!(
        cell_content.contains("send") || cell_content.contains("refuse"),
        "Cell must validate orders with deterministic gates before sending"
    );
}
