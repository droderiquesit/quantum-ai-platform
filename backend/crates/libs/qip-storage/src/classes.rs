//! The declared storage classes: the answer to "where may this platform
//! write" (ADR 0156 §3, R12-DATA-9055).
//!
//! A class is a blob namespace (or an archive role) with its write
//! discipline, its owner and the bucket resource that backs it. The list is
//! committed here, in a library every app can read, and it is checked where a
//! store is **made**, not where a key is validated: the store is reached two
//! ways with different key shapes (fabricd's archive under a configurable
//! prefix writing `segments/sha256/..`, `manifests/..` and a start-up probe;
//! the provider making the namespace the prefix and writing arbitrary keys
//! under it), so no prefix list inside `validate_key` could serve both. The
//! prefix stays configurable. The class does not.
//!
//! **An empty list refuses everything.** A build that forgot the list cannot
//! write; it does not write anywhere.
//!
//! **No raw-corpus class is declared.** A raw vendor corpus is a licensing
//! question `qip-data-finder` answers before a source is used, and a class
//! declared ahead of that answer is permission granted in advance. Adding a
//! class is a reviewed diff to this list with a test.
//!
//! Declaring a class opens no bucket. `enable_cloud_storage` is the owner's
//! switch in a protected tfvars file, and the buckets bill.

use qip_core::error::{Error, Result};

/// The lake's blob namespace ([`crate::lake::LAKE_NAMESPACE`] is this).
pub const LAKE_NAMESPACE: &str = "lake";
/// Model artifacts the deep brain writes.
pub const MODEL_ARTIFACTS_NAMESPACE: &str = "model-artifacts";
/// Knowledge packs the deep brain's connectors keep.
pub const KNOWLEDGE_PACK_NAMESPACE: &str = "knowledge-packs";
/// Signed reflex packages the research desk publishes.
pub const REFLEX_PACKAGES_NAMESPACE: &str = "reflex-packages";
/// The tick desk's model cards and opportunity maps.
pub const TICK_DESK_NAMESPACE: &str = "tick-desk";
/// The probe namespace [`crate::settings::StorageSettings::preflight`] writes.
pub const PREFLIGHT_NAMESPACE: &str = "preflight";
/// The role fabricd's segment archive is opened under. Not a provider
/// namespace: fabricd builds its own store under a configurable prefix.
pub const FABRIC_ARCHIVE_ROLE: &str = "fabric-archive";

/// How a class may be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteDiscipline {
    /// An object, once written, is never replaced or merged. The fabric
    /// archive's objects are the unit of evidence.
    Immutable,
    /// A key may be written again.
    Overwrite,
}

/// Which port a class is reached through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    /// `StorageProvider::blobs`, or an archive store built under a role.
    Blob,
    /// `StorageProvider::key_value`; never reaches a bucket.
    KeyValue,
}

/// What a class's objects are (R12-DATA-1041).
///
/// There is deliberately no variant for a document, a page or any other
/// body a world source published: the durable layer is derived knowledge,
/// and a world document behind it is re-fetched through its manifest rather
/// than kept (ADR 0180 decision 4). A class that wanted to keep bodies would
/// first have to add a variant here, in a reviewed change the acceptance
/// test `no_declared_storage_class_or_pack_body_kind_can_hold_a_document_body`
/// fails on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holds {
    /// Sealed event-fabric segments: the platform's own records.
    EventSegments,
    /// Venue market data kept under a retention right: decoded ticks, and
    /// the venue's frames only where the dataset's right keeps them (ADR
    /// 0181). Market data, not a document; ADR 0180 decision 4 says why the
    /// never-copy rule does not reach it.
    VenueMarketData,
    /// Trained model artifacts and the reflex packages built from them.
    ModelArtifacts,
    /// Knowledge packs: the compact derived form, never the source text
    /// (`qip_contracts::knowledge_pack`).
    KnowledgePacks,
    /// The tick desk's simulation output.
    SimulationOutput,
    /// A start-up write proving the store is writable.
    Probe,
}

/// One declared class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageClass {
    /// A short name for diagnostics.
    pub name: &'static str,
    /// The provider namespace, or the archive role for the fabric archive.
    pub namespace: &'static str,
    pub kind: StoreKind,
    pub discipline: WriteDiscipline,
    /// The binary that writes it.
    pub owner: &'static str,
    /// The `google_storage_bucket` resource label in
    /// `infrastructure/terraform/modules/data/main.tf` that backs the class,
    /// or `None` for a class that never reaches a bucket. An acceptance test
    /// holds this against the module in both directions.
    pub bucket: Option<&'static str>,
    /// What its objects are.
    pub holds: Holds,
}

/// The classes found in the tree.
pub const DECLARED: &[StorageClass] = &[
    StorageClass {
        name: "fabric archive",
        namespace: FABRIC_ARCHIVE_ROLE,
        kind: StoreKind::Blob,
        discipline: WriteDiscipline::Immutable,
        owner: "qip-fabricd",
        bucket: Some("archive"),
        holds: Holds::EventSegments,
    },
    StorageClass {
        name: "tick lake",
        namespace: LAKE_NAMESPACE,
        kind: StoreKind::Blob,
        discipline: WriteDiscipline::Immutable,
        owner: "qip-edge-node, qip-fastbrain, qip-deepbrain, qip-api",
        bucket: Some("archive"),
        holds: Holds::VenueMarketData,
    },
    StorageClass {
        name: "model artifacts",
        namespace: MODEL_ARTIFACTS_NAMESPACE,
        kind: StoreKind::Blob,
        discipline: WriteDiscipline::Overwrite,
        owner: "qip-deepbrain",
        bucket: Some("artifacts"),
        holds: Holds::ModelArtifacts,
    },
    StorageClass {
        name: "reflex packages",
        namespace: REFLEX_PACKAGES_NAMESPACE,
        kind: StoreKind::Blob,
        discipline: WriteDiscipline::Overwrite,
        owner: "qip-deepbrain",
        bucket: Some("artifacts"),
        holds: Holds::ModelArtifacts,
    },
    StorageClass {
        name: "knowledge packs",
        namespace: KNOWLEDGE_PACK_NAMESPACE,
        kind: StoreKind::Blob,
        discipline: WriteDiscipline::Overwrite,
        owner: "qip-deepbrain",
        bucket: Some("segments"),
        holds: Holds::KnowledgePacks,
    },
    StorageClass {
        name: "tick desk output",
        namespace: TICK_DESK_NAMESPACE,
        kind: StoreKind::Blob,
        discipline: WriteDiscipline::Overwrite,
        owner: "qip-deepbrain",
        bucket: Some("simulation"),
        holds: Holds::SimulationOutput,
    },
    StorageClass {
        name: "preflight probe",
        namespace: PREFLIGHT_NAMESPACE,
        kind: StoreKind::KeyValue,
        discipline: WriteDiscipline::Overwrite,
        owner: "every composition root",
        bucket: None,
        holds: Holds::Probe,
    },
];

/// The declared class for a blob `namespace` in `classes`, or a refusal
/// naming what is declared.
///
/// A key-value class does not satisfy a blob request, and the archive role is
/// not a provider namespace: it is built by [`archive_class`].
pub fn blob_class<'a>(classes: &'a [StorageClass], namespace: &str) -> Result<&'a StorageClass> {
    classes
        .iter()
        .find(|class| {
            class.kind == StoreKind::Blob
                && class.namespace == namespace
                && class.namespace != FABRIC_ARCHIVE_ROLE
        })
        .ok_or_else(|| undeclared(classes, "blob namespace", namespace))
}

/// The declared, immutable archive class for `role`, or a refusal. fabricd
/// builds its segment archive only after this succeeds, so an archive store
/// cannot be opened for a role that is not the declared one, nor for a class
/// whose discipline lets objects be replaced.
pub fn archive_class<'a>(classes: &'a [StorageClass], role: &str) -> Result<&'a StorageClass> {
    let class = classes
        .iter()
        .find(|class| class.kind == StoreKind::Blob && class.namespace == role)
        .ok_or_else(|| undeclared(classes, "archive role", role))?;
    if class.discipline != WriteDiscipline::Immutable {
        return Err(Error::denied(format!(
            "archive role '{role}' is declared as {:?}, and a segment archive must be \
             Immutable: its objects are fetched by exact key and chained by hash",
            class.discipline
        )));
    }
    Ok(class)
}

fn undeclared(classes: &[StorageClass], what: &str, asked: &str) -> Error {
    let declared: Vec<&str> = classes.iter().map(|c| c.namespace).collect();
    if declared.is_empty() {
        return Error::denied(format!(
            "no storage class is declared, so no store may be made ('{asked}' was asked for). \
             A build with an empty list writes nowhere: declare the class in \
             qip_storage::classes::DECLARED, with a test"
        ));
    }
    Error::denied(format!(
        "{what} '{asked}' is not a declared storage class (declared: {}). Writing somewhere \
         the list does not name is how a corpus lands in a bucket nobody classified: add the \
         class to qip_storage::classes::DECLARED in a reviewed change",
        declared.join(", ")
    ))
}
