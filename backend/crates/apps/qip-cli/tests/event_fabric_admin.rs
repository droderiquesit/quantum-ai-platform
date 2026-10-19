//! `qip event-fabric isolate` and `release` (FABRIC-028), driven through the
//! family's own `dispatch` and the production HTTP transport against the
//! real broker handler serving the committed local catalogue on loopback.
//!
//! The handler is `qip_streaming::event_fabric::service::Service` over a real
//! `Broker` on a temporary directory, behind this workspace's own HTTP
//! server — the same three pieces `qip-fabricd` composes. Nothing here is a
//! scripted answer: the refusal a producer reads after the isolation is the
//! broker's.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use qip_cli::event_fabric::admin::TOKEN_VARIABLE;
use qip_cli::event_fabric::{Environment, dispatch, grant};
use qip_core::SystemClock;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{AckProfile, BATCH_SCHEMA_VERSION, QosClass};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::partition::partition_for;
use qip_streaming::event_fabric::service::Service;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::{self, BearerToken, IdentityTable};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};
use qip_transport::server::{Request, Response, Server, ServerLimits};

const JOURNAL: &str = "reflex-journal.local";
const CELL: &str = "reflex:cell-local";
const OPERATOR: &str = "operator";
const PARTITIONS: u32 = 4;

/// A fixture token for `identity` in the broker `test` starts. Not a
/// credential for anything.
fn token(test: &str, identity: &str) -> String {
    format!(
        "fixture-{test}-token-for-{}-0000000000000000",
        identity.replace(':', "-")
    )
}

struct Fabric {
    root: PathBuf,
    address: String,
    broker: Arc<Broker>,
    test: &'static str,
}

impl Fabric {
    fn start(test: &'static str) -> Self {
        Self::start_granting(test, false)
    }

    /// [`Self::start`], optionally with the operator also granted `consume`
    /// on the journal stream. The committed catalogue grants the operator
    /// `admin` only, and a fetch needs `consume`; widening the committed
    /// grants is an access decision, so a replay test makes the grant in its
    /// own copy of the catalogue and the committed file stays as it is.
    fn start_granting(test: &'static str, operator_may_consume: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "qip-cli-event-fabric-admin-{}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut committed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../../infrastructure/event-fabric/streams.local.json"),
            )
            .expect("the committed local catalogue is readable"),
        )
        .expect("the committed local catalogue is JSON");
        if operator_may_consume {
            committed["grants"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "identity": OPERATOR,
                    "stream": JOURNAL,
                    "permission": "consume",
                    "key_scope": "any"
                }));
        }
        let catalogue = Catalogue::parse(&serde_json::to_vec(&committed).unwrap())
            .expect("the committed local catalogue parses");
        let identities = IdentityTable::parse(
            &[CELL, OPERATOR]
                .iter()
                .map(|identity| {
                    format!(
                        "{identity} {}\n",
                        qip_core::hash::sha256_hex(token(test, identity).as_bytes())
                    )
                })
                .collect::<String>(),
        )
        .unwrap();
        let broker = Arc::new(Broker::open(root.join("data"), Arc::new(SystemClock)).unwrap());
        let service = Arc::new(
            Service::new(
                broker.clone(),
                identities,
                &catalogue,
                PARTITIONS,
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
        // Detached: the listener lives as long as the test process, which is
        // as long as anything here needs it.
        std::thread::spawn(move || {
            let _ = server.serve();
        });
        Self {
            root,
            address,
            broker,
            test,
        }
    }

    fn producer(&self) -> Producer {
        let mut producer = Producer::new(ProducerConfig {
            transport: Box::new(HttpTransport::new(
                format!("http://{}", self.address),
                BearerToken::new(token(self.test, CELL)).unwrap(),
            )),
            stream: JOURNAL.to_string(),
            partition: own_partition(),
            producer_id: CELL.to_string(),
            qos_class: QosClass::P2MarketJournal,
            ack_profile: AckProfile::LeaderOnly,
            retry_policy: RetryPolicy {
                max_attempts: 1,
                ..RetryPolicy::default()
            },
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 3,
            breaker_seed: 3,
            timeouts: Timeouts::default(),
        })
        .unwrap();
        producer.init().unwrap();
        producer
    }

    /// The environment an operator's shell would give the command: the token
    /// of `identity` in a file, named by the `_FILE` variable, and the
    /// production HTTP transport.
    fn environment(&self, identity: &str) -> Environment {
        let path = self
            .root
            .join(format!("{}.token", identity.replace(':', "-")));
        std::fs::write(&path, token(self.test, identity)).unwrap();
        let variables: BTreeMap<String, String> =
            [(format!("{TOKEN_VARIABLE}_FILE"), path.display().to_string())].into();
        Environment::new(
            Box::new(move |name| variables.get(name).cloned()),
            Arc::new(SystemClock),
            Arc::new(ThreadSleeper),
            Box::new(grant::http_transport),
        )
    }

    fn arguments(&self, action: &str, operator: &str, reason: Option<&str>) -> Vec<String> {
        let partition = own_partition().to_string();
        let mut arguments = vec![
            action,
            "--peer",
            self.address.as_str(),
            "--stream",
            JOURNAL,
            "--partition",
            partition.as_str(),
            "--operator",
            operator,
        ];
        if let Some(reason) = reason {
            arguments.extend(["--reason", reason]);
        }
        arguments.into_iter().map(str::to_string).collect()
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn own_partition() -> u32 {
    partition_for("cell-local", PARTITIONS).unwrap()
}

/// A batch of one real event of a topic the committed journal stream admits:
/// the broker refuses a record whose body is not an event of one of the
/// stream's topics (R12-FABRIC-8090), so the tag cannot be the payload.
fn batch(tag: &str) -> Batch {
    let payload = serde_json::json!({ "tag": tag });
    let at = qip_core::Timestamp::from_secs(1_790_000_000);
    let event = qip_events::AnyEvent {
        event_id: qip_core::EventId::from_string(tag.to_string()),
        topic: qip_events::topic::Topic::ReflexJournalRecorded,
        schema_version: 1,
        occurred_at: at,
        recorded_at: at,
        sequence: 0,
        lineage: qip_core::Lineage::root(
            qip_core::CorrelationId::from_string("COR00000000000000000000009"),
            "qip-cli.test",
        ),
        idempotency_key: Some(tag.to_string()),
        payload_hash: qip_core::sha256_hex(
            qip_core::canonical::canonical_json(&payload).as_bytes(),
        ),
        payload,
    };
    Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap()],
    )
    .unwrap()
}

/// An operator isolates a partition with the command, the cell producing to
/// it is refused naming the operator and the reason, and the command lifts
/// it again — against the real broker handler and the committed catalogue's
/// own grants.
///
/// Asserts its premise first: the cell's produce is accepted before the
/// isolation, so the refusal after it is the isolation and not a broker
/// that refuses the cell anyway.
///
/// Mutation (run, failed, restored): `admin::run` building the release
/// request when asked to isolate — the command fails, because nothing is
/// isolated to release.
#[test]
fn the_isolate_and_release_commands_park_a_partition_and_restore_it_through_the_real_broker() {
    let fabric = Fabric::start("roundtrip");
    let mut producer = fabric.producer();
    producer
        .send(batch("before"))
        .expect("premise: the cell may produce to its own partition");

    let isolated = dispatch(
        &fabric.arguments("isolate", OPERATOR, Some("suspect feed")),
        &fabric.environment(OPERATOR),
    )
    .expect("the operator's isolate is carried out");
    assert_eq!(isolated.code, 0);
    assert!(
        isolated.lines[0].starts_with(&format!(
            "ISOLATED {JOURNAL}:{} at offset 1 by operator: suspect feed",
            own_partition()
        )),
        "{:?}",
        isolated.lines
    );

    let refused = producer
        .send(batch("during"))
        .expect_err("produce to the isolated partition is refused");
    assert!(
        refused
            .to_string()
            .contains("isolated by operator: suspect feed"),
        "the producer is told who and why: {refused}"
    );
    let held = fabric
        .broker
        .isolation(JOURNAL, own_partition())
        .unwrap()
        .expect("the broker holds the isolation");
    assert_eq!((held.operator.as_str(), held.at_offset), (OPERATOR, 1));

    let released = dispatch(
        &fabric.arguments("release", OPERATOR, None),
        &fabric.environment(OPERATOR),
    )
    .expect("the operator's release is carried out");
    assert!(
        released.lines[0].starts_with("RELEASED "),
        "{:?}",
        released.lines
    );
    producer
        .send(batch("after"))
        .expect("lifting the isolation restores produce");

    let actions: Vec<(String, String)> = fabric
        .broker
        .admin_log()
        .unwrap()
        .into_iter()
        .map(|entry| (entry.action, entry.operator))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("isolate".to_string(), OPERATOR.to_string()),
            ("release".to_string(), OPERATOR.to_string())
        ]
    );
}

/// The command refuses what it should never do: act under an identity that
/// holds no admin grant, reach a broker on another host, read a token given
/// directly in the environment, or isolate without a reason.
///
/// Asserts its premise first for the grant case: the same arguments succeed
/// under the operator's token.
///
/// Mutation (run, failed, restored): `admin::local_peer` no longer refusing a
/// non-loopback address — the off-host peer is not refused before the token.
#[test]
fn the_admin_commands_refuse_an_ungranted_identity_an_off_host_peer_and_a_missing_reason() {
    let fabric = Fabric::start("refusals");

    // The cell holds produce on this stream and no admin grant.
    let ungranted = dispatch(
        &fabric.arguments("isolate", CELL, Some("mine")),
        &fabric.environment(CELL),
    )
    .expect_err("an identity without an admin grant cannot isolate");
    assert!(
        ungranted.to_string().contains("no admin grant"),
        "{ungranted}"
    );
    assert!(
        fabric
            .broker
            .isolation(JOURNAL, own_partition())
            .unwrap()
            .is_none(),
        "nothing was isolated"
    );
    dispatch(
        &fabric.arguments("isolate", OPERATOR, Some("drill")),
        &fabric.environment(OPERATOR),
    )
    .expect("premise: the same request under the operator's token is carried out");

    // Off-host: refused before the token file is ever opened, so the
    // variable naming it is never even looked up.
    let looked_up = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = looked_up.clone();
    let recording = Environment::new(
        Box::new(move |name| {
            log.lock().unwrap().push(name.to_string());
            None
        }),
        Arc::new(SystemClock),
        Arc::new(ThreadSleeper),
        Box::new(grant::http_transport),
    );
    let mut off_host = fabric.arguments("release", OPERATOR, None);
    off_host[2] = "192.0.2.10:7100".to_string();
    let error = dispatch(&off_host, &recording).expect_err("an off-host peer is refused");
    assert!(error.to_string().contains("not loopback"), "{error}");
    assert!(
        looked_up.lock().unwrap().is_empty(),
        "no credential variable was consulted for a peer that was never going to be used"
    );

    // A token exported directly is refused rather than used.
    let direct = Environment::new(
        Box::new(|name| (name == TOKEN_VARIABLE).then(|| "anything".to_string())),
        Arc::new(SystemClock),
        Arc::new(ThreadSleeper),
        Box::new(grant::http_transport),
    );
    let error = dispatch(&fabric.arguments("release", OPERATOR, None), &direct)
        .expect_err("a token in the environment is refused");
    assert!(error.to_string().contains("set directly"), "{error}");

    // No reason, no isolation; and a reason on a release is refused.
    let error = dispatch(
        &fabric.arguments("isolate", OPERATOR, None),
        &fabric.environment(OPERATOR),
    )
    .expect_err("an isolation without a reason is refused");
    assert!(
        error.to_string().contains("--reason is required"),
        "{error}"
    );
    let error = dispatch(
        &fabric.arguments("release", OPERATOR, Some("because")),
        &fabric.environment(OPERATOR),
    )
    .expect_err("a reason on a release is refused");
    assert!(error.to_string().contains("belongs to isolate"), "{error}");
}

/// R12-FABRIC-7042: an operator reads a consumer group's lag on a partition
/// with the command, and the figures are the broker's own — the group's
/// committed offset and the partition's high watermark, and their gap. A
/// group that never committed is refused rather than reported caught up:
/// no checkpoint is not a lag of zero.
///
/// Asserts its premise first: three records are on the partition and the
/// group committed one, so the lag printed is a difference of two real
/// figures and not a default.
///
/// Mutation (run, failed, restored): `lag::run` printing the committed
/// offset where the lag belongs — the printed line no longer matches.
#[test]
fn the_lag_command_reports_a_groups_committed_offset_against_the_high_watermark() {
    let fabric = Fabric::start("lag");
    let mut producer = fabric.producer();
    for tag in ["one", "two", "three"] {
        producer.send(batch(tag)).expect("the cell may produce");
    }
    let high_watermark = fabric
        .broker
        .metadata(JOURNAL, own_partition())
        .unwrap()
        .high_watermark();
    fabric
        .broker
        .commit_offset("audit", JOURNAL, own_partition(), 1)
        .unwrap();
    assert_eq!(high_watermark, 3, "premise: three records were written");

    let arguments = |group: &str| -> Vec<String> {
        let partition = own_partition().to_string();
        [
            "lag",
            "--peer",
            fabric.address.as_str(),
            "--stream",
            JOURNAL,
            "--partition",
            partition.as_str(),
            "--group",
            group,
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    };
    let reported = dispatch(&arguments("audit"), &fabric.environment(OPERATOR))
        .expect("the operator may read lag");
    assert_eq!(reported.code, 0);
    assert_eq!(
        reported.lines,
        vec![format!(
            "LAG {JOURNAL}:{} group audit: 2 (committed offset 1, high watermark 3)",
            own_partition()
        )]
    );

    let never = dispatch(&arguments("nobody"), &fabric.environment(OPERATOR))
        .expect_err("a group with no checkpoint is refused, not reported caught up");
    assert!(
        never.to_string().contains("never committed"),
        "the refusal says why: {never}"
    );
}

/// A transport that answers as a broker predating the isolation field would:
/// every metadata answer comes back without it.
struct OmitsIsolation(Box<dyn qip_transport::event_fabric::transport::FabricTransport + Send>);

impl qip_transport::event_fabric::transport::FabricTransport for OmitsIsolation {
    fn call(
        &mut self,
        request: qip_transport::event_fabric::protocol::Request,
        timeouts: Timeouts,
    ) -> qip_core::error::Result<qip_transport::event_fabric::protocol::Response> {
        use qip_transport::event_fabric::protocol::{Metadata, Response};
        Ok(match self.0.call(request, timeouts)? {
            Response::Metadata(m) => Response::Metadata(Metadata::new(
                m.stream(),
                m.partition(),
                m.leader_epoch(),
                m.high_watermark(),
                m.archived_through(),
            )?),
            other => other,
        })
    }
}

impl Fabric {
    fn inspect_arguments(&self, view: &str) -> Vec<String> {
        let partition = own_partition().to_string();
        [
            view,
            "--peer",
            self.address.as_str(),
            "--stream",
            JOURNAL,
            "--partition",
            partition.as_str(),
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn environment_omitting_isolation(&self, identity: &str) -> Environment {
        let path = self.root.join("omits.token");
        std::fs::write(&path, token(self.test, identity)).unwrap();
        let variables: BTreeMap<String, String> =
            [(format!("{TOKEN_VARIABLE}_FILE"), path.display().to_string())].into();
        Environment::new(
            Box::new(move |name| variables.get(name).cloned()),
            Arc::new(SystemClock),
            Arc::new(ThreadSleeper),
            Box::new(|peer, identity| {
                Box::new(OmitsIsolation(grant::http_transport(peer, identity)))
            }),
        )
    }
}

/// R12-FABRIC-7042 (ADR 0156 §4b). `qip event-fabric health` prints the
/// isolation the broker computed, and refuses a response that omits it.
///
/// The failure this prevents: a health view that read a missing isolation
/// field as "not isolated" prints a parked partition as healthy whenever the
/// broker predates the field, or the field is dropped on the way.
///
/// A healthy partition is read first (exit 0, "not isolated"), so the
/// ISOLATED line after the isolation is the isolation's and not a view that
/// always says so.
///
/// Mutation: drop the isolation from the metadata answer (the service sets
/// `with_isolation`; remove that call). The health command then refuses, and
/// the first assertion fails because it cannot print the healthy line.
#[test]
fn the_health_command_reports_isolation_and_refuses_a_response_that_omits_it() {
    let fabric = Fabric::start("health");
    fabric.producer().send(batch("before")).unwrap();

    let healthy = dispatch(
        &fabric.inspect_arguments("health"),
        &fabric.environment(OPERATOR),
    )
    .expect("the operator reads the partition's health");
    assert_eq!(healthy.code, 0, "{:?}", healthy.lines);
    assert!(
        healthy.lines[0].contains("not isolated")
            && healthy.lines[0].contains("high watermark 1")
            && healthy.lines[0].contains("leader epoch"),
        "{:?}",
        healthy.lines
    );

    dispatch(
        &fabric.arguments("isolate", OPERATOR, Some("suspect feed")),
        &fabric.environment(OPERATOR),
    )
    .expect("the operator isolates the partition");
    let parked = dispatch(
        &fabric.inspect_arguments("health"),
        &fabric.environment(OPERATOR),
    )
    .expect("the operator reads the partition's health");
    assert_eq!(parked.code, 3, "{:?}", parked.lines);
    assert!(
        parked.lines[0].contains("ISOLATED by operator: suspect feed"),
        "the print does not name the isolation: {:?}",
        parked.lines
    );

    let error = dispatch(
        &fabric.inspect_arguments("health"),
        &fabric.environment_omitting_isolation(OPERATOR),
    )
    .expect_err("an answer without the isolation field is refused, not printed as healthy");
    assert!(
        error.to_string().contains("carries no isolation field"),
        "{error}"
    );
}

/// R12-FABRIC-7042. `qip event-fabric replay` fetches a partition, verifies
/// the batch chain and says AGREES; a partition whose chain is broken says
/// DIFFERS and exits 3. A partition with nothing at the offset is refused,
/// because nothing verified is not agreement.
///
/// The broken chain is made the only way a reader can meet one: the fetch
/// response is rewritten in transit to swap two batches, so the second no
/// longer follows the one before it.
///
/// Mutation: in `inspect::replay`, skip the `previous_batch_hash` comparison.
/// The swapped fetch is then reported AGREES and the DIFFERS assertion fails.
#[test]
fn the_replay_command_agrees_on_an_intact_chain_and_differs_on_a_broken_one() {
    let fabric = Fabric::start_granting("replay", true);
    let mut producer = fabric.producer();
    for tag in ["first", "second", "third"] {
        producer.send(batch(tag)).unwrap();
    }

    let intact = dispatch(
        &fabric.inspect_arguments("replay"),
        &fabric.environment(OPERATOR),
    )
    .expect("the operator replays the partition");
    assert_eq!(intact.code, 0, "{:?}", intact.lines);
    assert!(
        intact.lines[0].starts_with("AGREES ")
            && intact.lines[0].contains("3 batch(es) from offset 0")
            && intact.lines[0].contains("anchored at the partition's first batch"),
        "{:?}",
        intact.lines
    );

    struct Swaps(Box<dyn qip_transport::event_fabric::transport::FabricTransport + Send>);
    impl qip_transport::event_fabric::transport::FabricTransport for Swaps {
        fn call(
            &mut self,
            request: qip_transport::event_fabric::protocol::Request,
            timeouts: Timeouts,
        ) -> qip_core::error::Result<qip_transport::event_fabric::protocol::Response> {
            use qip_events::event_fabric::codec::{Batch, PrefixDecodeOutcome};
            use qip_transport::event_fabric::protocol::{FetchResponse, Response};
            Ok(match self.0.call(request, timeouts)? {
                Response::Fetch(f) if !f.batches().is_empty() => {
                    let bytes = qip_core::hash::from_hex(f.batches())
                        .ok_or_else(|| qip_core::error::Error::invalid("not hex"))?;
                    let mut frames = Vec::new();
                    let mut cursor = &bytes[..];
                    while !cursor.is_empty() {
                        let PrefixDecodeOutcome::Complete { consumed, .. } =
                            Batch::decode_prefix(cursor)?
                        else {
                            return Err(qip_core::error::Error::invalid("a torn frame"));
                        };
                        frames.push(cursor[..consumed].to_vec());
                        cursor = &cursor[consumed..];
                    }
                    frames.swap(1, 2);
                    Response::Fetch(
                        FetchResponse::new(
                            f.stream(),
                            f.partition(),
                            f.high_watermark(),
                            f.archived_through(),
                            qip_core::hash::to_hex(&frames.concat()),
                        )
                        .unwrap(),
                    )
                }
                other => other,
            })
        }
    }
    let path = fabric.root.join("swaps.token");
    std::fs::write(&path, token(fabric.test, OPERATOR)).unwrap();
    let variables: BTreeMap<String, String> =
        [(format!("{TOKEN_VARIABLE}_FILE"), path.display().to_string())].into();
    let swapping = Environment::new(
        Box::new(move |name| variables.get(name).cloned()),
        Arc::new(SystemClock),
        Arc::new(ThreadSleeper),
        Box::new(|peer, identity| Box::new(Swaps(grant::http_transport(peer, identity)))),
    );
    let broken = dispatch(&fabric.inspect_arguments("replay"), &swapping)
        .expect("a broken chain is a verdict, not an error");
    assert_eq!(broken.code, 3, "{:?}", broken.lines);
    assert!(
        broken.lines[0].starts_with("DIFFERS ") && broken.lines[0].contains("does not chain"),
        "{:?}",
        broken.lines
    );

    let mut beyond = fabric.inspect_arguments("replay");
    beyond.extend(["--from".to_string(), "99".to_string()]);
    let error = dispatch(&beyond, &fabric.environment(OPERATOR))
        .expect_err("nothing at the offset is refused, not reported as agreement");
    assert!(
        error.to_string().contains("Nothing was verified"),
        "{error}"
    );
}
