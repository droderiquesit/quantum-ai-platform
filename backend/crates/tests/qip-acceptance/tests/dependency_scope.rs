//! Where a dependency is admitted is part of its admission (ADR 0109: a line
//! on the allow-list is admitted for the crate and the caller it names).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_acceptance::{files_with_extension, repository_root};
use std::collections::BTreeSet;

/// The compression crates a manifest could name. `zstd` is the one admitted
/// (ADR 0113), for tick-lake segments in `qip-storage`; the rest are the ways
/// a second codec would arrive.
const COMPRESSION_CRATES: [&str; 14] = [
    "zstd",
    "zstd-safe",
    "zstd-sys",
    "flate2",
    "miniz_oxide",
    "libz-sys",
    "brotli",
    "lz4",
    "lz4_flex",
    "lz4-sys",
    "snap",
    "xz2",
    "bzip2",
    "lzma-rs",
];

/// The crate directory name whose manifest names `dependency` in a
/// dependency table, for every manifest under `backend/crates`.
fn dependents(dependency: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for manifest in files_with_extension("backend/crates", "toml") {
        if manifest.file_name().is_none_or(|n| n != "Cargo.toml") {
            continue;
        }
        let text = std::fs::read_to_string(&manifest).unwrap();
        let mut in_dependencies = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_dependencies = line.contains("dependencies");
                continue;
            }
            let key = line.split(['=', ' ', '.']).next().unwrap_or("");
            if in_dependencies && key == dependency {
                let name = manifest
                    .parent()
                    .and_then(|p| p.file_name())
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                found.insert(name);
            }
        }
    }
    found
}

/// R12-MESH-9181 (ADR 0156 §4e). No compression codec is admitted on the peer
/// mesh or any transport, and `zstd` stays admitted for lake segments in
/// `qip-storage` only. A codec on the mesh hop adds an inflate-amplification
/// surface at the receiver, on a channel adjacent to the decision loop, to
/// save bytes the frame bounds already cap; extending a codec elsewhere needs
/// a measurement and an allow-list line, which is a protected edit.
///
/// The first assertion is the premise: `qip-storage` does depend on `zstd`,
/// so "only" is not true of an empty set.
///
/// Mutation: add `zstd` to `qip-transport`'s manifest. The scan finds a second
/// dependent and fails naming it.
#[test]
fn only_qip_storage_depends_on_a_compression_crate() {
    assert_eq!(
        dependents("zstd"),
        BTreeSet::from(["qip-storage".to_string()]),
        "zstd is admitted for tick-lake segments in qip-storage and nowhere else"
    );
    for codec in COMPRESSION_CRATES {
        let others: Vec<String> = dependents(codec)
            .into_iter()
            .filter(|name| name != "qip-storage")
            .collect();
        assert!(
            others.is_empty(),
            "{others:?} depend on the compression crate {codec}; no codec is admitted outside \
             qip-storage (ADR 0156 4e)"
        );
    }
    let allow = std::fs::read_to_string(repository_root().join("scripts/dependency-allowlist.txt"))
        .unwrap();
    assert!(
        allow
            .lines()
            .any(|l| l.split('#').next().unwrap_or("").trim() == "zstd" && l.contains("ADR 0113")),
        "premise: the allow-list line for zstd names the ADR that scoped it"
    );
}
