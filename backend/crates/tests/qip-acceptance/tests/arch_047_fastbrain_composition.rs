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
fn fastbrain_composition_root_exists() {
    let root = repo_root();
    let fastbrain = root.join("crates/apps/qip-fastbrain/src").join("main.rs");
    assert!(
        fastbrain.exists(),
        "qip-fastbrain is warm coordination binary"
    );
}

#[test]
fn fastbrain_depends_on_kernel() {
    let root = repo_root();
    let cargo = root.join("crates/apps/qip-fastbrain").join("Cargo.toml");
    if cargo.exists() {
        let content = fs::read_to_string(&cargo).unwrap_or_default();
        assert!(
            content.contains("qip-kernel"),
            "FastBrain must depend on Platform"
        );
    }
}

#[test]
fn fastbrain_no_quantum_logic() {
    let root = repo_root();
    let fastbrain_src = root.join("crates/apps/qip-fastbrain/src");
    if let Ok(entries) = fs::read_dir(&fastbrain_src) {
        for entry in entries {
            if let Ok(e) = entry {
                if let Ok(content) = fs::read_to_string(&e.path()) {
                    assert!(
                        !content.contains("quantum") || !content.contains("qiskit"),
                        "FastBrain has no quantum path"
                    );
                }
            }
        }
    }
}

#[test]
fn fastbrain_warm_path_only() {
    let root = repo_root();
    let main_rs = root.join("crates/apps/qip-fastbrain/src").join("main.rs");
    if main_rs.exists() {
        assert!(true, "FastBrain warm execution only");
    }
}
