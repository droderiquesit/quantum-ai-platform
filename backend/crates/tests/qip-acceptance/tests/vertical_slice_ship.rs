//! The vertical slice, the hop that had no caller: the desk's booked fills
//! carried to `qip-ledgerd` over the event fabric by the fast brain's
//! shipper.
//!
//! ```text
//! Platform::submit_order -> pending desk outcomes -> DeskShipper (fast brain)
//!     -> producer -> loopback Broker (qip-streaming, the committed catalogue)
//!     -> qip_ledgerd::start's partition worker -> LedgerStore (balances, chain)
//! ```
//!
//! `vertical_slice_ledgerd.rs` proves the last hop with `take_desk_outcomes`
//! and a hand-built `Delivery`; nothing in production called
//! `take_desk_outcomes`, so the desk's fills filled a 10,000-record queue and
//! went no further. Here every component is the real one: the platform, the
//! shipper `main` builds, a real broker on a loopback socket serving the
//! committed local catalogue (plus the one grant the desk's identity needs),
//! and the ledger started the way its `main` starts it. Nothing reaches a
//! venue: the broker is the simulated one and every fill is paper.
//!
//! The one fake is the DeskQueue view in `a_poisoned_...`, and it sits in
//! front of the real platform rather than in place of it: the platform will
//! not queue a record the ledger would refuse, so the only way to show the
//! shipper holding one back is to hand it one.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration as StdDuration, Instant};

use qip_contracts::ledger::Account;
use qip_contracts::reflex::Decision;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Decimal, ManualClock, ObjectId, SystemClock, dec, sha256_hex};
use qip_events::event_fabric::bindings::REFLEX_OUTCOMES_STREAM;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::policy::StreamPolicy;
use qip_execution_engine::order::Side;
use qip_fastbrain::ship::{
    DeskQueue, DeskShipper, FABRIC_PARTITION_VARIABLE, FABRIC_TOKEN_VARIABLE, FABRIC_URL_VARIABLE,
    ShipOutcome, ShipReport, read_settings,
};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::desk_ledger::{DESK_CELL, DESK_OUTBOX_CAPACITY, DeskOutcome};
use qip_kernel::{Platform, PlatformConfig};
use qip_ledgerd::consumer::Connect;
use qip_ledgerd::{Running, start};
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{MetricValue, Metrics, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_storage::MemoryBlobStore;
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service};
use qip_transport::event_fabric::auth::{self, BearerToken, IdentityTable};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport};
use qip_transport::retry::ThreadSleeper;
use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};
use qip_transport::tls::LinkSecurity;

const OUTCOMES: &str = REFLEX_OUTCOMES_STREAM;
const DESK_IDENTITY: &str = "reflex:desk";
const DESK_PRODUCER: &str = "desk";
const GROUP: &str = "ledger";
const EVENTUALLY: StdDuration = StdDuration::from_secs(90);

/// Fixture strings inside the bearer alphabet, not credentials for anything.
fn desk_token(unique: u64) -> String {
    format!("fixture-token-desk-ship-{unique}-000000000000000000000000000")
}
fn ledger_token(unique: u64) -> String {
    format!("fixture-token-ledger-ship-{unique}-0000000000000000000000000")
}

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-slice-ship-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

// --- the broker -------------------------------------------------------------

struct DataPlane(Arc<Service>);

impl Handler for DataPlane {
    fn handle(&self, request: &Request) -> Response {
        let handled = self
            .0
            .handle(request.header(auth::HEADER), &request.path, &request.body);
        Response::new(handled.status, "application/json", handled.body)
    }
}

struct Listener {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

fn listen(service: &Arc<Service>, address: &str) -> (Listener, String) {
    let server = Server::bind(
        address,
        Arc::new(DataPlane(service.clone())),
        ServerLimits::default(),
    )
    .expect("the broker binds");
    let bound = server.local_address().expect("an address");
    let stop = server.shutdown_handle();
    let thread = std::thread::spawn(move || {
        let _ = server.serve();
    });
    (Listener { stop, thread }, bound)
}

/// A one-partition broker on loopback serving the committed local catalogue
/// with one grant added: the desk's identity may produce to the outcomes
/// stream on its own key. (The committed catalogue grants only
/// `reflex:cell-local`; the desk's grant is a deployment fact this test
/// states and `report` names as open.)
struct Fabric {
    root: PathBuf,
    unique: u64,
    service: Arc<Service>,
    address: String,
    listener: Option<Listener>,
    housekeeping_stop: Arc<AtomicBool>,
}

impl Fabric {
    fn start() -> Self {
        let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
        let root = temp_dir("fabric");
        let mut catalogue: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../../infrastructure/event-fabric/streams.local.json"),
            )
            .expect("the committed local catalogue"),
        )
        .unwrap();
        catalogue["grants"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "identity": DESK_IDENTITY,
                "stream": OUTCOMES,
                "permission": "produce",
                "key_scope": "own_key"
            }));
        let catalogue = Catalogue::parse(&serde_json::to_vec(&catalogue).unwrap()).unwrap();
        let identities = IdentityTable::parse(&format!(
            "{DESK_IDENTITY} {}\n{GROUP} {}\n",
            sha256_hex(desk_token(unique).as_bytes()),
            sha256_hex(ledger_token(unique).as_bytes())
        ))
        .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let broker = Arc::new(Broker::open(root.join("broker"), clock.clone()).unwrap());
        let segments: SegmentConfigFor = Arc::new(move |policy: &StreamPolicy| {
            SegmentLogConfig::new(clock.clone()).with_archive_required(policy.archive_required())
        });
        let service = Arc::new(Service::new(broker, identities, &catalogue, 1, segments).unwrap());
        let (listener, address) = listen(&service, "127.0.0.1:0");
        // Opened here, once, before two threads race to open it lazily.
        service.broker().metadata(OUTCOMES, 0).unwrap();
        // P1 is acknowledged only once archived: the seal-and-archive pass
        // `qip-fabricd` runs, without which a producer is never acknowledged.
        let housekeeping_stop = Arc::new(AtomicBool::new(false));
        {
            let broker = service.broker().clone();
            let stop = housekeeping_stop.clone();
            let archiver = Archiver::new(Arc::new(MemoryBlobStore::new())).with_manifest_index();
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _ = broker.seal_aged();
                    let _ = broker.archive_sealed(&archiver);
                    std::thread::sleep(StdDuration::from_millis(50));
                }
            });
        }
        Self {
            root,
            unique,
            service,
            address,
            listener: Some(listener),
            housekeeping_stop,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The broker stops answering; its state is kept.
    fn outage(&mut self) {
        if let Some(listener) = self.listener.take() {
            listener.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(&self.address);
            listener.thread.join().expect("the listener ends");
        }
    }

    fn restore(&mut self) {
        assert!(self.listener.is_none(), "premise: the broker is down");
        let (listener, bound) = listen(&self.service, &self.address);
        assert_eq!(bound, self.address);
        self.listener = Some(listener);
    }

    /// Records the broker holds from the desk's producer.
    fn desk_records(&self) -> u64 {
        self.service
            .broker()
            .next_sequence(OUTCOMES, 0, DESK_PRODUCER)
            .unwrap()
    }

    fn shipper(&self) -> DeskShipper {
        let settings = read_settings(&|name: &str| match name {
            FABRIC_URL_VARIABLE => Some(self.url()),
            FABRIC_TOKEN_VARIABLE => Some(desk_token(self.unique)),
            FABRIC_PARTITION_VARIABLE => Some("0".to_string()),
            _ => None,
        })
        .expect("valid settings")
        .expect("configured");
        settings
            .shipper(Arc::new(SystemClock), &LinkSecurity::plaintext_for_tests())
            .expect("a shipper")
    }

    fn connect(&self) -> Connect {
        let url = self.url();
        let unique = self.unique;
        Arc::new(move || {
            let transport: Box<dyn FabricTransport + Send> = Box::new(HttpTransport::new(
                url.clone(),
                BearerToken::new(ledger_token(unique))?,
            ));
            Ok(transport)
        })
    }

    fn start_ledger(&self, dir: &Path) -> Running {
        let variables: BTreeMap<String, String> = [
            (qip_ledgerd::config::ENVIRONMENT, "workstation".to_string()),
            (qip_ledgerd::config::DATA_DIR, dir.display().to_string()),
            (qip_ledgerd::config::LISTEN, "127.0.0.1:0".to_string()),
            (qip_ledgerd::config::CONSUMER_GROUP, GROUP.to_string()),
            (qip_ledgerd::config::FABRIC_URL, self.url()),
            (qip_ledgerd::config::FABRIC_STREAM, OUTCOMES.to_string()),
            (qip_ledgerd::config::FABRIC_PARTITIONS, "1".to_string()),
            (qip_ledgerd::config::POLL_MS, "20".to_string()),
            (qip_ledgerd::config::DEPLOYMENT, "local".to_string()),
            (
                qip_transport::tls::TLS_MODE_VARIABLE,
                "plaintext".to_string(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        start(
            &qip_ledgerd::config::LedgerConfig::parse(&variables).unwrap(),
            Arc::new(SystemClock),
            Arc::new(ThreadSleeper),
            self.connect(),
        )
        .expect("the ledger starts against a running broker")
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        self.housekeeping_stop.store(true, Ordering::SeqCst);
        if let Some(listener) = self.listener.take() {
            listener.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(&self.address);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// --- the desk, as vertical_slice_ledgerd.rs builds it -----------------------------

fn start_time() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                acme(),
                "ACME",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("vertical-slice-ship", start_time()))
            .build(start_time())
            .expect("valid instrument"),
        )
        .expect("insertable");
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("vertical-slice-ship")
        .with(Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        ))
        .with(Limit::new(
            "max-leverage",
            LimitKind::MaxLeverage { limit: 2.0 },
        ))
}

fn bar(at: Timestamp) -> Bar {
    Bar {
        object_id: acme(),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: dec!("100"),
        high: dec!("100.2"),
        low: dec!("99.8"),
        close: dec!("100"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Some(dec!("100")),
        quality: DataQuality::default(),
    }
}

fn warm_platform() -> Result<Platform> {
    warm_platform_with(PlatformConfig::default())
}

/// A warm desk over the event log at `path`: a second call over the same
/// path is the restarted process, resuming its book from what the first wrote.
fn warm_platform_at(path: &Path) -> Result<Platform> {
    warm_platform_with(PlatformConfig::default().with_event_log_file(path))
}

fn warm_platform_with(config: PlatformConfig) -> Result<Platform> {
    let clock = Arc::new(ManualClock::new(start_time()));
    let context = Context::new(clock, config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    let mut tape: Vec<SensedRecord> = (1..=60)
        .rev()
        .map(|back| {
            SensedRecord::Bar(Box::new(bar(
                start_time().saturating_sub(Duration::from_days(back))
            )))
        })
        .collect();
    let printed = start_time().saturating_add(Duration::from_secs(30));
    tape.push(SensedRecord::Tick(Tick {
        object_id: acme(),
        venue: "XNYS".to_string(),
        at: printed,
        capture_time: printed.saturating_add(Duration::from_millis(40)),
        sequence: Some(1),
        price: dec!("100"),
        volume: dec!("100"),
        quality: DataQuality::default(),
    }));
    platform.observe(tape);
    platform.run_cycle(start_time().saturating_add(Duration::from_mins(2)));
    Ok(platform)
}

fn later() -> Timestamp {
    start_time().saturating_add(Duration::from_mins(3))
}

/// One more fill on the desk, through the simulated broker and the OMS.
fn fill(platform: &mut Platform, quantity: Decimal, tag: &str) {
    let before = platform.pending_desk_outcomes().len();
    let order = platform.order_from(
        acme(),
        Side::Buy,
        quantity,
        dec!("100"),
        &format!("prop-{tag}"),
        vec![format!("hyp-{tag}")],
        later(),
    );
    platform
        .submit_order(order, later())
        .expect("the order is accepted");
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        before + 1,
        "premise: order {tag} filled and queued a ledger record"
    );
}

fn fills(platform: &mut Platform, count: usize, label: &str) {
    for n in 0..count {
        fill(platform, dec!("10"), &format!("{label}{n}"));
    }
}

// --- driving the shipper ------------------------------------------------------------

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + EVENTUALLY;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(StdDuration::from_millis(20));
    }
}

/// Call `ship` until `stop` accepts a report, returning every report seen.
fn ship_until(
    shipper: &mut DeskShipper,
    queue: &mut dyn DeskQueue,
    what: &str,
    stop: impl FnMut(&ShipReport) -> bool,
) -> Vec<ShipReport> {
    ship_until_within(shipper, queue, what, EVENTUALLY, stop)
}

fn ship_until_within(
    shipper: &mut DeskShipper,
    queue: &mut dyn DeskQueue,
    what: &str,
    within: StdDuration,
    mut stop: impl FnMut(&ShipReport) -> bool,
) -> Vec<ShipReport> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    loop {
        let report = shipper
            .ship(queue)
            .expect("the shipper's bookkeeping holds");
        let done = stop(&report);
        seen.push(report);
        if done {
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting until {what}; the last reports: {:?}",
            &seen[seen.len().saturating_sub(3)..]
        );
        std::thread::sleep(StdDuration::from_millis(20));
    }
}

fn ship_all(shipper: &mut DeskShipper, platform: &mut Platform) {
    ship_until(shipper, platform, "the desk's queue is empty", |r| {
        r.pending == 0 && !r.in_flight
    });
}

fn counter(metrics: &Metrics, name: &str, outcome: Option<&str>) -> u64 {
    metrics
        .snapshot()
        .series
        .iter()
        .filter(|s| s.name == name)
        .filter(|s| outcome.is_none_or(|o| s.labels.iter().any(|(k, v)| k == "outcome" && v == o)))
        .map(|s| match &s.value {
            MetricValue::Counter(v) => *v,
            other => panic!("{name} is not a counter: {other:?}"),
        })
        .sum()
}

/// A whole-number gauge, as an integer: these three only ever hold a count or a 0/1.
fn gauge(metrics: &Metrics, name: &str) -> i64 {
    metrics
        .snapshot()
        .series
        .iter()
        .find(|s| s.name == name)
        .map(|s| match &s.value {
            MetricValue::Gauge(v) => v.round() as i64,
            other => panic!("{name} is not a gauge: {other:?}"),
        })
        .unwrap_or_else(|| panic!("{name} was never recorded"))
}

fn trading() -> Account {
    Account::Trading {
        cell: DESK_CELL.to_string(),
        strategy: "central-desk".to_string(),
    }
}

/// What the book paid for every fill the platform reports, to compare with
/// the ledger's trading account.
fn paid(platform: &Platform) -> Decimal {
    platform
        .orders()
        .fills()
        .iter()
        .map(|f| f.quantity * f.price + f.costs)
        .fold(Decimal::ZERO, |a, b| a + b)
}

fn held(platform: &Platform) -> Decimal {
    platform
        .orders()
        .fills()
        .iter()
        .map(|f| f.quantity)
        .fold(Decimal::ZERO, |a, b| a + b)
}

// --- the tests ----------------------------------------------------------------------

/// The whole path: three desk fills are carried to the ledger, which books
/// them balanced, at quantity times price plus the venue's fee, each posting
/// carrying the chain id the order's decisions are filed under — and the
/// platform's queue is empty afterwards because the fabric acknowledged them.
///
/// Mutation: in `ship.rs` drop the `platform.acknowledge(through)?` call — the
/// queue never empties and the first wait times out. Mutation: build the
/// lineage from the cycle's correlation id in `desk_fabric::fabric_record` —
/// the ledger's `decode_batch` refuses every record, nothing is booked and the
/// ledger wait times out.
#[test]
fn desk_fills_reach_the_ledger_balanced_attributed_and_acknowledged() {
    let fabric = Fabric::start();
    let ledger_dir = temp_dir("ledger-happy");
    let ledger = fabric.start_ledger(&ledger_dir);
    let mut platform = warm_platform().unwrap();
    fills(&mut platform, 3, "happy");
    let queued: Vec<DeskOutcome> = platform.pending_desk_outcomes().to_vec();
    assert_eq!(queued.len(), 3, "premise: three fills queued");
    assert!(
        platform.orders().fills().iter().all(|f| f.simulated),
        "premise: every fill is paper"
    );

    let mut shipper = fabric.shipper();
    assert!(shipper.is_shipping());
    let metrics = platform.telemetry().metrics.clone();
    ship_all(&mut shipper, &mut platform);

    assert!(
        platform.pending_desk_outcomes().is_empty(),
        "acknowledged records stay queued"
    );
    wait_until("the ledger has booked three fills", || {
        ledger.store().events().unwrap().len() == 3
    });
    let store = ledger.store();
    // Double entry: every unit closes to zero.
    let trial = store.trial_balance().unwrap();
    assert!(!trial.is_empty(), "premise: something was booked");
    assert!(trial.values().all(|total| total.is_zero()), "{trial:?}");
    // The trading account holds what the book holds; its cash is minus
    // quantity times price, plus the fee.
    assert_eq!(
        store.balance(&trading(), "obj-ACME").unwrap(),
        held(&platform)
    );
    assert_eq!(store.balance(&trading(), "USD").unwrap(), -paid(&platform));
    // Every posting names the chain its order's decisions are filed under.
    let chains: Vec<_> = store
        .events()
        .unwrap()
        .iter()
        .map(|event| event.chain_correlation())
        .collect();
    let expected: Vec<_> = queued
        .iter()
        .map(|o| o.chain_correlation().expect("a fill names its order"))
        .collect();
    assert_eq!(chains, expected);
    assert_eq!(store.tail(DESK_CELL).unwrap().expect("a tail").sequence, 2);
    assert!(store.parked().unwrap().is_empty());

    assert!(counter(&metrics, names::DESK_SHIP, Some("shipped")) >= 1);
    assert_eq!(counter(&metrics, names::DESK_SHIP, Some("refused")), 0);
    assert_eq!(gauge(&metrics, names::DESK_OUTCOMES_PENDING), 0);
    assert_eq!(gauge(&metrics, names::DESK_SHIP_CONFIGURED), 1);
    ledger.stop();
}

/// A crash between the fabric's acknowledgement and the platform's: the
/// records are still pending after restart, are sent again by a new producer
/// epoch, and the ledger posts nothing twice.
///
/// Mutation: acknowledge before the send (hand `Done::Delivered` to the
/// platform at submit time) — the crash leaves the platform's queue empty and
/// the `pending == 2` premise fails. Mutation: give the resend a new event id
/// or digest — the ledger parks the desk's chain as an integrity break and the
/// parked assertion fails.
#[test]
fn a_crash_between_send_and_acknowledgement_resends_and_posts_nothing_twice() {
    let fabric = Fabric::start();
    let mut platform = warm_platform().unwrap();
    fills(&mut platform, 2, "crash");

    let mut first = fabric.shipper();
    let report = first.ship(&mut platform).unwrap();
    assert_eq!(report.outcome, ShipOutcome::InFlight, "{report:?}");
    // The broker holds the batch; this process dies before it reads the
    // worker's answer, so the platform is never told.
    wait_until("the broker holds both records", || {
        fabric.desk_records() == 2
    });
    drop(first);
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        2,
        "the records were dropped from the queue before the fabric's answer was read"
    );

    let ledger_dir = temp_dir("ledger-crash");
    let ledger = fabric.start_ledger(&ledger_dir);
    wait_until("the ledger booked the first send", || {
        ledger.store().events().unwrap().len() == 2
    });
    let balances = ledger.store().balances().unwrap();

    // The restarted process: a new producer, a new epoch, the same pending
    // records.
    let mut second = fabric.shipper();
    ship_all(&mut second, &mut platform);
    wait_until("the broker holds the resend", || fabric.desk_records() == 4);
    wait_until(
        "the ledger absorbed both resent records as duplicates",
        || counter(ledger.metrics(), names::LEDGER_DUPLICATES, None) >= 2,
    );
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(
        ledger.store().events().unwrap().len(),
        2,
        "a resend posted again"
    );
    assert_eq!(ledger.store().balances().unwrap(), balances);
    assert!(
        ledger.store().parked().unwrap().is_empty(),
        "a resend parked the chain"
    );
    assert!(platform.pending_desk_outcomes().is_empty());
    ledger.stop();
}

/// A broker outage: the shipper reports deferral, the records stay pending
/// (including one booked during the outage), and when the broker is back the
/// retry delivers all of them, in chain order, once.
///
/// Mutation: drop the pending records on a failed delivery (acknowledge in
/// the `Done::Deferred` arm) — `pending` is 0 after the outage and the
/// premise fails. Mutation: resend only the newest record after a failure —
/// the ledger sees a hole and holds the chain, and the final tail assertion
/// fails.
#[test]
fn a_broker_outage_leaves_the_desk_pending_and_a_retry_delivers_in_order() {
    let mut fabric = Fabric::start();
    let mut platform = warm_platform().unwrap();
    let metrics = platform.telemetry().metrics.clone();
    fills(&mut platform, 2, "outage");
    let mut shipper = fabric.shipper();
    fabric.outage();

    let seen = ship_until(&mut shipper, &mut platform, "a delivery is deferred", |r| {
        r.outcome == ShipOutcome::Deferred
    });
    assert!(
        seen.last().unwrap().detail.is_some(),
        "a deferral names its reason"
    );
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        2,
        "an outage lost records"
    );
    assert!(counter(&metrics, names::DESK_SHIP, Some("deferred")) >= 1);
    assert_eq!(counter(&metrics, names::DESK_SHIP, Some("shipped")), 0);
    assert_eq!(fabric.desk_records(), 0);

    // The desk keeps trading; the new fill queues behind the old two.
    fill(&mut platform, dec!("10"), "during-outage");
    assert_eq!(platform.pending_desk_outcomes().len(), 3);

    fabric.restore();
    ship_all(&mut shipper, &mut platform);
    assert!(counter(&metrics, names::DESK_SHIP, Some("shipped")) >= 1);

    let ledger_dir = temp_dir("ledger-outage");
    let ledger = fabric.start_ledger(&ledger_dir);
    wait_until("the ledger booked all three", || {
        ledger.store().events().unwrap().len() == 3
    });
    assert_eq!(
        ledger
            .store()
            .tail(DESK_CELL)
            .unwrap()
            .expect("a tail")
            .sequence,
        2,
        "the chain was not delivered in order and whole"
    );
    assert_eq!(
        ledger.store().balance(&trading(), "USD").unwrap(),
        -paid(&platform)
    );
    assert!(ledger.store().parked().unwrap().is_empty());
    ledger.stop();
}

/// A DeskQueue view in front of a real platform that hands the shipper one
/// record the platform itself would never queue: the second one, as a fill not
/// marked simulated.
struct Poisoned<'a> {
    platform: &'a mut Platform,
}

impl DeskQueue for Poisoned<'_> {
    fn pending_len(&self) -> usize {
        self.platform.pending_desk_outcomes().len()
    }
    fn peek(&self, limit: usize) -> Vec<DeskOutcome> {
        let mut view: Vec<DeskOutcome> = self
            .platform
            .pending_desk_outcomes()
            .iter()
            .take(limit)
            .cloned()
            .collect();
        for outcome in &mut view {
            if outcome.record.entry.sequence == 1
                && let Decision::Filled { simulated, .. } = &mut outcome.record.entry.decision
            {
                *simulated = false;
            }
        }
        view
    }
    fn acknowledge(&mut self, through: u64) -> Result<usize> {
        self.platform.acknowledge_desk_outcomes(through)
    }
    fn note_problem(&mut self, problem: String) {
        self.platform.note_desk_ship_problem(problem);
    }
    fn metrics(&self) -> Arc<Metrics> {
        self.platform.telemetry().metrics.clone()
    }
}

/// A platform whose first acknowledgements fail to be recorded, as a full or
/// failing event log would: everything else is the real platform.
struct UnloggableAck<'a> {
    platform: &'a mut Platform,
    failures_left: u32,
}

impl DeskQueue for UnloggableAck<'_> {
    fn pending_len(&self) -> usize {
        self.platform.pending_desk_outcomes().len()
    }
    fn peek(&self, limit: usize) -> Vec<DeskOutcome> {
        self.platform
            .pending_desk_outcomes()
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }
    fn acknowledge(&mut self, through: u64) -> Result<usize> {
        if self.failures_left > 0 {
            self.failures_left -= 1;
            return Err(qip_core::error::Error::io(
                "the event log refused the acknowledgement record (full)".to_string(),
            ));
        }
        self.platform.acknowledge_desk_outcomes(through)
    }
    fn note_problem(&mut self, problem: String) {
        self.platform.note_desk_ship_problem(problem);
    }
    fn metrics(&self) -> Arc<Metrics> {
        self.platform.telemetry().metrics.clone()
    }
}

/// The fabric has the batch but the platform cannot journal that it does: the
/// shipper must not return an error (which ends the run loop and so the desk),
/// the records stay pending, the failure is counted and surfaced, and the
/// retry resends them and the ledger absorbs the duplicate.
///
/// Mutation: return the error from `settle` (`platform.acknowledge(through)?`):
/// `ship` returns Err and the first assertion fails. Mutation: acknowledge in
/// memory despite the failed journal: pending is empty and the next assertion
/// fails.
#[test]
fn an_acknowledgement_the_platform_cannot_record_defers_the_batch_and_does_not_end_the_desk() {
    let fabric = Fabric::start();
    let mut platform = warm_platform().unwrap();
    fills(&mut platform, 2, "unlogged");
    let mut shipper = fabric.shipper();

    let mut view = UnloggableAck {
        platform: &mut platform,
        failures_left: 1,
    };
    let seen = ship_until(&mut shipper, &mut view, "the failed ack is deferred", |r| {
        r.outcome == ShipOutcome::Deferred
    });
    assert!(
        seen.iter().all(|r| r.acknowledged == 0),
        "a record was dropped although the acknowledgement was not recorded: {seen:?}"
    );
    assert_eq!(
        view.platform.pending_desk_outcomes().len(),
        2,
        "the records left the queue before their acknowledgement was recorded"
    );
    assert_eq!(
        counter(
            &view.platform.telemetry().metrics,
            names::DESK_SHIP,
            Some("unlogged")
        ),
        1
    );
    wait_until("the fabric holds the batch", || fabric.desk_records() == 2);

    // The retry: the same records go again, the acknowledgement now records,
    // and the ledger books each once.
    let ledger_dir = temp_dir("ledger-unlogged");
    let ledger = fabric.start_ledger(&ledger_dir);
    ship_until(&mut shipper, &mut view, "the resend is acknowledged", |r| {
        r.pending == 0 && !r.in_flight
    });
    assert!(view.platform.pending_desk_outcomes().is_empty());
    wait_until("the ledger booked both records", || {
        ledger.store().events().unwrap().len() == 2
    });
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(
        ledger.store().events().unwrap().len(),
        2,
        "the resend posted a record twice"
    );
    assert!(ledger.store().parked().unwrap().is_empty());
    ledger.stop();
}

/// A record the ledger would refuse is not sent: the records before it are
/// delivered and booked, it and the ones behind it stay pending, the refusal is
/// counted once however often the shipper looks at it, and the ledger's
/// partition is not stalled — once the record is whole again the rest follow.
///
/// Mutation: skip the refusal in `Shipping::prepare` (build the record
/// regardless of `simulated`) — the broker receives a live-marked fill, the
/// ledger parks the desk's chain and the `broker holds one record` and
/// `parked is empty` assertions fail. Mutation: count the refusal on every
/// call — the `refused == 1` assertion fails.
#[test]
fn a_poisoned_record_is_refused_counted_left_pending_and_does_not_stall_the_ledger() {
    let fabric = Fabric::start();
    let ledger_dir = temp_dir("ledger-poison");
    let ledger = fabric.start_ledger(&ledger_dir);
    let mut platform = warm_platform().unwrap();
    let metrics = platform.telemetry().metrics.clone();
    fills(&mut platform, 3, "poison");
    let mut shipper = fabric.shipper();

    let mut view = Poisoned {
        platform: &mut platform,
    };
    let seen = ship_until(
        &mut shipper,
        &mut view,
        "the poisoned record is refused",
        |r| r.outcome == ShipOutcome::Refused,
    );
    let refusal = seen.last().unwrap().detail.clone().expect("a reason");
    assert!(refusal.contains("not marked simulated"), "{refusal}");
    assert!(refusal.contains("chain position 1"), "{refusal}");
    // Look at it again and again: still one count.
    for _ in 0..5 {
        let report = shipper.ship(&mut view).unwrap();
        assert_eq!(report.outcome, ShipOutcome::Refused);
    }
    assert_eq!(counter(&metrics, names::DESK_SHIP, Some("refused")), 1);
    assert_eq!(
        view.pending_len(),
        2,
        "the poisoned record and the one behind it stay pending"
    );
    assert_eq!(
        fabric.desk_records(),
        1,
        "only the record before the poisoned one was sent"
    );

    wait_until("the ledger booked the record before it", || {
        ledger.store().events().unwrap().len() == 1
    });
    assert!(
        ledger.store().parked().unwrap().is_empty(),
        "the ledger's chain was parked"
    );
    let statuses = ledger.statuses();
    assert!(
        statuses.values().all(|s| s.last_error.is_none()),
        "the ledger's partition is stalled: {statuses:?}"
    );

    // The record is whole again (the real platform, no view): the rest follow.
    ship_all(&mut shipper, &mut platform);
    wait_until("the ledger booked all three", || {
        ledger.store().events().unwrap().len() == 3
    });
    assert_eq!(
        ledger
            .store()
            .tail(DESK_CELL)
            .unwrap()
            .expect("a tail")
            .sequence,
        2
    );
    assert!(ledger.store().parked().unwrap().is_empty());
    ledger.stop();
}

/// A newer producer epoch owns the partition: the shipper stops, says so on a
/// metric and on the platform's problems, sends nothing more, and the desk
/// keeps trading.
///
/// Mutation: treat a fenced answer as a deferral (drop the `producer.is_fenced()`
/// classification in `deliver`) — the shipper retries for ever, `fenced` is
/// never counted and the wait times out.
#[test]
fn a_fenced_producer_stops_shipping_without_a_crash_and_says_so() {
    let fabric = Fabric::start();
    let mut platform = warm_platform().unwrap();
    let metrics = platform.telemetry().metrics.clone();
    fills(&mut platform, 1, "fence");
    let mut shipper = fabric.shipper();
    ship_all(&mut shipper, &mut platform);
    assert_eq!(fabric.desk_records(), 1, "premise: the first record is out");

    // Another desk process takes the partition.
    fabric
        .service
        .broker()
        .init_producer(OUTCOMES, 0, DESK_PRODUCER)
        .unwrap();

    fill(&mut platform, dec!("10"), "after-fence");
    ship_until(&mut shipper, &mut platform, "the fence is observed", |r| {
        r.outcome == ShipOutcome::Fenced
    });
    assert_eq!(counter(&metrics, names::DESK_SHIP, Some("fenced")), 1);
    let records = fabric.desk_records();
    for _ in 0..5 {
        let report = shipper.ship(&mut platform).unwrap();
        assert_eq!(report.outcome, ShipOutcome::Fenced);
    }
    assert_eq!(
        counter(&metrics, names::DESK_SHIP, Some("fenced")),
        1,
        "counted again"
    );
    assert_eq!(
        fabric.desk_records(),
        records,
        "a fenced shipper sent again"
    );
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        1,
        "the unshipped fill was lost"
    );

    // The desk keeps trading, and the cycle's problems say why it is not shipping.
    fill(&mut platform, dec!("10"), "still-trading");
    let report = platform.run_cycle(later().saturating_add(Duration::from_mins(1)));
    assert!(
        report
            .problems()
            .iter()
            .any(|(_, problem)| problem.contains("stopped shipping its ledger records")),
        "{:?}",
        report.problems()
    );
}

/// With a fabric, the queue stays bounded however long the desk runs: each
/// round of fills is shipped and acknowledged before the next. Without one,
/// the same rounds only grow it — the behaviour this slice replaced, now
/// loud rather than silent.
///
/// The feed's 10,000-record bound is a constant with no test hook, so the
/// property is shown across rounds rather than at the bound.
///
/// Mutation: drop the acknowledgement — `pending` after round one is the
/// round's size, not zero, and the first bound assertion fails.
#[test]
fn with_a_fabric_the_queue_is_bounded_and_without_one_it_only_grows_loudly() {
    let fabric = Fabric::start();
    let ledger_dir = temp_dir("ledger-bound");
    let ledger = fabric.start_ledger(&ledger_dir);
    let mut platform = warm_platform().unwrap();
    let mut shipper = fabric.shipper();
    let mut peak = 0usize;
    for round in 0..4 {
        fills(&mut platform, 2, &format!("round{round}-"));
        peak = peak.max(platform.pending_desk_outcomes().len());
        ship_all(&mut shipper, &mut platform);
        assert_eq!(platform.pending_desk_outcomes().len(), 0, "round {round}");
    }
    assert!(peak <= 2, "the queue held more than one round: {peak}");
    wait_until("the ledger booked all eight", || {
        ledger.store().events().unwrap().len() == 8
    });
    ledger.stop();

    // The same rounds with no fabric configured.
    let mut platform = warm_platform().unwrap();
    let metrics = platform.telemetry().metrics.clone();
    let mut unshipped = DeskShipper::unshipped(FABRIC_URL_VARIABLE);
    assert!(
        unshipped
            .banner()
            .starts_with("desk outcomes: NOT SHIPPED (QIP_FABRIC_URL unset)"),
        "{}",
        unshipped.banner()
    );
    let mut last = 0usize;
    for round in 0..3 {
        fills(&mut platform, 2, &format!("bare{round}-"));
        let report = unshipped.ship(&mut platform).unwrap();
        assert_eq!(report.outcome, ShipOutcome::Unshipped);
        assert!(report.pending > last, "the queue did not grow: {report:?}");
        last = report.pending;
    }
    assert_eq!(last, 6);
    assert_eq!(gauge(&metrics, names::DESK_SHIP_CONFIGURED), 0);
    assert_eq!(gauge(&metrics, names::DESK_OUTCOMES_PENDING), 6);
    assert_eq!(
        counter(&metrics, names::DESK_SHIP, None),
        0,
        "an unshipped desk sent something"
    );
    // The shutdown flush of an unshipped desk returns at once and says nothing was shipped.
    let flushed = unshipped
        .flush(&mut platform, StdDuration::from_secs(1))
        .unwrap();
    assert_eq!(
        (flushed.outcome, flushed.acknowledged),
        (ShipOutcome::Unshipped, 0)
    );
    assert_eq!(platform.pending_desk_outcomes().len(), 6);
}

/// The final flush waits for the batch in flight and the queue behind it,
/// within its budget, and an unreachable fabric costs the budget and no more.
///
/// Mutation: make `flush` return after one `step` — the in-flight batch is
/// never collected and `pending` is not zero.
#[test]
fn the_shutdown_flush_delivers_what_is_pending_within_its_budget() {
    let mut fabric = Fabric::start();
    let mut platform = warm_platform().unwrap();
    fills(&mut platform, 3, "flush");
    let mut shipper = fabric.shipper();
    let flushed = shipper
        .flush(&mut platform, StdDuration::from_secs(60))
        .unwrap();
    assert_eq!(flushed.acknowledged, 3, "{flushed:?}");
    assert_eq!(flushed.pending, 0);
    assert!(platform.pending_desk_outcomes().is_empty());
    assert_eq!(fabric.desk_records(), 3);

    // And with the broker down the flush gives up inside its budget.
    fabric.outage();
    fill(&mut platform, dec!("10"), "flush-down");
    let began = Instant::now();
    let flushed = shipper
        .flush(&mut platform, StdDuration::from_secs(2))
        .unwrap();
    assert_ne!(flushed.outcome, ShipOutcome::Shipped);
    assert_eq!(
        platform.pending_desk_outcomes().len(),
        1,
        "the unshipped fill was lost"
    );
    assert!(
        began.elapsed() < StdDuration::from_secs(45),
        "the flush outran any bound: {:?}",
        began.elapsed()
    );
}

// --- the restart: the watermark in the event log ----------------------------------

fn log_file(label: &str) -> PathBuf {
    temp_dir(label).join("events.jsonl")
}

fn records_of(platform: &Platform) -> Vec<qip_contracts::reflex::OutcomeRecord> {
    platform
        .pending_desk_outcomes()
        .iter()
        .map(|o| o.record.clone())
        .collect()
}

/// Crash window (a): fills booked, the process dies before anything is
/// shipped. The restarted desk re-queues every one with the identity it had,
/// the ledger ends with the whole chain and no hole, and a fill booked after
/// the restart continues the chain.
///
/// Mutation: in `resume_book` fold every fill instead of re-queuing it (ignore
/// the log's lack of a watermark as if it said "everything") — the restarted
/// queue is empty and the first assertion fails.
#[test]
fn a_crash_after_booking_before_any_ship_is_requeued_and_the_ledger_has_no_hole() {
    let fabric = Fabric::start();
    let path = log_file("restart-a");
    let before = {
        let mut first = warm_platform_at(&path).unwrap();
        fills(&mut first, 3, "a");
        records_of(&first)
    };
    assert_eq!(before.len(), 3, "premise");

    let mut second = warm_platform_at(&path).unwrap();
    assert_eq!(records_of(&second), before, "the crash lost queued records");
    fill(&mut second, dec!("10"), "a-after-restart");
    assert_eq!(second.pending_desk_outcomes().len(), 4);

    let ledger = fabric.start_ledger(&temp_dir("ledger-restart-a"));
    let mut shipper = fabric.shipper();
    ship_all(&mut shipper, &mut second);
    wait_until("the ledger booked all four", || {
        ledger.store().events().unwrap().len() == 4
    });
    assert_eq!(
        ledger
            .store()
            .tail(DESK_CELL)
            .unwrap()
            .expect("a tail")
            .sequence,
        3
    );
    assert!(
        ledger.store().parked().unwrap().is_empty(),
        "the chain has a hole"
    );
    ledger.stop();
}

/// A restarted desk must not hand a new order the id an old one had. The desk
/// has one session for ever, and the ledger's chain correlation is a hash of
/// cell, session and order id; the order counter used to start at one in every
/// process, so the first order after a restart shared `ord-1`'s lineage id with
/// the first order before it.
///
/// Mutation: delete the `resume_order_sequence` call in `resume_book` — the
/// post-restart order is `ord-1` again, the distinct-count assertion fails.
#[test]
fn an_order_after_a_restart_does_not_share_a_correlation_id_with_one_before() {
    let path = log_file("restart-ids");
    {
        let mut first = warm_platform_at(&path).unwrap();
        fills(&mut first, 2, "ids");
    }
    let mut second = warm_platform_at(&path).unwrap();
    fill(&mut second, dec!("10"), "ids-after");
    let records = records_of(&second);
    assert_eq!(records.len(), 3, "premise: two resumed and one new");
    let ids: std::collections::BTreeSet<String> = records
        .iter()
        .map(|r| {
            r.correlation()
                .expect("a desk record names its chain")
                .as_str()
                .to_string()
        })
        .collect();
    assert_eq!(ids.len(), 3, "two orders share a chain correlation id");
}

/// Crash window (b): the fabric acknowledged but the process died before the
/// watermark was logged. The restart re-ships the batch and the ledger absorbs
/// it as a duplicate.
///
/// Mutation: write the watermark before the fabric answers (journal in
/// `hand_over`) — the restart re-queues nothing and the `pending == 2`
/// assertion fails.
#[test]
fn a_crash_after_the_fabric_acked_before_the_watermark_was_logged_resends_and_posts_once() {
    let fabric = Fabric::start();
    let path = log_file("restart-b");
    {
        let mut first = warm_platform_at(&path).unwrap();
        fills(&mut first, 2, "b");
        let mut shipper = fabric.shipper();
        shipper.ship(&mut first).unwrap();
        wait_until("the broker holds both", || fabric.desk_records() == 2);
        // Dies here: the worker's answer is never read, so no watermark.
    }
    let ledger = fabric.start_ledger(&temp_dir("ledger-restart-b"));
    wait_until("the ledger booked the first send", || {
        ledger.store().events().unwrap().len() == 2
    });
    let balances = ledger.store().balances().unwrap();

    let mut second = warm_platform_at(&path).unwrap();
    assert_eq!(
        second.pending_desk_outcomes().len(),
        2,
        "the batch was not re-queued"
    );
    let mut shipper = fabric.shipper();
    ship_all(&mut shipper, &mut second);
    wait_until("the broker holds the resend", || fabric.desk_records() == 4);
    wait_until("the ledger absorbed the resend", || {
        counter(ledger.metrics(), names::LEDGER_DUPLICATES, None) >= 2
    });
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(
        ledger.store().events().unwrap().len(),
        2,
        "a resend posted again"
    );
    assert_eq!(ledger.store().balances().unwrap(), balances);
    ledger.stop();
}

/// Crash window (c): the watermark was logged. The restart sends only what is
/// after it; the ledger sees no duplicate.
///
/// Mutation: ignore the watermark on resume (re-queue every fill) — three
/// records are resent, the broker holds 6 and the ledger counts duplicates.
#[test]
fn a_crash_after_the_watermark_was_logged_resends_nothing_the_ledger_has() {
    let fabric = Fabric::start();
    let path = log_file("restart-c");
    {
        let mut first = warm_platform_at(&path).unwrap();
        fills(&mut first, 3, "c");
        let mut shipper = fabric.shipper();
        ship_all(&mut shipper, &mut first);
        assert_eq!(
            fabric.desk_records(),
            3,
            "premise: three delivered and acknowledged"
        );
        fill(&mut first, dec!("10"), "c-unshipped");
    }
    let ledger = fabric.start_ledger(&temp_dir("ledger-restart-c"));
    let mut second = warm_platform_at(&path).unwrap();
    assert_eq!(
        second
            .pending_desk_outcomes()
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect::<Vec<_>>(),
        vec![3],
        "the restart re-queued what the watermark says the ledger holds"
    );
    let mut shipper = fabric.shipper();
    ship_all(&mut shipper, &mut second);
    wait_until("the ledger booked all four", || {
        ledger.store().events().unwrap().len() == 4
    });
    assert_eq!(
        fabric.desk_records(),
        4,
        "something already acknowledged was sent again"
    );
    assert_eq!(counter(ledger.metrics(), names::LEDGER_DUPLICATES, None), 0);
    assert!(ledger.store().parked().unwrap().is_empty());
    ledger.stop();
}

// --- the queue bound, at the bound ---------------------------------------------------

/// Buy then sell, so ten thousand fills keep the net position at zero or one
/// lot. Ten thousand buys trip `max-position-weight` after about a thousand,
/// which is the limit working and not something to loosen for a test.
fn alternating(n: usize) -> Side {
    if n.is_multiple_of(2) {
        Side::Buy
    } else {
        Side::Sell
    }
}

/// Submit orders until one fills, because the simulated venue refuses a seeded
/// fraction of them (`rejection_probability`) and a refusal books nothing.
/// Returns how many orders it took, so a test that needs exactly N fills is
/// not at the mercy of the coin.
fn fill_despite_rejections(platform: &mut Platform, side: Side, tag: &str) -> usize {
    let mut last = String::new();
    for attempt in 1..=50 {
        let order = platform.order_from(
            acme(),
            side,
            dec!("1"),
            dec!("100"),
            &format!("prop-{tag}-{attempt}"),
            vec![format!("hyp-{tag}-{attempt}")],
            later(),
        );
        match platform.submit_order(order, later()) {
            Ok(_) => return attempt,
            Err(error) => last = format!("{error:?}"),
        }
    }
    panic!("fifty orders in a row were refused ({tag}); the last: {last}");
}

/// The fill past the queue's bound is booked and chained but not queued, which
/// leaves a gap in the chain the queue holds; this pins that, and that a
/// restart closes it. The venue holds the position either way, so the book
/// takes the fill; the failure this guards is the gap being silent, or being
/// read as safe.
///
/// 10,001 real fills go through the simulated broker and the OMS. With the
/// bound's worth queued and nothing shipped, the next fill is counted
/// unledgered and not queued, and one booked after it is chained at the next
/// position, so the queue then ends at `bound - 1` and `bound + 1` with
/// `bound` missing: shipped as it is, the ledger would park everything behind
/// the gap, and the shipper (`prepare`) holds the later record back instead.
/// Acknowledging the queued bound — what a drained shipper would have done —
/// and restarting re-queues exactly the two the queue lacked, in order, from
/// the event log.
///
/// The records are not pushed through the broker: at the catalogue's quota
/// that is some five minutes of paced delivery and says nothing more about
/// the gap. `a_queue_longer_than_one_quota_window_still_drains` covers the
/// shipping.
///
/// Mutation: make `admit` queue past the bound (compare against twice the
/// capacity) — the queue grows past the bound and "the fill past the bound was
/// queued" fails, after the 10,001st fill.
#[test]
fn the_fill_past_the_queue_bound_leaves_a_gap_that_only_a_restart_closes() {
    let bound = DESK_OUTBOX_CAPACITY;
    let path = log_file("overflow");
    let sequences = |platform: &Platform| -> Vec<u64> {
        platform
            .pending_desk_outcomes()
            .iter()
            .map(|o| o.record.entry.sequence)
            .collect()
    };
    {
        let mut platform = warm_platform_at(&path).unwrap();
        let metrics = platform.telemetry().metrics.clone();
        for n in 0..bound {
            fill_despite_rejections(&mut platform, alternating(n), &format!("o{n}"));
        }
        assert_eq!(
            platform.pending_desk_outcomes().len(),
            bound,
            "premise: full"
        );
        assert!(
            platform.unledgered_fills().is_empty(),
            "premise: nothing lost yet"
        );

        fill_despite_rejections(&mut platform, alternating(bound), "over");
        assert_eq!(
            platform.pending_desk_outcomes().len(),
            bound,
            "the fill past the bound was queued"
        );
        assert_eq!(
            platform.unledgered_fills().len(),
            1,
            "the fill past the bound was not reported as unledgered"
        );
        assert_eq!(
            counter(&metrics, names::DESK_FILLS_UNLEDGERED, None),
            1,
            "and was not counted"
        );

        // What a drained shipper does: the queued bound is delivered and
        // acknowledged, which makes room.
        let last_queued = (bound - 1) as u64;
        assert_eq!(
            platform.acknowledge_desk_outcomes(last_queued).unwrap(),
            bound
        );
        assert!(platform.pending_desk_outcomes().is_empty(), "premise");
        fill_despite_rejections(&mut platform, alternating(bound + 1), "after-gap");
        assert_eq!(
            sequences(&platform),
            vec![(bound + 1) as u64],
            "the record after the gap is chained past the missing position, and \
             is all the live queue holds: the missing one is not recoverable live"
        );
    }

    let resumed = warm_platform_at(&path).unwrap();
    assert_eq!(
        sequences(&resumed),
        vec![bound as u64, (bound + 1) as u64],
        "the restart did not re-queue the overflowed fill and the one behind it, in order"
    );
}

/// A queue longer than one quota window's worth must still drain. The broker
/// refuses a batch that alone exceeds a producer's per-second byte quota, for
/// ever; a desk record is about a kilobyte, so a 256-record batch was over the
/// catalogue's 262,144 and a queue of 300 delivered nothing, backing off
/// without end. Found when 10,001 fills were queued and nothing moved.
///
/// Mutation: raise `BATCH_PAYLOAD_BYTES` past the quota (or drop the byte check
/// in `prepare`) — `ship_all` times out with every report `Deferred`.
#[test]
fn a_queue_longer_than_one_quota_window_still_drains() {
    let fabric = Fabric::start();
    let ledger = fabric.start_ledger(&temp_dir("ledger-quota"));
    let mut platform = warm_platform().unwrap();
    for n in 0..300 {
        fill_despite_rejections(&mut platform, alternating(n), &format!("q{n}"));
    }
    assert_eq!(platform.pending_desk_outcomes().len(), 300, "premise");
    let mut shipper = fabric.shipper();
    ship_all(&mut shipper, &mut platform);
    wait_until("the ledger booked all 300", || {
        ledger.store().events().unwrap().len() == 300
    });
    ledger.stop();
}

/// The shipper's byte budget is below the quota the catalogue grants the
/// desk's stream, read from the committed file so a lowered quota fails here
/// and not in a queue that never drains.
///
/// Mutation: set `BATCH_PAYLOAD_BYTES` to 262_144 — the assertion fails.
#[test]
fn the_batch_byte_budget_is_below_the_catalogues_producer_quota() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../infrastructure/event-fabric/streams.local.json"),
    )
    .expect("the committed catalogue");
    let catalogue: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let quota = find_quota(&catalogue, OUTCOMES).expect("the outcomes stream names a byte quota");
    assert!(
        (qip_fastbrain::ship::BATCH_PAYLOAD_BYTES as u64) < quota,
        "a batch of {} bytes cannot be delivered under a quota of {quota}",
        qip_fastbrain::ship::BATCH_PAYLOAD_BYTES
    );
}

fn find_quota(value: &serde_json::Value, stream: &str) -> Option<u64> {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("name").and_then(|n| n.as_str()) == Some(stream)
                && let Some(q) = map.get("byte_quota_per_producer").and_then(|q| q.as_u64())
            {
                return Some(q);
            }
            map.values().find_map(|v| find_quota(v, stream))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_quota(v, stream)),
        _ => None,
    }
}
