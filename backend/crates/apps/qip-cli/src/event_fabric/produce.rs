//! `qip event-fabric produce`: paced, bounded, synthetic load on one stream
//! of a loopback broker, by QoS class (R12-RES-10153, the frozen-sink drill).
//!
//! The backpressure drill needs producers of P0, P1 and P2 running at a
//! stated rate while a consumer is frozen, so that what the broker does to
//! each class is something the broker's own `/metrics` shows rather than
//! something a unit test inside one process asserts. Before this command the
//! only producers that reached a broker process were the edge node's outbox
//! and the fixture grant, neither of which can be told "this class, this
//! rate, this long", so no drill could load the classes against each other.
//!
//! # What it sends
//!
//! Records whose body says, in its own fields, that it is synthetic drill
//! load, on a topic the committed local catalogue admits on the class's
//! stream at the version the embedded schema lock holds — the broker refuses
//! any other topic (FABRIC-024), so a body cannot simply be padding:
//!
//! | Class | Topic | Why this one |
//! |---|---|---|
//! | P0 | `system.kill_switch_engaged` | of the three P0 topics it is the only one whose misreading fails closed. `risk.approved` misread would widen; `policy.distributed` would be an unsigned policy. A cell that ever decoded this body would refuse it, and the restrictive reading of a halt topic is a halt |
//! | P1 | `reflex.fabric_gap` | the ledger books nothing for a gap (`qip-ledgerd`'s `decode_batch`), so a ledger consuming this load commits its checkpoint and moves on; an outcome body would stall it on a record that does not decode |
//! | P2 | `reflex.pass_marked` | a P2-bound topic, so the record is on its own class's stream |
//!
//! P3 and P4 are refused: the drill is about the classes ADR 0100 §5 says
//! are never dropped and the one it says is throttled, and a load generator
//! for telemetry is a different tool.
//!
//! # Bounded, paced, and honest about what it achieved
//!
//! Every quantity is bounded before anything is sent: the rate, the
//! duration, the records per batch, the payload padding, the number of
//! concurrent producers and their product. Each producer has one batch in
//! flight (the SDK's `&mut self` rule) and holds counters, not records, so
//! the command's own memory does not grow with the run. Pacing never
//! bursts to catch up: a slot whose time passed while the previous send was
//! still waiting (a P1 producer waits for the archive) is counted as
//! `behind` and skipped, so the achieved rate printed is the rate that was
//! sent, never the one asked for.
//!
//! A broker refusal is a measurement here, not a failure of the command: a
//! P2 producer over its quota is told to wait, and this command waits as
//! told and counts the refusal. The command fails (exit 1) only when it
//! could not run its schedule at all — arguments, the peer, the token, or a
//! producer the broker would not initialise.
//!
//! # Where it may point
//!
//! Loopback only, plaintext, through [`super::admin::local_peer`]: the same
//! fence `isolate` and `lag` hold, for the same reason — a bearer token on a
//! plaintext link, and a P0 control stream on the other end. The token is
//! read from `QIP_EVENT_FABRIC_PRODUCE_TOKEN_FILE` only.
//!
//! PAPER TRADING. This sends no order, grants nothing, signs nothing and
//! changes no autonomy level.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{CorrelationId, EventId, Lineage, TraceId, sha256_hex};
use qip_events::AnyEvent;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Provenance, Record};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};
use qip_events::topic::Topic;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::Timeouts;
use qip_transport::retry::RetryPolicy;

use super::admin::local_peer;
use super::{Environment, Outcome};

/// `qip event-fabric produce`.
pub const SUBCOMMAND: &str = "produce";

/// The variable whose `_FILE` form names the producing identity's token.
/// The direct form is refused ([`super::grant::file_only`]).
pub const TOKEN_VARIABLE: &str = "QIP_EVENT_FABRIC_PRODUCE_TOKEN";

/// The highest rate one producer may be asked for, in batches per second.
pub const MAXIMUM_RATE: u32 = 1_000;
/// The longest run, in milliseconds: ten minutes. A drill's hold is seconds;
/// a load that outlived its drill would be loading whatever came next.
pub const MAXIMUM_DURATION_MS: u64 = 600_000;
/// The most records one batch may carry.
pub const MAXIMUM_RECORDS_PER_BATCH: u32 = 256;
/// The most padding one record's body may carry, in bytes.
pub const MAXIMUM_PAYLOAD_BYTES: u32 = 16 * 1024;
/// The most concurrent producers one run may hold.
pub const MAXIMUM_PRODUCERS: u32 = 32;
/// The most batches one run may attempt, across every producer. Bounds the
/// product the per-flag limits would otherwise allow (a thousand batches a
/// second, from thirty-two producers, for ten minutes).
pub const MAXIMUM_BATCHES: u64 = 2_000_000;

/// The transport timeouts every call this command makes carries. Stated
/// rather than defaulted: a paused broker must cost a send seconds, not the
/// read default's fifteen, or one frozen peer stalls the schedule it is
/// meant to be measuring.
pub fn timeouts() -> Timeouts {
    Timeouts::new(
        StdDuration::from_secs(1),
        StdDuration::from_secs(5),
        StdDuration::from_secs(5),
    )
}

/// The SDK's default ladder, deliberately. It is also the budget an
/// `archived` producer (P0, P1) waits for the archive within
/// (`RetryPolicy::worst_case_backoff`); a shorter ladder would make every P1
/// batch the broker had admitted read as a timeout here, because the local
/// catalogue seals on a one-second cadence and a two-attempt budget is fifty
/// milliseconds.
fn retry_policy() -> RetryPolicy {
    RetryPolicy::default()
}

const USAGE: &str = "usage: qip event-fabric produce --peer <loopback-address:port> --stream <name> \
     --class <p0|p1|p2> --partition <n> --producer-id <id> --rate <batches-per-second> \
     --duration-ms <ms> [--records-per-batch <n>] [--payload-bytes <n>] [--producers <n>]";

/// The classes this command loads, by the name the flag takes.
fn class_named(name: &str) -> Result<QosClass> {
    match name {
        "p0" | "p0_control" => Ok(QosClass::P0Control),
        "p1" | "p1_outcomes" => Ok(QosClass::P1Outcomes),
        "p2" | "p2_market_journal" => Ok(QosClass::P2MarketJournal),
        other => Err(Error::invalid(format!(
            "--class {other:?} is not one this command loads: p0, p1 or p2. P3 and P4 are \
             refused on purpose; this is the never-dropped and throttled classes' drill"
        ))),
    }
}

/// The topic a class's synthetic records carry. See the module table.
pub fn topic_for(class: QosClass) -> Result<Topic> {
    match class {
        QosClass::P0Control => Ok(Topic::KillSwitchEngaged),
        QosClass::P1Outcomes => Ok(Topic::EventFabricGap),
        QosClass::P2MarketJournal => Ok(Topic::ReflexPassMarked),
        other => Err(Error::invalid(format!(
            "no synthetic topic is chosen for {}; this command loads p0, p1 and p2",
            other.as_str()
        ))),
    }
}

/// The highest version the embedded schema lock holds for `topic`, refused
/// when it holds none: the broker admits a body only at a locked version,
/// and a constant written here would drift from the lock silently.
pub fn locked_version(topic: Topic) -> Result<u32> {
    qip_events::registry::locked_descriptors()?
        .iter()
        .filter(|d| d.topic == topic)
        .map(|d| d.version)
        .max()
        .ok_or_else(|| {
            Error::invalid(format!(
                "the embedded schema lock holds no row for {}, so no broker would admit it",
                topic.name()
            ))
        })
}

/// What the subcommand was told, every quantity already bounded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProduceArguments {
    pub peer: String,
    pub stream: String,
    pub class: QosClass,
    pub partition: u32,
    pub producer_id: String,
    /// Batches per second, per producer.
    pub rate: u32,
    pub duration_ms: u64,
    pub records_per_batch: u32,
    pub payload_bytes: u32,
    pub producers: u32,
}

fn bounded<T: std::str::FromStr + PartialOrd + Copy + std::fmt::Display>(
    flag: &str,
    text: &str,
    least: T,
    most: T,
) -> Result<T> {
    let value = text.parse::<T>().map_err(|_| {
        Error::invalid(format!(
            "{flag} must be a whole number, not {text:?}; {USAGE}"
        ))
    })?;
    // Refused, not clamped: a rate silently lowered is a drill that measured
    // a load nobody asked for and reported the one they did.
    if value < least || value > most {
        return Err(Error::invalid(format!(
            "{flag} {value} is outside {least}..={most}; {USAGE}"
        )));
    }
    Ok(value)
}

/// Parse the arguments after `produce`, refusing an unknown or repeated
/// flag, a missing one, and any quantity outside its bound.
pub fn parse_arguments(arguments: &[String]) -> Result<ProduceArguments> {
    const FLAGS: [&str; 10] = [
        "--peer",
        "--stream",
        "--class",
        "--partition",
        "--producer-id",
        "--rate",
        "--duration-ms",
        "--records-per-batch",
        "--payload-bytes",
        "--producers",
    ];
    let mut found: BTreeMap<&str, &str> = BTreeMap::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let Some(flag) = FLAGS.iter().find(|flag| **flag == argument.as_str()) else {
            return Err(Error::invalid(format!(
                "unknown argument {argument:?}; {USAGE}"
            )));
        };
        let Some(value) = rest.next() else {
            return Err(Error::invalid(format!("{flag} needs a value; {USAGE}")));
        };
        if found.insert(flag, value.as_str()).is_some() {
            return Err(Error::invalid(format!("{flag} was given twice; {USAGE}")));
        }
    }
    let required = |flag: &str| {
        found
            .get(flag)
            .filter(|value| !value.trim().is_empty())
            .map(|value| (*value).to_string())
            .ok_or_else(|| Error::invalid(format!("{flag} is required; {USAGE}")))
    };
    let optional = |flag: &str, default: &'static str| -> String {
        found.get(flag).map_or(default, |v| *v).to_string()
    };
    let parsed = ProduceArguments {
        peer: required("--peer")?,
        stream: required("--stream")?,
        class: class_named(&required("--class")?)?,
        partition: bounded("--partition", &required("--partition")?, 0, u32::MAX)?,
        producer_id: required("--producer-id")?,
        rate: bounded("--rate", &required("--rate")?, 1, MAXIMUM_RATE)?,
        duration_ms: bounded(
            "--duration-ms",
            &required("--duration-ms")?,
            1,
            MAXIMUM_DURATION_MS,
        )?,
        records_per_batch: bounded(
            "--records-per-batch",
            &optional("--records-per-batch", "1"),
            1,
            MAXIMUM_RECORDS_PER_BATCH,
        )?,
        payload_bytes: bounded(
            "--payload-bytes",
            &optional("--payload-bytes", "0"),
            0,
            MAXIMUM_PAYLOAD_BYTES,
        )?,
        producers: bounded(
            "--producers",
            &optional("--producers", "1"),
            1,
            MAXIMUM_PRODUCERS,
        )?,
    };
    let planned = parsed.planned_batches();
    if planned > MAXIMUM_BATCHES {
        return Err(Error::invalid(format!(
            "--rate x --duration-ms x --producers plans {planned} batches, past the bound of \
             {MAXIMUM_BATCHES}; shorten the run or lower the rate"
        )));
    }
    Ok(parsed)
}

impl ProduceArguments {
    /// The batches the schedule holds slots for, across every producer.
    pub fn planned_batches(&self) -> u64 {
        // Slots per producer: one every 1/rate seconds over the duration,
        // the first at zero.
        let per_producer = (u64::from(self.rate) * self.duration_ms).div_ceil(1_000);
        per_producer.saturating_mul(u64::from(self.producers))
    }
}

/// What one run did, summed over its producers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Batches sent to the broker.
    pub attempted: u64,
    /// Batches the broker acknowledged at the class's own ack floor.
    pub admitted: u64,
    /// Records in the admitted batches.
    pub admitted_records: u64,
    /// Sends that did not end in an acknowledgement, by why: `quota` (the
    /// broker said wait), or the error class the SDK returned (`denied` for
    /// a lag-limit or other refusal, `timeout`, `unavailable`, `io`, ...).
    pub refused: BTreeMap<String, u64>,
    /// Schedule slots skipped because the previous send was still in flight.
    pub behind: u64,
    /// Wall time the schedule ran, in milliseconds.
    pub elapsed_ms: u64,
}

impl Tally {
    fn absorb(&mut self, other: Tally) {
        self.attempted += other.attempted;
        self.admitted += other.admitted;
        self.admitted_records += other.admitted_records;
        self.behind += other.behind;
        self.elapsed_ms = self.elapsed_ms.max(other.elapsed_ms);
        for (why, count) in other.refused {
            *self.refused.entry(why).or_insert(0) += count;
        }
    }

    /// One line of JSON, so a drill can keep what the producer saw beside
    /// what the broker counted.
    pub fn to_json(&self, arguments: &ProduceArguments) -> String {
        let value = serde_json::json!({
            "stream": arguments.stream,
            "class": arguments.class.as_str(),
            "producers": arguments.producers,
            "rate_per_producer": arguments.rate,
            "duration_ms": arguments.duration_ms,
            "records_per_batch": arguments.records_per_batch,
            "attempted": self.attempted,
            "admitted": self.admitted,
            "admitted_records": self.admitted_records,
            "refused": self.refused,
            "behind": self.behind,
            "elapsed_ms": self.elapsed_ms,
        });
        canonical_json(&value)
    }
}

/// The synthetic body: it says what it is in its own fields, so a reader
/// that ever met one in a log could not mistake it for a decision.
fn body(arguments: &ProduceArguments, producer_id: &str, sequence: u64) -> serde_json::Value {
    serde_json::json!({
        "synthetic": true,
        "source": "qip event-fabric produce (game-day load; not a decision, not a control)",
        "producer_id": producer_id,
        "sequence": sequence,
        "padding": "x".repeat(arguments.payload_bytes as usize),
    })
}

/// One synthetic event on `topic` at `version`.
fn event(
    arguments: &ProduceArguments,
    topic: Topic,
    version: u32,
    producer_id: &str,
    sequence: u64,
    at: qip_core::Timestamp,
) -> AnyEvent {
    let payload = body(arguments, producer_id, sequence);
    let id = format!("{producer_id}-{sequence}");
    AnyEvent {
        event_id: EventId::from_string(id.clone()),
        topic,
        schema_version: version,
        occurred_at: at,
        recorded_at: at,
        sequence,
        lineage: Lineage {
            correlation_id: CorrelationId::from_string(format!("game-day-load-{producer_id}")),
            causation_id: None,
            trace_id: Some(TraceId::new(format!("game-day-load-{id}"))),
            producer: producer_id.to_string(),
        },
        idempotency_key: Some(id),
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    }
}

/// The batch for slot `slot` of `producer_id`.
pub fn batch(
    arguments: &ProduceArguments,
    producer_id: &str,
    slot: u64,
    at: qip_core::Timestamp,
) -> Result<Batch> {
    let topic = topic_for(arguments.class)?;
    let version = locked_version(topic)?;
    let per_batch = u64::from(arguments.records_per_batch);
    let records = (0..per_batch)
        .map(|index| {
            let sequence = slot.saturating_mul(per_batch).saturating_add(index);
            Record::from_any_event(
                &event(arguments, topic, version, producer_id, sequence, at),
                PayloadCodec::CanonicalJson,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Batch::new(
        MessageType::Data,
        arguments.class.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )?
    .with_provenance(Provenance::Cli))
}

/// Run one producer's schedule: a slot every `1/rate` seconds until the
/// duration ends, one batch per slot, never two in flight, never catching
/// up a missed slot.
fn drive(
    mut producer: Producer,
    arguments: &ProduceArguments,
    clock: &dyn qip_core::Clock,
) -> Tally {
    let mut tally = Tally::default();
    let interval = StdDuration::from_nanos(1_000_000_000 / u64::from(arguments.rate));
    let duration = StdDuration::from_millis(arguments.duration_ms);
    let started = Instant::now();
    let producer_id = producer.producer_id().to_string();
    let mut slot: u64 = 0;
    loop {
        let due = interval.saturating_mul(u32::try_from(slot).unwrap_or(u32::MAX));
        if due >= duration {
            break;
        }
        let now = started.elapsed();
        if now < due {
            std::thread::sleep(due - now);
        } else if now >= due + interval {
            // The previous send held this slot's time. Skip it, counted,
            // rather than burst: a burst would load the broker at a rate
            // nobody asked for and report it as the asked one.
            let missed = (now - due).as_nanos() / interval.as_nanos().max(1);
            let missed = u64::try_from(missed).unwrap_or(u64::MAX);
            tally.behind = tally.behind.saturating_add(missed);
            slot = slot.saturating_add(missed);
            continue;
        }
        tally.attempted += 1;
        let sent = batch(arguments, &producer_id, slot, clock.now())
            .and_then(|batch| producer.send(batch));
        match sent {
            Ok(_) => {
                tally.admitted += 1;
                tally.admitted_records += u64::from(arguments.records_per_batch);
            }
            Err(error) => {
                let why = if producer.quota_wait().is_some() {
                    "quota"
                } else {
                    error.code()
                };
                *tally.refused.entry(why.to_string()).or_insert(0) += 1;
                // Told to wait: wait, inside the run's own end. A producer
                // that ignored the broker's wait would be measuring its own
                // impatience.
                if let Some(wait) = producer.quota_wait() {
                    let left = duration.saturating_sub(started.elapsed());
                    std::thread::sleep(wait.min(left));
                }
            }
        }
        slot = slot.saturating_add(1);
    }
    tally.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tally
}

/// The token, read fresh from its file each time: [`BearerToken`] is not
/// `Clone` on purpose, and each producer's transport holds its own.
pub fn read_token(environment: &Environment) -> Result<BearerToken> {
    let path = super::grant::file_only(environment, TOKEN_VARIABLE)?;
    BearerToken::resolve(TOKEN_VARIABLE, None, path)
}

/// `qip event-fabric produce …`.
///
/// The order is the family's: arguments refused before anything is read,
/// the peer refused before the token is opened, every producer initialised
/// before the first batch is sent, so a run that cannot reach every
/// producer sends nothing rather than a partial load reported as a whole.
pub fn run(arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let arguments = parse_arguments(arguments)?;
    let peer = local_peer(&arguments.peer)?;
    // Refused before any producer exists: a class with no topic, or a topic
    // the lock holds no version of, would refuse every batch.
    locked_version(topic_for(arguments.class)?)?;

    let mut producers = Vec::with_capacity(arguments.producers as usize);
    for index in 0..arguments.producers {
        let producer_id = if arguments.producers == 1 {
            arguments.producer_id.clone()
        } else {
            format!("{}-{index}", arguments.producer_id)
        };
        let mut producer = Producer::new(ProducerConfig {
            transport: environment.connect(peer, read_token(environment)?),
            stream: arguments.stream.clone(),
            partition: arguments.partition,
            producer_id,
            qos_class: arguments.class,
            ack_profile: arguments.class.ack_floor(),
            retry_policy: retry_policy(),
            breaker_policy: BreakerPolicy::default(),
            clock: environment.clock(),
            sleeper: environment.sleeper(),
            retry_seed: 0x5eed_1015_3000 + u64::from(index),
            breaker_seed: 0x5eed_1015_3100 + u64::from(index),
            timeouts: timeouts(),
        })?;
        producer.init()?;
        producers.push(producer);
    }

    let clock: Arc<dyn qip_core::Clock> = environment.clock();
    let mut tally = Tally::default();
    std::thread::scope(|scope| {
        let handles: Vec<_> = producers
            .into_iter()
            .map(|producer| {
                let arguments = &arguments;
                let clock = Arc::clone(&clock);
                scope.spawn(move || drive(producer, arguments, clock.as_ref()))
            })
            .collect();
        for handle in handles {
            match handle.join() {
                Ok(one) => tally.absorb(one),
                // A producer thread that panicked sent an unknown number of
                // batches; it is counted as such rather than as zero.
                Err(_) => *tally.refused.entry("panicked".to_string()).or_insert(0) += 1,
            }
        }
    });

    let achieved = if tally.elapsed_ms == 0 {
        0.0
    } else {
        // A statistic for the operator's eye; nothing decides on it.
        tally.admitted as f64 * 1_000.0 / tally.elapsed_ms as f64
    };
    let refused = if tally.refused.is_empty() {
        "none".to_string()
    } else {
        tally
            .refused
            .iter()
            .map(|(why, count)| format!("{why}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let lines = vec![
        format!(
            "PRODUCE {}:{} class {} — {} producer(s) as {}, {} batch(es)/s each for {} ms, {} \
             record(s) per batch (synthetic game-day load, loopback {peer})",
            arguments.stream,
            arguments.partition,
            arguments.class.as_str(),
            arguments.producers,
            arguments.producer_id,
            arguments.rate,
            arguments.duration_ms,
            arguments.records_per_batch,
        ),
        format!(
            "attempted {}, admitted {} ({} records, {achieved:.1} batches/s achieved), \
             refused: {refused}, behind: {} slot(s) skipped while a send was in flight",
            tally.attempted, tally.admitted, tally.admitted_records, tally.behind
        ),
        format!("RESULT {}", tally.to_json(&arguments)),
        "PAPER TRADING — synthetic records only; no order, grant or autonomy change.".to_string(),
    ];
    Ok(Outcome { lines, code: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(extra: &[&str]) -> Vec<String> {
        let mut all = vec![
            "--peer",
            "127.0.0.1:1",
            "--stream",
            "reflex-journal.local",
            "--class",
            "p2",
            "--partition",
            "0",
            "--producer-id",
            "load",
            "--rate",
            "10",
            "--duration-ms",
            "1000",
        ];
        all.extend_from_slice(extra);
        all.into_iter().map(str::to_string).collect()
    }

    /// Every bound is refused at its edge plus one rather than clamped, and
    /// the edge itself is admitted — a bound that refused everything would
    /// pass the first half alone. Mutation (run, failed, restored): drop
    /// `|| value > most` from `bounded` — the over-bound rows are admitted
    /// and the `unwrap_err` on the first of them fails.
    #[test]
    fn every_quantity_is_refused_past_its_bound_and_admitted_at_it() {
        assert!(parse_arguments(&arguments(&[])).is_ok(), "premise");
        for (flag, edge, past) in [
            (
                "--rate",
                MAXIMUM_RATE.to_string(),
                (MAXIMUM_RATE + 1).to_string(),
            ),
            ("--records-per-batch", "256".to_string(), "257".to_string()),
            ("--payload-bytes", "16384".to_string(), "16385".to_string()),
            ("--producers", "32".to_string(), "33".to_string()),
        ] {
            let mut ok = arguments(&[]);
            if flag == "--rate" {
                ok[11] = edge.clone();
            } else {
                ok.extend([flag.to_string(), edge.clone()]);
            }
            // At the edge the per-run product may itself be past its bound
            // for the rate row; shorten the run so only the flag is judged.
            ok[13] = "100".to_string();
            assert!(parse_arguments(&ok).is_ok(), "{flag} {edge} is the edge");
            let mut bad = ok.clone();
            if flag == "--rate" {
                bad[11] = past.clone();
            } else {
                let at = bad.len() - 1;
                bad[at] = past.clone();
            }
            let refused = parse_arguments(&bad).unwrap_err();
            assert!(refused.message().contains(flag), "{}", refused.message());
        }
        for zero in ["--rate", "--duration-ms"] {
            let mut bad = arguments(&[]);
            let at = bad.iter().position(|a| a == zero).unwrap() + 1;
            bad[at] = "0".to_string();
            assert!(parse_arguments(&bad).is_err(), "{zero} 0");
        }
        let mut long = arguments(&[]);
        long[13] = (MAXIMUM_DURATION_MS + 1).to_string();
        assert!(parse_arguments(&long).is_err());
    }

    /// The product of the per-flag bounds is bounded too.
    #[test]
    fn a_run_whose_product_is_past_the_batch_bound_is_refused() {
        let mut big = arguments(&["--producers", "32"]);
        big[11] = MAXIMUM_RATE.to_string();
        big[13] = MAXIMUM_DURATION_MS.to_string();
        let refused = parse_arguments(&big).unwrap_err();
        assert!(refused.message().contains("plans"), "{}", refused.message());
    }

    #[test]
    fn telemetry_and_research_classes_and_unknown_flags_are_refused() {
        for class in ["p3", "p4", "p4_telemetry", "P0"] {
            let mut bad = arguments(&[]);
            bad[5] = class.to_string();
            assert!(parse_arguments(&bad).is_err(), "{class}");
        }
        assert!(parse_arguments(&arguments(&["--burst", "1"])).is_err());
        assert!(
            parse_arguments(&arguments(&["--rate", "5"])).is_err(),
            "twice"
        );
    }

    /// Each class's synthetic topic is one the committed local catalogue
    /// admits on that class's stream, and the lock holds a version of it;
    /// a reflex topic is also bound to the same class, so a reader opening
    /// one finds no class mismatch. Mutation: send P2 as
    /// `Topic::EventFabricGap` — the P2 stream admits it, but its binding is
    /// P1 and the binding assertion fails.
    #[test]
    fn each_class_sends_a_topic_its_own_stream_admits_at_a_locked_version() {
        let catalogue: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../../infrastructure/event-fabric/streams.local.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let streams = catalogue["streams"].as_array().unwrap();
        for class in [
            QosClass::P0Control,
            QosClass::P1Outcomes,
            QosClass::P2MarketJournal,
        ] {
            let topic = topic_for(class).unwrap();
            let stream = streams
                .iter()
                .find(|s| s["qos_class"] == class.as_str())
                .unwrap_or_else(|| panic!("premise: a {} stream is declared", class.as_str()));
            let admitted: Vec<&str> = stream["topics"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t.as_str())
                .collect();
            assert!(
                !admitted.is_empty(),
                "premise: {} names topics",
                class.as_str()
            );
            assert!(
                admitted.contains(&topic.name()),
                "{} does not admit {}",
                stream["name"],
                topic.name()
            );
            assert!(locked_version(topic).unwrap() >= 1);
            if let Some(binding) = qip_events::event_fabric::bindings::binding_for(topic) {
                assert_eq!(binding.qos_class, class, "{topic:?}");
            }
        }
        assert!(topic_for(QosClass::P4Telemetry).is_err());
    }

    /// A batch's records carry their own identity and a payload that hashes
    /// to its stated hash, so the ledger's envelope check opens them.
    #[test]
    fn a_batch_is_well_formed_and_says_it_is_synthetic() {
        let parsed = parse_arguments(&arguments(&["--records-per-batch", "3"])).unwrap();
        let at = qip_core::Timestamp::from_secs(1_790_000_000);
        let built = batch(&parsed, "load", 2, at).unwrap();
        assert_eq!(built.records.len(), 3);
        assert_eq!(built.schema_id, QosClass::P2MarketJournal.batch_schema_id());
        for (index, record) in built.records.iter().enumerate() {
            let event = record.decode_payload(PayloadCodec::CanonicalJson).unwrap();
            assert_eq!(event.event_id.as_str(), format!("load-{}", 6 + index));
            assert_eq!(event.payload["synthetic"], serde_json::json!(true));
            assert_eq!(
                event.payload_hash,
                sha256_hex(canonical_json(&event.payload).as_bytes())
            );
        }
    }
}
