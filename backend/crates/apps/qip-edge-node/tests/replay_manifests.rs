//! `qip-edge-node replay-manifests` (R12-CONTRACT-7073, ADR 0155 section
//! 16): manifests built from the pass markers a real spool holds, refused on
//! the marker's configuration digest when the run was under other gateway
//! settings than the replay assumes.
//!
//! The spool is written by the node's own `SpoolWriter` and read back
//! through `replay::run`, the function the binary's subcommand calls, so
//! what is asserted is what a node put on disk.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_brokers::exchange::ExchangeSettings;
use qip_contracts::replay::{AppliedReadings, PassMarker, ReplayManifest};
use qip_core::{Clock, Decimal, ManualClock, Timestamp};
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{self, Handoff, Offer, PassInputs, SpoolWriter, Step};
use qip_edge_node::gateway::{SimulatedGateway, widen_config_digest};
use qip_edge_node::replay;
use qip_observability::metrics::Metrics;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::path::PathBuf;
use std::sync::Arc;

const CELL: &str = "cell-replay";
const CELL_DIGEST: &str = "cell-configuration-digest";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn marker(pass: u64, config_digest: &str) -> PassMarker {
    PassMarker {
        cell: CELL.to_string(),
        session: "1".to_string(),
        pass,
        now_ns: t(pass as i64).as_nanos(),
        tape_digest: "tape".to_string(),
        tape_from: pass,
        tape_to: pass,
        control: Vec::new(),
        readings: AppliedReadings::default(),
        config_digest: config_digest.to_string(),
        plan_digest: "plan".to_string(),
        gateway_seed: 7,
        binary_version: "test".to_string(),
    }
}

/// A spool at a fresh directory holding `markers`, written by the node's own
/// writer and closed again, as a stopped node leaves it.
fn spooled(label: &str, markers: Vec<PassMarker>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qip-replay-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let log = SegmentLog::open(&dir, SegmentLogConfig::new(Arc::clone(&clock))).unwrap();
    let thresholds = Thresholds::new(
        1 << 40,
        Decimal::parse("0.5").unwrap(),
        Decimal::parse("0.9").unwrap(),
    )
    .unwrap();
    let (_gauge, publisher, _drain) =
        PressureGauge::new(thresholds, clock, qip_core::Duration::from_secs(5)).unwrap();
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::new(Metrics::new(
        "qip-edge-node",
    ))));
    let (sender, receiver) = writer::channel(64).unwrap();
    let config =
        writer::WriterConfig::new(CELL, t(0), std::time::Duration::from_millis(5), 64).unwrap();
    let mut spool_writer = SpoolWriter::open(log, receiver, publisher, telemetry, config).unwrap();
    for marker in markers {
        let offered = sender.offer(Handoff::Pass(Box::new(PassInputs {
            marker,
            applied: Vec::new(),
        })));
        assert!(matches!(offered, Offer::Accepted));
    }
    for _ in 0..64 {
        if spool_writer.step().unwrap() == Step::Idle {
            break;
        }
    }
    drop(spool_writer);
    dir
}

fn args(dir: &std::path::Path) -> Vec<String> {
    vec![
        "--spool".to_string(),
        dir.to_string_lossy().to_string(),
        "--cell-digest".to_string(),
        CELL_DIGEST.to_string(),
    ]
}

#[test]
fn a_node_journal_replayed_under_different_gateway_settings_is_refused_on_the_marker_digest() {
    // The failure this prevents: a manifest that states the simulator's
    // default fill model beside a pass that ran under another, whose
    // reproducibility hash then certifies a statement nobody checked. Before
    // this the only replay module in the node was three lines of doc comment,
    // and nothing built a manifest from a spooled marker at all.
    let defaults = SimulatedGateway::default_settings();
    let run_digest = widen_config_digest(CELL_DIGEST, &defaults);

    // Premise: a spool of passes run under the default settings replays, one
    // manifest per marker, each stating the assumptions and hashing
    // consistently, and the line read back is a manifest that deserialises.
    let dir = spooled("same", vec![marker(1, &run_digest), marker(2, &run_digest)]);
    let lines = replay::run(&args(&dir)).expect("passes run under the replay's settings replay");
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        let manifest: ReplayManifest = serde_json::from_str(line).unwrap();
        assert!(manifest.is_consistent().unwrap());
        assert_eq!(
            manifest.assumptions(),
            Some(&qip_edge_node::gateway::assumptions_of(&defaults))
        );
    }

    // The same cell under a gateway refusing a fifth of orders: the node
    // widened its digest with those settings, so the replay, which assumes
    // the defaults, refuses rather than stating the defaults' fill model.
    let refusing = ExchangeSettings {
        rejection_probability: 0.2,
        ..SimulatedGateway::default_settings()
    };
    let other_digest = widen_config_digest(CELL_DIGEST, &refusing);
    assert_ne!(
        other_digest, run_digest,
        "premise: the settings move the digest"
    );
    let dir = spooled(
        "other",
        vec![marker(1, &run_digest), marker(2, &other_digest)],
    );
    let refused = replay::run(&args(&dir)).expect_err("a pass under other settings must refuse");
    assert!(
        refused.message().contains("pass 2") && refused.message().contains(&other_digest),
        "the refusal names the pass and the digest it was recorded under: {}",
        refused.message()
    );

    // And a spool with no markers is refused, not reported as agreeing.
    let dir = spooled("empty", Vec::new());
    assert!(replay::run(&args(&dir)).is_err());
}

/// Every file under `dir`, by relative path, with its bytes.
fn snapshot(dir: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap().to_path_buf();
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

#[test]
fn replaying_a_spool_with_a_torn_tail_leaves_the_spool_byte_for_byte_as_it_was() {
    // The failure this prevents: the replay read opened the spool with the
    // writer's open, which truncates a torn tail — on a running node's
    // spool, the batch its writer is mid-way through appending. A read
    // must not be able to cut a live node's journal.
    let run_digest = widen_config_digest(CELL_DIGEST, &SimulatedGateway::default_settings());
    let dir = spooled("torn", vec![marker(1, &run_digest)]);
    // A partial append: bytes past the last whole batch of the newest
    // segment file, which recovery would cut.
    let (segment, _) = snapshot(&dir)
        .into_iter()
        .filter(|(path, _)| path.parent() == Some(std::path::Path::new("")))
        .max_by_key(|(_, bytes)| bytes.len())
        .expect("premise: the spool holds a segment file");
    let mut torn = std::fs::read(dir.join(&segment)).unwrap();
    torn.extend_from_slice(&[0x5a; 11]);
    std::fs::write(dir.join(&segment), &torn).unwrap();
    let before = snapshot(&dir);

    let lines = replay::run(&args(&dir)).expect("the whole marker before the tear replays");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        snapshot(&dir),
        before,
        "the replay read changed the spool it was pointed at"
    );
}
