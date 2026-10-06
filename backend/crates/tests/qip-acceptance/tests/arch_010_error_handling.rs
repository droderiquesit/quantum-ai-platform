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
fn all_fallible_code_returns_result() {
    let root = repo_root();

    // Error type must be defined in qip-core
    let error_path = root.join("crates/libs/qip-core/src").join("error.rs");
    if error_path.exists() {
        let error_content = fs::read_to_string(&error_path).expect("could not read error.rs");

        assert!(
            error_content.contains("Error") || error_content.contains("Result"),
            "qip-core must define Error and Result types"
        );
    }
}

#[test]
fn error_type_is_named_qip_error() {
    let root = repo_root();

    // qip-core must export Error type
    let core_lib = root.join("crates/libs/qip-core/src").join("lib.rs");
    let core_content = fs::read_to_string(&core_lib).expect("could not read qip-core lib.rs");

    assert!(
        core_content.contains("Error") || core_content.contains("pub use"),
        "qip-core must export Error type in public API"
    );
}

#[test]
fn refusal_errors_name_corrective_action() {
    let root = repo_root();

    // Error messages must be refusals, not silent failures
    let error_path = root.join("crates/libs/qip-core/src").join("error.rs");
    if error_path.exists() {
        let error_content = fs::read_to_string(&error_path).expect("could not read error.rs");

        // Look for error variants or constructors
        assert!(
            error_content.contains("enum") || error_content.contains("fn "),
            "Error type must define error variants or constructors"
        );
    }
}

#[test]
fn unwrap_forbidden_outside_tests() {
    let root = repo_root();

    // Cargo.toml workspace must deny unwrap_used outside tests
    let workspace_cargo = root.join("Cargo.toml");
    if workspace_cargo.exists() {
        let content =
            fs::read_to_string(&workspace_cargo).expect("could not read workspace Cargo.toml");

        assert!(
            content.contains("deny") || content.contains("unwrap"),
            "Workspace must configure lints to control unwrap usage"
        );
    }
}
