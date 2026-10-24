//! The desk's fabric record and the edge node's, held equal where the ledger
//! reads them.
//!
//! `qip-ledgerd` consumes one shape of P1 outcome. The edge node's spool
//! writer builds it for a cell's fills (an app crate the kernel cannot depend
//! on); `qip_kernel::desk_fabric` builds it for the desk's. They are two
//! implementations of one wire contract, and two implementations drift: a
//! desk record whose lineage, event id or payload hash is built by a slightly
//! different rule is acknowledged by the broker and then refused by the
//! ledger, which stalls the desk's partition and never skips. This suite builds
//! the same fills through both and compares everything the ledger consumes.
//!
//! Two things differ on purpose and are asserted to, so a "fix" that
//! harmonised them would fail here rather than quietly change the contract:
//! the lineage's `producer` (who wrote the record; the ledger does not read
//! it), and the session — the desk's chain is session 0 for ever, while the
//! edge writer starts at the spool's counter plus one, so the edge side is
//! compared under its own session and normalised to the desk's for the
//! payload.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::reflex::{OutcomeRecord, chain_correlation};
use qip_core::canonical::canonical_json;
use qip_core::{Clock, ManualClock, Timestamp, dec, sha256_hex};
use qip_edge::journal::{Journal, ship};
use qip_edge_node::event_fabric::drain::OUTCOMES_STREAM;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{
    self, RecordKind, SpoolWriter, SpooledRecord, Step, WriterConfig,
};
use qip_events::Topic;
use qip_events::event_fabric::bindings::{self, REFLEX_OUTCOME_RECORDED};
use qip_events::event_fabric::codec::PayloadCodec;
use qip_execution_engine::order::Side;
use qip_kernel::desk_fabric::{self, fabric_record, p1_batch};
use qip_kernel::desk_ledger::{DESK_CELL, DESK_SESSION, DeskLedgerFeed, DeskOutcome};
use qip_kernel::platform::FillBooked;
use qip_observability::metrics::Metrics;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::sync::Arc;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn desk_outcomes() -> Vec<DeskOutcome> {
    let mut feed = DeskLedgerFeed::new();
    for (n, (order, quantity, price)) in [("ord-1", "10", "100"), ("ord-2", "4", "101.25")]
        .into_iter()
        .enumerate()
    {
        feed.admit(
            &FillBooked {
                order_id: order.to_string(),
                fill_id: "fill-1".to_string(),
                object_id: "obj-ACME".to_string(),
                venue: "paper-xnys".to_string(),
                side: Side::Buy,
                quantity: qip_core::Decimal::parse(quantity).unwrap(),
                price: qip_core::Decimal::parse(price).unwrap(),
                costs: dec!("1.5"),
                at: at(n as i64 + 1),
                simulated: true,
            },
            "central-desk",
            "cycle-corr",
        )
        .unwrap();
    }
    let outcomes = feed.pending().to_vec();
    assert_eq!(outcomes.len(), 2, "premise: two fills, two records");
    outcomes
}

/// The same decisions, journalled and written by the edge node's real spool
/// writer under cell `desk`, read back as the outcome records it spooled.
fn through_the_edge_writer(outcomes: &[DeskOutcome]) -> (Vec<SpooledRecord>, u64) {
    let dir = std::env::temp_dir().join(format!("qip-desk-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(at(0)));
    let log = SegmentLog::open(&dir, SegmentLogConfig::new(clock.clone())).unwrap();
    let (sender, receiver) = writer::channel(8).unwrap();
    let (gauge, publisher, _drain) = {
        let thresholds = Thresholds::new(
            1 << 40,
            qip_core::Decimal::parse("0.5").unwrap(),
            qip_core::Decimal::parse("0.9").unwrap(),
        )
        .unwrap();
        let (gauge, spool, drain) =
            PressureGauge::new(thresholds, clock, qip_core::Duration::from_secs(5)).unwrap();
        (gauge, spool, drain)
    };
    let _ = gauge;
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::new(Metrics::new(
        "qip-edge-node",
    ))));
    let config =
        WriterConfig::new(DESK_CELL, at(0), std::time::Duration::from_millis(5), 64).unwrap();
    let mut spool_writer = SpoolWriter::open(log, receiver, publisher, telemetry, config).unwrap();
    let mut mirror = FabricMirror::new(DESK_CELL, sender).unwrap();
    let mut journal = Journal::new();
    for outcome in outcomes {
        let entry = &outcome.record.entry;
        journal.record(entry.decision.clone(), entry.at);
    }
    // The premise that makes the comparison meaningful: sealing the same
    // decisions at the same positions yields the same digests, so any
    // difference below is the envelope's and not the chain's.
    for (outcome, entry) in outcomes.iter().zip(journal.entries()) {
        assert_eq!(entry.digest, outcome.record.entry.digest, "premise");
    }
    ship(&mut journal, &mut mirror, DESK_CELL, Vec::new(), at(10)).expect("room in the channel");
    for _ in 0..64 {
        if spool_writer.step().unwrap() == Step::Idle {
            break;
        }
    }
    let session = spool_writer.session();
    let records = writer::records(spool_writer.spool())
        .unwrap()
        .into_iter()
        .filter(|r| r.event.topic == Topic::ReflexOutcomeRecorded)
        .collect::<Vec<_>>();
    let _ = std::fs::remove_dir_all(&dir);
    (records, session)
}

/// Everything the ledger reads of a desk record equals what the edge writer
/// writes for the same fill.
///
/// Mutation: in `desk_fabric::fabric_record`, build the lineage from
/// `outcome.correlation_id` (the cycle's id) instead of the chain id — the
/// correlation assertion fails; the ledger would refuse the record.
/// Mutation: in `desk_fabric::event_id` change the separator or drop the kind
/// — the id assertion fails.
#[test]
fn a_desk_fill_is_the_record_the_edge_writer_would_have_written() {
    let outcomes = desk_outcomes();
    let (edge, edge_session) = through_the_edge_writer(&outcomes);
    assert_eq!(
        edge.len(),
        outcomes.len(),
        "premise: one P1 record per fill"
    );
    assert_ne!(
        edge_session, DESK_SESSION,
        "premise: the edge writer's session is its counter, so it cannot be the desk's 0"
    );

    for (outcome, edge) in outcomes.iter().zip(&edge) {
        let sequence = outcome.record.entry.sequence;
        let desk_record = fabric_record(outcome).unwrap();
        let desk = desk_record
            .decode_payload(PayloadCodec::CanonicalJson)
            .unwrap();
        let theirs = &edge.event;

        // The envelope's own header.
        assert_eq!(desk.topic, theirs.topic);
        assert_eq!(desk.topic, REFLEX_OUTCOME_RECORDED.topic);
        assert_eq!(desk.schema_version, theirs.schema_version);
        assert_eq!(desk.schema_version, REFLEX_OUTCOME_RECORDED.schema_version);
        assert_eq!(desk.occurred_at, theirs.occurred_at);
        assert_eq!(desk.recorded_at, theirs.recorded_at);
        assert_eq!(desk.sequence, theirs.sequence);
        assert_eq!(desk.lineage.causation_id, theirs.lineage.causation_id);
        assert_eq!(desk.lineage.trace_id, theirs.lineage.trace_id);

        // The id: one scheme, evaluated at each side's own session.
        assert_eq!(
            theirs.event_id.as_str(),
            writer::event_id(
                DESK_CELL,
                edge_session,
                &sequence.to_string(),
                RecordKind::OutcomeRecorded
            )
        );
        assert_eq!(
            desk.event_id.as_str(),
            writer::event_id(
                DESK_CELL,
                DESK_SESSION,
                &sequence.to_string(),
                RecordKind::OutcomeRecorded
            ),
            "the desk's event id is not the edge writer's scheme"
        );
        assert_eq!(
            desk.idempotency_key.as_deref(),
            Some(desk.event_id.as_str())
        );
        assert_eq!(
            theirs.idempotency_key.as_deref(),
            Some(theirs.event_id.as_str())
        );
        assert_eq!(desk_fabric::event_id(sequence), desk.event_id.as_str());

        // The lineage the ledger requires: the chain id the body derives.
        let order = outcome
            .record
            .entry
            .decision
            .order_id()
            .expect("a fill names its order");
        assert_eq!(
            theirs.lineage.correlation_id,
            chain_correlation(DESK_CELL, edge_session, order)
        );
        assert_eq!(
            desk.lineage.correlation_id,
            chain_correlation(DESK_CELL, DESK_SESSION, order),
            "the desk's lineage is not the chain correlation the ledger requires"
        );
        assert_ne!(desk.lineage.correlation_id.as_str(), outcome.correlation_id);

        // The payload: the same OutcomeRecord but for the session.
        let mut normalised = theirs.payload.clone();
        normalised["session"] = serde_json::json!(DESK_SESSION);
        assert_eq!(
            desk.payload, normalised,
            "payloads differ beyond the session"
        );
        for event in [&desk, theirs] {
            assert_eq!(
                event.payload_hash,
                sha256_hex(canonical_json(&event.payload).as_bytes())
            );
        }
        let body: OutcomeRecord = serde_json::from_value(desk.payload.clone()).unwrap();
        assert_eq!(body, outcome.record);

        // The wire record's header.
        assert_eq!(desk_record.trace_id, edge_record_header(edge).trace_id);
        assert_eq!(
            desk_record.source_timestamp_ns,
            edge_record_header(edge).source_timestamp_ns
        );

        // The one deliberate difference.
        assert_ne!(desk.lineage.producer, theirs.lineage.producer);
    }
}

/// The header fields of the record the edge writer encoded, rebuilt from its
/// event through the same codec path.
fn edge_record_header(edge: &SpooledRecord) -> qip_events::event_fabric::codec::Record {
    qip_events::event_fabric::codec::Record::from_any_event(
        &edge.event,
        PayloadCodec::CanonicalJson,
    )
    .unwrap()
}

/// The ledger's own decoder takes the desk's batch, and its envelope check
/// agrees with the producer's preflight.
///
/// Mutation: stamp the desk batch with the P2 schema id, or build the
/// lineage from the cycle id — `decode_batch` refuses and this fails.
#[test]
fn the_ledgers_decoder_accepts_the_desks_batch_as_it_accepts_a_cells() {
    let outcomes = desk_outcomes();
    let records: Vec<_> = outcomes.iter().map(|o| fabric_record(o).unwrap()).collect();
    let mut batch = p1_batch(records).unwrap();
    // The batch is stamped as the edge writer stamps a P1 batch, and the edge
    // drain reads it back as bound for the outcomes stream.
    assert_eq!(batch.schema_id, writer::P1_BATCH_SCHEMA_ID);
    assert_eq!(batch.schema_version, writer::BATCH_SCHEMA_VERSION);
    assert_eq!(
        writer::stream_of(&batch).unwrap(),
        qip_events::event_fabric::policy::QosClass::P1Outcomes
    );
    batch.producer_id = "desk".to_string();
    batch.base_offset = 0;
    // As the broker that accepted it stamps it (ADR 0129): the ledger reads
    // the producing region and principal off the batch, never its own.
    qip_events::event_fabric::codec::stamp_broker_origin(
        &mut batch,
        qip_events::event_fabric::codec::BrokerOrigin {
            region: "europe-west2".to_string(),
            partition: 0,
            principal: "desk".to_string(),
        },
    );
    let context = qip_ledgerd::consumer::fabric_context(OUTCOMES_STREAM, 0, "ledger");
    let decoded = qip_ledgerd::consumer::decode_batch("p0", &context, &batch, 0)
        .expect("the ledger's decoder takes the desk's records");
    assert_eq!(decoded.len(), 2);
    assert!(
        decoded
            .iter()
            .all(|(_, d)| matches!(d, qip_ledgerd::consumer::Decoded::Ledger(_))),
        "{decoded:?}"
    );
}

/// What the two producers must agree on to write to the same stream under the
/// same configuration is one definition, not two that happen to match.
#[test]
fn the_desk_and_the_edge_node_name_one_stream_and_one_set_of_variables() {
    assert_eq!(OUTCOMES_STREAM, bindings::REFLEX_OUTCOMES_STREAM);
    let catalogue = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../infrastructure/event-fabric/streams.local.json"),
    )
    .unwrap();
    assert!(
        catalogue.contains(&format!("\"name\": \"{OUTCOMES_STREAM}\"")),
        "the committed catalogue has no stream named {OUTCOMES_STREAM}"
    );
    assert_eq!(
        qip_fastbrain::ship::FABRIC_URL_VARIABLE,
        qip_edge_node::outbox::FABRIC_URL_VARIABLE
    );
    assert_eq!(
        qip_fastbrain::ship::FABRIC_TOKEN_VARIABLE,
        qip_edge_node::outbox::FABRIC_TOKEN_VARIABLE
    );
    assert_eq!(
        qip_fastbrain::ship::FABRIC_PARTITION_VARIABLE,
        qip_edge_node::outbox::FABRIC_PARTITION_VARIABLE
    );
}
