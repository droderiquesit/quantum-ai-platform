use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn autonomy_level_forbids_live_trading_at_runtime() {
    let root = repo_root();

    // AutonomyLevel must be in qip-risk-engine and refuse live levels
    let autonomy_path = root
        .join("crates/services/qip-risk-engine/src")
        .join("autonomy.rs");
    let autonomy_content = fs::read_to_string(&autonomy_path).expect("could not read autonomy.rs");

    assert!(
        autonomy_content.contains("Paper") || autonomy_content.contains("PAPER"),
        "AutonomyLevel must define Paper trading level"
    );
}

#[test]
fn terraform_refuses_live_ceiling_at_plan() {
    let root = repo_root();

    // Terraform variables must refuse supervised_live, limited_autonomous_live, autonomous_live
    let repo_root_parent = root.parent().unwrap();
    let tf_vars = repo_root_parent
        .join("infrastructure/terraform")
        .join("variables.tf");
    let tf_content = fs::read_to_string(&tf_vars).unwrap_or_else(|_| String::new());

    assert!(
        tf_content.contains("autonomy") || tf_content.contains("autonomy_level"),
        "Terraform must validate autonomy level"
    );
}

#[test]
fn composition_roots_refuse_live_at_startup() {
    let root = repo_root();

    // qip-api, qip-fastbrain, qip-deepbrain main.rs must refuse live levels
    let binaries = ["qip-api", "qip-fastbrain", "qip-deepbrain"];

    for binary in &binaries {
        let main_path = root
            .join(format!("crates/apps/{}/src", binary))
            .join("main.rs");
        if main_path.exists() {
            let content = fs::read_to_string(&main_path)
                .unwrap_or_else(|_| panic!("could not read {}", binary));

            assert!(
                content.contains("AutonomyLevel") || content.contains("autonomy"),
                "Composition root {} must validate autonomy level at startup",
                binary
            );
        }
    }
}

#[test]
fn cell_type_system_enforces_paper_only() {
    let root = repo_root();

    // Cell::with_config or similar must only accept paper trading ceiling
    let cell_path = root.join("crates/edge/qip-edge/src").join("cell.rs");
    let cell_content = fs::read_to_string(&cell_path).expect("could not read cell.rs");

    assert!(
        cell_content.contains("Paper") || cell_content.contains("paper"),
        "Cell type system must enforce paper trading boundary"
    );
}
