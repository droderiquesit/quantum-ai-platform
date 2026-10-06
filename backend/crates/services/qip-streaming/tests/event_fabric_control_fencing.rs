//! SEC-026: a superseded controller cannot append to the P0 control stream,
//! whatever order the two instances' calls reach the broker in.
//!
//! `event_fabric_broker.rs` already proves one ordering on a P1 stream: the
//! old instance writes, the new one registers, the old one is refused. The
//! failure a single ordering leaves open is the race around the handover
//! itself — a replacement that registers *between* two of the old instance's
//! writes, or an old instance that re-registers and so becomes the current
//! one again. A halt, a capital grant or a policy written by the instance
//! that lost that race is a control record nobody current issued, and a cell
//! acts on it.
//!
//! So this suite does not pick an ordering. It enumerates every one, against
//! the real [`Broker`], on the stream the committed catalogue declares as
//! `p0_control`, and checks each write against a model that knows only the
//! rule: a write is accepted exactly when its instance holds the latest
//! registered epoch.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::error::Result;
use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::catalogue::{Catalogue, StreamDeclaration};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_events::event_fabric::policy::QosClass;
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::partition;

/// The catalogue a local fabric is started from. Read from the repository so
/// the stream under test is the one a deployment would declare, not a policy
/// this file invented and called P0.
const CATALOGUE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../infrastructure/event-fabric/streams.local.json"
);

/// Both instances are the same controller: fencing is per producer id, and
/// two different ids do not supersede each other.
const CONTROLLER: &str = "risk-controller";
const KEY: &str = "cell-eu-1";

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-control-fencing-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)))
}

/// The committed catalogue's P0 control stream.
fn control_stream() -> StreamDeclaration {
    let bytes = std::fs::read(CATALOGUE).expect("the committed stream catalogue is readable");
    let catalogue = Catalogue::parse(&bytes).expect("the committed stream catalogue parses");
    let control: Vec<&StreamDeclaration> = catalogue
        .streams()
        .values()
        .filter(|stream| stream.policy.qos_class() == QosClass::P0Control)
        .collect();
    assert_eq!(
        control.len(),
        1,
        "premise: the committed catalogue declares exactly one p0_control stream; with none \
         this suite would be fencing a stream of some other class and calling it P0"
    );
    control[0].clone()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Register as the controller's newest incarnation.
    Register,
    /// Write one control record under the epoch this instance holds.
    Write,
}

/// What each instance does, in its own order. `OLD` re-registers after two
/// writes: an instance that learns it was fenced and starts again is the
/// current one from then on, and its *rival* is the stale one — the half of
/// the rule a script with a fixed loser never reaches.
const OLD: [Step; 5] = [
    Step::Register,
    Step::Write,
    Step::Write,
    Step::Register,
    Step::Write,
];
const NEW: [Step; 3] = [Step::Register, Step::Write, Step::Write];

/// Every way to merge the two scripts that keeps each instance's own steps
/// in order: `true` takes the old instance's next step, `false` the new
/// one's.
fn interleavings(old_left: usize, new_left: usize) -> Vec<Vec<bool>> {
    if old_left == 0 && new_left == 0 {
        return vec![Vec::new()];
    }
    let mut all = Vec::new();
    if old_left > 0 {
        for mut rest in interleavings(old_left - 1, new_left) {
            rest.insert(0, true);
            all.push(rest);
        }
    }
    if new_left > 0 {
        for mut rest in interleavings(old_left, new_left - 1) {
            rest.insert(0, false);
            all.push(rest);
        }
    }
    all
}

fn control_record(epoch: u64, sequence: u64, payload: &str) -> Batch {
    let record = Record {
        event_id: format!("evt-{payload}"),
        trace_id: None,
        source_timestamp_ns: 0,
        payload: payload.as_bytes().to_vec(),
    };
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .expect("a one-record batch is always constructible");
    stamp_drain(&mut batch, CONTROLLER, epoch, sequence);
    batch
}

/// Every payload in the partition, in offset order.
fn stored_payloads(broker: &Broker, stream: &str, partition: u32) -> Vec<String> {
    let response = broker
        .fetch(stream, partition, 0, 1_048_576)
        .expect("a declared partition is fetchable");
    let mut bytes = qip_core::hash::from_hex(response.batches()).expect("fetch returns hex");
    let mut payloads = Vec::new();
    while !bytes.is_empty() {
        let DecodeOutcome::Complete(batch) = Batch::decode(&bytes).expect("stored batches decode")
        else {
            panic!("a fetch never returns a torn batch");
        };
        let consumed = batch.encode().expect("a decoded batch re-encodes").len();
        payloads.extend(
            batch
                .records
                .iter()
                .map(|record| String::from_utf8_lossy(&record.payload).into_owned()),
        );
        bytes.drain(..consumed);
    }
    payloads
}

/// Tallies across the whole enumeration, so the test can assert it actually
/// reached both verdicts rather than passing on a run that fenced nothing.
#[derive(Default)]
struct Tally {
    accepted: usize,
    fenced_old: usize,
    fenced_new: usize,
}

fn run(order: &[bool], control: &StreamDeclaration, tally: &mut Tally) -> Result<()> {
    let dir = temp_dir();
    let stream = control.name.as_str();
    let broker = Broker::open(&dir, clock())?;
    broker.declare_stream(
        stream,
        1,
        control.policy.clone(),
        SegmentLogConfig::new(clock()),
    )?;
    let shape = Shape::of(&1u32).expect("a plain integer always serialises to a shape");
    broker.register_schema(stream, 1, 1, shape)?;
    let partition = partition::partition_for(KEY, 1)?;

    // The model: nothing but the rule under test.
    let mut latest_registered = 0u64;
    let mut held: [Option<u64>; 2] = [None, None];
    let mut expected_log: Vec<String> = Vec::new();
    let mut cursor = [0usize, 0usize];

    for (position, &old_moves) in order.iter().enumerate() {
        let (who, script): (usize, &[Step]) = if old_moves { (0, &OLD) } else { (1, &NEW) };
        let name = if old_moves { "old" } else { "new" };
        let step = script[cursor[who]];
        cursor[who] += 1;

        match step {
            Step::Register => {
                let epoch = broker.init_producer(stream, partition, CONTROLLER)?;
                assert_eq!(
                    epoch,
                    latest_registered + 1,
                    "premise: registration hands out successive epochs ({order:?} step {position})"
                );
                latest_registered = epoch;
                held[who] = Some(epoch);
            }
            Step::Write => {
                let epoch = held[who].expect("every script registers before it writes");
                let payload = format!("{name}-instance-step-{position}");
                // The sequence the partition expects next, so the sequence
                // rule can never be what refuses a stale write: only the
                // epoch can.
                let sequence = expected_log.len() as u64;
                let before = broker.metadata(stream, partition)?.high_watermark();
                let outcome =
                    broker.produce(stream, KEY, control_record(epoch, sequence, &payload));

                if epoch < latest_registered {
                    let refusal = outcome.expect_err(&format!(
                        "epoch {epoch} wrote to the P0 control stream after epoch \
                         {latest_registered} registered ({order:?} step {position})"
                    ));
                    assert!(
                        refusal
                            .to_string()
                            .contains(&format!("is fenced by epoch {latest_registered}")),
                        "a stale control write must be refused as fenced, naming the epoch \
                         that fenced it; got: {refusal}"
                    );
                    assert_eq!(
                        broker.metadata(stream, partition)?.high_watermark(),
                        before,
                        "a refused control write moved the high watermark ({order:?} step \
                         {position})"
                    );
                    if old_moves {
                        tally.fenced_old += 1;
                    } else {
                        tally.fenced_new += 1;
                    }
                } else {
                    outcome.unwrap_or_else(|refusal| {
                        panic!(
                            "the instance holding the latest registered epoch {epoch} was \
                             refused ({order:?} step {position}): {refusal}"
                        )
                    });
                    expected_log.push(payload);
                    tally.accepted += 1;
                }
            }
        }
    }

    assert_eq!(
        stored_payloads(&broker, stream, partition),
        expected_log,
        "the P0 control partition must hold exactly the writes made under the latest \
         registered epoch, in order ({order:?})"
    );
    drop(broker);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Mutation: in `ProducerTable::admit`, drop the `epoch < current_epoch`
/// refusal — a stale instance's write then appends, since its sequence is the
/// one the stream expects, and the `expect_err` on the stale write fires. Or
/// make `ProducerTable::init` return the next epoch without recording it —
/// two registrations with no append between them are then handed the same
/// epoch, so neither fences the other, and the successive-epochs premise
/// fails on the first ordering that registers twice in a row. Both were run
/// on 2026-10-04 and both failed this test.
#[test]
fn across_every_interleaving_of_two_controller_instances_the_p0_control_stream_accepts_no_write_below_the_latest_registered_epoch()
-> Result<()> {
    let control = control_stream();
    let orders = interleavings(OLD.len(), NEW.len());
    assert_eq!(
        orders.len(),
        56,
        "premise: every order-preserving merge of a five-step and a three-step script is \
         enumerated, C(8,3) of them"
    );

    let mut tally = Tally::default();
    for order in &orders {
        run(order, &control, &mut tally)?;
    }

    assert!(
        tally.fenced_old > 0 && tally.fenced_new > 0,
        "premise: the enumeration fenced the old instance {} times and the new one {} times; \
         a run that never fenced either side proves nothing about it",
        tally.fenced_old,
        tally.fenced_new
    );
    assert!(
        tally.accepted > 0,
        "premise: the current instance's writes were accepted; a broker refusing everything \
         would also accept nothing stale"
    );
    Ok(())
}
