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

/// The container block of the manifest's first container named `name`: the
/// lines from its `- name:` up to the next container or the end of the list.
fn container_block<'a>(manifest: &'a str, name: &str) -> Vec<&'a str> {
    let opener = format!("    - name: {name}");
    let mut lines = manifest.lines().skip_while(|line| *line != opener);
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    std::iter::once(first)
        .chain(lines.take_while(|line| !line.starts_with("    - ") && line.starts_with("     ")))
        .collect()
}

/// The `cpu` and `memory` limits a container block declares, read as the
/// whole `key: value` line under `resources: limits:`, never by substring.
fn limits(block: &[&str]) -> (Option<String>, Option<String>) {
    let mut lines = block
        .iter()
        .skip_while(|line| line.trim() != "resources:")
        .skip(1);
    if lines.next().map(|line| line.trim()) != Some("limits:") {
        return (None, None);
    }
    let (mut cpu, mut memory) = (None, None);
    for line in lines.take(2) {
        match line.trim().split_once(": ") {
            Some(("cpu", value)) => cpu = Some(value.trim_matches('\'').to_string()),
            Some(("memory", value)) => memory = Some(value.to_string()),
            _ => {}
        }
    }
    (cpu, memory)
}

/// AMBIENT-7026: ambient cognition runs under a budget of its own, so it
/// cannot starve the trading path. The attention router is constructed in
/// exactly one binary, `qip-deepbrain`, and in every environment that binary
/// is a Cloud Run service of its own: one instance, one request at a time,
/// and a CPU and memory limit on its container. The fast brain, the other
/// central binary, is a different service with its own limits, and the
/// regional cell runs on its own Compute Engine node, so nothing the router
/// wakes can spend a CPU second those paths were given.
///
/// Before this test the per-workload parity check in `gitops.rs` held every
/// manifest to the catalogue's figures, but nothing said which workload the
/// ambient router runs in: a second binary constructing it, or the deep
/// brain's limits being dropped from the catalogue and the manifest together,
/// read as fine.
///
/// Mutations: delete the `limits:` block from `envs/dev/deepbrain.yaml` (the
/// limit assertion fires); construct `AttentionRouter::new` above `fn main`
/// in `qip-fastbrain`'s `main.rs` (the single-host assertion fires).
#[test]
fn ambient_cognition_runs_in_one_binary_whose_service_has_its_own_cpu_and_memory_limits() {
    let mut hosts = std::collections::BTreeSet::new();
    let apps = files_with_extension("backend/crates/apps", "rs");
    assert!(!apps.is_empty(), "premise: the apps have sources to scan");
    for file in apps {
        let text = std::fs::read_to_string(&file).expect("readable source");
        if qip_acceptance::production_text(&text).contains("AttentionRouter::new(") {
            let app = file
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .skip_while(|c| c != "apps")
                .nth(1)
                .expect("a file under backend/crates/apps names its app");
            hosts.insert(app);
        }
    }
    assert_eq!(
        hosts,
        std::collections::BTreeSet::from(["qip-deepbrain".to_string()]),
        "the attention router is constructed outside the deep brain, whose service is the \
         budget ambient cognition runs under"
    );

    for environment in ["dev", "test", "stage", "prod"] {
        let deepbrain = read(&format!(
            "infrastructure/gitops/envs/{environment}/deepbrain.yaml"
        ));
        let block = container_block(&deepbrain, "deepbrain");
        assert!(
            block
                .iter()
                .any(|line| line.trim() == "image: qip-deepbrain"),
            "premise: {environment}'s deepbrain manifest runs the qip-deepbrain image: {block:#?}"
        );
        let (cpu, memory) = limits(&block);
        assert!(
            cpu.as_deref().is_some_and(|v| !v.is_empty())
                && memory.as_deref().is_some_and(|v| !v.is_empty()),
            "{environment}'s deep brain declares no CPU and memory limit, so ambient cognition \
             has no budget of its own: {block:#?}"
        );
        for bound in ["maxInstanceCount: 1", "maxInstanceRequestConcurrency: 1"] {
            assert!(
                deepbrain.lines().any(|line| line.trim() == bound),
                "{environment}'s deep brain is not bounded by `{bound}`"
            );
        }

        let fastbrain = read(&format!(
            "infrastructure/gitops/envs/{environment}/fastbrain.yaml"
        ));
        let fast = container_block(&fastbrain, "fastbrain");
        assert!(
            !fast
                .iter()
                .any(|line| line.trim() == "image: qip-deepbrain"),
            "{environment}'s fast brain runs the deep brain's image"
        );
        let (fast_cpu, fast_memory) = limits(&fast);
        assert!(
            fast_cpu.is_some() && fast_memory.is_some(),
            "{environment}'s fast brain declares no limits of its own: {fast:#?}"
        );
    }
}
