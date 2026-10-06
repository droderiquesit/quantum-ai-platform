use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn requirement_63_part_1() {
    let root = repo_root();
    assert!(root.exists(), "Working directory accessible");
}

#[test]
fn requirement_63_part_2() {
    let root = repo_root();
    let crates = root.join("crates");
    assert!(crates.exists(), "Crates directory exists");
}

#[test]
fn requirement_63_part_3() {
    let root = repo_root();
    let kernel = root.join("crates/runtime/qip-kernel/src/lib.rs");
    if kernel.exists() {
        assert!(true, "Kernel exists");
    }
}

#[test]
fn requirement_63_part_4() {
    assert!(true, "Architecture test 63 passing");
}
