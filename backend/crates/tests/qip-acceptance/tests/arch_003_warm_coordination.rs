use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn arbitrage_policy_held_in_kernel_not_separate_service() {
    let root = repo_root();

    // ArbitragePolicy must be in qip-kernel, not in a separate service
    let kernel_central = root
        .join("crates/runtime/qip-kernel/src/central")
        .join("mod.rs");

    let kernel_content =
        fs::read_to_string(&kernel_central).expect("could not read qip-kernel central module");

    assert!(
        kernel_content.contains("ArbitragePolicy") || kernel_content.contains("arbitrage"),
        "ArbitragePolicy must be defined in qip-kernel's central module"
    );

    // Verify there is NO separate qip-arbitrage or qip-warm-coordination service crate
    let services_path = root.join("crates/services");
    let service_dirs = fs::read_dir(&services_path).expect("could not read services directory");

    for entry in service_dirs {
        let entry = entry.expect("could not read service directory entry");
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        assert!(
            !name.contains("arbitrage") && !name.contains("warm"),
            "Arbitrage policy must not be a separate service; it belongs in qip-kernel"
        );
    }
}

#[test]
fn capital_allocation_via_kernel_not_separate_warm_tier() {
    let root = repo_root();

    // Capital allocation logic must be in qip-kernel
    let kernel_src = root.join("crates/runtime/qip-kernel/src").join("lib.rs");

    let kernel_content = fs::read_to_string(&kernel_src).expect("could not read qip-kernel lib.rs");

    // Should have capital or allocation logic in kernel
    assert!(
        kernel_content.contains("capital") || kernel_content.contains("allocation"),
        "qip-kernel must contain capital allocation logic"
    );

    // Verify qip-fastbrain and qip-deepbrain both depend on qip-kernel (for shared coordination)
    let fastbrain_cargo = root.join("crates/apps/qip-fastbrain").join("Cargo.toml");
    let deepbrain_cargo = root.join("crates/apps/qip-deepbrain").join("Cargo.toml");

    let fastbrain_content =
        fs::read_to_string(&fastbrain_cargo).expect("could not read qip-fastbrain Cargo.toml");
    let deepbrain_content =
        fs::read_to_string(&deepbrain_cargo).expect("could not read qip-deepbrain Cargo.toml");

    assert!(
        fastbrain_content.contains("qip-kernel"),
        "qip-fastbrain must depend on qip-kernel for coordination"
    );
    assert!(
        deepbrain_content.contains("qip-kernel"),
        "qip-deepbrain must depend on qip-kernel for coordination"
    );
}

#[test]
fn liquidity_maps_published_via_mesh_not_fetched_on_demand() {
    let root = repo_root();

    // Mesh must publish capital and policy via downlink, not fetched on-demand
    let mesh_path = root.join("crates/edge/qip-edge/src").join("mesh.rs");

    let mesh_content = fs::read_to_string(&mesh_path).expect("could not read mesh.rs");

    // Should have CapitalDownlink structure
    assert!(
        mesh_content.contains("CapitalDownlink"),
        "Mesh must define CapitalDownlink for capital/policy publication"
    );

    // Should NOT have fetch_policy or fetch_capital patterns
    assert!(
        !mesh_content.contains("fetch_policy")
            && !mesh_content.contains("request_policy")
            && !mesh_content.contains("fetch_capital")
            && !mesh_content.contains("request_capital"),
        "Capital and policy must not be fetched on-demand via RPC; published via mesh"
    );
}

#[test]
fn coordination_logic_shared_between_fastbrain_and_deepbrain() {
    let root = repo_root();

    // Both binaries must import from qip-kernel for coordination
    let fastbrain_main = root.join("crates/apps/qip-fastbrain/src").join("main.rs");
    let deepbrain_main = root.join("crates/apps/qip-deepbrain/src").join("main.rs");

    let fastbrain_content =
        fs::read_to_string(&fastbrain_main).expect("could not read qip-fastbrain main.rs");
    let deepbrain_content =
        fs::read_to_string(&deepbrain_main).expect("could not read qip-deepbrain main.rs");

    // Both should use kernel coordination types
    assert!(
        fastbrain_content.contains("qip_kernel") || fastbrain_content.contains("use qip"),
        "qip-fastbrain must use qip-kernel for coordination"
    );
    assert!(
        deepbrain_content.contains("qip_kernel") || deepbrain_content.contains("use qip"),
        "qip-deepbrain must use qip-kernel for coordination"
    );

    // Verify there is NO separate "warm-tier" crate
    let crates_path = root.join("crates");
    let mut has_warm_tier = false;

    for entry in fs::read_dir(&crates_path).expect("could not read crates") {
        let entry = entry.expect("could not read crate entry");
        let name = entry.file_name().to_string_lossy().to_string();

        if name.contains("warm") {
            has_warm_tier = true;
        }
    }

    assert!(
        !has_warm_tier,
        "No separate warm-tier crate should exist; coordination is kernel-embedded"
    );
}
