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
//!    fail every apply, retrying for ever with nothing booked.
//! 3. **The read API's port is bound.**
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

pub mod config;
pub mod consumer;
pub mod read_api;
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
use qip_transport::retry::Sleeper;
use qip_transport::server::{Server, ServerLimits};

use crate::config::LedgerConfig;
use crate::consumer::{Connect, PartitionWorker, Statuses, WorkerConfig};
use crate::read_api::{ReadApi, Readiness};
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
    std::fs::create_dir_all(&config.archive_path).map_err(|e| {
        Error::io(format!(
            "the ledger directory {} cannot be created: {e}",
            config.archive_path.display()
        ))
    })?;
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let telemetry = LedgerTelemetry::new(metrics.clone());
    let store = Arc::new(LedgerStore::open(
        &config.archive_path,
        EngineConfig::new(clock.clone()),
        telemetry.clone(),
    )?);
    prove_storage_writable(&config.archive_path)?;
    let readiness = Arc::new(Readiness::default());
    readiness.storage_proven();

    let statuses: Statuses = Arc::new(Mutex::new(BTreeMap::new()));
    let server = Server::bind(
        &config.listen_addr.to_string(),
        Arc::new(ReadApi::new(
            store.clone(),
            readiness.clone(),
            metrics.clone(),
            statuses.clone(),
        )),
        ServerLimits::default(),
    )?;
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
    })
}
