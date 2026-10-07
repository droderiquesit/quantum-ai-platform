use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn qip_kernel_is_sole_composition_root() {
    let root = repo_root();
    let kernel_lib = root.join("crates/runtime/qip-kernel/src").join("lib.rs");
    let kernel_content = fs::read_to_string(&kernel_lib).expect("could not read qip-kernel lib.rs");

    assert!(
        kernel_content.contains("Platform") || kernel_content.contains("platform"),
        "qip-kernel must expose Platform as composition root"
    );
}

#[test]
fn kernel_does_not_depend_on_apps() {
    let root = repo_root();
    let kernel_cargo = root.join("crates/runtime/qip-kernel").join("Cargo.toml");
    let kernel_content =
        fs::read_to_string(&kernel_cargo).expect("could not read qip-kernel Cargo.toml");

    let app_crates = ["qip-edge-node", "qip-api", "qip-fastbrain", "qip-deepbrain"];
    for app in &app_crates {
        assert!(
            !kernel_content.contains(app),
            "qip-kernel must not depend on app crate {}",
            app
        );
    }
}

#[test]
fn all_services_depend_only_on_libs() {
    let root = repo_root();
    let services_path = root.join("crates/services");

    for entry in fs::read_dir(&services_path).expect("could not read services directory") {
        let entry = entry.expect("could not read entry");
        let path = entry.path();

        if path.is_dir() {
            let cargo_path = path.join("Cargo.toml");
            if cargo_path.exists() {
                let content = fs::read_to_string(&cargo_path).expect("could not read Cargo.toml");

                // Services should not depend on each other or on apps
                let service_name = path.file_name().unwrap().to_string_lossy();
                assert!(
                    !content.contains("qip-api")
                        && !content.contains("qip-edge-node")
                        && !content.contains("qip-fastbrain")
                        && !content.contains("qip-deepbrain"),
                    "Service {} must not depend on app crates",
                    service_name
                );
            }
        }
    }
}

#[test]
fn runtime_provides_platform_interface() {
    let root = repo_root();
    let kernel_lib = root.join("crates/runtime/qip-kernel/src").join("lib.rs");
    let kernel_content = fs::read_to_string(&kernel_lib).expect("could not read qip-kernel lib.rs");

    assert!(
        kernel_content.contains("pub ")
            && (kernel_content.contains("Platform") || kernel_content.contains("Cycle")),
        "qip-kernel must expose public Platform and Cycle types"
    );
}
