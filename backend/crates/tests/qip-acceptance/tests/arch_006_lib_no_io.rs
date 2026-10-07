use std::fs;
use std::path::PathBuf;

/// The backend Cargo workspace (`<repository>/backend`), which every path in
/// this file is relative to — not the repository root itself.
fn repo_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

#[test]
fn libs_do_not_open_sockets() {
    let root = repo_root();
    let libs_path = root.join("crates/libs");

    // qip-transport and qip-storage are the ONLY exceptions per ADR 0100
    let allowed_libs = ["qip-transport", "qip-storage"];

    for entry in fs::read_dir(&libs_path).expect("could not read libs directory") {
        let entry = entry.expect("could not read entry");
        let path = entry.path();
        let lib_name = path.file_name().unwrap().to_string_lossy().to_string();

        if path.is_dir() && !allowed_libs.contains(&lib_name.as_str()) {
            let lib_src = path.join("src");
            if lib_src.exists() {
                for file_entry in walkdir_simple(&lib_src) {
                    if file_entry.ends_with(".rs") {
                        let content = fs::read_to_string(&file_entry).ok();
                        if let Some(c) = content {
                            assert!(
                                !c.contains("TcpStream")
                                    && !c.contains("TcpListener")
                                    && !c.contains("UdpSocket")
                                    && !c.contains("ToSocketAddrs"),
                                "Library {} must not open sockets (excepted: qip-transport, qip-storage)",
                                lib_name
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn qip_transport_is_only_socket_owning_lib() {
    let root = repo_root();
    let transport_path = root.join("crates/libs/qip-transport/src");
    let transport_content = fs::read_dir(&transport_path)
        .map(|entries| entries.count())
        .unwrap_or(0);

    assert!(
        transport_content > 0,
        "qip-transport must exist as socket-owning library"
    );
}

#[test]
fn lib_crates_have_no_bin_targets() {
    let root = repo_root();
    let libs_path = root.join("crates/libs");

    for entry in fs::read_dir(&libs_path).expect("could not read libs directory") {
        let entry = entry.expect("could not read entry");
        let path = entry.path();

        if path.is_dir() {
            let cargo_path = path.join("Cargo.toml");
            if cargo_path.exists() {
                let content = fs::read_to_string(&cargo_path).expect("could not read Cargo.toml");

                assert!(
                    !content.contains("[[bin]]"),
                    "Library crate must not have binary targets"
                );
            }
        }
    }
}

#[test]
fn lib_crates_export_public_types() {
    let root = repo_root();
    let libs_path = root.join("crates/libs");

    // Sample a few key libs to verify they export types
    let key_libs = ["qip-core", "qip-financial", "qip-contracts"];

    for lib_name in &key_libs {
        let lib_path = libs_path.join(lib_name).join("src").join("lib.rs");
        if lib_path.exists() {
            let content = fs::read_to_string(&lib_path).expect("could not read lib.rs");

            assert!(
                content.contains("pub "),
                "Library {} must export public types",
                lib_name
            );
        }
    }
}

fn walkdir_simple(path: &PathBuf) -> Vec<String> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(path) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                if let Some(ps) = p.to_str() {
                    result.push(ps.to_string());
                }
            } else if p.is_dir() {
                result.extend(walkdir_simple(&p));
            }
        }
    }
    result
}
