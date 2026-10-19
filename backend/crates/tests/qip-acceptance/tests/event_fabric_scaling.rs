//! Event Fabric scaling, deployment, and operational requirements: FABRIC-066 through FABRIC-084 and FABRIC-086 through FABRIC-109.
//!
//! Regional project organization, admin tooling, broker deployment, scaling policies, upgrade procedures,
//! disaster recovery, and service integration.
//!
//! Every test here reads the platform: the broker library, the sink, the
//! group coordinator, the cell's capital downlink, the stream policy and
//! catalogue parsers, or the Terraform. Where the requirement names something
//! the platform does not build, the test is a pin on the gap that fails when
//! the thing appears, and says so in its name.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod event_fabric_support;

use event_fabric_support::{
    STREAM, all_terraform, batch, batch_of, broker_module, broker_with, fresh_dir, has_line,
    hcl_block, norm, resources, spec,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Clock, CorrelationId, Decimal, Duration, Id, Lineage, ManualClock, Timestamp, dec};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{CapitalDownlink, CapitalGrantTopic, ScriptedFrames};
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, Record};
use qip_events::event_fabric::policy::{
    AckProfile, Mirroring, QosClass, StreamPolicy, StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::group::GroupCoordinator;
use qip_streaming::event_fabric::managed::{ManagedLog, MemoryManagedLog, PartitionRef};
use qip_streaming::event_fabric::sink::{LedgerSink, OutcomeWriter, SinkStep};
use qip_transport::event_fabric::protocol::Metadata;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

// --- FABRIC-060 --------------------------------------------------------------

/// The `google_project` resources in `source` whose block names the fabric as
/// a word. A word, not a substring: the point is a project dedicated to the
/// fabric, and `fabric` inside another word would be a different thing.
fn fabric_projects(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (kind, name) in resources(source) {
        if kind != "google_project" {
            continue;
        }
        let opening = format!("resource \"google_project\" \"{name}\" {{");
        let Some(block) = hcl_block(source, &opening) else {
            continue;
        };
        let names_fabric = block.iter().any(|line| {
            line.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word.eq_ignore_ascii_case("fabric"))
        });
        if names_fabric {
            found.push(name);
        }
    }
    found
}

/// GAP PIN. FABRIC-060 asks for a dedicated `prod/fabric-<region>` project
/// per region, holding the fabric's VPC, broker VMs, journal disks and
/// archiver identities. What the Terraform declares is one project per
/// *environment* (`foundation`, ADR 0118: `"${var.prefix}-${each.key}"`), and
/// the broker module is instantiated inside that environment project with the
/// environment's `project_id`. The earlier version built a `FabricProject`
/// struct whose id was `prod-fabric-us-central1` and asserted it contained
/// `fabric`. This test asserts the production facts; any one changing makes
/// it fail, and it should then assert the requirement.
///
/// Mutation: name the environment project `"${var.prefix}-fabric-${each.key}"`
/// in `foundation/main.tf` — the project-id assertion fires.
#[test]
fn fabric_060_gap_the_fabric_has_no_project_of_its_own_and_lives_in_the_environment_project() {
    let foundation = qip_acceptance::read("infrastructure/foundation/main.tf");
    let project = hcl_block(&foundation, "resource \"google_project\" \"environment\" {")
        .expect("premise: the foundation declares the environment projects");
    assert!(
        has_line(&project, "project_id = \"${var.prefix}-${each.key}\""),
        "the environment project is no longer named by environment alone: {project:#?}"
    );
    assert!(
        has_line(&project, "for_each = local.enabled"),
        "premise: one project per enabled environment: {project:#?}"
    );

    let files = all_terraform();
    assert!(files.len() > 10, "premise: the Terraform tree was read");
    let named: Vec<(String, Vec<String>)> = files
        .iter()
        .map(|(path, text)| (path.clone(), fabric_projects(text)))
        .filter(|(_, found)| !found.is_empty())
        .collect();
    assert!(
        named.is_empty(),
        "a project naming the fabric is declared ({named:?}) — assert it is per region and holds \
         the VPC, brokers, journal disks and archiver identities"
    );
    // The detector can see one: a project the fabric owns, injected.
    let injected =
        "resource \"google_project\" \"fabric\" {\n  project_id = \"prod-fabric-us-central1\"\n}\n";
    assert_eq!(fabric_projects(injected), vec!["fabric".to_string()]);

    let fabric = qip_acceptance::read("infrastructure/terraform/fabric.tf");
    let module = hcl_block(&fabric, "module \"event_fabric_broker\" {")
        .expect("premise: the root instantiates the broker module");
    assert!(
        has_line(&module, "project_id = var.project_id"),
        "the broker is no longer placed in the environment project: {module:#?}"
    );
}

#[test]
fn fabric_066_reflex_cells_must_not_emit_orders_through_fabric_topics() {
    // Inverted from `fabric_066_reflex_cells_emit_orders_through_fabric_topics`,
    // whose fixture modelled a cell publishing `orders_placed` and
    // `orders_cancelled` to fabric topics and asserted the list was non-empty.
    // That is the §28 anti-pattern — an order whose path to the venue runs
    // through a broker — written down as the requirement. FABRIC-003 says the
    // opposite: orders go directly to the venue, never through a fabric topic.
    // This test asserts that, against the router that decides it and the crate
    // that sends orders.
    //
    // Mutation: in `MessageTypeRouter::new`, register `Topic::OrderSubmitted`
    // to `FabricPath::LocalJournal` — the refusal assertion fires.
    // Mutation: add `use qip_transport::event_fabric::producer::Producer;` to
    // `qip-edge/src/cell.rs` — the scan fires.
    use qip_events::Topic;
    use qip_events::event_fabric::message_routing::{FabricPath, MessageTypeRouter};

    let router = MessageTypeRouter::new();
    let orders = [
        Topic::OrderSubmitted,
        Topic::OrderAmended,
        Topic::OrderCancelled,
        Topic::OrderFilled,
    ];
    for topic in orders {
        // Premise: the router knows the topic, so a refusal is a decision.
        assert_eq!(
            router.required_path(topic),
            Ok(FabricPath::VenueIo),
            "{topic:?}"
        );
        for fabric in [FabricPath::LocalJournal, FabricPath::MeshLink] {
            assert!(
                router.validate(topic, fabric).is_err(),
                "{topic:?} may travel on {fabric}"
            );
        }
    }
    // Nothing else is routed to the venue: the venue path carries orders only.
    let on_venue: Vec<Topic> = router
        .routes()
        .filter(|(_, path)| *path == FabricPath::VenueIo)
        .map(|(topic, _)| topic)
        .collect();
    assert_eq!(on_venue.len(), orders.len(), "{on_venue:?}");

    // The crate that sends orders holds no fabric producer or consumer.
    let sources = qip_acceptance::files_with_extension("backend/crates/edge/qip-edge/src", "rs");
    assert!(
        sources.iter().any(|path| path.ends_with("cell.rs")),
        "premise: the scan reached cell.rs"
    );
    let forbidden = [
        "event_fabric::producer",
        "event_fabric::consumer",
        "event_fabric::batching_producer",
        "BatchingProducer",
        "qip_streaming",
    ];
    for path in &sources {
        let text = qip_acceptance::production_text(
            &std::fs::read_to_string(path).expect("a readable source file"),
        );
        for token in forbidden {
            assert!(
                !text.contains(token),
                "{} names `{token}` in production code",
                path.display()
            );
        }
    }
}

// --- FABRIC-081 --------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantBody(CapitalEnvelope);

impl EventBody for GrantBody {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;
}

const CELL: &str = "london-1";
const KEY: &[u8] = b"fabric-081-cell-envelope-key";
const OTHER_KEY: &[u8] = b"a-key-this-cell-has-never-held";

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + seconds)
}

/// A grant for `strategy` to `cell`, signed with `key`, or carrying the
/// literal signature `unsigned`.
fn grant(
    strategy: &str,
    cell: &str,
    gross: Decimal,
    key: Option<&[u8]>,
) -> qip_core::error::Result<CapitalEnvelope> {
    let build = |gross: Decimal, signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            cell,
            gross,
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            at(0),
            at(7_200),
            "approver-1",
            signature,
        )
    };
    let unsigned = build(gross, "unsigned")?;
    match key {
        Some(key) => build(gross, &sign_payload(key, &unsigned.signing_payload())),
        None => Ok(unsigned),
    }
}

fn frame(envelope: CapitalEnvelope, event_id: &str) -> qip_core::error::Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        at(5),
        at(5),
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-acceptance",
        ),
        GrantBody(envelope),
    )
    .erase()
}

/// FABRIC-081: a control payload is signed by the authorised producer and
/// verified before it is consumed; no decision is applied without the check.
/// Driven through the cell's own `CapitalDownlink` — the one verify path the
/// fabric's control consumer also hands every record to — over a scripted
/// source: one honest grant, a grant whose limit was raised after signing, a
/// grant signed with a key this cell never held, an unsigned grant, and a
/// correctly signed grant for another cell. Only the honest one comes out as
/// a value; the other four come out as named refusals. Then the direct
/// `VerifiedEnvelope::verify` path, and the refusal to build a verifier with
/// no key. The earlier version signed with `DefaultHasher` in the test body
/// and verified with the same function.
///
/// **Not claimed:** "authorised producer" as an identity. The envelope key is
/// a shared HMAC secret (`sign_payload`'s own doc: it proves possession, not
/// signer identity); who may *publish* to the P0 stream is the catalogue's
/// rule, FABRIC-111.
///
/// Mutation: in `VerifiedEnvelope::verify_with`, ignore the `root.check`
/// result — the forged, wrong-key and unsigned grants are verified and the
/// downlink assertion fires. Mutation: in `VerifiedEnvelope::verify`, skip the
/// signature comparison — the direct-path assertions fire.
#[test]
fn fabric_081_a_control_grant_is_consumed_only_after_its_signature_verifies() {
    let honest = grant("s-honest", CELL, dec!("1000"), Some(KEY)).unwrap();
    let signed_then_raised = {
        let original = grant("s-raised", CELL, dec!("1000"), Some(KEY)).unwrap();
        CapitalEnvelope::new(
            StrategyId::new("s-raised"),
            CELL,
            dec!("9000000"),
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            at(0),
            at(7_200),
            "approver-1",
            original.signature(),
        )
        .expect("a well-formed envelope carrying the old signature")
    };
    let wrong_key = grant("s-wrong-key", CELL, dec!("1000"), Some(OTHER_KEY)).unwrap();
    let unsigned = grant("s-unsigned", CELL, dec!("1000"), None).unwrap();
    let other_cell = grant("s-other-cell", "paris-1", dec!("1000"), Some(KEY)).unwrap();

    let mut script = ScriptedFrames::new(8).unwrap();
    for (envelope, id) in [
        (honest.clone(), "EVT-honest"),
        (signed_then_raised.clone(), "EVT-raised"),
        (wrong_key.clone(), "EVT-wrong-key"),
        (unsigned.clone(), "EVT-unsigned"),
        (other_cell.clone(), "EVT-other-cell"),
    ] {
        script.push(frame(envelope, id).unwrap()).unwrap();
    }
    let mut downlink = CapitalDownlink::from_source(CELL, KEY, 16, Box::new(script)).unwrap();
    let batch = downlink.poll(at(10)).unwrap();

    assert_eq!(downlink.stats().frames, 5, "premise: five frames arrived");
    let verified: Vec<&str> = batch
        .verified
        .iter()
        .map(|v| v.strategy().as_str())
        .collect();
    assert_eq!(
        verified,
        vec!["s-honest"],
        "only the honest grant is a value"
    );
    let refused: BTreeSet<&str> = batch.refused.iter().map(|r| r.event_id.as_str()).collect();
    assert_eq!(
        refused,
        BTreeSet::from([
            "EVT-raised",
            "EVT-wrong-key",
            "EVT-unsigned",
            "EVT-other-cell"
        ])
    );
    assert_eq!(downlink.stats().verified, 1);
    assert_eq!(downlink.stats().refused, 4);

    // The direct path, same inputs.
    assert!(VerifiedEnvelope::verify(honest.clone(), KEY, CELL, at(10)).is_ok());
    for (what, envelope) in [
        ("raised after signing", signed_then_raised),
        ("wrong key", wrong_key),
        ("unsigned", unsigned),
    ] {
        assert!(
            VerifiedEnvelope::verify(envelope, KEY, CELL, at(10)).is_err(),
            "a grant {what} verified"
        );
    }
    assert!(VerifiedEnvelope::verify(other_cell, KEY, CELL, at(10)).is_err());
    assert!(
        VerifiedEnvelope::verify(honest.clone(), b"", CELL, at(10)).is_err(),
        "a cell with no key verified a grant"
    );
    assert!(
        VerifiedEnvelope::verify(honest, KEY, CELL, at(8_000)).is_err(),
        "an expired grant verified"
    );
    let no_verifier =
        CapitalDownlink::from_source(CELL, b"", 16, Box::new(ScriptedFrames::new(1).unwrap()));
    assert!(no_verifier.is_err(), "a downlink was built with no key");
}

// --- FABRIC-082 --------------------------------------------------------------

/// A ledger store that can be taken down from outside and is idempotent on
/// `event_id`.
#[derive(Debug)]
struct Ledger {
    down: Arc<AtomicBool>,
    effects: BTreeMap<String, u32>,
}

impl OutcomeWriter for Ledger {
    fn apply(&mut self, record: &Record) -> qip_core::error::Result<()> {
        if self.down.load(AtomicOrdering::SeqCst) {
            return Err(qip_core::error::Error::io("the ledger store is down"));
        }
        *self.effects.entry(record.event_id.clone()).or_insert(0) += 1;
        Ok(())
    }
}

/// FABRIC-082: the ledger sink retries a failed commit indefinitely with
/// backoff, never skips or drops the record, and does not advance its offset
/// past a record until that record is committed. Driven through the real
/// `LedgerSink` and a real broker: the store is down for forty consecutive
/// steps, the sink answers `Retrying` every time with the attempt count
/// rising and the wait doubling to its cap and staying there, the group's
/// checkpoint stays unset and the lag stays at the head; when the store
/// returns, the record that waited is committed, then the one produced during
/// the outage, each once in effect. The earlier version looped a counter in a
/// struct it had defined until it said "committed".
///
/// Mutation: in `LedgerSink::step`, stop treating a failed write as failed
/// after five attempts (`&& self.attempts < 5`) — the batch is committed
/// unwritten and the `Retrying` assertion fires at attempt six. Mutation: in
/// `backoff_ms`, delete `.min(cap_ms)` — the cap assertion fires.
#[test]
fn fabric_082_a_ledger_outage_is_retried_without_end_and_nothing_is_skipped_or_committed_early() {
    let (broker, dir) = broker_with("fabric-082", 1, spec(QosClass::P1Outcomes));
    let down = Arc::new(AtomicBool::new(true));
    let ledger = Ledger {
        down: Arc::clone(&down),
        effects: BTreeMap::new(),
    };
    let mut sink = LedgerSink::new("ledger", STREAM, 0, ledger, 10, 1_000).unwrap();
    broker
        .produce(STREAM, "acct", batch("venue", 1, 0, "fill-a"))
        .unwrap();

    let mut waits = Vec::new();
    for attempt in 1..=40_u32 {
        match sink.step(&broker).unwrap() {
            SinkStep::Retrying {
                offset,
                attempts,
                backoff_ms,
            } => {
                assert_eq!(offset, 0);
                assert_eq!(
                    attempts, attempt,
                    "the attempt count rises by one each step"
                );
                waits.push(backoff_ms);
            }
            other => panic!("attempt {attempt} during the outage was {other:?}, not a retry"),
        }
        assert_eq!(
            broker.committed_offset("ledger", STREAM, 0).unwrap(),
            None,
            "the checkpoint moved at attempt {attempt}, before the record was written"
        );
    }
    assert_eq!(sink.retries(), 40);
    assert_eq!(sink.lag(&broker).unwrap(), 1, "the head is still unread");
    assert_eq!(&waits[..7], &[10, 20, 40, 80, 160, 320, 640]);
    assert!(
        waits[7..].iter().all(|wait| *wait == 1_000),
        "the wait left its cap: {waits:?}"
    );
    assert!(sink.writer().effects.is_empty(), "nothing was written");

    // A record produced during the outage waits behind the first.
    broker
        .produce(STREAM, "acct", batch("venue", 1, 1, "fill-b"))
        .unwrap();
    down.store(false, AtomicOrdering::SeqCst);
    assert_eq!(
        sink.step(&broker).unwrap(),
        SinkStep::Committed { offset: 0 }
    );
    assert_eq!(
        sink.step(&broker).unwrap(),
        SinkStep::Committed { offset: 1 }
    );
    assert_eq!(sink.step(&broker).unwrap(), SinkStep::Idle);
    assert_eq!(
        broker.committed_offset("ledger", STREAM, 0).unwrap(),
        Some(1)
    );
    assert_eq!(sink.lag(&broker).unwrap(), 0);
    assert_eq!(
        sink.writer().effects,
        BTreeMap::from([("fill-a".to_string(), 1), ("fill-b".to_string(), 1)]),
        "each outcome committed exactly once in effect"
    );

    // Refused: a sink that would spin or has no name.
    let idle = |down| Ledger {
        down,
        effects: BTreeMap::new(),
    };
    let flag = || Arc::new(AtomicBool::new(false));
    assert!(LedgerSink::new("", STREAM, 0, idle(flag()), 10, 1_000).is_err());
    assert!(LedgerSink::new("g", STREAM, 0, idle(flag()), 0, 1_000).is_err());
    assert!(LedgerSink::new("g", STREAM, 0, idle(flag()), 100, 10).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

// --- FABRIC-083 --------------------------------------------------------------

/// GAP PIN. FABRIC-083 asks that under pressure the *mirroring* of P2 topics
/// be throttled before local P2 ingest is slowed. There is no mirror to
/// throttle: `StreamPolicy::new` refuses every mirroring policy but `none`,
/// for every class, naming conflict C8 ("fabric-mirror is not built"), and the
/// committed catalogue declares `mirroring: none` throughout. ADR 0119
/// decides the mirror's shape; nothing builds it. The earlier version
/// assigned two `f64`s to a struct and compared them. This test asserts the
/// refusal for every class and both non-`none` policies, with `none`
/// admitted first so each refusal is about mirroring and not about the rest
/// of the policy.
///
/// Mutation: delete the `spec.mirroring != Mirroring::None` refusal in
/// `StreamPolicy::new` — a mirrored stream is admitted and the refusal
/// assertion fires.
#[test]
fn fabric_083_gap_mirroring_is_refused_for_every_class_so_there_is_nothing_to_throttle() {
    let mut refusals = 0;
    for class in QosClass::ALL {
        StreamPolicy::new(spec(class)).unwrap_or_else(|error| {
            panic!("premise: {class:?} is admitted with no mirroring: {error}")
        });
        for mirroring in [Mirroring::Selective, Mirroring::Full] {
            let refused = StreamPolicy::new(StreamPolicySpec {
                mirroring,
                ..spec(class)
            })
            .expect_err("a mirrored stream was admitted");
            assert!(refused.to_string().contains("(C8)"), "{refused}");
            refusals += 1;
        }
    }
    assert_eq!(refusals, 10, "premise: five classes, two policies each");

    let catalogue = Catalogue::parse(
        qip_acceptance::read("infrastructure/event-fabric/streams.local.json").as_bytes(),
    )
    .expect("premise: the committed catalogue parses");
    assert!(!catalogue.streams().is_empty(), "premise: streams declared");
    for (name, stream) in catalogue.streams() {
        assert_eq!(stream.policy.mirroring(), Mirroring::None, "{name}");
    }
}

#[test]
fn fabric_084_gap_brokers_are_one_per_zone_on_a_c4_shape_but_capped_at_three_with_no_instance_group()
 {
    // GAP PIN, narrowed. FABRIC-084 asks for five C4D/C4-class broker VMs
    // across three zones per production region. Since lane L22 (2026-10-18)
    // each broker runs in the zone `broker_zones` names for its address,
    // two in one zone are refused at plan, and `machine_type` defaults to
    // and is validated as a C4/C4D shape — this pin asserted the opposite of
    // all three until then (one `var.zone`, a C3 default). What is still
    // short: a region holds at most three brokers (ADR 0111's lease
    // ceiling), not five, and there is no instance group to replace one.
    //
    // Mutation: put the instance block back on `zone = var.zone`, drop the
    // one-per-zone precondition, or default the machine to `c3-standard-4`;
    // the matching assertion fires. Add a
    // `google_compute_region_instance_group_manager`; the group assertion
    // fires.
    let main = qip_acceptance::read("infrastructure/terraform/modules/event-fabric-broker/main.tf");
    let variables =
        qip_acceptance::read("infrastructure/terraform/modules/event-fabric-broker/variables.tf");
    let norm = |line: &str| line.split_whitespace().collect::<Vec<_>>().join(" ");

    let resources: Vec<String> = main
        .lines()
        .map(norm)
        .filter(|line| line.starts_with("resource \""))
        .collect();
    assert!(
        resources
            .iter()
            .any(|line| line.starts_with("resource \"google_compute_instance_template\"")),
        "premise: the template is declared: {resources:?}"
    );
    assert!(
        resources
            .iter()
            .any(|line| { line.starts_with("resource \"google_compute_instance_from_template\"") }),
        "premise: the module instantiates brokers from the template: {resources:?}"
    );
    assert!(
        !resources
            .iter()
            .any(|line| line.contains("instance_group_manager")),
        "an instance group now exists ({resources:?}) — assert how it spreads and replaces brokers"
    );

    // Each broker instance takes its own address's zone, and two in one zone
    // are refused.
    let block: Vec<String> = main
        .lines()
        .skip_while(|line| !line.contains("resource \"google_compute_instance_from_template\""))
        .take_while(|line| !line.starts_with("}"))
        .map(norm)
        .collect();
    assert!(!block.is_empty(), "premise: the instance block was found");
    assert!(
        block
            .iter()
            .any(|line| line == "zone = local.broker_zone[each.value]"),
        "the instances no longer take their own address's zone: {block:?}"
    );
    assert!(
        block.iter().any(|line| line
            == "condition = length(distinct([for address in local.brokers : local.broker_zone[address]])) == length(local.brokers)"),
        "nothing refuses two brokers in one zone: {block:?}"
    );

    let machine = variables
        .lines()
        .skip_while(|line| !line.starts_with("variable \"machine_type\""))
        .take_while(|line| *line != "}")
        .map(norm)
        .collect::<Vec<_>>();
    assert!(
        machine
            .iter()
            .any(|line| line == "default = \"c4-standard-4\""),
        "the default broker shape is not C4: {machine:?}"
    );
    assert!(
        machine
            .iter()
            .any(|line| line.starts_with("condition = can(regex(\"^c4d?-")),
        "nothing refuses a non-C4 broker shape: {machine:?}"
    );

    // GAP: three per region, not five.
    let addresses = hcl_block(&variables, "variable \"broker_internal_addresses\" {")
        .expect("premise: the address variable exists");
    assert!(
        has_line(
            &addresses,
            "condition = length(var.broker_internal_addresses) <= 3"
        ),
        "the ceiling moved: assert five brokers across three zones: {addresses:#?}"
    );
}

#[test]
fn fabric_086_gap_no_stream_may_declare_replication_so_durability_rests_on_one_broker() {
    // GAP PIN. FABRIC-086: a record counts as durable only when replicated as
    // its class requires, never on one device. The previous test built
    // `required_replicas: 3` and asserted `let on_one_device = false;` was
    // false. The production truth is the opposite and is enforced in code:
    // `StreamPolicy::new` refuses every replication factor but one (ADR 0100
    // §3, blocked on C2), so every durable class — P0 and P1 included — is
    // durable on one broker's fsync. The test asserts that refusal for each
    // class, so the gap is evidence and its closing is noticed.
    //
    // Mutation: delete the `spec.replication_factor != 1` refusal in
    // `StreamPolicy::new` — RF3 is admitted and the assertion fires.
    use qip_events::event_fabric::policy::{
        Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy, StreamPolicySpec,
    };
    let spec = |class: QosClass, replication_factor: u32| StreamPolicySpec {
        qos_class: class,
        partition_key: "key".to_string(),
        ordering: Ordering::PerPartition,
        retention: qip_events::retention::RetentionClass::Irreplaceable,
        replication_factor,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: class.ack_floor(),
        byte_quota_per_producer: 1 << 20,
        message_quota_per_producer: 1_000,
        lag_limit: 10_000,
        entitlement: Entitlement::new("ledger", "trade").expect("valid"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 16,
    };
    for class in QosClass::ALL {
        let admitted = StreamPolicy::new(spec(class, 1)).expect("premise: RF1 is admitted");
        assert_eq!(admitted.replication_factor(), 1);
        for rf in [2, 3, 5] {
            let refused = StreamPolicy::new(spec(class, rf))
                .expect_err("a replication factor the fabric cannot honour was admitted");
            assert!(refused.to_string().contains("(C2)"), "{refused}");
        }
    }
}

// --- FABRIC-087 --------------------------------------------------------------

/// The value of the first `key = "value"` line in `block`, unquoted.
fn attribute(block: &[String], key: &str) -> Option<String> {
    block.iter().find_map(|line| {
        let line = norm(line);
        let value = line.strip_prefix(&format!("{key} = "))?;
        Some(value.trim_matches('"').to_string())
    })
}

/// What is wrong with the broker module's discovery records, as findings.
/// The A record names the brokers; the SRV record must point at it, on the
/// broker port, under a `_service._tcp` owner name.
fn srv_discovery_defects(main_tf: &str) -> Vec<String> {
    let mut defects = Vec::new();
    let a = hcl_block(
        main_tf,
        "resource \"google_dns_record_set\" \"event_fabric_broker_srv\" {",
    );
    let srv = hcl_block(
        main_tf,
        "resource \"google_dns_record_set\" \"event_fabric_broker_srv_tcp\" {",
    );
    let (Some(a), Some(srv)) = (a, srv) else {
        return vec!["the A record or the SRV record is not declared".to_string()];
    };
    if !has_line(&a, "type = \"A\"") {
        defects.push("the broker name is not an A record".to_string());
    }
    if !has_line(&a, "rrdatas = var.broker_internal_addresses") {
        defects.push("the A record does not carry the brokers' internal addresses".to_string());
    }
    if !has_line(&srv, "type = \"SRV\"") {
        defects.push("the discovery record is not type SRV".to_string());
    }
    match attribute(&srv, "name") {
        Some(name) if name.starts_with('_') && name.contains("._tcp.") => {}
        other => defects.push(format!(
            "the SRV owner name is not _service._tcp: {other:?}"
        )),
    }
    let target_of_a = attribute(&a, "name");
    let rdata = srv.iter().map(|line| norm(line)).find_map(|line| {
        line.trim_end_matches(',')
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .filter(|rest| rest.contains("${var.broker_port}"))
            .map(str::to_string)
    });
    match rdata
        .as_deref()
        .map(|rdata| rdata.split(' ').collect::<Vec<_>>())
    {
        Some(parts) if parts.len() == 4 => {
            if parts[0].parse::<u16>().is_err() || parts[1].parse::<u16>().is_err() {
                defects.push("the SRV priority or weight is not a number".to_string());
            }
            if parts[2] != "${var.broker_port}" {
                defects.push("the SRV port is not the broker port".to_string());
            }
            if Some(parts[3].to_string()) != target_of_a {
                defects.push(format!(
                    "the SRV target {} is not the A record {target_of_a:?}",
                    parts[3]
                ));
            }
        }
        other => defects.push(format!(
            "the SRV record data is not `p w port target`: {other:?}"
        )),
    }
    defects
}

/// FABRIC-087, the half that exists: the broker module publishes a private
/// SRV record whose owner name is `_qip-event-fabric._tcp.<zone>`, whose
/// target is the A record that carries the brokers' addresses, and whose port
/// is the broker port. The same checker is run on the real module and on two
/// copies of it with the record broken, so a pass is not the checker saying
/// yes to everything. **Gap, pinned:** no client resolves it. A Rust client
/// is handed its endpoint by configuration; no production source in the
/// transport, broker, ledger or node names the SRV owner or an SRV lookup, so
/// "clients bootstrap from Cloud DNS SRV" is a record nobody reads. The
/// earlier version built a `ClientBootstrap` struct with the SRV name in it.
///
/// Mutation: change the SRV record's `type` to `"TXT"` — the type assertion
/// fires. Mutation: point the SRV target at a name other than the A record —
/// the target assertion fires.
#[test]
fn fabric_087_the_broker_module_publishes_an_srv_record_that_points_at_its_brokers_and_no_client_reads_it()
 {
    let main = broker_module("main.tf");
    assert_eq!(srv_discovery_defects(&main), Vec::<String>::new());

    let as_txt = main.replace("type         = \"SRV\"", "type         = \"TXT\"");
    assert_ne!(as_txt, main, "premise: the SRV type was found to break");
    assert!(
        srv_discovery_defects(&as_txt)
            .iter()
            .any(|defect| defect.contains("not type SRV")),
        "the checker did not notice a TXT record"
    );
    let retargeted = main.replace(
        "0 10 ${var.broker_port} qip-event-fabric-broker.",
        "0 10 ${var.broker_port} qip-event-fabric-elsewhere.",
    );
    assert_ne!(
        retargeted, main,
        "premise: the SRV target was found to break"
    );
    assert!(
        srv_discovery_defects(&retargeted)
            .iter()
            .any(|defect| defect.contains("is not the A record")),
        "the checker did not notice a target that is not the A record"
    );

    // GAP: nothing that talks to the fabric resolves SRV.
    let mut scanned = 0;
    let mut readers = Vec::new();
    for root in [
        "backend/crates/libs/qip-transport/src",
        "backend/crates/services/qip-streaming/src",
        "backend/crates/apps/qip-fabricd/src",
        "backend/crates/apps/qip-ledgerd/src",
        "backend/crates/apps/qip-edge-node/src",
    ] {
        for path in qip_acceptance::files_with_extension(root, "rs") {
            let text = qip_acceptance::production_text(
                &std::fs::read_to_string(&path).expect("a readable source file"),
            );
            scanned += 1;
            let names_srv = text.contains("_qip-event-fabric")
                || text
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|word| word.eq_ignore_ascii_case("srv"));
            if names_srv {
                readers.push(path.display().to_string());
            }
        }
    }
    assert!(
        scanned > 30,
        "premise: the client and broker sources were scanned: {scanned}"
    );
    assert!(
        readers.is_empty(),
        "a production source now names an SRV lookup ({readers:?}) — assert the bootstrap"
    );
}

// --- FABRIC-088 --------------------------------------------------------------

/// GAP PIN. FABRIC-088 asks that after bootstrap a client obtain the live
/// broker/partition map from fabric metadata and send each partition's
/// records to its current leader. What the metadata route answers is one
/// partition's `{stream, partition, leader_epoch, high_watermark,
/// archived_through}` — an epoch, not a leader's address and not a map — and
/// a producer or consumer is built bound to one `(stream, partition)` and one
/// transport. `partition::leader_for` (rendezvous hashing over a broker set)
/// exists and is tested in `qip-streaming`, but no production source outside
/// its own file calls it. The earlier version built a `PartitionMap` of its
/// own and asserted its first leader was 1.
///
/// Mutation: rename the wire field `leader_epoch` in `MetadataWire`
/// (`qip-transport`) — the key-set assertion fires. Mutation: add a
/// production line naming `leader_for(` to any file under `backend/crates` —
/// the caller scan fires.
#[test]
fn fabric_088_gap_metadata_carries_an_epoch_not_a_leader_map_and_nothing_routes_by_leader() {
    let metadata = Metadata::new("fills", 3, 7, 40, 12).expect("a coherent metadata answer");
    let wire = serde_json::to_value(&metadata).expect("metadata serialises");
    let keys: BTreeSet<&str> = wire
        .as_object()
        .expect("metadata is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "archived_through",
            "high_watermark",
            "leader_epoch",
            "partition",
            "stream"
        ]),
        "the metadata answer changed shape — if it now names a leader or a broker map, assert \
         that clients route by it"
    );

    // The router exists, and exactly one production file calls it: the
    // broker, to decide which instance contends at once for a managed
    // partition's lease (ADR 0111 Amendment 1 §2, R12-FABRIC-8085). No client
    // routes by it; the metadata answer above names no leader.
    let sources = qip_acceptance::files_with_extension("backend/crates", "rs");
    let mut scanned = 0;
    let mut callers = Vec::new();
    for path in &sources {
        let shown = path.display().to_string().replace('\\', "/");
        if shown.contains("/tests/") || shown.ends_with("/event_fabric/partition.rs") {
            continue;
        }
        let text = qip_acceptance::production_text(
            &std::fs::read_to_string(path).expect("a readable source file"),
        );
        scanned += 1;
        if text.contains("leader_for(") {
            callers.push(shown);
        }
    }
    assert!(
        scanned > 300,
        "premise: the whole workspace was scanned: {scanned}"
    );
    let router =
        qip_acceptance::read("backend/crates/services/qip-streaming/src/event_fabric/partition.rs");
    assert!(
        router.contains("pub fn leader_for<"),
        "premise: the router exists where this test says it does"
    );
    assert!(
        callers.len() == 1
            && callers[0]
                .ends_with("backend/crates/services/qip-streaming/src/event_fabric/broker.rs"),
        "the only production caller of leader_for is the broker's lease placement; a client or \
         another service calling it ({callers:?}) is a client routing by leader — assert that"
    );
    let broker =
        qip_acceptance::read("backend/crates/services/qip-streaming/src/event_fabric/broker.rs");
    assert!(
        broker.contains("partition::leader_for(stream, partition, &names)"),
        "premise: the caller is the lease placement and not some other use"
    );
}

// --- FABRIC-089 --------------------------------------------------------------

/// What breaks "no external address" in the broker module's Terraform, as
/// findings: an `access_config` anywhere (an external address exists only
/// where one is declared), a machine not pinned to one of the module's
/// internal addresses, or a template not tagged internal-only.
fn external_address_defects(main_tf: &str) -> Vec<String> {
    let mut defects = Vec::new();
    for line in main_tf.lines() {
        if line.trim_start().starts_with("access_config") {
            defects.push(format!("an external address is declared: {}", norm(line)));
        }
    }
    let machine = hcl_block(
        main_tf,
        "resource \"google_compute_instance_from_template\" \"event_fabric_broker\" {",
    );
    match machine {
        Some(machine) => {
            if !has_line(&machine, "for_each = local.brokers") {
                defects.push("the machines are not one per internal address".to_string());
            }
            let interface = hcl_block(&machine.join("\n"), "network_interface {");
            if !interface.as_deref().is_some_and(|block| {
                has_line(
                    block,
                    "network_ip = google_compute_address.event_fabric_broker[each.value].address",
                )
            }) {
                defects.push("a machine is not pinned to its stable internal address".to_string());
            }
        }
        None => defects.push("the broker machine is not declared".to_string()),
    }
    // FABRIC-8099: the address is a reservation, not only a literal on the
    // interface, so a replaced machine leaves it held rather than free.
    match hcl_block(
        main_tf,
        "resource \"google_compute_address\" \"event_fabric_broker\" {",
    ) {
        Some(reserved) => {
            for want in [
                "for_each = local.brokers",
                "address_type = \"INTERNAL\"",
                "subnetwork = var.subnet_id",
                "address = each.value",
            ] {
                if !has_line(&reserved, want) {
                    defects.push(format!("a broker address reservation lacks `{want}`"));
                }
            }
        }
        None => defects.push("no broker address is reserved as an internal address".to_string()),
    }
    if !main_tf.lines().any(|line| {
        norm(line)
            == "tags = concat(var.network_tags, [\"event-fabric-broker\", \"internal-only\"])"
    }) {
        defects.push("the template is not tagged internal-only".to_string());
    }
    defects
}

/// FABRIC-089: every broker has a stable internal address and no public
/// endpoint or external IP. Two layers are read. The module: no
/// `access_config` anywhere, each machine pinned by `network_ip` to one of
/// the addresses its A record carries, the template tagged `internal-only`;
/// and the folder: an organisation policy denying every external address to
/// every VM in it (`compute.vmExternalIpAccess`, `deny_all`), so a future
/// edit of the module still cannot create one. The module checker is run on
/// the real text and on a copy with an `access_config` injected, so a pass is
/// not the checker saying yes to everything. **Gap, pinned:** the address
/// variable's validation accepts any dotted IPv4 literal, public ones
/// included; "internal" is held by the subnet and the folder policy, not by
/// the variable. The earlier version built a `BrokerNetwork { external_ip:
/// None }` and asserted it was `None`.
///
/// Mutation: add `access_config {}` to the machine's `network_interface` —
/// the module assertion fires. Mutation: set the folder policy's `deny_all`
/// to `"FALSE"` — the policy assertion fires. Mutation: set the address
/// reservation's `address_type` to `"EXTERNAL"` — the reservation defect
/// fires (FABRIC-8099).
#[test]
fn fabric_089_brokers_are_pinned_to_internal_addresses_and_no_layer_declares_a_public_one() {
    let main = broker_module("main.tf");
    assert_eq!(external_address_defects(&main), Vec::<String>::new());
    let injected = main.replacen(
        "network_ip = google_compute_address.event_fabric_broker[each.value].address",
        "network_ip = google_compute_address.event_fabric_broker[each.value].address\n    access_config {}",
        1,
    );
    assert_ne!(
        injected, main,
        "premise: the machine's interface was found to break"
    );
    assert!(
        external_address_defects(&injected)
            .iter()
            .any(|defect| defect.contains("an external address is declared")),
        "the checker did not notice an injected access_config"
    );
    let unpinned = main.replacen(
        "network_ip = google_compute_address.event_fabric_broker[each.value].address",
        "",
        1,
    );
    assert!(
        external_address_defects(&unpinned)
            .iter()
            .any(|defect| defect.contains("stable internal address")),
        "the checker did not notice a machine with no pinned address"
    );
    let unreserved = main.replacen(
        "address_type = \"INTERNAL\"",
        "address_type = \"EXTERNAL\"",
        1,
    );
    assert_ne!(
        unreserved, main,
        "premise: the reservation's type was found to break"
    );
    assert!(
        external_address_defects(&unreserved)
            .iter()
            .any(|defect| defect.contains("address_type = \"INTERNAL\"")),
        "the checker did not notice an external reservation"
    );

    let foundation = qip_acceptance::read("infrastructure/foundation/main.tf");
    let policy = hcl_block(
        &foundation,
        "resource \"google_org_policy_policy\" \"vm_external_ip\" {",
    )
    .expect("premise: the folder policy is declared");
    assert!(
        has_line(
            &policy,
            "name = \"folders/${local.folder_id}/policies/compute.vmExternalIpAccess\""
        ),
        "{policy:#?}"
    );
    assert!(
        has_line(&policy, "deny_all = \"TRUE\""),
        "the folder no longer denies every external address: {policy:#?}"
    );

    // GAP: the address variable does not itself require a private range.
    let variables = broker_module("variables.tf");
    let addresses = hcl_block(&variables, "variable \"broker_internal_addresses\" {")
        .expect("premise: the address variable exists");
    assert!(
        addresses
            .iter()
            .any(|line| line.contains("([0-9]{1,3}\\\\.){3}[0-9]{1,3}")),
        "premise: the dotted-IPv4 validation was found: {addresses:#?}"
    );
    assert!(
        !addresses.iter().any(|line| {
            let line = norm(line);
            line.contains("cidrhost") || line.contains("cidrsubnet") || line.contains("10\\\\.")
        }),
        "the variable now requires a private range — assert it refuses 8.8.8.8"
    );
}

// --- FABRIC-092 --------------------------------------------------------------

/// GAP PIN. FABRIC-092 asks that capacity be scaled deliberately by adding
/// brokers and rebalancing, and by *increasing partitions per domain ahead of
/// peaks*. What exists: no autoscaler anywhere near the broker (so scaling is
/// deliberate, which is the half that holds), a ceiling of three addresses
/// per region, and a broker that **refuses** to change a declared stream's
/// partition count — because that would move every existing key — so the
/// second half has no operation. Driven against a real broker: redeclaring
/// the same count is idempotent, redeclaring with more partitions is denied
/// and leaves the count at what it was. The earlier version compared two
/// integers in a struct called `CapacityPlan`.
///
/// Mutation: delete the partition-count refusal in `Broker::declare_stream` —
/// the stream grows and the refusal assertion fires.
#[test]
fn fabric_092_gap_capacity_is_added_by_hand_and_a_streams_partitions_cannot_be_raised() {
    let (broker, dir) = broker_with("fabric-092", 2, spec(QosClass::P1Outcomes));
    assert_eq!(broker.partition_count(STREAM).unwrap(), 2);

    let redeclare = |count: u32| {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)));
        broker.declare_stream(
            STREAM,
            count,
            StreamPolicy::new(spec(QosClass::P1Outcomes)).unwrap(),
            SegmentLogConfig::new(clock).with_roll_after_bytes(10_000_000),
        )
    };
    redeclare(2).expect("premise: declaring the same count again is idempotent");
    let refused = redeclare(4).expect_err("a stream's partitions were raised in place");
    assert!(
        refused
            .to_string()
            .contains("silently change which partition"),
        "{refused}"
    );
    assert_eq!(
        broker.partition_count(STREAM).unwrap(),
        2,
        "a refused redeclaration still changed the count"
    );

    // Deliberate scaling: no autoscaler or group manager scales the brokers.
    let scaling: Vec<String> = ["main.tf", "variables.tf"]
        .into_iter()
        .flat_map(|file| resources(&broker_module(file)))
        .chain(resources(&qip_acceptance::read(
            "infrastructure/terraform/fabric.tf",
        )))
        .filter(|(kind, _)| kind.contains("autoscaler") || kind.contains("instance_group_manager"))
        .map(|(kind, name)| format!("{kind}.{name}"))
        .collect();
    assert!(
        scaling.is_empty(),
        "something now scales the brokers ({scaling:?}) — assert how, and that it is ahead of peak"
    );
    let injected = "resource \"google_compute_region_autoscaler\" \"brokers\" {\n}\n";
    assert_eq!(
        resources(injected),
        vec![(
            "google_compute_region_autoscaler".to_string(),
            "brokers".to_string()
        )],
        "premise: the resource reader sees an autoscaler"
    );
    let variables = broker_module("variables.tf");
    let addresses = hcl_block(&variables, "variable \"broker_internal_addresses\" {").unwrap();
    assert!(
        addresses
            .iter()
            .any(|line| norm(line) == "condition = length(var.broker_internal_addresses) <= 3"),
        "the three-broker ceiling is gone: {addresses:#?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// --- FABRIC-093 --------------------------------------------------------------

/// GAP PIN. FABRIC-093 asks that a broker upgrade proceed one broker at a
/// time: drain its leadership, verify replica health, replace its immutable
/// image, let it rejoin, advance. Nothing orchestrates that. The broker
/// module declares exactly five resources — a reserved internal address and
/// one machine per address, an instance template, an A record and an SRV
/// record — and no managed
/// instance group, no update policy and no rolling step; the module says why
/// (a group's surge would put a second writer on one chain). The CI/CD
/// module's rolling replacement is the execution node's and "nothing else
/// about compute". The earlier version set three booleans in a struct in the
/// order the requirement lists them and asserted the last was `Advanced`.
///
/// Mutation: add a `google_compute_region_instance_group_manager` to the
/// module — the resource-set assertion fires.
#[test]
fn fabric_093_gap_no_orchestration_upgrades_brokers_one_at_a_time() {
    let main = broker_module("main.tf");
    let declared: BTreeSet<(String, String)> = resources(&main).into_iter().collect();
    // The address reservation (FABRIC-8099) holds an IP and neither rolls
    // nor drains anything, so it joins the set without moving this pin.
    let expected: BTreeSet<(String, String)> = [
        ("google_compute_address", "event_fabric_broker"),
        ("google_compute_instance_template", "event_fabric_broker"),
        (
            "google_compute_instance_from_template",
            "event_fabric_broker",
        ),
        ("google_dns_record_set", "event_fabric_broker_srv"),
        ("google_dns_record_set", "event_fabric_broker_srv_tcp"),
    ]
    .into_iter()
    .map(|(kind, name)| (kind.to_string(), name.to_string()))
    .collect();
    assert_eq!(
        declared, expected,
        "the broker module declares something new — if it rolls or drains, assert one at a time"
    );
    assert!(
        !main.lines().any(|line| {
            let line = norm(line);
            line.starts_with("update_policy") || line.starts_with("max_unavailable")
        }),
        "an update policy is declared"
    );

    let cicd = qip_acceptance::read("infrastructure/terraform/modules/cicd/main.tf");
    assert!(
        cicd.contains("execution-node rolling replacement"),
        "premise: the rolling replacement this test compares against exists"
    );
    let fabric_words: Vec<&str> = cicd
        .lines()
        .filter(|line| {
            line.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word.eq_ignore_ascii_case("fabric"))
        })
        .collect();
    assert!(
        fabric_words.is_empty(),
        "the CI/CD module now names the fabric ({fabric_words:?}) — assert its upgrade is serial"
    );
}

// --- the managed log, for FABRIC-094 and FABRIC-098 -------------------------------

const TTL_MS: u64 = 3_000;

fn managed_spec() -> StreamPolicySpec {
    StreamPolicySpec {
        replication_factor: 3,
        ack_profile: AckProfile::ManagedMajority,
        ..spec(QosClass::P1Outcomes)
    }
}

/// A broker seated at the managed log as `instance`, with the managed stream
/// declared on one partition.
fn seated(
    label: &str,
    log: Arc<dyn ManagedLog>,
    clock: Arc<dyn Clock>,
    instance: &str,
) -> (Broker, std::path::PathBuf) {
    let dir = fresh_dir(label);
    let broker = Broker::open(&dir, clock.clone())
        .unwrap()
        .with_managed_log(log, instance)
        .unwrap();
    broker
        .declare_stream(
            STREAM,
            1,
            StreamPolicy::new(managed_spec()).unwrap(),
            SegmentLogConfig::new(clock).with_archive_required(true),
        )
        .unwrap();
    broker
        .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    (broker, dir)
}

fn bytes_at(broker: &Broker, offset: u64) -> Vec<u8> {
    let response = broker.fetch(STREAM, 0, offset, 1).unwrap();
    qip_core::hash::from_hex(response.batches()).unwrap()
}

fn payload_at(broker: &Broker, offset: u64) -> Vec<u8> {
    match Batch::decode(&bytes_at(broker, offset)).unwrap() {
        DecodeOutcome::Complete(batch) => batch.records[0].payload.clone(),
        DecodeOutcome::Torn => panic!("a fetched batch is complete"),
    }
}

fn fill(sequence: u64) -> Batch {
    batch_of(
        "cell-a",
        1,
        sequence,
        &[&format!("fill-{sequence}")],
        format!("fill-{sequence}").as_bytes(),
    )
}

// --- FABRIC-094 --------------------------------------------------------------

/// FABRIC-094 as the managed log makes it true: the replicas a P1 record
/// needs live in the managed log, not on the brokers, so taking brokers away
/// for maintenance cannot take the quorum with them, and no replacement can
/// write while the old lease is live. Driven with two brokers sharing a
/// `MemoryManagedLog`: the follower refuses to write while the leader's lease
/// stands; then **both** brokers are dropped, the lease expires, and a broker
/// built from an empty directory takes the lease at the next epoch, reads
/// every committed record back byte for byte and continues the log densely.
/// **Gap, not claimed:** nothing refuses an operator who takes two of three
/// brokers down at once — the guard is the data's location, not a
/// maintenance controller. The earlier version asserted `1 <= 1`.
///
/// Mutation: in `MemoryManagedLog::acquire`, keep the old epoch on takeover —
/// the epoch assertion fires. Mutation: in `MemoryManagedLog::read`, return
/// no rows — the replacement cannot read the committed records back.
#[test]
fn fabric_094_losing_every_broker_loses_no_committed_record_and_no_replacement_writes_early() {
    let time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let log = Arc::new(MemoryManagedLog::new(time.clone()));
    let (a, dir_a) = seated("fabric-094-a", log.clone(), time.clone(), "zone-a");
    let (b, dir_b) = seated("fabric-094-b", log.clone(), time.clone(), "zone-b");
    assert_eq!(
        a.stream_policy(STREAM).unwrap().replication_factor(),
        3,
        "premise: the stream is the replicated kind"
    );
    assert_eq!(a.tend_leases(TTL_MS).unwrap().leading.len(), 1);
    for sequence in 0..3 {
        a.produce_to(STREAM, 0, fill(sequence)).unwrap().unwrap();
    }
    assert_eq!(b.tend_leases(TTL_MS).unwrap().following.len(), 1);
    b.produce_to(STREAM, 0, fill(3))
        .expect_err("a broker without the lease wrote while the leader's lease stood");

    // Every broker is taken away; the lease runs out.
    drop(a);
    drop(b);
    time.advance(Duration::from_millis(i64::try_from(TTL_MS).unwrap() + 1));

    let (c, dir_c) = seated("fabric-094-c", log.clone(), time.clone(), "zone-c");
    let report = c.tend_leases(TTL_MS).unwrap();
    assert_eq!(
        report.leading.get(&(STREAM.to_string(), 0)),
        Some(&2),
        "the replacement takes the lease at the next epoch"
    );
    for offset in 0..3 {
        assert_eq!(
            payload_at(&c, offset),
            format!("fill-{offset}").into_bytes(),
            "committed record {offset} was lost with the brokers"
        );
    }
    let ack = c.produce_to(STREAM, 0, fill(3)).unwrap().unwrap();
    assert_eq!(ack.base_offset(), 3, "dense across the loss");
    assert_eq!(
        log.committed_epochs(&PartitionRef::new(STREAM, 0)),
        vec![1, 1, 1, 2]
    );
    for dir in [dir_a, dir_b, dir_c] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

// --- FABRIC-096 --------------------------------------------------------------

/// FABRIC-096: consumer capacity scales on consumer lag, up to the partition
/// count. What is driven: the lag is a number the sink reads off the broker
/// (batches between its checkpoint and the head); the group coordinator gives
/// each member a distinct share and a member beyond the partition count
/// holds nothing, so the useful ceiling is the partition count; and a stream
/// that never drops refuses producers once a checkpointed group trails past
/// its limit. **Gap, pinned:** nothing reads lag and adds consumers —
/// no Terraform resource or alert names the group-lag series. The earlier
/// version set `current_consumers = 5` inside an `if` it wrote itself.
///
/// Mutation: in `GroupCoordinator`'s `assign`, give every member every
/// partition — the distinct-share assertion fires. Mutation: in
/// `LedgerSink::lag`, return zero — the lag assertion fires.
#[test]
fn fabric_096_lag_is_measured_and_membership_is_capped_by_partitions_but_nothing_scales_on_it() {
    let (broker, dir) = broker_with(
        "fabric-096",
        3,
        StreamPolicySpec {
            lag_limit: 2,
            ..spec(QosClass::P1Outcomes)
        },
    );
    let down = Arc::new(AtomicBool::new(true));
    let mut sink = LedgerSink::new(
        "ledger",
        STREAM,
        0,
        Ledger {
            down: Arc::clone(&down),
            effects: BTreeMap::new(),
        },
        10,
        1_000,
    )
    .unwrap();
    assert_eq!(
        sink.lag(&broker).unwrap(),
        0,
        "premise: nothing to read yet"
    );
    for sequence in 0..5 {
        broker
            .produce_to(STREAM, 0, fill(sequence))
            .unwrap()
            .unwrap_or_else(|_| panic!("premise: batch {sequence} admitted"));
    }
    assert_eq!(sink.lag(&broker).unwrap(), 5, "lag is the unread batches");
    down.store(false, AtomicOrdering::SeqCst);
    sink.step(&broker).unwrap();
    sink.step(&broker).unwrap();
    assert_eq!(
        sink.lag(&broker).unwrap(),
        3,
        "lag falls as the sink catches up"
    );

    // Past the limit, a checkpointed group stops the producer.
    let refused = broker
        .produce_to(STREAM, 0, fill(5))
        .expect_err("a producer was admitted while a group trailed past the lag limit");
    assert!(refused.to_string().contains("trails"), "{refused}");

    // Members beyond the partition count hold nothing.
    let mut coordinator = GroupCoordinator::new(broker.leader_epoch(), 30_000_000_000).unwrap();
    let all = [0_u32, 1, 2];
    for member in ["m0", "m1", "m2", "m3", "m4"] {
        coordinator
            .join(1, "svc", "sinks", STREAM, Some(member), &all)
            .unwrap();
    }
    let shares = coordinator.members("sinks", STREAM);
    assert_eq!(shares.len(), 5, "premise: five members, three partitions");
    let held: Vec<u32> = shares.values().flatten().copied().collect();
    let distinct: BTreeSet<u32> = held.iter().copied().collect();
    assert_eq!(held.len(), 3, "each partition is held once: {shares:?}");
    assert_eq!(distinct, BTreeSet::from([0, 1, 2]));
    assert_eq!(
        shares
            .values()
            .filter(|partitions| partitions.is_empty())
            .count(),
        2,
        "the two members beyond the partition count hold nothing"
    );

    // GAP: no infrastructure scales on lag.
    let files = all_terraform();
    assert!(files.len() > 10, "premise: the Terraform tree was read");
    let lag_readers: Vec<&String> = files
        .iter()
        .filter(|(_, text)| {
            text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|word| word == "qip_fabric_group_lag" || word == "group_lag")
        })
        .map(|(path, _)| path)
        .collect();
    assert!(
        lag_readers.is_empty(),
        "Terraform now names the group-lag series ({lag_readers:?}) — assert it scales consumers"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// --- FABRIC-097 --------------------------------------------------------------

/// FABRIC-097: each sink has its own consumer, backpressure and failure
/// isolation. Driven with two `LedgerSink`s on one partition under two
/// groups, one whose store is permanently down and one healthy: the healthy
/// sink commits every record and its lag reaches zero while the broken one
/// retries without moving its checkpoint, and neither checkpoint is the
/// other's. **Gap, pinned:** the sinks that exist are the ledger (Spanner) and
/// the archiver (object storage); no Bigtable or BigQuery sink is in the
/// fabric's broker, ledger or daemon sources. The earlier version listed
/// three sink names in a struct and asserted the consumer ids were unique.
///
/// Mutation: in `LedgerSink::step`, commit the offset under a fixed group name
/// instead of `self.group` — the two groups collide and the independence
/// assertions fire.
#[test]
fn fabric_097_one_sinks_outage_does_not_move_or_hold_back_another_sinks_checkpoint() {
    let (broker, dir) = broker_with("fabric-097", 1, spec(QosClass::P1Outcomes));
    let healthy_ledger = Ledger {
        down: Arc::new(AtomicBool::new(false)),
        effects: BTreeMap::new(),
    };
    let broken_ledger = Ledger {
        down: Arc::new(AtomicBool::new(true)),
        effects: BTreeMap::new(),
    };
    let mut healthy = LedgerSink::new("sink-ledger", STREAM, 0, healthy_ledger, 10, 1_000).unwrap();
    let mut broken = LedgerSink::new("sink-archive", STREAM, 0, broken_ledger, 10, 1_000).unwrap();
    for sequence in 0..3 {
        broker
            .produce_to(STREAM, 0, fill(sequence))
            .unwrap()
            .unwrap_or_else(|_| panic!("premise: batch {sequence} admitted"));
    }
    for round in 0..3 {
        assert!(
            matches!(broken.step(&broker).unwrap(), SinkStep::Retrying { .. }),
            "premise: the broken sink retries in round {round}"
        );
        assert_eq!(
            healthy.step(&broker).unwrap(),
            SinkStep::Committed { offset: round }
        );
    }
    assert_eq!(healthy.step(&broker).unwrap(), SinkStep::Idle);
    assert_eq!(healthy.lag(&broker).unwrap(), 0);
    assert_eq!(healthy.retries(), 0);
    assert_eq!(broken.lag(&broker).unwrap(), 3);
    assert_eq!(broken.retries(), 3);
    assert_eq!(
        broker.committed_offset("sink-ledger", STREAM, 0).unwrap(),
        Some(2)
    );
    assert_eq!(
        broker.committed_offset("sink-archive", STREAM, 0).unwrap(),
        None,
        "the broken sink's checkpoint moved, or took the healthy one's"
    );
    assert_eq!(healthy.writer().effects.len(), 3);
    assert!(broken.writer().effects.is_empty());

    // GAP: only the ledger and the archiver are sinks.
    let mut scanned = 0;
    let mut other_sinks = Vec::new();
    for root in [
        "backend/crates/services/qip-streaming/src",
        "backend/crates/apps/qip-fabricd/src",
        "backend/crates/apps/qip-ledgerd/src",
    ] {
        for path in qip_acceptance::files_with_extension(root, "rs") {
            let text = qip_acceptance::production_text(
                &std::fs::read_to_string(&path).expect("a readable source file"),
            );
            scanned += 1;
            let names = text
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| {
                    word.eq_ignore_ascii_case("bigtable") || word.eq_ignore_ascii_case("bigquery")
                });
            if names {
                other_sinks.push(path.display().to_string());
            }
        }
    }
    assert!(
        scanned > 20,
        "premise: the sink sources were scanned: {scanned}"
    );
    assert!(
        other_sinks.is_empty(),
        "a Bigtable or BigQuery sink now exists ({other_sinks:?}) — assert its isolation"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// --- FABRIC-098 --------------------------------------------------------------

/// FABRIC-098 as the managed log makes it true: after a broker is lost, a
/// replacement built from an empty directory (the immutable image, nothing
/// else) catches up from the managed log and holds every committed record
/// byte for byte, and the partition is again held by two brokers — the new
/// leader and the replacement — while the replicated stream's policy still
/// says RF3 on a managed majority. The deposed leader's late write is fenced.
/// **Gap, pinned:** nothing *starts* the replacement. The module's machines
/// are `for_each` singletons with no group manager, so a lost machine stays
/// lost until `terraform apply` is run by a person; the earlier version set
/// `all_replicas_rf3: true` in a struct and asserted it.
///
/// Mutation: in `MemoryManagedLog::read`, return no rows — the replacement
/// holds nothing and the byte-for-byte assertion fires. Mutation: delete the
/// fencing of the deposed leader's commit — the late write is admitted.
#[test]
fn fabric_098_a_replacement_broker_catches_up_to_the_full_log_and_the_deposed_leader_is_fenced() {
    let time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let log = Arc::new(MemoryManagedLog::new(time.clone()));
    let (leader, dir_a) = seated("fabric-098-a", log.clone(), time.clone(), "zone-a");
    let (follower, dir_b) = seated("fabric-098-b", log.clone(), time.clone(), "zone-b");
    let policy = leader.stream_policy(STREAM).unwrap();
    assert_eq!(policy.replication_factor(), 3, "premise: an RF3 stream");
    assert_eq!(policy.ack_profile(), AckProfile::ManagedMajority);

    assert_eq!(leader.tend_leases(TTL_MS).unwrap().leading.len(), 1);
    for sequence in 0..5 {
        leader
            .produce_to(STREAM, 0, fill(sequence))
            .unwrap()
            .unwrap();
    }
    assert_eq!(follower.tend_leases(TTL_MS).unwrap().following.len(), 1);

    // The leader is lost: it stops renewing; the follower takes over.
    time.advance(Duration::from_millis(i64::try_from(TTL_MS).unwrap() + 1));
    let report = follower.tend_leases(TTL_MS).unwrap();
    assert_eq!(report.leading.get(&(STREAM.to_string(), 0)), Some(&2));

    // The replacement starts from nothing.
    let (replacement, dir_r) = seated("fabric-098-r", log.clone(), time.clone(), "zone-r");
    assert_eq!(
        replacement.metadata(STREAM, 0).unwrap().high_watermark(),
        0,
        "premise: the replacement starts empty"
    );
    let report = replacement.tend_leases(TTL_MS).unwrap();
    assert_eq!(report.following.len(), 1, "the replacement follows");
    for offset in 0..5 {
        assert_eq!(
            bytes_at(&replacement, offset),
            bytes_at(&follower, offset),
            "the replacement does not hold offset {offset} as the leader does"
        );
    }
    follower.produce_to(STREAM, 0, fill(5)).unwrap().unwrap();
    replacement.tend_leases(TTL_MS).unwrap();
    assert_eq!(
        bytes_at(&replacement, 5),
        bytes_at(&follower, 5),
        "the replacement stopped following"
    );

    let late = leader
        .produce_to(STREAM, 0, fill(5))
        .expect_err("the deposed leader's late write was admitted");
    assert!(late.to_string().contains("superseded"), "{late}");

    // GAP: nothing starts the replacement.
    let main = broker_module("main.tf");
    let machine = hcl_block(
        &main,
        "resource \"google_compute_instance_from_template\" \"event_fabric_broker\" {",
    )
    .expect("premise: the machine resource exists");
    assert!(has_line(&machine, "for_each = local.brokers"));
    assert!(
        !resources(&main)
            .iter()
            .any(|(kind, _)| kind.contains("instance_group_manager")),
        "a group manager now replaces lost brokers — assert it, and that the partition rebalances"
    );
    for dir in [dir_a, dir_b, dir_r] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

// --- FABRIC-101 --------------------------------------------------------------

/// GAP PIN. FABRIC-101 asks for headroom to absorb the loss of one zone and
/// one broker under maintenance at peak. Since lane L22 (2026-10-18) each
/// broker runs in the zone its address is named into and two in one zone are
/// refused, so a zone loss is one broker rather than the region's whole
/// fleet — this pin asserted the single shared `var.zone` until then. What is
/// still open: a region caps at three addresses, and nothing carries a
/// capacity or headroom figure — the catalogue's `peak_bytes_per_second`
/// sizes a stream, not a fleet — so whether two brokers carry the peak is a
/// number nobody computed. The earlier version computed `9 - 3 - 1 > 0` over
/// a struct it had built.
///
/// Mutation: put the machine back on `zone = var.zone` — the spread assertion
/// fires. Mutation: raise the address ceiling above three — the ceiling
/// assertion fires.
#[test]
fn fabric_101_gap_brokers_are_spread_one_per_zone_but_no_fleet_declares_headroom_for_a_zone_loss() {
    let main = broker_module("main.tf");
    let machine = hcl_block(
        &main,
        "resource \"google_compute_instance_from_template\" \"event_fabric_broker\" {",
    )
    .expect("premise: the machine resource exists");
    // Since lane L22 each broker is in a zone of its own, so a zone loss is
    // one broker, not all of them; the headroom half below is still open.
    assert!(
        has_line(&machine, "zone = local.broker_zone[each.value]"),
        "the machines no longer take their own address's zone: {machine:#?}"
    );
    assert!(
        !main.lines().any(|line| {
            let line = norm(line);
            line.starts_with("distribution_policy_zones") || line.starts_with("target_shape")
        }),
        "a multi-zone distribution is declared"
    );

    let variables = broker_module("variables.tf");
    let zones = hcl_block(&variables, "variable \"broker_zones\" {")
        .expect("premise: the per-address zone variable");
    assert!(has_line(&zones, "type = map(string)"), "{zones:#?}");
    let addresses = hcl_block(&variables, "variable \"broker_internal_addresses\" {").unwrap();
    assert!(
        addresses
            .iter()
            .any(|line| norm(line) == "condition = length(var.broker_internal_addresses) <= 3"),
        "the ceiling moved: {addresses:#?}"
    );

    let catalogue: serde_json::Value = serde_json::from_str(&qip_acceptance::read(
        "infrastructure/event-fabric/streams.local.json",
    ))
    .expect("the committed catalogue is JSON");
    let streams = catalogue["streams"].as_array().expect("streams is a list");
    assert!(!streams.is_empty(), "premise: streams declared");
    for stream in streams {
        assert!(
            stream.get("peak_bytes_per_second").is_some(),
            "premise: every stream is sized: {stream}"
        );
        let words: Vec<&str> = stream
            .as_object()
            .expect("a stream is an object")
            .keys()
            .map(String::as_str)
            .filter(|key| key.contains("headroom") || key.contains("spare"))
            .collect();
        assert!(
            words.is_empty(),
            "a stream now declares headroom ({words:?}) — assert it covers a zone and a broker"
        );
    }
}
