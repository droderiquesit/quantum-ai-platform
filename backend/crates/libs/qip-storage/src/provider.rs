//! Storage provider selection.
//!
//! Which backing service a workload uses is a deployment decision, and the
//! choice is deliberate rather than "one database for everything" (charter
//! section 7). The mapping below records *why* each target exists; the managed
//! adapters are ports awaiting credentials, and say so.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A backing store the platform can be configured to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageTarget {
    /// Process memory. Simulation, tests, and the Fast Brain's hot state.
    Memory,
    /// Local files, one JSON document per namespace. Development, and small
    /// hand-inspectable state.
    File,
    /// The in-tree embedded engine: write-ahead log, checkpoints, atomic
    /// transactions, crash recovery. The durable choice for a single node.
    Engine,
    /// Analytical warehouse: research queries, attribution, backtest results.
    BigQuery,
    /// Object storage: event log archives, model artifacts, reports.
    CloudStorage,
    /// Relational: entities, portfolios, orders — anything needing transactions.
    AlloyDb,
    /// Globally consistent relational, where cross-region transactions justify it.
    Spanner,
    /// Wide-column: tick and order-book history at high write throughput.
    Bigtable,
    /// In-memory cache: hot quotes, feature values, rate limits.
    Memorystore,
}

impl StorageTarget {
    /// Whether an adapter for this target exists in this build.
    ///
    /// This is a fact about the *binary*, not about a deployment.
    /// [`StorageTarget::Memorystore`] is implemented — `crate::redis` speaks
    /// RESP over a TCP socket — and a process with no instance address still
    /// cannot build one, so [`StorageProvider::key_value`] can refuse a target
    /// that is implemented. The two questions are separate on purpose:
    /// "this build cannot do that" and "this deployment did not say where"
    /// send an operator to different places.
    ///
    /// [`StorageTarget::CloudStorage`] and [`StorageTarget::BigQuery`] are
    /// implemented for the same reason Memorystore is — their protocol is one
    /// this workspace can speak. Both have JSON REST APIs, and `crate::gcp`
    /// reaches them over [`qip_transport::HttpClient`]. They are the only two
    /// of the six managed targets that do; the remaining four need a
    /// PostgreSQL wire client or a gRPC stack, which is what
    /// [`StorageTarget::required_configuration`] says at length.
    ///
    /// Implemented does not mean shaped like a [`crate::KeyValueStore`]. Cloud
    /// Storage is a blob store and BigQuery is neither — a warehouse is a third
    /// shape, reached through [`crate::BigQueryWarehouse`] rather than through
    /// this provider. [`StorageProvider::key_value`] and
    /// [`StorageProvider::blobs`] each say so for the target they cannot
    /// serve, instead of reporting the adapter as absent.
    pub fn is_implemented(&self) -> bool {
        matches!(
            self,
            Self::Memory
                | Self::File
                | Self::Engine
                | Self::Memorystore
                | Self::CloudStorage
                | Self::BigQuery
        )
    }

    /// Whether an acknowledged write to this target survives loss of power.
    ///
    /// Only [`StorageTarget::Engine`] both flushes every write and recovers a
    /// crash mid-write. [`StorageTarget::File`] flushes too, but rewrites the
    /// whole document each time, so it is durable without being an engine.
    ///
    /// [`StorageTarget::Memorystore`] is deliberately absent even though it
    /// has an adapter. The instance is provisioned with
    /// `persistence_mode = DISABLED`, so an acknowledged write there can be
    /// taken back by a restart, a failover, an eviction or an expiry. Gaining
    /// an adapter changed what the platform can *reach*, not what it can
    /// *promise*, and this is the method that has to keep saying so — it is
    /// what [`crate::StorageSettings::is_durable`] answers with, and therefore
    /// what the start-up banner tells an operator.
    ///
    /// [`StorageTarget::CloudStorage`] and [`StorageTarget::BigQuery`] are
    /// absent for a different reason again: this question is asked of the
    /// key-value store a deployment runs on, and neither of them is one. An
    /// acknowledged Cloud Storage write is durable — far more so than a local
    /// `fsync` — but saying so here would be answering about a store that
    /// [`StorageProvider::key_value`] refuses to build.
    pub fn is_crash_safe(&self) -> bool {
        matches!(self, Self::File | Self::Engine)
    }

    /// What the target is for, and why it was chosen over the alternatives.
    pub fn rationale(&self) -> &'static str {
        match self {
            Self::Memory => "hot state and deterministic simulation; nothing survives a restart",
            Self::File => "one readable JSON document per namespace; small, rarely written state",
            Self::Engine => {
                "the in-tree engine: durable, crash-recoverable, transactional, single node"
            }
            Self::BigQuery => "columnar scans over research history; not for transactional writes",
            Self::CloudStorage => "immutable large objects: log archives, model artifacts, reports",
            Self::AlloyDb => {
                "transactional records with foreign keys: entities, orders, portfolios"
            }
            Self::Spanner => {
                "only where a transaction must span regions; otherwise AlloyDB is cheaper"
            }
            Self::Bigtable => "high-throughput time series keyed by instrument and time",
            Self::Memorystore => "sub-millisecond reads of values that can be recomputed if lost",
        }
    }

    /// Every target, in declaration order.
    ///
    /// Used to build the error a bad configuration value gets. An operator who
    /// misspells a target name should be told the whole valid set rather than
    /// left to grep for it.
    pub const ALL: [Self; 9] = [
        Self::Memory,
        Self::File,
        Self::Engine,
        Self::BigQuery,
        Self::CloudStorage,
        Self::AlloyDb,
        Self::Spanner,
        Self::Bigtable,
        Self::Memorystore,
    ];

    /// The name this target is written as in configuration.
    ///
    /// Matches the `snake_case` serde representation, so a target read from an
    /// environment variable and one read from a serialized document cannot
    /// disagree about what `alloy_db` means.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::File => "file",
            Self::Engine => "engine",
            Self::BigQuery => "big_query",
            Self::CloudStorage => "cloud_storage",
            Self::AlloyDb => "alloy_db",
            Self::Spanner => "spanner",
            Self::Bigtable => "bigtable",
            Self::Memorystore => "memorystore",
        }
    }

    /// Parse a target from configuration.
    ///
    /// Deliberately not `serde_json::from_str`: an operator setting an
    /// environment variable writes `engine`, not `"engine"`, and the error
    /// from a JSON parser would be about quoting rather than about which
    /// targets exist. A rejected name lists the valid set, because the failure
    /// this prevents is a typo resolving to the default and a deployment that
    /// believes it is durable running in memory.
    pub fn parse(value: &str) -> Result<Self> {
        let normalised = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Self::ALL
            .into_iter()
            .find(|target| target.as_str() == normalised)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{value:?} is not a storage target. Valid targets: {}",
                    Self::ALL
                        .iter()
                        .map(|target| target.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    /// The credential or configuration a deployment must supply.
    ///
    /// For the three targets that have no adapter, this doubles as the reason
    /// there is none. "Needs credentials" is not a reason — it tells a reader
    /// they are blocked without telling them what to do next, and it is wrong
    /// besides: a credential would not help, because the obstacle is a
    /// protocol this workspace has deliberately not implemented. Each string
    /// below names the protocol, says how large the in-tree implementation
    /// would be, and points at the alternative that already works, so that a
    /// reader can decide between "route this elsewhere" and "add the
    /// dependency and put the decision in the diff".
    pub fn required_configuration(&self) -> Option<&'static str> {
        match self {
            Self::Memory | Self::File | Self::Engine => None,
            Self::BigQuery => Some(
                "the REST adapter is built into this binary; what a deployment still supplies is \
                 the warehouse and the way to reach it. Set QIP_BIGQUERY_PROJECT and \
                 QIP_BIGQUERY_DATASET, and create the dataset and its tables outside this code, \
                 which creates nothing. Set QIP_GCP_ENDPOINT to an http:// TLS-terminating proxy \
                 that forwards to bigquery.googleapis.com: qip_transport::http has no TLS stack \
                 and refuses https by name, so there is no address of Google's it can use \
                 directly, and a bearer token sent to a public endpoint in clear text is a \
                 credential given away. Supply a token through exactly one of \
                 QIP_GCP_METADATA_SERVER=1, QIP_GCP_TOKEN_FILE or QIP_GCP_ACCESS_TOKEN — this \
                 build cannot mint one, because that means RS256-signing a JWT and ADR 0009 \
                 forbids in-tree cryptography. The service account needs \
                 roles/bigquery.dataEditor on the dataset and roles/bigquery.jobUser on the \
                 project, since a query is a billed job",
            ),
            Self::CloudStorage => Some(
                "the REST adapter is built into this binary; what a deployment still supplies is \
                 the bucket and the way to reach it. Set QIP_CLOUD_STORAGE_BUCKET, and \
                 QIP_GCP_ENDPOINT to an http:// TLS-terminating proxy that forwards to \
                 storage.googleapis.com: qip_transport::http has no TLS stack and refuses https \
                 by name, so a bearer token sent straight to a public endpoint would cross the \
                 internet in clear text. Supply a token through exactly one of \
                 QIP_GCP_METADATA_SERVER=1, QIP_GCP_TOKEN_FILE or QIP_GCP_ACCESS_TOKEN — this \
                 build cannot mint one, because that means RS256-signing a JWT and ADR 0009 \
                 forbids in-tree cryptography. The service account needs \
                 roles/storage.objectAdmin on the bucket. Retention is the bucket's lifecycle \
                 policy and not this code's behaviour: the adapter deletes exactly what it is \
                 asked to and expires nothing",
            ),
            Self::AlloyDb => Some(
                "an AlloyDB cluster, instance, database and IAM database credentials — and a \
                 PostgreSQL client, which is the part that stops here. AlloyDB has no REST data \
                 plane: its REST API is admin-only (create a cluster, restore a backup, list \
                 instances), so the only route to a row is the PostgreSQL wire protocol: \
                 startup and parameter negotiation, SCRAM-SHA-256 authentication, the simple and \
                 extended query protocols with their Parse/Bind/Describe/Execute/Sync \
                 sequencing, and text \
                 and binary decoding for every type a column can hold. That is thousands of lines \
                 of security-sensitive parsing standing between untrusted bytes and the record of \
                 every order. Use StorageTarget::Engine, which is transactional and crash-safe on \
                 a single node, or add a PostgreSQL driver deliberately and record it in \
                 scripts/check-dependencies.sh",
            ),
            Self::Spanner => Some(
                "a Spanner instance, database and a service account with Cloud Spanner Database \
                 User — and a client this build does not have. Spanner is the honest borderline \
                 case of the three: it does publish a REST data plane, so this is a judgement \
                 rather than an impossibility. Reaching it means implementing sessions (create, \
                 keep alive against a one-hour idle expiry, recreate on NOT_FOUND, pool them per \
                 database), transaction selectors for single-use, read-only and read-write work, \
                 TypeCode-tagged parameter binding, and streaming partial result sets reassembled \
                 across resume tokens — a protocol implementation rather than an adapter. The \
                 efficient path is gRPC, so the HTTP/2-and-protobuf objection under Bigtable \
                 applies to anything built here for throughput, and the REST path needs an \
                 outbound TLS client this workspace does not have either. Use \
                 StorageTarget::Engine where one node suffices, or add a Spanner client \
                 deliberately",
            ),
            Self::Bigtable => Some(
                "a Bigtable instance and table and an account with roles/bigtable.user — and a \
                 gRPC client, which is the part that stops here. Bigtable's data plane (ReadRows, \
                 MutateRows, SampleRowKeys) is gRPC only; there is no JSON or REST surface for \
                 reading and writing rows. Reaching it in-tree would mean HTTP/2 framing, HPACK \
                 header compression, stream multiplexing and a protobuf codec, all written here, \
                 because the dependency policy permits serde and serde_json alone. That is a far \
                 larger and less reviewable surface than the database access is worth. Route \
                 high-throughput time series through Cloud Storage or BigQuery instead, or add a \
                 gRPC stack deliberately and record the decision in scripts/check-dependencies.sh",
            ),
            Self::Memorystore => Some(
                "the RESP adapter is built into this binary; what a deployment still supplies is \
                 the instance. Set QIP_MEMORYSTORE_ADDRESS to the instance address (the \
                 redis_host Terraform output) and QIP_MEMORYSTORE_AUTH to its AUTH string, since \
                 the instance is provisioned with auth_enabled = true. The instance uses \
                 PRIVATE_SERVICE_ACCESS, so the process needs VPC connectivity to the peered \
                 network and is unreachable from anywhere else. This client speaks RESP over \
                 plaintext TCP and has no TLS, so an instance with transit_encryption_mode = \
                 SERVER_AUTHENTICATION needs a TLS-terminating proxy in front of it, or an \
                 instance provisioned without transit encryption. Nothing stored there is \
                 durable: persistence_mode is DISABLED on purpose",
            ),
        }
    }
}

/// Resolves a storage target into a working adapter.
///
/// It builds adapters from what it was given and reads nothing itself: a
/// managed target's address, bucket and credential arrive through
/// [`StorageProvider::with_managed`], resolved by the composition root, and a
/// provider that was given none refuses the target naming the variable that
/// is missing. This crate once read those variables here, at the moment the
/// adapter was built — which put `std::env::var` in a library and read the
/// two credentials without the `_FILE` indirection every other secret has.
#[derive(Debug, Clone)]
pub struct StorageProvider {
    target: StorageTarget,
    root: std::path::PathBuf,
    clock: std::sync::Arc<dyn qip_core::Clock>,
    managed: crate::managed::ManagedSettings,
}

impl StorageProvider {
    /// A provider reading the host wall clock, with nothing resolved for a
    /// managed target.
    ///
    /// The provider is a composition-root helper, which is the one place a
    /// live [`qip_core::SystemClock`] belongs; a simulation or a replay calls
    /// [`StorageProvider::with_clock`] to inject its own.
    pub fn new(target: StorageTarget, root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            target,
            root: root.into(),
            clock: std::sync::Arc::new(qip_core::SystemClock),
            managed: crate::managed::ManagedSettings::none(),
        }
    }

    /// Build stores against an injected clock. Required for deterministic
    /// replay, where the timestamps the engine stamps on commits must be the
    /// simulated ones.
    pub fn with_clock(mut self, clock: std::sync::Arc<dyn qip_core::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Supply what a managed target needs. See
    /// [`crate::managed::ManagedSettings`].
    pub fn with_managed(mut self, managed: crate::managed::ManagedSettings) -> Self {
        self.managed = managed;
        self
    }

    pub fn target(&self) -> StorageTarget {
        self.target
    }

    /// Build a key-value store for `namespace`.
    ///
    /// A managed target returns [`Error::Unavailable`] naming the missing
    /// configuration. Failing at construction is deliberate: a deployment
    /// pointed at BigQuery must not quietly fall back to local files and
    /// appear to work.
    pub fn key_value(&self, namespace: &str) -> Result<std::sync::Arc<dyn crate::KeyValueStore>> {
        match self.target {
            StorageTarget::Memory => Ok(std::sync::Arc::new(crate::MemoryKeyValueStore::new())),
            StorageTarget::File => {
                let path = self.root.join(format!("{namespace}.json"));
                Ok(std::sync::Arc::new(crate::FileKeyValueStore::open(path)?))
            }
            StorageTarget::Engine => Ok(std::sync::Arc::new(crate::DurableStore::open(
                self.root.join(namespace),
                crate::EngineConfig::new(self.clock.clone()),
            )?)),
            // The one target whose adapter exists and whose *deployment* may
            // still be missing. The address and AUTH string are whatever the
            // composition root resolved into `with_managed` — a credential is
            // a property of the deployment, never of the build, and the root
            // is the one place that may read one. `redis_config` returns
            // `Unavailable` naming the variables when nothing was given, and
            // `connect` proves the instance answers before this returns, so a
            // bad address fails at start-up rather than during the first
            // cycle.
            StorageTarget::Memorystore => Ok(std::sync::Arc::new(
                crate::redis::RedisKeyValueStore::connect(self.managed.redis_config()?, namespace)?,
            )),
            // Implemented, and not this shape. Reporting these as "not built
            // into this binary" would send an operator looking for a missing
            // adapter that is right there; the useful answer names the shape
            // the target actually has.
            StorageTarget::CloudStorage => Err(Error::invalid(
                "Cloud Storage has an adapter in this build, but it is a blob store, not a \
                 key-value store: use StorageProvider::blobs. An object store can be made to \
                 look like a key-value store — one object per key — and it would be a trap. \
                 Every `len` would be a full bucket listing, every `put` a network round trip, \
                 and none of it transactional",
            )),
            StorageTarget::BigQuery => Err(Error::invalid(
                "BigQuery has an adapter in this build, but a warehouse is neither a key-value \
                 store nor a blob store: use qip_storage::BigQueryWarehouse directly, which \
                 streams rows in and runs queries. BigQuery has no primary-key lookup and \
                 charges by bytes scanned, so a `get` per key would be a full scan per key",
            )),
            other => Err(Error::unavailable(format!(
                "the {other:?} adapter is not built into this binary. It requires: {}. \
                 See docs/operations/external-dependencies.md",
                other
                    .required_configuration()
                    .unwrap_or("additional configuration")
            ))),
        }
    }

    /// Build a blob store, with the same failure semantics.
    pub fn blobs(&self, namespace: &str) -> Result<std::sync::Arc<dyn crate::BlobStore>> {
        match self.target {
            StorageTarget::Memory => Ok(std::sync::Arc::new(crate::MemoryBlobStore::new())),
            // Blobs are whole files either way: the engine's log buys nothing
            // for objects written once and never edited.
            StorageTarget::File | StorageTarget::Engine => Ok(std::sync::Arc::new(
                crate::FileBlobStore::open(self.root.join(namespace))?,
            )),
            // Refused on purpose, not for want of an adapter. Blobs are the
            // platform's evidence — simulation outputs, model artifacts,
            // reports someone will later be asked to justify — and the
            // Memorystore instance holds nothing durably and evicts under
            // memory pressure. Serving one from a cache would be the single
            // most direct way to turn "values that can be recomputed if lost"
            // into "the only copy of something that cannot".
            StorageTarget::Memorystore => Err(Error::unavailable(
                "Memorystore has a key-value adapter but deliberately no blob adapter: it holds \
                 nothing durably (persistence_mode = DISABLED) and evicts under memory pressure, \
                 so an artifact stored there would be evidence the platform could silently lose. \
                 Use the Cloud Storage or file blob store",
            )),
            // The target this provider was always meant to reach for an
            // archive. Configuration is whatever the composition root resolved
            // into `with_managed`, for the same reason Memorystore's is: a
            // credential is a property of the deployment, never of the build.
            // `cloud_storage_config` returns `Unavailable` naming the bucket
            // variable when nothing was given, and the store refuses every
            // operation — opening no connection — rather than writing to local
            // disk, which is the failure this whole target exists to avoid: a
            // deployment configured for a bucket that quietly wrote to a
            // container filesystem would pass every smoke test and lose the
            // archive when the pod was rescheduled.
            //
            // The namespace becomes a key prefix. One bucket holds many
            // namespaces, and without it two of them both writing `model.bin`
            // would be one object.
            StorageTarget::CloudStorage => {
                let config = self
                    .managed
                    .cloud_storage_config(self.clock.clone())?
                    .with_prefix(namespace);
                Ok(std::sync::Arc::new(crate::gcp::CloudStorageBlobStore::new(
                    config,
                )?))
            }
            StorageTarget::BigQuery => Err(Error::invalid(
                "BigQuery has an adapter in this build and it is not a blob store: a warehouse \
                 holds rows, not objects. Archive the object to Cloud Storage and record a row \
                 pointing at it — storing an artifact as a column would be billed on every scan \
                 of the table that never reads it",
            )),
            other => Err(Error::unavailable(format!(
                "the {other:?} blob adapter is not built into this binary. It requires: {}",
                other
                    .required_configuration()
                    .unwrap_or("additional configuration")
            ))),
        }
    }
}
