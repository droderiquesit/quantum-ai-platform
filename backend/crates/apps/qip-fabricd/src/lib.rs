//! Event fabric composition root: the in-tree single-node broker, sole writer
//! of every partition's batch chain and holder of its own segment-roll clock
//! (ADR 0100 § 1, § 2). Not a spool: the producer-side durable spool is
//! `apps/qip-edge-node::event_fabric`, behind the existing
//! `qip_edge::journal::Mirror` seam.
//!
//! [`start`] is the whole composition, and `main` does nothing but read the
//! environment and call it, so the test that drives a real listener drives
//! the same function a deployment would. The order inside it is the
//! composition-root order every binary here holds to, and each step exists
//! because of what goes wrong without it:
//!
//! 1. **The catalogue and the identities file are read and refused first.**
//!    A broker that opened its data directory and then found its catalogue
//!    unparseable has bumped a leader epoch for a process that never served.
//! 2. **Storage is proven writable before anything is bound.** `Broker::open`
//!    persists the leader epoch, the archive directory takes and returns a
//!    probe object, and every declared partition is opened so that a segment
//!    that cannot be recovered stops the start rather than the first produce.
//! 3. **Both listeners are bound.** The protocol's, and health's own.
//! 4. **Only then does it serve**, and only then does health say ready.
//!
//! # What this process is not
//!
//! It holds no order type, no venue and no autonomy level: it stores and
//! serves batches of records other processes wrote. The paper-trading
//! boundary's three layers are in Terraform, in the trading composition
//! roots and in `qip-edge`'s types, and nothing here can reach any of them.
//!
//! By default it is one broker with one disk (ADR 0100 §3): no follower, no
//! replication, no failover. With `QIP_EVENT_FABRIC_MANAGED_LOG=spanner` it
//! is seated at ADR 0111's managed consensus: every P0 and P1 batch is
//! committed by a Spanner transaction, under a leadership lease the database
//! fences, before its producer is answered; two or three instances per
//! region contend for each partition's lease, and the ones that do not hold
//! it follow the committed log onto their own disks. P2 to P4 stay on the
//! local path (ADR 0111 §2).
//! And it is not deployed: ADR 0010 keeps it out of the image matrix until
//! its placement is decided (ADR 0099 C8).

pub mod archiver;
pub mod bench;
pub mod config;
pub mod health;
pub mod spanner;
pub mod telemetry;

use std::io::Read;
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use qip_core::Clock;
use qip_core::error::{Error, Result};
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::policy::StreamPolicy;
use qip_observability::metrics::Metrics;
use qip_storage::gcp::GcpAccess;
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_storage::{BlobStore, CloudStorageBlobStore, CloudStorageConfig, FileBlobStore};
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::managed::ManagedLog;
use qip_streaming::event_fabric::service::{Handled, SegmentConfigFor, Service};
use qip_transport::cas::CasIdentity;
use qip_transport::event_fabric::auth::{self, IdentityTable};
use qip_transport::event_fabric::protocol::Route;
use qip_transport::server::{Handler, Method, Request, Response, Server, ServerLimits};
use qip_transport::tls::{PeerNames, TlsServer, TlsSource, WorkloadIdentity};

use crate::config::{ArchiveTarget, Config, ManagedBackend, ManagedConfig};
use crate::health::Health;
use crate::telemetry::FabricdTelemetry;

/// The largest catalogue read. The committed one is a few kilobytes; a file
/// past this is not the file that was meant.
const MAXIMUM_CATALOGUE_BYTES: u64 = 1024 * 1024;

/// The largest request body the protocol listener reads: a batch of four
/// mebibytes, hex-encoded, plus its envelope.
///
/// ponytail: one fixed ceiling for every route, well under the protocol's
/// own 64 MiB produce limit, because each connection may buffer this much
/// and sixty-four of them may be live (FABRIC-063). Raise it, per route, if
/// a producer ever needs to send a larger batch.
const MAXIMUM_REQUEST_BYTES: usize = 8 * 1024 * 1024 + 4096;

/// The key the archive's start-up probe is written under.
const ARCHIVE_PROBE_KEY: &str = "fabricd/startup-probe";

/// Read `path` whole, refusing a file larger than `limit`.
fn read_bounded(path: &Path, limit: u64, what: &str) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|e| {
        Error::io(format!(
            "{what} {} could not be opened: {e}",
            path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io(format!("{what} {} could not be read: {e}", path.display())))?;
    if bytes.len() as u64 > limit {
        return Err(Error::invalid(format!(
            "{what} {} is larger than {limit} bytes; check the path names the right file",
            path.display()
        )));
    }
    Ok(bytes)
}

fn parse_catalogue(path: &Path, bytes: &[u8]) -> Result<Catalogue> {
    Catalogue::parse(bytes).map_err(|error| {
        let message = format!(
            "the stream catalogue {}: {}",
            path.display(),
            error.message()
        );
        error.relabelled(message)
    })
}

/// Prove the archive takes a write and gives it back, before the process
/// says it is ready. An archive directory on a read-only or full volume
/// would otherwise be discovered by the first sealed segment, with every
/// quorum producer already waiting on it.
fn prove_archive_writable(store: &dyn BlobStore, leader_epoch: u64) -> Result<()> {
    let probe = leader_epoch.to_string().into_bytes();
    store.put(ARCHIVE_PROBE_KEY, probe.clone())?;
    if store.get(ARCHIVE_PROBE_KEY)?.as_deref() != Some(probe.as_slice()) {
        return Err(Error::io(
            "the archive did not return the probe object it was just given; refusing to start \
             on an archive that cannot be read back",
        ));
    }
    store.delete(ARCHIVE_PROBE_KEY)?;
    Ok(())
}

/// Open the configured archive target. A bucket the configuration cannot
/// reach — no endpoint, no credential — is refused here, and
/// `prove_archive_writable` then proves it takes and returns an object, so a
/// bucket that exists but this broker may not write is refused at start too.
fn open_archive(target: &ArchiveTarget, clock: Arc<dyn Clock>) -> Result<Arc<dyn BlobStore>> {
    match target {
        ArchiveTarget::Directory(directory) => Ok(Arc::new(FileBlobStore::open(directory)?)),
        ArchiveTarget::CloudStorage {
            bucket,
            prefix,
            endpoint,
            metadata_server,
            token_file,
        } => {
            let metadata = metadata_server.then_some("1");
            let access = GcpAccess::from_values(
                Some(endpoint),
                metadata,
                token_file.as_deref(),
                None,
                clock,
            )?;
            let store = CloudStorageBlobStore::new(
                CloudStorageConfig::new(bucket.clone())
                    .with_prefix(prefix.clone())
                    .with_access(access),
            )?;
            if !store.is_available() {
                return Err(Error::unavailable(format!(
                    "the archive bucket {bucket} cannot be reached: {}",
                    store.requirement()
                )));
            }
            Ok(Arc::new(store))
        }
    }
}

/// The protocol listener's handler: every request goes to the [`Service`],
/// and what it did is recorded here, at the seam where it became known.
struct DataPlane {
    service: Arc<Service>,
    telemetry: Arc<FabricdTelemetry>,
    clock: Arc<dyn Clock>,
}

impl DataPlane {
    fn record(&self, handled: &Handled, elapsed_ms: f64) {
        let seen = &handled.observation;
        if seen.route != Some(Route::Produce) {
            return;
        }
        // A produce to a stream the catalogue does not declare has no class;
        // it is counted under one literal rather than under the caller's
        // own stream name, which would be a label an unauthenticated typo
        // could mint.
        let class = seen.class.map_or("undeclared", |class| class.as_str());
        let stream = match (&seen.stream, seen.class) {
            (Some(stream), Some(_)) => stream.as_str(),
            _ => "undeclared",
        };
        self.telemetry.append(stream, class, seen.outcome);
        self.telemetry.append_latency_ms(class, elapsed_ms);
        if let Some(reason) = seen.reason {
            self.telemetry.refusal(class, reason);
        }
        if let (Some(partition), Some(high_watermark), Some(archived_through), Some(_)) = (
            seen.partition,
            seen.high_watermark,
            seen.archived_through,
            seen.class,
        ) {
            self.telemetry
                .high_watermark(stream, partition, high_watermark);
            self.telemetry
                .archived_through(stream, partition, archived_through);
        }
    }
}

impl Handler for DataPlane {
    fn handle(&self, request: &Request) -> Response {
        if request.method != Method::Post {
            return Response::json(
                405,
                r#"{"error":"every event-fabric route is a POST with a JSON body"}"#,
            );
        }
        let started = self.clock.now();
        let handled =
            self.service
                .handle(request.header(auth::HEADER), &request.path, &request.body);
        // Nanoseconds to milliseconds for a latency histogram: a statistic,
        // so an f64, and the one place this crate crosses from integers.
        let elapsed_ms = (self.clock.now().as_nanos() - started.as_nanos()) as f64 / 1_000_000.0;
        self.record(&handled, elapsed_ms);
        Response::new(handled.status, "application/json", handled.body)
    }
}

/// The health listener's handler. Holds the health state and the metric
/// registry and nothing that can reach a stream (FABRIC-073).
struct HealthPlane {
    health: Arc<Health>,
    metrics: Arc<Metrics>,
}

impl Handler for HealthPlane {
    fn handle(&self, request: &Request) -> Response {
        self.health.respond(&self.metrics, request)
    }
}

/// A started broker: its two addresses, and the handles that stop it.
pub struct Running {
    address: String,
    health_address: String,
    service: Arc<Service>,
    health: Arc<Health>,
    metrics: Arc<Metrics>,
    telemetry: Arc<FabricdTelemetry>,
    stop_data: Arc<AtomicBool>,
    stop_health: Arc<AtomicBool>,
    stop_housekeeping: Arc<AtomicBool>,
    data_thread: Option<JoinHandle<()>>,
    other_threads: Vec<JoinHandle<()>>,
    /// The listener's certificate and its renewal loop, under
    /// `QIP_TLS_MODE=cas`. Held here because dropping it stops the loop, and
    /// a broker whose loop stopped would serve until its certificate lapsed
    /// and then refuse everyone with nothing renewing it.
    issued: Option<CasIdentity>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("address", &self.address)
            .field("health_address", &self.health_address)
            .finish_non_exhaustive()
    }
}

/// Set `flag` and make one connection to `address`, which is what wakes a
/// blocking accept loop so that it reads the flag.
fn stop_listener(flag: &AtomicBool, address: &str) {
    flag.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(address);
}

impl Running {
    /// The address the fabric protocol is served on, as bound.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The address health and metrics are served on, as bound.
    pub fn health_address(&self) -> &str {
        &self.health_address
    }

    /// The protocol handler, for a caller in the same process.
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    pub fn health(&self) -> &Arc<Health> {
        &self.health
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    /// Stop serving the protocol and leave health and metrics up, reporting
    /// that the protocol is no longer served (FABRIC-073).
    pub fn stop_data_plane(&mut self) {
        self.health.set_serving(false);
        self.telemetry.serving(false);
        stop_listener(&self.stop_data, &self.address);
        if let Some(thread) = self.data_thread.take() {
            let _ = thread.join();
        }
    }

    /// Stop every thread and release both ports and the data directory.
    pub fn stop(mut self) {
        self.stop_data_plane();
        self.stop_housekeeping.store(true, Ordering::SeqCst);
        stop_listener(&self.stop_health, &self.health_address);
        for thread in self.other_threads.drain(..) {
            let _ = thread.join();
        }
        if let Some(issued) = self.issued.take() {
            issued.stop();
        }
    }

    /// Block until the protocol listener stops, which in a deployment is
    /// when the process is killed.
    pub fn wait(mut self) {
        if let Some(thread) = self.data_thread.take() {
            let _ = thread.join();
        }
    }
}

/// How a stream's partitions open their segment logs: archive-required
/// exactly when the stream's policy says so, rolled at the configured size
/// or the segment log's own default.
fn segment_config(clock: Arc<dyn Clock>, segment_bytes: Option<u64>) -> SegmentConfigFor {
    Arc::new(move |policy: &StreamPolicy| {
        let config =
            SegmentLogConfig::new(clock.clone()).with_archive_required(policy.archive_required());
        match segment_bytes {
            Some(bytes) => config.with_roll_after_bytes(bytes),
            None => config,
        }
    })
}

/// Pick up a changed catalogue: declare what it adds and replace the grants
/// in force. A catalogue that does not parse, or that the broker refuses,
/// changes nothing — the grants in force stay in force — and the refusal is
/// what this returns, for the health body to carry until the file is
/// corrected.
fn reload_catalogue(
    config: &Config,
    service: &Service,
    applied_catalogue: &mut Vec<u8>,
) -> Result<()> {
    let bytes = read_bounded(
        &config.catalogue,
        MAXIMUM_CATALOGUE_BYTES,
        "the stream catalogue",
    )?;
    if bytes == *applied_catalogue {
        return Ok(());
    }
    let catalogue = parse_catalogue(&config.catalogue, &bytes)?;
    service.apply_catalogue(&catalogue)?;
    *applied_catalogue = bytes;
    Ok(())
}

/// Start the broker described by `config`, archiving to the directory or
/// Cloud Storage bucket it names. See the crate documentation for the order and why it is the order.
pub fn start(config: &Config, clock: Arc<dyn Clock>) -> Result<Running> {
    start_with_archive(config, clock, None)
}

/// [`start`], archiving to `archive` instead of the configured target when
/// one is given. The seam a test stalls the archive through; `main`
/// never passes one.
pub fn start_with_archive(
    config: &Config,
    clock: Arc<dyn Clock>,
    archive: Option<Arc<dyn BlobStore>>,
) -> Result<Running> {
    start_with_seams(config, clock, archive, None)
}

/// Open the managed log `config` names (ADR 0111). Only Spanner exists.
fn open_managed_log(managed: &ManagedConfig, clock: Arc<dyn Clock>) -> Result<Arc<dyn ManagedLog>> {
    match &managed.backend {
        ManagedBackend::Spanner {
            database,
            endpoint,
            metadata_server,
            token_file,
        } => {
            let access = GcpAccess::from_values(
                Some(endpoint),
                metadata_server.then_some("1"),
                token_file.as_deref(),
                None,
                clock,
            )?;
            let rpc = spanner::HttpSpannerRpc::new(access)?;
            Ok(Arc::new(spanner::SpannerManagedLog::new(
                Arc::new(rpc),
                database,
                &managed.region,
            )))
        }
    }
}

/// [`start_with_archive`], and with `managed` as the managed log instead of
/// the one the configuration names. The seam the failover and chaos suites
/// put two brokers on one in-memory managed log through; `main` never passes
/// one. A managed log supplied to a configuration that names none is
/// refused: the instance name and lease length come from the configuration,
/// and a seam that silently invented them would test a broker no deployment
/// runs.
pub fn start_with_seams(
    config: &Config,
    clock: Arc<dyn Clock>,
    archive: Option<Arc<dyn BlobStore>>,
    managed: Option<Arc<dyn ManagedLog>>,
) -> Result<Running> {
    start_composed(config, clock, archive, managed, None)
}

/// [`start`] for a listener whose certificate Certificate Authority Service
/// issues (`QIP_TLS_MODE=cas`, ADR 0112 §2). `issued` is the identity and
/// renewal loop `main` started; the listener presents whatever certificate
/// it holds at each handshake, so renewals reach new connections with no
/// restart, and the broker reports not-ready whenever it holds none.
pub fn start_issued(
    config: &Config,
    clock: Arc<dyn Clock>,
    issued: CasIdentity,
) -> Result<Running> {
    start_composed(config, clock, None, None, Some(issued))
}

fn start_composed(
    config: &Config,
    clock: Arc<dyn Clock>,
    archive: Option<Arc<dyn BlobStore>>,
    managed: Option<Arc<dyn ManagedLog>>,
    issued: Option<CasIdentity>,
) -> Result<Running> {
    let catalogue_bytes = read_bounded(
        &config.catalogue,
        MAXIMUM_CATALOGUE_BYTES,
        "the stream catalogue",
    )?;
    let catalogue = parse_catalogue(&config.catalogue, &catalogue_bytes)?;
    let identities = IdentityTable::load(&config.identities_file)?;

    let archive_store: Arc<dyn BlobStore> = match archive {
        Some(store) => store,
        None => open_archive(&config.archive, clock.clone())?,
    };
    // Every broker reads its archive back (FABRIC-8079): an offset whose
    // local segment is gone hydrates from the archive, verified, rather than
    // reading as the end of the partition.
    let broker =
        Broker::open(&config.data_dir, clock.clone())?.with_archive_reader(archive_store.clone());
    let broker = match (&config.managed, managed) {
        (Some(settings), Some(log)) => broker.with_managed_log(log, &settings.instance)?,
        (Some(settings), None) => broker.with_managed_log(
            open_managed_log(settings, clock.clone())?,
            &settings.instance,
        )?,
        (None, Some(_)) => {
            return Err(Error::invalid(
                "a managed log was supplied to a configuration that names none; set \
                 QIP_EVENT_FABRIC_MANAGED_LOG and the instance it runs as",
            ));
        }
        (None, None) => broker,
    };
    let broker = Arc::new(broker);
    prove_archive_writable(archive_store.as_ref(), broker.leader_epoch())?;
    let service = Arc::new(Service::new(
        broker.clone(),
        identities,
        &catalogue,
        config.partitions,
        segment_config(clock.clone(), config.segment_bytes),
    )?);
    for (stream, partition_count) in broker.declared_streams() {
        for partition in 0..partition_count {
            broker.metadata(&stream, partition)?;
        }
    }

    let metrics = Arc::new(Metrics::new("qip-fabricd"));
    let telemetry = Arc::new(FabricdTelemetry::new(metrics.clone()));
    telemetry.leader_epoch(broker.leader_epoch());
    let health = Arc::new(Health::new(broker.leader_epoch()));
    health.storage_proven();
    // One lease pass before serving, so an instance that can lead starts
    // leading rather than refusing its first P0/P1 produce for a third of a
    // TTL. A pass that cannot reach the database is reported, not fatal: the
    // P2-P4 streams this broker also serves do not depend on it.
    if let Some(settings) = &config.managed {
        let report = broker.tend_leases(settings.lease_ttl_ms);
        if let Ok(report) = &report {
            telemetry.leases(report);
        }
        health.lease_pass(&report);
    }

    let data_plane = Arc::new(DataPlane {
        service: service.clone(),
        telemetry: telemetry.clone(),
        clock,
    });
    let data_limits = ServerLimits {
        max_body: MAXIMUM_REQUEST_BYTES,
        ..ServerLimits::default()
    };
    // ADR 0112: the protocol listener is mutual TLS whenever the
    // configuration carries a certificate, which outside a `local`
    // deployment it must. Loaded here — after storage is proven, before the
    // port is bound — and a certificate that will not load stops the broker:
    // there is no fallback to the plaintext listener below.
    //
    // Under `cas` the identity is the one `main` started issuance for, and
    // the readiness gate is its status: a broker with no current
    // certificate refuses every handshake (the identity presents nothing)
    // and says so on `/health`. A `cas` configuration handed no issued
    // identity is refused rather than bound with no certificate source.
    let server_config = match (&config.listener_tls, &issued) {
        (Some(TlsSource::Files(paths)), _) => {
            Some(WorkloadIdentity::from_files(paths)?.server_config()?)
        }
        (Some(TlsSource::Issued(_)), Some(issued)) => {
            health.require_certificate(issued.status());
            Some(issued.identity().server_config()?)
        }
        (Some(TlsSource::Issued(_)), None) => {
            return Err(Error::invalid(
                "QIP_TLS_MODE=cas and no issued identity was handed to the broker; start it \
                 with `start_issued` after `qip_storage::gcp::workload_certificate::\
                 workload_certificate`, as `main` does",
            ));
        }
        (None, _) => None,
    };
    let data = match server_config {
        Some(server_config) => Server::bind_tls(
            &config.listen,
            data_plane,
            data_limits,
            TlsServer {
                config: server_config,
                peers: PeerNames::new(config.admitted_peers.clone())?,
            },
        )?,
        None => Server::bind(&config.listen, data_plane, data_limits)?,
    };
    let probe = Server::bind(
        &config.health_listen,
        Arc::new(HealthPlane {
            health: health.clone(),
            metrics: metrics.clone(),
        }),
        ServerLimits::default(),
    )?;
    let address = data.local_address()?;
    let health_address = probe.local_address()?;
    let stop_data = data.shutdown_handle();
    let stop_health = probe.shutdown_handle();
    let stop_housekeeping = Arc::new(AtomicBool::new(false));

    health.set_serving(true);
    telemetry.serving(true);
    let data_thread = std::thread::spawn(move || {
        let _ = data.serve();
    });
    let health_thread = std::thread::spawn(move || {
        let _ = probe.serve();
    });
    // Two threads, not one loop doing both: an archive that has stopped
    // answering holds its thread inside one upload for as long as it likes,
    // and a grant an operator is trying to add or withdraw must not wait
    // behind it.
    let interval = Duration::from_millis(config.housekeeping_ms);
    let archive_thread = {
        let service = service.clone();
        let health = health.clone();
        let telemetry = telemetry.clone();
        let stop = stop_housekeeping.clone();
        // Indexed, so a segment can be read back once its local copy is
        // retained away or the disk holding it is gone (FABRIC-8079).
        let archiver = Archiver::new(archive_store).with_manifest_index();
        let hot_batches = config.hot_batches;
        std::thread::spawn(move || {
            let seals = archiver::SealCounts::default();
            while !stop.load(Ordering::SeqCst) {
                let outcome = archiver::pass(service.broker(), &archiver, &telemetry, &seals)
                    .and_then(|archived| {
                        archiver::retain(service.broker(), hot_batches, &telemetry)
                            .map(|_| archived)
                    });
                health.archive_pass(&outcome);
                std::thread::sleep(interval);
            }
        })
    };
    // ADR 0111's lease loop, on a thread of its own: a Spanner round trip
    // that stalls holds this thread, never the produce path's.
    let lease_thread = config.managed.as_ref().map(|settings| {
        let broker = service.broker().clone();
        let health = health.clone();
        let telemetry = telemetry.clone();
        let stop = stop_housekeeping.clone();
        let ttl_ms = settings.lease_ttl_ms;
        let renew_every = Duration::from_millis(ttl_ms / 3);
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(renew_every);
                let report = broker.tend_leases(ttl_ms);
                if let Ok(report) = &report {
                    telemetry.leases(report);
                }
                health.lease_pass(&report);
            }
        })
    });
    let catalogue_thread = {
        let config = config.clone();
        let service = service.clone();
        let health = health.clone();
        let stop = stop_housekeeping.clone();
        let mut applied_catalogue = catalogue_bytes;
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                health.catalogue_check(&reload_catalogue(
                    &config,
                    &service,
                    &mut applied_catalogue,
                ));
                std::thread::sleep(interval);
            }
        })
    };

    Ok(Running {
        address,
        health_address,
        service,
        health,
        metrics,
        telemetry,
        stop_data,
        stop_health,
        stop_housekeeping,
        data_thread: Some(data_thread),
        other_threads: [health_thread, archive_thread, catalogue_thread]
            .into_iter()
            .chain(lease_thread)
            .collect(),
        issued,
    })
}
