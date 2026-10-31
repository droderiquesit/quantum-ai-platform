//! `qip event-fabric produce` (R12-RES-10153) driven through the family's own
//! `dispatch` and the production HTTP transport against the real broker
//! handler serving the committed local catalogue on loopback — the same
//! three pieces `qip-fabricd` composes, as `event_fabric_admin.rs` does.
//!
//! What is proven is that the command's tally and the broker's log are two
//! independent counts of the same load and agree, and that a producer the
//! broker throttles is counted as throttled rather than as sent.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use qip_cli::event_fabric::produce::TOKEN_VARIABLE;
use qip_cli::event_fabric::{Environment, dispatch, grant};
use qip_core::SystemClock;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::Service;
use qip_transport::event_fabric::auth::{self, IdentityTable};
use qip_transport::retry::ThreadSleeper;
use qip_transport::server::{Request, Response, Server, ServerLimits};

const JOURNAL: &str = "reflex-journal.local";
const CELL: &str = "reflex:cell-local";

fn token(test: &str) -> String {
    format!("fixture-{test}-token-for-reflex-cell-local-0000000000000000")
}

struct Fabric {
    root: PathBuf,
    address: String,
    broker: Arc<Broker>,
}

impl Fabric {
    fn start(test: &'static str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "qip-cli-event-fabric-produce-{}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let catalogue = Catalogue::parse(
            &std::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../../infrastructure/event-fabric/streams.local.json"),
            )
            .unwrap(),
        )
        .expect("the committed local catalogue parses");
        let identities = IdentityTable::parse(&format!(
            "{CELL} {}\n",
            qip_core::hash::sha256_hex(token(test).as_bytes())
        ))
        .unwrap();
        let broker = Arc::new(Broker::open(root.join("data"), Arc::new(SystemClock)).unwrap());
        let service = Arc::new(
            Service::new(
                broker.clone(),
                identities,
                &catalogue,
                1,
                Arc::new(|policy| {
                    SegmentLogConfig::new(Arc::new(SystemClock))
                        .with_archive_required(policy.archive_required())
                }),
            )
            .unwrap(),
        );
        let server = Server::bind(
            "127.0.0.1:0",
            Arc::new(move |request: &Request| {
                let handled = service.handle(
                    request.header(auth::HEADER),
                    None,
                    &request.path,
                    &request.body,
                );
                Response::new(handled.status, "application/json", handled.body)
            }),
            ServerLimits::default(),
        )
        .unwrap();
        let address = server.local_address().unwrap();
        std::thread::spawn(move || {
            let _ = server.serve();
        });
        let path = root.join("cell.token");
        std::fs::write(&path, token(test)).unwrap();
        Self {
            root,
            address,
            broker,
        }
    }

    fn environment(&self) -> Environment {
        let variables: BTreeMap<String, String> = [(
            format!("{TOKEN_VARIABLE}_FILE"),
            self.root.join("cell.token").display().to_string(),
        )]
        .into();
        Environment::new(
            Box::new(move |name| variables.get(name).cloned()),
            Arc::new(SystemClock),
            Arc::new(ThreadSleeper),
            Box::new(grant::http_transport),
        )
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn result_line(lines: &[String]) -> serde_json::Value {
    let line = lines
        .iter()
        .find_map(|l| l.strip_prefix("RESULT "))
        .expect("the command prints its RESULT line");
    serde_json::from_str(line).unwrap()
}

/// A P2 producer driven past the journal stream's per-second byte quota is
/// admitted up to the quota and then told to wait; the command counts both,
/// and the batches it counts as admitted are exactly the batches the
/// broker's log holds.
///
/// Asserts its premise first: something was admitted, so the agreement below
/// is not two zeros agreeing.
///
/// Mutations (run, failed, restored): count a quota refusal as admitted
/// instead of refused in `drive` — the throttle assertion fails, no quota
/// having been counted; count it as admitted as well as refused — the tally
/// reads 22 against the broker's 20 and the agreement assertion fails.
#[test]
fn a_throttled_p2_load_is_counted_as_throttled_and_its_admitted_batches_are_the_brokers() {
    let fabric = Fabric::start("p2");
    let arguments: Vec<String> = [
        "produce",
        "--peer",
        fabric.address.as_str(),
        "--stream",
        JOURNAL,
        "--class",
        "p2",
        "--partition",
        "0",
        "--producer-id",
        "test-load",
        "--rate",
        "200",
        "--records-per-batch",
        "64",
        "--payload-bytes",
        "1024",
        "--duration-ms",
        "1500",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let outcome = dispatch(&arguments, &fabric.environment()).expect("the load runs");
    assert_eq!(outcome.code, 0, "{:?}", outcome.lines);
    let result = result_line(&outcome.lines);
    let admitted = result["admitted"].as_u64().unwrap();
    assert!(admitted > 0, "premise: some load was admitted: {result}");
    assert!(
        result["refused"]["quota"].as_u64().unwrap_or(0) > 0,
        "a P2 producer past its byte quota is throttled: {result}"
    );
    let held = fabric.broker.metadata(JOURNAL, 0).unwrap().high_watermark();
    assert_eq!(
        admitted, held,
        "the command's admitted count and the broker's log disagree"
    );
    assert!(
        outcome.lines.iter().any(|l| l.starts_with("PAPER TRADING")),
        "{:?}",
        outcome.lines
    );
}

/// The peer fence holds before the token is read: a routable peer is refused
/// even when no token is configured at all.
#[test]
fn a_peer_off_loopback_is_refused_before_the_token_is_read() {
    let arguments: Vec<String> = [
        "produce",
        "--peer",
        "192.0.2.10:9000",
        "--stream",
        JOURNAL,
        "--class",
        "p2",
        "--partition",
        "0",
        "--producer-id",
        "test-load",
        "--rate",
        "1",
        "--duration-ms",
        "10",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let environment = Environment::new(
        Box::new(|_| None),
        Arc::new(SystemClock),
        Arc::new(ThreadSleeper),
        Box::new(grant::http_transport),
    );
    let refused = dispatch(&arguments, &environment).unwrap_err();
    assert!(
        refused.message().contains("not loopback"),
        "{}",
        refused.message()
    );
}
