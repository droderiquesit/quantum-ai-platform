//! Ledger composition root: the sole writer of the ledger chain of
//! double-entry postings for fills, and its own read-side API (ADR 0100 §1,
//! §2). Posting logic itself is pure double-entry arithmetic in
//! `qip-portfolio`; this crate is the durable store and the write/read seams
//! around it.
//!
//! [`start`] is the whole composition, and `main` does nothing but read the
//! environment, resolve the fabric token and call it — so the test that
//! drives a ledger against a real broker listener drives the function a
//! deployment runs. The order is the composition-root order every binary
//! here holds to (`.claude/rules/architecture/00-boundaries.md`):
//!
//! 1. **Configuration is validated first** ([`config::LedgerConfig::parse`]),
//!    before anything is opened.
//! 2. **Storage is proven writable.** The store is opened — recovering
//!    whatever the last process committed — and a probe file is written,
//!    fsynced, read back and removed in its directory. A ledger whose
//!    directory had gone read-only would otherwise report healthy and then
//!    fail every apply, retrying for ever with nothing booked. On Spanner
//!    (ADR 0113, every deployment) the process first claims the writer row
//!    that fences out any earlier ledger process, then commits a probe row
//!    in its own transaction and reads it back; a database it cannot reach
//!    stops the start, and nothing falls back to a disk. Either way, the
//!    store refuses to open if its balances are not the fold of its own
//!    history ([`store::LedgerStore::verify_history`]).
//! 3. **The read API's port is bound** — over mutual TLS with the process's
//!    workload certificate (ADR 0112), requiring a client certificate the
//!    roots chain, and over plaintext only where the deployment is `local`
//!    and `QIP_TLS_MODE=plaintext` says so. Every balance and posting is
//!    served here, and a plaintext read API in a deployment would hand them
//!    to anything that can route a packet to the port.
//! 4. **One worker per partition starts consuming**, resuming where the
//!    store — not the broker — says ([`consumer`]).
//! 5. **Only then is the process ready.** `/health` answers 503 until both
//!    the storage probe and the bind have succeeded.
//!
//! Until this existed the binary exited at start: the read API's `serve`
//! always returned an error and `main` returned it, and the consumer was a
//! placeholder built against a hard-coded `127.0.0.1:9090` that nothing
//! called. Its comments described steps 3 and 4 as done.
//!
//! # What this process is not
//!
//! It submits nothing anywhere and holds no venue: it books fills other
//! processes simulated. A fill not marked simulated is refused and its cell
//! parked by the store (ADR 0106), counted on
//! `qip_ledger_live_fill_refused_total`, and nothing in this crate can book
//! one.

pub mod backend;
pub mod config;
pub mod consumer;
pub mod read_api;
pub mod spanner;
pub mod store;
pub mod telemetry;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration};
use qip_observability::metrics::Metrics;
use qip_storage::EngineConfig;
use qip_storage::GcpAccess;
use qip_storage::gcp::spanner::{SpannerConfig, SpannerKv};
use qip_transport::retry::Sleeper;
use qip_transport::server::{Server, ServerLimits};
use qip_transport::tls::{PeerNames, TlsServer, TlsSource, WorkloadIdentity};

use crate::config::{LedgerConfig, LedgerStorage};
use crate::consumer::{Connect, PartitionWorker, Statuses, WorkerConfig};
use crate::read_api::{ReadApi, Readiness};
use crate::spanner::SpannerBackend;
use crate::store::LedgerStore;
use crate::telemetry::LedgerTelemetry;

/// The probe file written and removed at start.
const STORAGE_PROBE: &str = ".qip-ledgerd-startup-probe";

/// Prove `directory` takes a durable write and gives it back.
fn prove_storage_writable(directory: &Path) -> Result<()> {
    let path = directory.join(STORAGE_PROBE);
    let probe = b"qip-ledgerd storage probe";
    {
        let mut file = std::fs::File::create(&path).map_err(|e| {
            Error::io(format!(
                "the ledger directory {} does not take a write ({e}); refusing to start a \
                 ledger that could book nothing",
                directory.display()
            ))
        })?;
        file.write_all(probe)?;
        file.sync_all()?;
    }
    let mut read_back = Vec::new();
    std::fs::File::open(&path)?.read_to_end(&mut read_back)?;
    if read_back != probe {
        return Err(Error::io(format!(
            "the ledger directory {} did not return the probe it was just given",
            directory.display()
        )));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// A started ledger: its address, and the handles that stop it.
pub struct Running {
    address: String,
    store: Arc<LedgerStore>,
    readiness: Arc<Readiness>,
    statuses: Statuses,
    metrics: Arc<Metrics>,
    stop_server: Arc<AtomicBool>,
    stop_workers: Arc<AtomicBool>,
    server_thread: Option<JoinHandle<()>>,
    workers: Vec<JoinHandle<()>>,
    /// The fabric link's issued certificate and its renewal loop, under
    /// `cas`; held so the loop lives as long as the ledger does.
    issued: Option<qip_transport::cas::CasIdentity>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Running {
    /// The read API's address, as bound.
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn store(&self) -> &Arc<LedgerStore> {
        &self.store
    }

    pub fn is_ready(&self) -> bool {
        self.readiness.is_ready()
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    /// Every partition worker's status.
    pub fn statuses(&self) -> BTreeMap<u32, consumer::PartitionStatus> {
        self.statuses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Stop the workers and the listener and release the store.
    pub fn stop(mut self) {
        self.stop_workers.store(true, Ordering::SeqCst);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        self.readiness.set_serving(false);
        self.stop_server.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.server_thread.take() {
            let _ = thread.join();
        }
        if let Some(issued) = self.issued.take() {
            issued.stop();
        }
    }

    /// Block until the listener stops, which in a deployment is when the
    /// process is killed.
    pub fn wait(mut self) {
        if let Some(thread) = self.server_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start the ledger described by `config`. `connect` builds one fabric
/// transport per partition worker; `main` gives it one that presents the
/// ledger's bearer token over HTTP. See the crate documentation for the
/// order and why it is the order.
pub fn start(
    config: &LedgerConfig,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    connect: Connect,
) -> Result<Running> {
    start_with_access(config, clock, sleeper, connect, GcpAccess::unconfigured())
}

/// Open the store `config` selects, proven writable. A Spanner store that
/// `access` cannot reach refuses here, and nothing falls back to a disk
/// (ADR 0113 §1).
fn open_store(
    config: &LedgerConfig,
    clock: &Arc<dyn Clock>,
    access: GcpAccess,
    telemetry: LedgerTelemetry,
) -> Result<LedgerStore> {
    match &config.storage {
        LedgerStorage::Local { archive_path } => {
            std::fs::create_dir_all(archive_path).map_err(|e| {
                Error::io(format!(
                    "the ledger directory {} cannot be created: {e}",
                    archive_path.display()
                ))
            })?;
            let store =
                LedgerStore::open(archive_path, EngineConfig::new(clock.clone()), telemetry)?;
            prove_storage_writable(archive_path)?;
            Ok(store)
        }
        LedgerStorage::Spanner { database, table } => {
            let kv =
                SpannerKv::new(SpannerConfig::new(database.clone(), access).with_table(table)?)?;
            // Unique per process start: the fence names the newest writer,
            // and a restarted process must displace its own predecessor.
            let writer = format!(
                "qip-ledgerd-{}-{}",
                std::process::id(),
                clock.now().as_nanos()
            );
            let store = LedgerStore::with_backend(
                Box::new(SpannerBackend::claim(kv, &writer)?),
                telemetry,
            )?;
            store.prove_writable(&writer)?;
            Ok(store)
        }
    }
}

/// [`start`], with the Google access a Spanner store is reached through.
/// `main` resolves it from the environment; a local store ignores it.
pub fn start_with_access(
    config: &LedgerConfig,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    connect: Connect,
    access: GcpAccess,
) -> Result<Running> {
    start_with_link(config, clock, sleeper, connect, access, None)
}

/// [`start_with_access`] for a fabric link whose certificate Certificate
/// Authority Service issues (`QIP_TLS_MODE=cas`, ADR 0112 §2). `issued` is
/// the identity `connect`'s transports present and the loop renewing it;
/// the ledger holds it for its life (dropping it stops renewal) and is
/// ready only while it holds a current certificate.
pub fn start_with_link(
    config: &LedgerConfig,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    connect: Connect,
    access: GcpAccess,
    issued: Option<qip_transport::cas::CasIdentity>,
) -> Result<Running> {
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let telemetry = LedgerTelemetry::new(metrics.clone());
    let store = Arc::new(open_store(config, &clock, access, telemetry.clone())?);
    let readiness = Arc::new(Readiness::default());
    if let Some(issued) = &issued {
        readiness.require_certificate(issued.status());
    }
    readiness.storage_proven();

    let statuses: Statuses = Arc::new(Mutex::new(BTreeMap::new()));
    let read_api = Arc::new(ReadApi::new(
        store.clone(),
        readiness.clone(),
        metrics.clone(),
        statuses.clone(),
    ));
    // ADR 0112: the read API is mutual TLS whenever this process has a
    // workload certificate, which outside a `local` deployment it must —
    // the same identity the fabric client presents. Loaded here, after
    // storage is proven and before the port is bound, as `qip-fabricd`
    // loads its listener's; a certificate that will not load stops the
    // ledger, and there is no fallback to the plaintext bind below.
    //
    // `/health` and `/metrics` stay behind the same handshake rather than on
    // a plaintext probe port of their own, as `qip-fabricd`'s are. A probe
    // port is admissible only if it carries nothing but readiness, and this
    // one would not: `/health` names every stream and partition with its
    // offset and last error, and nothing probes this process yet that a
    // separate listener would serve.
    //
    // Under `cas` the listener serves the issued identity — the same slot
    // the renewal loop replaces — so a renewed certificate reaches the next
    // reader without a restart, and a lapsed one is presented to nobody.
    let server_config = match (&config.workload_tls, &issued) {
        (Some(TlsSource::Files(paths)), _) => {
            Some(WorkloadIdentity::from_files(paths)?.server_config()?)
        }
        (Some(TlsSource::Issued(_)), Some(issued)) => Some(issued.identity().server_config()?),
        (Some(TlsSource::Issued(_)), None) => {
            return Err(Error::invalid(
                "QIP_TLS_MODE=cas and no issued identity was handed to the ledger; start it with \
                 `start_with_link` after `qip_storage::gcp::workload_certificate::\
                 workload_certificate`, as `main` does",
            ));
        }
        (None, _) => None,
    };
    let server = match server_config {
        Some(server_config) => Server::bind_tls(
            &config.listen_addr.to_string(),
            read_api,
            ServerLimits::default(),
            TlsServer {
                config: server_config,
                peers: PeerNames::new(config.admitted_peers.clone())?,
            },
        )?,
        None => Server::bind(
            &config.listen_addr.to_string(),
            read_api,
            ServerLimits::default(),
        )?,
    };
    let address = server.local_address()?;
    let stop_server = server.shutdown_handle();

    // Workers are built before any is started, so a transport or consumer
    // that cannot be constructed stops the start rather than leaving some
    // partitions consumed and others silently not.
    let stop_workers = Arc::new(AtomicBool::new(false));
    let poll = Duration::from_millis(i64::try_from(config.poll_ms).map_err(|_| {
        Error::invalid(format!(
            "{} of {} ms is too long",
            config::POLL_MS,
            config.poll_ms
        ))
    })?);
    let mut built = Vec::new();
    for partition in 0..config.fabric_partitions {
        built.push(PartitionWorker::new(
            WorkerConfig {
                stream: config.fabric_stream.clone(),
                partition,
                group: config.fabric_consumer_group.clone(),
                poll,
                clock: clock.clone(),
                sleeper: sleeper.clone(),
            },
            connect()?,
            store.clone(),
            telemetry.clone(),
            statuses.clone(),
            stop_workers.clone(),
        )?);
    }

    readiness.set_serving(true);
    let server_thread = std::thread::spawn(move || {
        let _ = server.serve();
    });
    let workers = built
        .into_iter()
        .map(|worker| std::thread::spawn(move || worker.run()))
        .collect();

    Ok(Running {
        address,
        store,
        readiness,
        statuses,
        metrics,
        stop_server,
        stop_workers,
        server_thread: Some(server_thread),
        workers,
        issued,
    })
}
