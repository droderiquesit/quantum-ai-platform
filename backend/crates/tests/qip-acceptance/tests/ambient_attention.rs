//! The ambient records through the event log, and the ambient promotion fence.
//!
//! AMBIENT-019: a detection is an `AmbientSignal`, an activation an
//! `AttentionEvent` naming it, and both must come back from the hash-chained
//! log unchanged — a discovery that does not replay cannot be attributed.
//! AMBIENT-016: an ambient output reaches nothing that acts except through
//! `promote`.
//!
//! The two bodies below borrow existing Discover and Reason topics because the
//! ambient topics are not yet registered in `qip-events`; registering them is a
//! separate change and nothing here claims it.

#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, read};
use qip_contracts::ambient::{
    AmbientSignal, AttentionEvent, AttentionRouter, Detection, Pathway, RoutingPolicy, SignalClass,
    Trigger,
};
use qip_core::{CorrelationId, Duration, EventId, Lineage, Timestamp, dec};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct SignalBody(AmbientSignal);
impl EventBody for SignalBody {
    const TOPIC: Topic = Topic::SignalGenerated;
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct AttentionBody(AttentionEvent);
impl EventBody for AttentionBody {
    const TOPIC: Topic = Topic::AgentRunCompleted;
    const SCHEMA_VERSION: u32 = 1;
}

#[test]
fn an_ambient_signal_and_the_attention_event_it_caused_round_trip_through_the_event_log_unchanged()
{
    let at = Timestamp::from_secs(1_000);
    let signal = AmbientSignal::new(
        "sig-1",
        SignalClass::Surprise,
        "EURUSD",
        8_000,
        Trigger::Event("world-model-updated".into()),
        at,
        Detection {
            observed_deviation: dec!("0.031"),
            expected_baseline: dec!("1.0842"),
            horizon: Duration::from_secs(3_600),
            novelty_bp: 7_500,
            affected_entities: vec!["EURUSD".into()],
            urgency_bp: 6_000,
            wake_targets: vec![Pathway::SpecialistActivation],
            evidence_ids: vec!["evt-world-model-7".into()],
            expiry: Timestamp::from_secs(4_600),
        },
    )
    .expect("a valid signal");
    let policy = RoutingPolicy::standard(4_000, 2, Duration::from_secs(60), 4).expect("policy");
    let mut router = AttentionRouter::new(policy);
    let attention = router
        .route(&signal, at)
        .expect("routes")
        .into_iter()
        .next()
        .expect("premise: a material signal produced an attention event");
    assert_eq!(
        attention.signal_id,
        signal.id(),
        "the event references its signal"
    );

    let lineage = Lineage::root(CorrelationId::from_string("cor-ambient-1"), "ambient-mesh");
    let signal_event_id = EventId::from_string("evt-signal-1");
    let mut log = EventLog::in_memory();
    log.append(
        &Envelope::new(
            signal_event_id.clone(),
            at,
            at,
            lineage.clone(),
            SignalBody(signal.clone()),
        )
        .erase()
        .expect("erases"),
    )
    .expect("appends the signal");
    log.append(
        &Envelope::new(
            EventId::from_string("evt-attention-1"),
            at,
            at,
            lineage.caused_by(&signal_event_id, "attention-router"),
            AttentionBody(attention.clone()),
        )
        .erase()
        .expect("erases"),
    )
    .expect("appends the attention event");

    assert_eq!(log.verify_chain(), Ok(()));
    let read_signal = log
        .by_topic(Topic::SignalGenerated)
        .first()
        .expect("signal in log")
        .decode::<SignalBody>()
        .expect("decodes")
        .body
        .0;
    let read_attention = log
        .by_topic(Topic::AgentRunCompleted)
        .first()
        .expect("attention event in log")
        .decode::<AttentionBody>()
        .expect("decodes")
        .body
        .0;
    assert_eq!(read_signal, signal);
    assert_eq!(read_attention, attention);
}

const ORDER_PATH: [&str; 7] = [
    "backend/crates/services/qip-execution-engine",
    "backend/crates/services/qip-brokers",
    "backend/crates/services/qip-risk-engine",
    "backend/crates/services/qip-portfolio-engine",
    "backend/crates/services/qip-capital",
    "backend/crates/edge",
    "backend/crates/runtime/qip-kernel",
];

#[test]
fn no_order_path_crate_names_an_ambient_type_and_an_advisory_cannot_be_unwrapped_but_by_promote() {
    // Premise: the fence has something to hold over, and the order path has
    // sources to scan — an empty directory would make the loop below vacuous.
    let ambient = read("backend/crates/libs/qip-contracts/src/ambient.rs");
    assert!(ambient.contains("pub fn promote<"));
    for dir in ORDER_PATH {
        let files = files_with_extension(dir, "rs");
        assert!(!files.is_empty(), "{dir} has no sources to scan");
        for file in files {
            let text = std::fs::read_to_string(&file).expect("readable source");
            assert!(
                !text.contains("qip_contracts::ambient") && !text.contains("Advisory<"),
                "{} reaches an ambient type; an ambient output may only cross `promote`, \
                 and the order path takes no ambient type at all",
                file.display()
            );
        }
    }
    // The payload of both wrappers is private, so `promote` is the one door.
    assert!(!ambient.contains("pub output"));
    assert!(!ambient.contains("impl<T> std::ops::Deref"));
    assert_eq!(ambient.matches("    output: T,").count(), 2);
}
