//! The declared storage classes (ADR 0156 §3, R12-DATA-9055): a store is made
//! only for a class the committed list names, no app spells a namespace of its
//! own, and the list and the data module's buckets name each other.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use qip_storage::classes::{
    DECLARED, FABRIC_ARCHIVE_ROLE, StorageClass, StoreKind, WriteDiscipline, archive_class,
};
use qip_storage::{StorageProvider, StorageTarget};

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("qip-storage sits two levels below crates/")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A blob store for an undeclared namespace is refused at construction, and
/// every namespace the apps use is declared. Each declared blob namespace is
/// built first, so the refusal is the list's and not the provider's.
///
/// The failure this prevents: a corpus written to a namespace nobody
/// classified, which the licensing check in `qip-data-finder` never saw and
/// which a bucket's lifecycle rule would then expire or keep by accident.
///
/// Mutation: ignore `classes::blob_class`'s verdict in `StorageProvider::blobs`.
/// The undeclared namespaces construct and the refusal assertion fails. Giving
/// `tick_desk::OUTPUT_NAMESPACE` its own literal fails the app sweep.
#[test]
fn a_blob_store_for_an_undeclared_namespace_is_refused_at_construction_and_every_namespace_the_apps_use_is_declared()
 {
    let dir = std::env::temp_dir().join(format!("qip-classes-{}", std::process::id()));
    let provider = StorageProvider::new(StorageTarget::Memory, &dir);

    let blob_namespaces: Vec<&str> = DECLARED
        .iter()
        .filter(|c| c.kind == StoreKind::Blob && c.namespace != FABRIC_ARCHIVE_ROLE)
        .map(|c| c.namespace)
        .collect();
    assert!(
        blob_namespaces.len() >= 5,
        "premise: the tree's blob classes are declared"
    );
    for namespace in &blob_namespaces {
        provider
            .blobs(namespace)
            .unwrap_or_else(|e| panic!("declared namespace {namespace} must construct: {e}"));
    }

    for undeclared in ["scratch", "raw-vendor-corpus", "archive", "lake/", ""] {
        let error = provider
            .blobs(undeclared)
            .err()
            .unwrap_or_else(|| panic!("{undeclared:?} is not declared and must be refused"));
        assert!(
            error.to_string().contains("not a declared storage class"),
            "{undeclared:?}: {error}"
        );
    }
    // A key-value class and the archive role are not provider blob namespaces.
    assert!(
        provider
            .blobs(qip_storage::classes::PREFLIGHT_NAMESPACE)
            .is_err()
    );
    assert!(provider.blobs(FABRIC_ARCHIVE_ROLE).is_err());

    // Every namespace an app passes to `.blobs(` comes from a constant that
    // is the list's own, never a literal an app typed. Without this the
    // construction check is the only thing standing between a typo and a
    // runtime refusal on a deployed node.
    let mut sources = Vec::new();
    rust_sources(&crates_root().join("apps"), &mut sources);
    let mut calls = 0usize;
    for path in &sources {
        let text = std::fs::read_to_string(path).unwrap();
        // Tests may name a namespace to prove a refusal.
        if path.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        for (at, _) in text.match_indices(".blobs(") {
            let argument: String = text[at + ".blobs(".len()..]
                .chars()
                .take_while(|c| *c != ')')
                .collect();
            let argument = argument.trim();
            // `namespace` is a pass-through parameter of a settings helper.
            if argument == "namespace" {
                continue;
            }
            calls += 1;
            assert!(
                !argument.contains('"') && argument.ends_with("_NAMESPACE"),
                "{} passes `{argument}` to .blobs(; pass a *_NAMESPACE constant that points at \
                 qip_storage::classes",
                path.display()
            );
        }
        for (at, _) in text.match_indices("_NAMESPACE: &str = ") {
            let tail = &text[at + "_NAMESPACE: &str = ".len()..];
            let value: String = tail.chars().take_while(|c| *c != ';').collect();
            assert!(
                value.trim().starts_with("qip_storage::classes::"),
                "{} defines a namespace constant with its own value `{}`; point it at \
                 qip_storage::classes",
                path.display(),
                value.trim()
            );
        }
    }
    assert!(calls >= 5, "premise: the sweep found the apps' blob calls");
}

/// An empty list refuses everything: a build that forgot the list cannot
/// write, not "can write anywhere".
///
/// Mutation: make `blob_class` answer a declared class when handed an empty
/// list. The empty list then constructs.
#[test]
fn an_empty_class_list_refuses_every_store() {
    static NONE: [StorageClass; 0] = [];
    let dir = std::env::temp_dir().join(format!("qip-classes-empty-{}", std::process::id()));
    let provider = StorageProvider::new(StorageTarget::Memory, &dir).with_classes(&NONE);
    for class in DECLARED.iter().filter(|c| c.kind == StoreKind::Blob) {
        let error = provider
            .blobs(class.namespace)
            .err()
            .unwrap_or_else(|| panic!("{} constructed from an empty list", class.namespace));
        assert!(
            error.to_string().contains("no storage class is declared"),
            "{error}"
        );
    }
    assert!(archive_class(&NONE, FABRIC_ARCHIVE_ROLE).is_err());
}

/// fabricd's archive is built only for the declared, immutable archive role.
///
/// Mutation: change the archive class's discipline to `Overwrite` in
/// `DECLARED`. `archive_class` refuses and the first assertion fails.
#[test]
fn the_archive_role_is_declared_immutable_and_an_overwritable_or_unknown_role_is_refused() {
    let class = archive_class(DECLARED, FABRIC_ARCHIVE_ROLE).expect("the archive role is declared");
    assert_eq!(class.discipline, WriteDiscipline::Immutable);
    assert!(archive_class(DECLARED, "scratch").is_err());
    // A role that is declared but overwritable is refused for an archive.
    assert!(archive_class(DECLARED, qip_storage::classes::MODEL_ARTIFACTS_NAMESPACE).is_err());
}

/// The list and the data module's buckets name each other: a bucket cannot
/// appear unclassified and a class cannot point at nothing.
///
/// Mutation: point the simulation class at a bucket label the module does not
/// declare; the second assertion names it and the first names the module's
/// now-unclassified `simulation` bucket.
#[test]
fn every_bucket_the_data_module_declares_has_a_class_and_every_bucket_backed_class_names_one() {
    let module = crates_root()
        .ancestors()
        .nth(2)
        .expect("crates/ sits two levels below the repository root")
        .join("infrastructure/terraform/modules/data/main.tf");
    let text = std::fs::read_to_string(&module).unwrap();
    let declared: BTreeSet<String> = text
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("resource \"google_storage_bucket\" \"")
                .and_then(|rest| rest.split('"').next())
                .map(str::to_string)
        })
        .collect();
    assert!(
        declared.len() >= 4,
        "premise: the data module declares its buckets, found {declared:?}"
    );
    let named: BTreeSet<String> = DECLARED
        .iter()
        .filter_map(|c| c.bucket.map(str::to_string))
        .collect();
    assert_eq!(
        declared.difference(&named).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "a bucket the data module declares has no storage class"
    );
    assert_eq!(
        named.difference(&declared).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "a storage class names a bucket the data module does not declare"
    );
    for class in DECLARED {
        assert!(
            class.bucket.is_some() || class.kind == StoreKind::KeyValue,
            "{} is a blob class and names no bucket",
            class.name
        );
    }
}
