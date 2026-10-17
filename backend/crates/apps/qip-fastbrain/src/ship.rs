//! The desk's ledger records, carried to `qip-ledgerd` over the event fabric.
//!
//! Until this existed `Platform` turned every booked desk fill into a P1
//! [`OutcomeRecord`](qip_contracts::reflex::OutcomeRecord) and queued it
//! (`DeskLedgerFeed`), and **nothing in production took it**: the queue filled
//! to its 10,000-record bound, every fill after that was counted
//! `qip_desk_fills_unledgered_total{reason="capacity"}`, and the double-entry
//! ledger booked every cell's fills and none of the desk's.
//!
//! # Ack after delivery, never before
//!
//! [`DeskShipper::ship`] peeks the platform's pending records, builds the
//! fabric records ([`qip_kernel::desk_fabric`]), has a producer deliver them,
//! and only when the fabric has acknowledged the batch calls
//! `Platform::acknowledge_desk_outcomes`. A failed delivery therefore leaves
//! every record pending, and the retry resends the same bytes: the same event
//! ids, the same chain positions, so the producer's idempotence and the
//! ledger's duplicate rule (ADR 0105: chain position and digest) absorb the
//! repeat. A crash between the fabric's acknowledgement and the platform's
//! costs one resend and no posting. `Platform::take_desk_outcomes` is
//! destructive and is not used here for that reason.
//!
//! # Why a worker thread
//!
//! A P1 producer does not return until the broker has archived the batch
//! (`AckProfile::Archived`), which is at least the stream's seal age —
//! seconds under the producer's retry ladder when the broker is slow, and a
//! full timeout ladder when it is down. Run inline, that is the fast brain's
//! cycle loop parked behind a network call after every fill. So the producer
//! lives on a thread of its own ([`WORKER_THREAD`]) and the cycle loop's part
//! is a non-blocking exchange: collect what finished, acknowledge it, hand
//! over at most one batch more. Only [`DeskShipper::flush`] waits, once, on
//! shutdown, for a bound it is given.
//!
//! # What is not sent
//!
//! * A record the ledger would refuse. Every record is opened as the
//!   `FabricEnvelope` the ledger opens it as *before* the producer is touched
//!   (as `qip-edge-node`'s `FabricSink` does, R12-CONTRACT-7089): the broker
//!   would acknowledge it and the ledger, which never skips a record it
//!   cannot read, would stall the partition for ever. It is held back, left
//!   pending, counted `qip_desk_ship_total{outcome="refused"}` once, and put
//!   on the platform's capture problems so a person sees it.
//! * A record past a hole in the chain. If the feed was full when a fill was
//!   booked, that fill's record was never queued; the records behind it would
//!   arrive at a ledger that parks the chain on the discontinuity. They wait,
//!   named, rather than park it.
//! * Anything after a fence. A newer producer epoch owns the partition
//!   (another desk process); nothing this one sends will be accepted again.
//!   Shipping stops, the desk keeps trading, and the platform says why.
//!
//! # Configuration, and why an unset endpoint is not a refusal to start
//!
//! The variables are the edge node's, with the same meaning
//! ([`FABRIC_URL_VARIABLE`], [`FABRIC_TOKEN_VARIABLE`],
//! [`FABRIC_PARTITION_VARIABLE`]); the token comes through `qip_core::secret`'s
//! `_FILE` indirection only. A configured but invalid endpoint stops the
//! process. An **unset** endpoint does not: a developer's local desk has no
//! fabric, and refusing to start would make the desk unusable there. It is
//! instead loud — the start-up banner says `NOT SHIPPED`,
//! `qip_desk_ship_configured` reads `0`, and `qip_desk_outcomes_pending`
//! climbs — and bounded: the feed holds at most `DESK_OUTBOX_CAPACITY`
//! records, past which each fill is counted
//! `qip_desk_fills_unledgered_total{reason="capacity"}` and the book carries
//! on following the venue.

use qip_core::Clock;
use qip_core::error::{Error, Result};
use qip_core::sha256_hex;
use qip_events::event_fabric::bindings;
use qip_events::event_fabric::codec::{Batch, Record};
use qip_events::event_fabric::envelope::{FabricContext, FabricEnvelope};
use qip_events::event_fabric::policy::QosClass;
use qip_kernel::Platform;
use qip_kernel::desk_fabric::{fabric_record, p1_batch};
use qip_kernel::desk_ledger::DeskOutcome;
use qip_observability::metrics::{Metrics, labels, names};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::tls::LinkSecurity;
use qip_transport::{RetryPolicy, ThreadSleeper};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant};

/// The fabric endpoint, `https://host:port` over mutual TLS (ADR 0112), or
/// `http://host:port` on a `local` deployment that chose plaintext. The same
/// name and meaning as `qip-edge-node`'s. Unset: the desk's records are not
/// shipped.
pub const FABRIC_URL_VARIABLE: &str = "QIP_FABRIC_URL";
/// The bearer token the fabric checks, directly or through `_FILE`.
pub const FABRIC_TOKEN_VARIABLE: &str = "QIP_FABRIC_TOKEN";
/// The partition the desk's chain is produced to.
pub const FABRIC_PARTITION_VARIABLE: &str = "QIP_FABRIC_PARTITION";

/// The producer id the broker fences by: one desk, one id. A second desk
/// process starting against the same partition fences this one, which is
/// the intent — two desks writing one chain is a forked ledger.
pub const PRODUCER_ID: &str = "desk";

/// The region stamped on the envelopes opened before a send. The desk is the
/// centre's own book, and the ledger reads as `central`.
pub const DESK_REGION: &str = "central";

/// The worker thread's name.
pub const WORKER_THREAD: &str = "qip-desk-ship";

/// Records per batch. It bounds both what one failed delivery repeats and what
/// the platform clones per call.
pub const BATCH_RECORDS: usize = 256;

/// Payload bytes per batch. The broker charges each producer's payload bytes
/// against a per-second quota (262,144 in `infrastructure/event-fabric/
/// streams.local.json`), and refuses a batch that alone exceeds it for ever,
/// not for a second: a desk record is about a kilobyte of canonical JSON, so
/// 256 of them were over the quota, and a queue past a couple of hundred
/// records never delivered a single one. Half the quota, so a batch fits with
/// room for the one behind it; the acceptance suite refuses a value above the
/// catalogue's quota.
pub const BATCH_PAYLOAD_BYTES: usize = 131_072;

/// The longest the producer waits on any one call that leaves the process.
/// Explicit rather than the transport's default (2s connect, 15s read, 30s
/// write): the worker is off the cycle loop, but a wedged call also delays
/// every delivery behind it, and a shutdown flush waits for the call in
/// flight.
const CALL_TIMEOUTS: Timeouts = Timeouts::new(
    Duration::from_secs(2),
    Duration::from_secs(10),
    Duration::from_secs(10),
);

/// The first wait after a failed delivery, doubled per consecutive failure.
const BACKOFF_FLOOR: Duration = Duration::from_millis(500);
/// The longest wait between attempts.
const BACKOFF_CEILING: Duration = Duration::from_secs(30);

/// Read the fabric settings through `lookup`. `None` when no endpoint is set.
/// A partition without an endpoint, or an endpoint that is not a bare
/// `scheme://host:port`, is refused: a value that reads as a destination and
/// delivers nowhere is worse than an absent one.
pub fn read_settings(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<ShipSettings>> {
    let set = |name: &str| lookup(name).filter(|value| !value.trim().is_empty());
    match set(FABRIC_URL_VARIABLE) {
        Some(url) => ShipSettings::read(url.trim(), lookup).map(Some),
        None if set(FABRIC_PARTITION_VARIABLE).is_some() => Err(Error::invalid(format!(
            "configuration: {FABRIC_PARTITION_VARIABLE} is set and {FABRIC_URL_VARIABLE} is \
             not; a partition nothing produces to reads as a destination. Set \
             {FABRIC_URL_VARIABLE} or unset the partition"
        ))),
        None => Ok(None),
    }
}

/// Where the fabric is and how the desk authenticates to it.
#[derive(Debug)]
pub struct ShipSettings {
    pub url: String,
    pub partition: u32,
    token: BearerToken,
}

impl ShipSettings {
    fn read(url: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        if !(url.starts_with("http://") || url.starts_with("https://"))
            || url.trim_end_matches('/') != url
            || url.matches('/').count() > 2
        {
            return Err(Error::invalid(format!(
                "configuration: {FABRIC_URL_VARIABLE}={url} must be a scheme and authority, \
                 `https://host:port` (or `http://` on a local deployment), with no path; the \
                 producer appends each route"
            )));
        }
        let partition = match lookup(FABRIC_PARTITION_VARIABLE).filter(|v| !v.trim().is_empty()) {
            Some(value) => value.trim().parse::<u32>().map_err(|_| {
                Error::invalid(format!(
                    "configuration: {FABRIC_PARTITION_VARIABLE}={value} is not a partition number"
                ))
            })?,
            None => 0,
        };
        let token = BearerToken::resolve(
            FABRIC_TOKEN_VARIABLE,
            lookup(FABRIC_TOKEN_VARIABLE),
            lookup(&format!(
                "{FABRIC_TOKEN_VARIABLE}{}",
                qip_core::secret::FILE_SUFFIX
            )),
        )
        .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
        Ok(Self {
            url: url.to_string(),
            partition,
            token,
        })
    }

    /// The shipper, with its producer on the P1 outcomes stream.
    ///
    /// `link` is the security the composition root chose (ADR 0112) and the
    /// endpoint's scheme must agree with it: refused rather than upgraded or
    /// downgraded, because the configuration that wrote the URL believed the
    /// link was the other kind.
    pub fn shipper(self, clock: Arc<dyn Clock>, link: &LinkSecurity) -> Result<DeskShipper> {
        let scheme = format!("{}://", link.scheme());
        let Some(authority) = self.url.strip_prefix(&scheme) else {
            return Err(Error::invalid(format!(
                "configuration: {FABRIC_URL_VARIABLE}={} and the fabric link is {}; the \
                 endpoint's scheme must be `{scheme}` for this link",
                self.url,
                link.describe()
            )));
        };
        let seed = u64::from_str_radix(
            sha256_hex(PRODUCER_ID.as_bytes()).get(..16).unwrap_or("0"),
            16,
        )
        .unwrap_or(0);
        let class = QosClass::P1Outcomes;
        let producer = Producer::new(ProducerConfig {
            transport: Box::new(HttpTransport::over(link, authority, self.token)?),
            stream: bindings::REFLEX_OUTCOMES_STREAM.to_string(),
            partition: self.partition,
            producer_id: PRODUCER_ID.to_string(),
            qos_class: class,
            ack_profile: class.ack_floor(),
            retry_policy: RetryPolicy::default(),
            breaker_policy: BreakerPolicy::default(),
            clock,
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: seed,
            breaker_seed: seed.wrapping_add(1),
            timeouts: CALL_TIMEOUTS,
        })?;
        DeskShipper::shipping(
            producer,
            DESK_REGION,
            format!("{} over {}", self.url, link.describe()),
        )
    }
}

/// The queue of desk ledger records the shipper drains, and the places it
/// reports to. `Platform` is the production implementation; the seam exists so
/// a test can put a real platform behind a view that hands the shipper a
/// record the platform itself would never queue (a fill not marked simulated,
/// a hole in the chain), which is the only way to drive the shipper's refusal
/// end to end.
pub trait DeskQueue {
    /// How many records are queued and not acknowledged.
    fn pending_len(&self) -> usize;
    /// Up to `limit` of the oldest queued records, without removing any.
    fn peek(&self, limit: usize) -> Vec<DeskOutcome>;
    /// Drop every record through chain position `through_sequence`. Called only
    /// once the fabric has acknowledged them.
    fn acknowledge(&mut self, through_sequence: u64) -> Result<usize>;
    /// Surface a problem beside the platform's other capture problems.
    fn note_problem(&mut self, problem: String);
    /// Where the shipper's series are recorded.
    fn metrics(&self) -> Arc<Metrics>;
}

impl DeskQueue for Platform {
    fn pending_len(&self) -> usize {
        self.pending_desk_outcomes().len()
    }

    fn peek(&self, limit: usize) -> Vec<DeskOutcome> {
        let pending = self.pending_desk_outcomes();
        pending[..pending.len().min(limit)].to_vec()
    }

    fn acknowledge(&mut self, through_sequence: u64) -> Result<usize> {
        self.acknowledge_desk_outcomes(through_sequence)
    }

    fn note_problem(&mut self, problem: String) {
        self.note_desk_ship_problem(problem);
    }

    fn metrics(&self) -> Arc<Metrics> {
        self.telemetry().metrics.clone()
    }
}

/// What a [`DeskShipper::ship`] call found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipOutcome {
    /// Nothing is pending and nothing is in flight.
    Idle,
    /// A batch was acknowledged by the fabric in this call and dropped from
    /// the platform's queue.
    Shipped,
    /// A batch is with the producer; its result is collected by a later call.
    InFlight,
    /// The fabric did not take a batch (or the shipper is backing off after
    /// one that failed). The records stay pending and are resent.
    Deferred,
    /// The next record cannot be sent: the ledger would refuse it, or the
    /// chain has a hole before it. It stays pending.
    Refused,
    /// A newer producer owns the partition. Shipping has stopped.
    Fenced,
    /// No fabric is configured; records only queue.
    Unshipped,
}

/// What one [`DeskShipper::ship`] call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShipReport {
    pub outcome: ShipOutcome,
    /// Records dropped from the platform's queue in this call, because the
    /// fabric acknowledged them.
    pub acknowledged: usize,
    /// Records still pending after the call.
    pub pending: usize,
    pub in_flight: bool,
    /// The reason, for an outcome that has one.
    pub detail: Option<String>,
}

/// What the worker found when it tried a batch.
#[derive(Debug)]
enum Done {
    Delivered { through: u64 },
    Deferred(String),
    Fenced(String),
}

#[derive(Debug)]
struct Job {
    batch: Batch,
    through: u64,
}

/// What the shipper needs of the producer after the producer has moved to its
/// thread, to open envelopes the way the ledger will.
#[derive(Debug)]
struct Route {
    stream: String,
    partition: u32,
    producer_id: String,
}

#[derive(Debug)]
struct Shipping {
    route: Route,
    region: String,
    jobs: SyncSender<Job>,
    results: Receiver<Done>,
    /// The chain position of the batch with the worker, if any.
    in_flight: Option<u64>,
    /// The chain position the next record must carry, once one has been
    /// delivered. `None` until then: after a restart the first pending record
    /// is whatever the platform's chain continues at.
    next_expected: Option<u64>,
    /// The chain position last reported refused, so a record that stays
    /// refused is counted and put before a person once and not once a cycle.
    refused_at: Option<u64>,
    fenced: bool,
    worker_lost: bool,
    failures: u32,
    not_before: Option<Instant>,
}

#[derive(Debug)]
enum Mode {
    Unshipped { variable: &'static str },
    Shipping(Box<Shipping>),
}

/// Ships the desk's pending ledger records. See the module documentation.
#[derive(Debug)]
pub struct DeskShipper {
    mode: Mode,
    destination: String,
    described: bool,
}

impl DeskShipper {
    /// A shipper with no fabric: records queue, and every report and the
    /// metrics say so.
    pub fn unshipped(variable: &'static str) -> Self {
        Self {
            mode: Mode::Unshipped { variable },
            destination: String::new(),
            described: false,
        }
    }

    /// A shipper delivering through `producer`, which must be the P1 outcomes
    /// producer: one pointed at another stream would put the desk's fills
    /// where the ledger never reads them.
    pub fn shipping(
        producer: Producer,
        region: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self> {
        let region = region.into();
        if region.trim().is_empty() {
            return Err(Error::invalid(
                "the desk shipper needs a region: every envelope it sends is attributed to one",
            ));
        }
        if producer.stream() != bindings::REFLEX_OUTCOMES_STREAM {
            return Err(Error::invalid(format!(
                "the desk ships its fills to {}; it was handed a producer for {}",
                bindings::REFLEX_OUTCOMES_STREAM,
                producer.stream()
            )));
        }
        let route = Route {
            stream: producer.stream().to_string(),
            partition: producer.partition(),
            producer_id: producer.producer_id().to_string(),
        };
        let (jobs, job_rx) = std::sync::mpsc::sync_channel::<Job>(1);
        let (done_tx, results) = std::sync::mpsc::channel::<Done>();
        // Detached: dropping the shipper closes the job channel, which ends the
        // worker after the call it is in; joining here would hang a shutdown on
        // an unreachable fabric.
        std::thread::Builder::new()
            .name(WORKER_THREAD.to_string())
            .spawn(move || work(producer, &job_rx, &done_tx))
            .map_err(|error| {
                Error::io(format!("cannot start the {WORKER_THREAD} thread: {error}"))
            })?;
        Ok(Self {
            mode: Mode::Shipping(Box::new(Shipping {
                route,
                region,
                jobs,
                results,
                in_flight: None,
                next_expected: None,
                refused_at: None,
                fenced: false,
                worker_lost: false,
                failures: 0,
                not_before: None,
            })),
            destination: destination.into(),
            described: false,
        })
    }

    /// The start-up banner line: where the desk's records go, or loudly that
    /// they go nowhere.
    pub fn banner(&self) -> String {
        match &self.mode {
            Mode::Unshipped { variable } => format!(
                "desk outcomes: NOT SHIPPED ({variable} unset); the desk's fills queue for the \
                 ledger up to {} records, past which each is counted \
                 qip_desk_fills_unledgered_total{{reason=\"capacity\"}} and qip-ledgerd never \
                 sees it",
                qip_kernel::desk_ledger::DESK_OUTBOX_CAPACITY
            ),
            Mode::Shipping(shipping) => format!(
                "desk outcomes: shipping to {} as producer {PRODUCER_ID} on {}:{}",
                self.destination, shipping.route.stream, shipping.route.partition
            ),
        }
    }

    /// Whether a fabric is configured.
    pub fn is_shipping(&self) -> bool {
        matches!(self.mode, Mode::Shipping(_))
    }

    /// Collect what the worker finished, acknowledge it on the platform, and
    /// hand the worker at most one batch more. Never waits on the fabric.
    ///
    /// `Err` only for an inconsistency the shipper cannot repair — the
    /// platform refusing an acknowledgement for a batch the shipper itself
    /// took from it. A delivery that failed is a [`ShipReport`], not an
    /// error: the desk keeps trading and the records stay pending.
    pub fn ship(&mut self, platform: &mut dyn DeskQueue) -> Result<ShipReport> {
        self.step(platform, Wait::No)
    }

    /// The final, bounded attempt on the way out: keep shipping until nothing
    /// is pending, something refuses, or `budget` is spent. Waits for the
    /// batch in flight, which [`Self::ship`] never does, and ignores the
    /// back-off, because this is the last chance this process has.
    pub fn flush(&mut self, platform: &mut dyn DeskQueue, budget: Duration) -> Result<ShipReport> {
        let deadline = Instant::now() + budget;
        let mut acknowledged = 0usize;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut report = self.step(platform, Wait::Final(remaining))?;
            acknowledged += report.acknowledged;
            report.acknowledged = acknowledged;
            let finished = report.pending == 0 && !report.in_flight;
            let stuck = matches!(
                report.outcome,
                ShipOutcome::Refused
                    | ShipOutcome::Fenced
                    | ShipOutcome::Unshipped
                    | ShipOutcome::Deferred
            );
            if finished || stuck || Instant::now() >= deadline {
                return Ok(report);
            }
        }
    }

    fn step(&mut self, platform: &mut dyn DeskQueue, wait: Wait) -> Result<ShipReport> {
        let metrics = platform.metrics();
        if !self.described {
            self.described = true;
            metrics.describe(
                names::DESK_SHIP,
                "delivery of the desk's ledger records to the event fabric, by outcome: shipped, \
                 deferred, refused (held back so the ledger's partition is not stalled), fenced",
            );
            metrics.describe(
                names::DESK_OUTCOMES_PENDING,
                "desk ledger records booked and not yet acknowledged as delivered to the fabric",
            );
            metrics.describe(
                names::DESK_SHIP_CONFIGURED,
                "1 when the desk has a fabric to ship its ledger records to, 0 when it only queues them",
            );
        }
        metrics.gauge(
            names::DESK_SHIP_CONFIGURED,
            labels([]),
            if self.is_shipping() { 1.0 } else { 0.0 },
        );
        let report = match &mut self.mode {
            Mode::Unshipped { variable } => ShipReport {
                outcome: ShipOutcome::Unshipped,
                acknowledged: 0,
                pending: platform.pending_len(),
                in_flight: false,
                detail: Some(format!("{variable} is unset")),
            },
            Mode::Shipping(shipping) => shipping.step(platform, wait)?,
        };
        metrics.gauge(
            names::DESK_OUTCOMES_PENDING,
            labels([]),
            report.pending as f64,
        );
        Ok(report)
    }
}

#[derive(Clone, Copy, Debug)]
enum Wait {
    /// The cycle loop: never wait.
    No,
    /// Shutdown: wait up to this long for the batch in flight, and ignore
    /// the back-off.
    Final(Duration),
}

impl Shipping {
    fn step(&mut self, platform: &mut dyn DeskQueue, wait: Wait) -> Result<ShipReport> {
        let mut acknowledged = 0usize;
        let mut shipped = false;
        let mut failed: Option<(ShipOutcome, String)> = None;

        // 1. What the worker finished.
        if self.in_flight.is_some() {
            let first = match wait {
                Wait::No => self.results.try_recv().map_err(|e| match e {
                    TryRecvError::Empty => RecvTimeoutError::Timeout,
                    TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
                }),
                Wait::Final(budget) => self.results.recv_timeout(budget),
            };
            match first {
                Ok(done) => {
                    self.settle(platform, done, &mut acknowledged, &mut shipped, &mut failed)?
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.lose_worker(platform, &mut failed),
            }
        }

        // 2. At most one batch more.
        let mut refused: Option<String> = None;
        if self.fenced {
            // Nothing to do, and nothing to count again.
        } else if self.worker_lost || self.in_flight.is_some() {
            // The worker is gone, or busy with the batch already out.
        } else if matches!(wait, Wait::No)
            && self.not_before.is_some_and(|until| Instant::now() < until)
        {
            failed.get_or_insert((
                ShipOutcome::Deferred,
                "backing off after a failed delivery".to_string(),
            ));
        } else {
            refused = self.hand_over(platform)?;
        }

        let pending = platform.pending_len();
        let in_flight = self.in_flight.is_some();
        let (outcome, detail) = if self.fenced {
            (
                ShipOutcome::Fenced,
                Some("a newer producer owns the partition".to_string()),
            )
        } else if let Some(why) = refused {
            (ShipOutcome::Refused, Some(why))
        } else if let Some((outcome, why)) = failed {
            (outcome, Some(why))
        } else if shipped {
            (ShipOutcome::Shipped, None)
        } else if in_flight {
            (ShipOutcome::InFlight, None)
        } else {
            (ShipOutcome::Idle, None)
        };
        Ok(ShipReport {
            outcome,
            acknowledged,
            pending,
            in_flight,
            detail,
        })
    }

    fn settle(
        &mut self,
        platform: &mut dyn DeskQueue,
        done: Done,
        acknowledged: &mut usize,
        shipped: &mut bool,
        failed: &mut Option<(ShipOutcome, String)>,
    ) -> Result<()> {
        let metrics = platform.metrics();
        let expected = self.in_flight.take();
        match done {
            Done::Delivered { through } => {
                if expected != Some(through) {
                    return Err(Error::invalid(format!(
                        "the desk worker delivered through chain position {through} but the \
                         batch in flight ended at {expected:?}"
                    )));
                }
                // The only place a desk record leaves the queue: the fabric
                // has it. Recording that, and dropping the records, can fail
                // (the platform journals the acknowledgement before it applies
                // it, and a full or failing log refuses the write). The fabric
                // already holds the batch, so nothing is lost either way: the
                // records stay pending and are resent, which the producer's
                // idempotence and the ledger's chain-position dedupe absorb. It
                // must not end the desk, which is trading, so it is counted,
                // surfaced and retried under the usual back-off instead of
                // returned.
                match platform.acknowledge(through) {
                    Ok(dropped) => {
                        *acknowledged += dropped;
                        self.next_expected = through.checked_add(1);
                        self.failures = 0;
                        self.not_before = None;
                        *shipped = true;
                        metrics.count(names::DESK_SHIP, labels([("outcome", "shipped")]));
                    }
                    Err(error) => {
                        self.failures = self.failures.saturating_add(1);
                        self.not_before = Some(Instant::now() + backoff(self.failures));
                        metrics.count(names::DESK_SHIP, labels([("outcome", "unlogged")]));
                        let why = format!(
                            "the fabric holds the desk's ledger records through chain position \
                             {through}, but the platform could not record the acknowledgement: {}",
                            error.message()
                        );
                        if self.failures == 1 {
                            platform.note_problem(format!(
                                "{why}; they stay pending and are resent, and the ledger \
                                 absorbs the duplicate"
                            ));
                        }
                        *failed = Some((ShipOutcome::Deferred, why));
                    }
                }
            }
            Done::Deferred(why) => {
                self.failures = self.failures.saturating_add(1);
                self.not_before = Some(Instant::now() + backoff(self.failures));
                metrics.count(names::DESK_SHIP, labels([("outcome", "deferred")]));
                if self.failures == 1 {
                    platform.note_problem(format!(
                        "the desk's ledger records were not delivered to the event fabric ({why}); \
                         they stay pending and are resent"
                    ));
                }
                *failed = Some((ShipOutcome::Deferred, why));
            }
            Done::Fenced(why) => {
                self.fenced = true;
                metrics.count(names::DESK_SHIP, labels([("outcome", "fenced")]));
                platform.note_problem(format!(
                    "the desk stopped shipping its ledger records: {why}. A newer desk process \
                     owns the partition; this one keeps trading and its records stay pending"
                ));
            }
        }
        Ok(())
    }

    fn lose_worker(
        &mut self,
        platform: &mut dyn DeskQueue,
        failed: &mut Option<(ShipOutcome, String)>,
    ) {
        self.in_flight = None;
        self.worker_lost = true;
        let why = "the desk shipper's worker thread ended".to_string();
        platform
            .metrics()
            .count(names::DESK_SHIP, labels([("outcome", "deferred")]));
        platform.note_problem(format!(
            "{why}; the desk's ledger records stay pending until the process restarts"
        ));
        *failed = Some((ShipOutcome::Deferred, why));
    }

    /// Build, preflight and hand over the next batch. `Ok(Some(why))` when the
    /// next record is held back.
    fn hand_over(&mut self, platform: &mut dyn DeskQueue) -> Result<Option<String>> {
        let candidates = platform.peek(BATCH_RECORDS);
        if candidates.is_empty() {
            return Ok(None);
        }
        let (batch, through) = match self.prepare(&candidates) {
            Ok(prepared) => prepared,
            Err((sequence, why)) => {
                let why = format!(
                    "desk record at chain position {sequence} is held back and every record \
                     behind it waits: {why}. It stays pending; a person should look, because \
                     sending it would let the broker acknowledge a record the ledger cannot \
                     read and stall the ledger's partition for ever"
                );
                if self.refused_at != Some(sequence) {
                    self.refused_at = Some(sequence);
                    platform
                        .metrics()
                        .count(names::DESK_SHIP, labels([("outcome", "refused")]));
                    platform.note_problem(why.clone());
                }
                return Ok(Some(why));
            }
        };
        self.refused_at = None;
        match self.jobs.try_send(Job { batch, through }) {
            Ok(()) => {
                self.in_flight = Some(through);
                Ok(None)
            }
            Err(TrySendError::Full(_)) => Err(Error::invalid(
                "the desk worker's queue was full with nothing in flight",
            )),
            Err(TrySendError::Disconnected(_)) => {
                self.worker_lost = true;
                platform.note_problem(
                    "the desk shipper's worker thread ended; the desk's ledger records stay \
                     pending until the process restarts"
                        .to_string(),
                );
                Ok(Some("the desk shipper's worker thread ended".to_string()))
            }
        }
    }

    /// R12-CONTRACT-7089, as `FabricSink` does it: open every record as the
    /// envelope the ledger will open it as before the producer is touched.
    /// The first record that would be refused cuts the batch; `Err((index,
    /// why))` when that is the first record.
    fn preflight(&self, records: Vec<Record>) -> std::result::Result<Batch, (usize, String)> {
        let context = FabricContext {
            stream: self.route.stream.clone(),
            region: self.region.clone(),
            partition: self.route.partition,
            qos_class: QosClass::P1Outcomes,
            auth_context: format!("producer:{}", self.route.producer_id),
            provenance: "qip-fastbrain.desk".to_string(),
        };
        let mut probe = p1_batch(records.clone()).map_err(|e| (0, e.message().to_string()))?;
        probe.producer_id = self.route.producer_id.clone();
        let mut sendable = 0usize;
        let mut first_refusal: Option<String> = None;
        for index in 0..probe.records.len() {
            match FabricEnvelope::open(&probe, index, &context, &bindings::ALL) {
                Ok(_) => sendable += 1,
                Err(error) => {
                    first_refusal = Some(error.message().to_string());
                    break;
                }
            }
        }
        if sendable == 0 {
            return Err((
                0,
                first_refusal
                    .unwrap_or_else(|| "the ledger's envelope check refused it".to_string()),
            ));
        }
        let mut kept = records;
        kept.truncate(sendable);
        let batch = p1_batch(kept).map_err(|e| (0, e.message().to_string()))?;
        let mut check = batch.clone();
        check.producer_id = self.route.producer_id.clone();
        FabricEnvelope::open_all(&check, &context, &bindings::ALL)
            .map_err(|e| (0, e.message().to_string()))?;
        Ok(batch)
    }

    /// The longest prefix of `candidates` the ledger would take, as a batch,
    /// and the chain position it ends at. `Err((sequence, why))` when not even
    /// the first can be sent.
    fn prepare(
        &self,
        candidates: &[DeskOutcome],
    ) -> std::result::Result<(Batch, u64), (u64, String)> {
        let head = candidates
            .first()
            .map(|o| o.record.entry.sequence)
            .unwrap_or_default();
        let mut records = Vec::with_capacity(candidates.len());
        let mut payload_bytes = 0usize;
        let mut previous: Option<u64> = None;
        for outcome in candidates {
            let sequence = outcome.record.entry.sequence;
            let expected = match previous {
                Some(last) => last.checked_add(1),
                None => self.next_expected,
            };
            if let Some(expected) = expected
                && sequence != expected
            {
                if records.is_empty() {
                    return Err((
                        sequence,
                        format!(
                            "the chain has a hole: position {expected} was expected next and \
                             position {sequence} is next in the queue (a fill booked while the \
                             feed was full has no queued record); the ledger parks a chain on \
                             a discontinuity"
                        ),
                    ));
                }
                break;
            }
            match fabric_record(outcome) {
                Ok(record) => {
                    // The first record always goes: a record over the byte
                    // budget on its own is the broker's refusal to make, not
                    // a reason to hold the whole chain behind it.
                    if !records.is_empty()
                        && payload_bytes + record.payload.len() > BATCH_PAYLOAD_BYTES
                    {
                        break;
                    }
                    payload_bytes += record.payload.len();
                    records.push(record);
                }
                Err(error) => {
                    if records.is_empty() {
                        return Err((sequence, error.message().to_string()));
                    }
                    break;
                }
            }
            previous = Some(sequence);
        }
        if records.is_empty() {
            return Err((head, "no record could be built".to_string()));
        }

        let batch = self.preflight(records).map_err(|(index, why)| {
            (
                candidates[index.min(candidates.len() - 1)]
                    .record
                    .entry
                    .sequence,
                why,
            )
        })?;
        let sendable = batch.records.len();
        let through = candidates[sendable - 1].record.entry.sequence;
        Ok((batch, through))
    }
}

/// The wait after `failures` consecutive failed deliveries.
fn backoff(failures: u32) -> Duration {
    let doubled = BACKOFF_FLOOR.saturating_mul(1u32 << failures.saturating_sub(1).min(10));
    doubled.min(BACKOFF_CEILING)
}

/// The worker: one producer, one batch at a time, off the cycle loop.
fn work(mut producer: Producer, jobs: &Receiver<Job>, done: &std::sync::mpsc::Sender<Done>) {
    while let Ok(job) = jobs.recv() {
        let result = deliver(&mut producer, job);
        if done.send(result).is_err() {
            break;
        }
    }
}

fn deliver(producer: &mut Producer, job: Job) -> Done {
    let fenced = |producer: &Producer, error: &Error| {
        producer
            .is_fenced()
            .then(|| Done::Fenced(error.message().to_string()))
    };
    // An epoch is taken on first use; `init` is the broker's statement of
    // where this producer's dense sequence continues.
    if producer.epoch().is_none()
        && let Err(error) = producer.init()
    {
        return fenced(producer, &error)
            .unwrap_or_else(|| Done::Deferred(error.message().to_string()));
    }
    if producer.is_fenced() {
        return Done::Fenced(format!(
            "producer {} on {}:{} holds an epoch a newer producer has fenced",
            producer.producer_id(),
            producer.stream(),
            producer.partition()
        ));
    }
    match producer.send(job.batch) {
        Ok(_) => Done::Delivered {
            through: job.through,
        },
        Err(error) => {
            fenced(producer, &error).unwrap_or_else(|| Done::Deferred(error.message().to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    const FAKE_BEARER: &str = "obviously-fake-desk-test-bearer-0123456789";

    #[test]
    fn an_unset_endpoint_is_no_fabric_and_a_lone_partition_is_refused() {
        assert!(
            read_settings(&lookup(&[]))
                .expect("unset is allowed")
                .is_none()
        );
        let error = read_settings(&lookup(&[(FABRIC_PARTITION_VARIABLE, "3")]))
            .expect_err("a partition nothing produces to is a stray value");
        assert!(error.message().starts_with("configuration:"));
    }

    #[test]
    fn a_configured_but_invalid_endpoint_is_refused_at_start() {
        for url in [
            "fabricd:7100",
            "http://fabricd:7100/v1",
            "http://fabricd:7100/",
        ] {
            let error = read_settings(&lookup(&[
                (FABRIC_URL_VARIABLE, url),
                (FABRIC_TOKEN_VARIABLE, FAKE_BEARER),
            ]))
            .expect_err(url);
            assert!(error.message().starts_with("configuration:"), "{url}");
        }
        // A configured endpoint with no token is refused too, naming the _FILE form.
        let error = read_settings(&lookup(&[(FABRIC_URL_VARIABLE, "http://fabricd:7100")]))
            .expect_err("no token");
        assert!(error.message().contains("_FILE"), "{}", error.message());
        // And the premise: the same endpoint with a token is accepted.
        let settings = read_settings(&lookup(&[
            (FABRIC_URL_VARIABLE, "http://fabricd:7100"),
            (FABRIC_TOKEN_VARIABLE, FAKE_BEARER),
            (FABRIC_PARTITION_VARIABLE, "2"),
        ]))
        .expect("valid")
        .expect("configured");
        assert_eq!(settings.partition, 2);
    }

    #[test]
    fn an_endpoint_whose_scheme_disagrees_with_the_link_is_refused() {
        let clock: Arc<dyn Clock> = Arc::new(qip_core::ManualClock::new(
            qip_core::Timestamp::from_secs(1_800_000_000),
        ));
        let tls_endpoint = read_settings(&lookup(&[
            (FABRIC_URL_VARIABLE, "https://fabricd:7100"),
            (FABRIC_TOKEN_VARIABLE, FAKE_BEARER),
        ]))
        .expect("https is accepted")
        .expect("configured");
        assert!(
            tls_endpoint
                .shipper(Arc::clone(&clock), &LinkSecurity::plaintext_for_tests())
                .is_err(),
            "an https endpoint was downgraded onto a plaintext link"
        );
        let plaintext = read_settings(&lookup(&[
            (FABRIC_URL_VARIABLE, "http://fabricd:7100"),
            (FABRIC_TOKEN_VARIABLE, FAKE_BEARER),
        ]))
        .expect("valid")
        .expect("configured");
        assert!(
            plaintext
                .shipper(clock, &LinkSecurity::plaintext_for_tests())
                .expect("the endpoint over the link it names builds a shipper")
                .is_shipping()
        );
    }

    #[test]
    fn the_unshipped_banner_names_the_variable_and_the_queue_bound() {
        let banner = DeskShipper::unshipped(FABRIC_URL_VARIABLE).banner();
        assert!(
            banner.starts_with("desk outcomes: NOT SHIPPED (QIP_FABRIC_URL unset)"),
            "{banner}"
        );
        assert!(banner.contains("10000"), "{banner}");
    }

    // --- the refusal path, driven on the shipper's own parts -----------------

    use qip_core::{Timestamp, dec};
    use qip_events::event_fabric::codec::PayloadCodec;
    use qip_execution_engine::order::Side;
    use qip_kernel::desk_ledger::DeskLedgerFeed;
    use qip_kernel::platform::FillBooked;

    fn queued(count: usize) -> Vec<DeskOutcome> {
        let mut feed = DeskLedgerFeed::new();
        for n in 0..count {
            feed.admit(
                &FillBooked {
                    order_id: format!("o{n}"),
                    fill_id: "f1".to_string(),
                    object_id: "obj-ACME".to_string(),
                    venue: "paper-xnys".to_string(),
                    side: Side::Buy,
                    quantity: dec!("10"),
                    price: dec!("100"),
                    costs: dec!("1.5"),
                    at: Timestamp::from_secs(1_760_000_100 + n as i64),
                    simulated: true,
                },
                "central-desk",
                "cycle",
            )
            .expect("admitted");
        }
        feed.pending().to_vec()
    }

    fn idle_shipper() -> DeskShipper {
        // Never sent to: nothing here calls `ship`, so the address is only a
        // string the transport holds.
        let class = QosClass::P1Outcomes;
        let producer = Producer::new(ProducerConfig {
            transport: Box::new(HttpTransport::new(
                "http://127.0.0.1:9",
                BearerToken::new(FAKE_BEARER.to_string()).expect("a well-formed token"),
            )),
            stream: bindings::REFLEX_OUTCOMES_STREAM.to_string(),
            partition: 0,
            producer_id: PRODUCER_ID.to_string(),
            qos_class: class,
            ack_profile: class.ack_floor(),
            retry_policy: RetryPolicy::default(),
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(qip_core::ManualClock::new(Timestamp::from_secs(
                1_760_000_000,
            ))),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 1,
            breaker_seed: 2,
            timeouts: CALL_TIMEOUTS,
        })
        .expect("a producer");
        DeskShipper::shipping(producer, DESK_REGION, "test").expect("a shipper")
    }

    fn shipping_of(shipper: &mut DeskShipper) -> &mut Shipping {
        match &mut shipper.mode {
            Mode::Shipping(shipping) => shipping,
            Mode::Unshipped { .. } => unreachable!("built with a producer"),
        }
    }

    /// `record` re-encoded with its payload edited and its hash left alone:
    /// what a record corrupted after it was built looks like.
    fn corrupted(record: &Record) -> Record {
        let mut event = record
            .decode_payload(PayloadCodec::CanonicalJson)
            .expect("decodes");
        event.payload["session"] = serde_json::json!(99);
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("re-encodes")
    }

    #[test]
    fn a_record_the_ledger_would_refuse_is_cut_from_the_batch_before_the_producer_is_touched() {
        // The failure this prevents: the broker acknowledging a record whose
        // payload does not match its hash, and the ledger — which never skips
        // a record it cannot read — stalling the partition behind it for ever.
        let outcomes = queued(3);
        let good: Vec<Record> = outcomes
            .iter()
            .map(|o| fabric_record(o).expect("builds"))
            .collect();
        let mut shipper = idle_shipper();
        let shipping = shipping_of(&mut shipper);

        let whole = shipping
            .preflight(good.clone())
            .expect("premise: untampered passes");
        assert_eq!(whole.records.len(), 3);

        let cut = shipping
            .preflight(vec![good[0].clone(), corrupted(&good[1]), good[2].clone()])
            .expect("the record before the bad one still ships");
        assert_eq!(
            cut.records.len(),
            1,
            "the batch was not cut at the bad record"
        );

        let refused = shipping
            .preflight(vec![corrupted(&good[0]), good[1].clone()])
            .expect_err("a bad first record leaves nothing to send");
        assert!(!refused.1.is_empty());
    }

    #[test]
    fn a_hole_in_the_chain_holds_the_records_behind_it() {
        // The failure this prevents: sending position 5 when 3 was next, which
        // the ledger parks the desk's chain on.
        let outcomes = queued(6);
        let mut shipper = idle_shipper();
        let shipping = shipping_of(&mut shipper);
        shipping.next_expected = Some(3);

        let error = shipping
            .prepare(&outcomes[5..])
            .expect_err("position 5 is not 3");
        assert_eq!(error.0, 5);
        assert!(error.1.contains("hole"), "{}", error.1);

        let (batch, through) = shipping.prepare(&outcomes[3..]).expect("3 is next");
        assert_eq!((batch.records.len(), through), (3, 5));

        // A hole inside the candidates cuts the batch there.
        let with_hole = vec![outcomes[3].clone(), outcomes[5].clone()];
        let (batch, through) = shipping.prepare(&with_hole).expect("3 ships");
        assert_eq!((batch.records.len(), through), (1, 3));
    }

    #[test]
    fn a_non_simulated_outcome_is_held_back_by_the_shipper() {
        let mut outcomes = queued(2);
        if let qip_contracts::reflex::Decision::Filled { simulated, .. } =
            &mut outcomes[0].record.entry.decision
        {
            *simulated = false;
        }
        let mut shipper = idle_shipper();
        let error = shipping_of(&mut shipper)
            .prepare(&outcomes)
            .expect_err("a live fill is never built into a record");
        assert_eq!(error.0, 0);
        assert!(error.1.contains("not marked simulated"), "{}", error.1);
    }

    #[test]
    fn the_back_off_doubles_to_a_ceiling() {
        assert_eq!(backoff(1), BACKOFF_FLOOR);
        assert_eq!(backoff(2), BACKOFF_FLOOR * 2);
        assert_eq!(backoff(40), BACKOFF_CEILING);
    }
}
