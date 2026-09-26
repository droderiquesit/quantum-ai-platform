//! The typed producer: one `(stream, partition, producer_id)`, one epoch,
//! one batch in flight at a time (ADR 0100 §1, §4).
//!
//! # An explicit acknowledgement profile, always
//!
//! CONTRACT-044 and FABRIC-067 make the acknowledgement profile a decision
//! every producer must state, never a broker default: [`ProducerConfig::ack_profile`]
//! takes [`AckProfile`], a type with no `Default` impl anywhere in this
//! workspace, so [`ProducerConfig`] itself cannot be built by omission — see
//! the module-level doctest below. `AckProfile::Quorum` is more than a label
//! here: [`Producer::send`] does not return `Ok` for it until a follow-up
//! [`super::protocol::Metadata`] poll shows `archived_through` has caught up
//! to the batch's own offset, because a leader-accepted-but-not-yet-durable
//! batch is exactly the gap between "sent" and "safe" this profile exists to
//! close.
//!
//! ```compile_fail
//! // A producer's acknowledgement profile has no default. If this line ever
//! // compiles, something has added `impl Default for AckProfile` and this
//! // doctest — the proof that an ack profile can never be silently
//! // omitted — no longer proves anything.
//! use qip_events::event_fabric::policy::AckProfile;
//! let _profile: AckProfile = Default::default();
//! ```
//!
//! # The fenced latch is scoped to the epoch that earned it
//!
//! RES-058: a broker fences a producer epoch, never a producer. When
//! [`super::protocol::Refusal::Fenced`] arrives, this type records *which*
//! epoch was fenced ([`Producer::fenced_epoch`]) rather than flipping a bare
//! "this producer is dead" flag. [`Producer::init`] hands out a fresh epoch
//! whenever it is called, and [`Producer::send`] refuses locally, without
//! touching the network, only when the epoch it currently holds is the one
//! the latch names. A stale `Fenced` answer for an epoch this producer no
//! longer holds — the one m5 in the design review named — therefore cannot
//! latch the *reconnected* producer: reconnecting through [`Producer::init`]
//! obtains a new epoch, and the latch's old value stops matching it.
//!
//! # One batch in flight per partition, structurally
//!
//! [`Producer::send`] takes `&mut self`. Rust's borrow checker refuses a
//! second call while the first is still executing from safe code, so "one
//! batch in flight" is not a counter this type keeps and could get wrong —
//! it is the ordinary rule against calling a `&mut` method twice at once.
//! Concurrency across partitions is obtained by holding one `Producer` per
//! partition, never by sharing one across threads.
//!
//! # Retry and the breaker are for the transport, not for the broker's answer
//!
//! A transport-level failure (the call to [`super::transport::FabricTransport::call`]
//! returned `Err`) is retried up to [`crate::retry::RetryPolicy::max_attempts`]
//! times, spaced by the seeded backoff, and recorded against a per-producer
//! [`crate::breaker::CircuitBreaker`] circuit so a broker that is genuinely
//! down stops costing a full ladder on every subsequent send. A
//! [`super::protocol::Refusal`] is a different fact — the broker answered,
//! and said no — and is never retried here: it is surfaced immediately with
//! whatever corrective detail it carries (a resend sequence, a wait, an
//! isolating operator), because swallowing a named refusal into a retry loop
//! is exactly the failure mode this crate's own module documentation warns
//! against for a full outbound queue, wearing different clothes. The one
//! exception that is not a retry is [`super::protocol::Refusal::OutOfOrderSequence`]:
//! this producer adopts the broker's own `expected` sequence so the *next*
//! `send` is well-formed, but still returns `Err` for the batch that was
//! actually refused.
//!
//! Never reported as acknowledged unless the wire says so: [`Producer::send`]
//! returns `Ok` only for [`super::protocol::Response::Produce`], and treats
//! any other successfully-decoded route (a protocol bug, not a network
//! failure) as an error naming the route it actually got.

use std::fmt;
use std::sync::Arc;

use qip_core::error::{Error, Result};
use qip_core::hash::to_hex;
use qip_core::rng::Xoshiro256;
use qip_core::time::Clock;
use qip_events::event_fabric::codec::{Batch, stamp_drain};
use qip_events::event_fabric::policy::AckProfile;

use crate::breaker::{
    BreakerPolicy, BreakerState, CircuitBreaker, Decision, Outcome, Refusal as BreakerRefusal,
};
use crate::retry::{RetryPolicy, Sleeper};

use super::protocol::{
    MetadataRequest, ProduceAck, ProduceRequest, ProducerInitRequest, Refusal, Request, Response,
    Route,
};
use super::transport::{FabricTransport, Timeouts};

/// Everything [`Producer::new`] needs, gathered so the constructor takes one
/// argument that names its fields rather than nine positional ones two of
/// which would eventually be swapped — the same reasoning
/// `qip_events::event_fabric::policy::StreamPolicySpec` gives for itself.
///
/// Deliberately has no `Default`: it could not derive one even if asked,
/// because [`ProducerConfig::ack_profile`]'s own type has none, and that is
/// the point (CONTRACT-044).
pub struct ProducerConfig {
    /// The seam this producer sends every call over. `+ Send` so a caller may
    /// still move a [`Producer`] into a thread if it chooses to; this type
    /// itself never spawns one.
    pub transport: Box<dyn FabricTransport + Send>,
    pub stream: String,
    pub partition: u32,
    pub producer_id: String,
    /// FABRIC-067: never weaker than the stream's own class requires. This
    /// producer cannot check that locally — the wire protocol does not yet
    /// carry a stream's `QosClass` to a producer that has not fetched its
    /// catalogue entry — so a profile that is too weak surfaces as
    /// [`Refusal::AckTooWeak`] from the broker, not as a local refusal here.
    pub ack_profile: AckProfile,
    pub retry_policy: RetryPolicy,
    pub breaker_policy: BreakerPolicy,
    pub clock: Arc<dyn Clock>,
    pub sleeper: Arc<dyn Sleeper>,
    /// Seeds the retry backoff's jitter. Two producers built from the same
    /// seed produce the same schedule — see [`crate::retry`]'s own module
    /// documentation for why that is what makes the ladder testable at all.
    pub retry_seed: u64,
    pub breaker_seed: u64,
    pub timeouts: Timeouts,
}

impl fmt::Debug for ProducerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProducerConfig")
            .field("stream", &self.stream)
            .field("partition", &self.partition)
            .field("producer_id", &self.producer_id)
            .field("ack_profile", &self.ack_profile)
            .field("retry_policy", &self.retry_policy)
            .field("breaker_policy", &self.breaker_policy)
            .field("retry_seed", &self.retry_seed)
            .field("breaker_seed", &self.breaker_seed)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

/// A typed producer for one `(stream, partition, producer_id)`. See the
/// module documentation for the fenced latch, the retry/breaker split and
/// why one batch is ever in flight.
pub struct Producer {
    transport: Box<dyn FabricTransport + Send>,
    stream: String,
    partition: u32,
    producer_id: String,
    ack_profile: AckProfile,
    retry_policy: RetryPolicy,
    retry_rng: Xoshiro256,
    sleeper: Arc<dyn Sleeper>,
    breaker: CircuitBreaker,
    /// The breaker's key for this producer's one peer. Computed once because
    /// every call site needs the identical string.
    peer_key: String,
    timeouts: Timeouts,
    epoch: Option<u64>,
    /// The epoch a [`Refusal::Fenced`] answer named, if any. Compared against
    /// [`Self::epoch`] rather than read alone — see the module documentation.
    fenced_epoch: Option<u64>,
    next_sequence: u64,
    archived_through: Option<u64>,
}

impl fmt::Debug for Producer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Producer")
            .field("stream", &self.stream)
            .field("partition", &self.partition)
            .field("producer_id", &self.producer_id)
            .field("ack_profile", &self.ack_profile)
            .field("epoch", &self.epoch)
            .field("fenced_epoch", &self.fenced_epoch)
            .field("next_sequence", &self.next_sequence)
            .field("archived_through", &self.archived_through)
            .field("breaker_state", &self.breaker.state(&self.peer_key))
            .finish_non_exhaustive()
    }
}

impl Producer {
    /// Refuses an empty stream or producer id (the request would round-trip
    /// but name nothing a broker could route), and an invalid retry or
    /// breaker policy (see [`RetryPolicy::validate`] and
    /// [`BreakerPolicy::validate`]) — both are configuration mistakes cheaper
    /// to catch here than during the incident they would otherwise surface
    /// in.
    pub fn new(config: ProducerConfig) -> Result<Self> {
        if config.stream.trim().is_empty() {
            return Err(Error::invalid(
                "a producer must name a non-empty stream to send to",
            ));
        }
        if config.producer_id.trim().is_empty() {
            return Err(Error::invalid(
                "a producer must have a non-empty producer id: the broker fences by it",
            ));
        }
        config.retry_policy.validate()?;
        config.breaker_policy.validate()?;
        let peer_key = format!("{}#{}", config.stream, config.partition);
        let breaker =
            CircuitBreaker::new(config.breaker_policy, config.clock, config.breaker_seed, 1)?;
        Ok(Self {
            transport: config.transport,
            stream: config.stream,
            partition: config.partition,
            producer_id: config.producer_id,
            ack_profile: config.ack_profile,
            retry_policy: config.retry_policy,
            retry_rng: Xoshiro256::seeded(config.retry_seed),
            sleeper: config.sleeper,
            breaker,
            peer_key,
            timeouts: config.timeouts,
            epoch: None,
            fenced_epoch: None,
            next_sequence: 0,
            archived_through: None,
        })
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn partition(&self) -> u32 {
        self.partition
    }

    pub fn producer_id(&self) -> &str {
        &self.producer_id
    }

    pub fn ack_profile(&self) -> AckProfile {
        self.ack_profile
    }

    /// The epoch this producer currently holds, or `None` before the first
    /// [`Self::init`].
    pub fn epoch(&self) -> Option<u64> {
        self.epoch
    }

    /// The next `archived_through` this producer has been told about, from
    /// the most recent [`super::protocol::ProduceAck`] — the release signal a
    /// caller's own spool uses to know what it may forget.
    pub fn archived_through(&self) -> Option<u64> {
        self.archived_through
    }

    /// The breaker's view of this producer's one peer.
    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.state(&self.peer_key)
    }

    /// Whether the epoch this producer currently holds is the one a
    /// [`Refusal::Fenced`] answer named. See the module documentation: this
    /// is never true for an epoch this producer no longer holds.
    pub fn is_fenced(&self) -> bool {
        matches!((self.epoch, self.fenced_epoch), (Some(held), Some(fenced)) if held == fenced)
    }

    /// Establish (or re-establish) this producer's epoch. ADR 0100 §4: this
    /// fences whatever producer previously held `producer_id` on this
    /// partition, and resets this producer's own sequence to zero — a fresh
    /// epoch is a fresh dedup window, never a continuation of the old one.
    pub fn init(&mut self) -> Result<u64> {
        let request = Request::ProducerInit(ProducerInitRequest {
            stream: self.stream.clone(),
            partition: self.partition,
            producer_id: self.producer_id.clone(),
        });
        let response = self.call(request)?;
        match response {
            Response::ProducerInit(init) => {
                self.epoch = Some(init.producer_epoch);
                self.next_sequence = 0;
                Ok(init.producer_epoch)
            }
            Response::Refused(refusal) => Err(describe_refusal(Route::ProducerInit, refusal)),
            other => Err(wrong_route(Route::ProducerInit, &other)),
        }
    }

    /// Send one batch, stamping it with this producer's epoch and next
    /// sequence (ADR 0100 §4) before it ever leaves the process.
    ///
    /// `batch` must not already carry a producer id, epoch or sequence — it
    /// is the writer's own stamp, not the drain's, and this method is the
    /// drain. See the module documentation for retry, the breaker, the
    /// fenced latch and the acknowledgement profile.
    pub fn send(&mut self, mut batch: Batch) -> Result<ProduceAck> {
        let epoch = self.epoch.ok_or_else(|| {
            Error::invalid(
                "send() was called before init(): a producer has no epoch to stamp a batch \
                 with until init() assigns one",
            )
        })?;
        if self.is_fenced() {
            return Err(Error::denied(format!(
                "producer {} on {}:{} holds epoch {epoch}, which a newer producer has fenced; \
                 call init() again for a fresh epoch before sending",
                self.producer_id, self.stream, self.partition
            )));
        }

        let sequence = self.next_sequence;
        stamp_drain(&mut batch, &self.producer_id, epoch, sequence);
        let encoded = batch.encode()?;
        let request = Request::Produce(ProduceRequest::new(
            self.stream.clone(),
            self.partition,
            to_hex(&encoded),
        )?);

        let response = self.call(request)?;
        match response {
            Response::Produce(ack) => {
                self.next_sequence = sequence.checked_add(1).ok_or_else(|| {
                    Error::invalid(format!(
                        "the producer sequence for {}:{} would overflow past {sequence}; this \
                         producer must be retired and a new producer id issued",
                        self.stream, self.partition
                    ))
                })?;
                self.archived_through = Some(ack.archived_through());
                self.await_ack_profile(ack)
            }
            Response::Refused(Refusal::Fenced) => {
                // Scoped to the epoch this call actually used, per the
                // module documentation — never a bare "this producer is
                // dead" flag.
                self.fenced_epoch = Some(epoch);
                Err(describe_refusal(Route::Produce, Refusal::Fenced))
            }
            Response::Refused(Refusal::OutOfOrderSequence { expected }) => {
                // A corrective fact, adopted so the *next* send is
                // well-formed — but this batch was still refused, so `Err`
                // either way. See the module documentation's "not a retry".
                self.next_sequence = expected;
                Err(describe_refusal(
                    Route::Produce,
                    Refusal::OutOfOrderSequence { expected },
                ))
            }
            Response::Refused(refusal) => Err(describe_refusal(Route::Produce, refusal)),
            other => Err(wrong_route(Route::Produce, &other)),
        }
    }

    /// `AckProfile::Quorum`'s condition: block, within the retry policy's own
    /// attempt budget, until a [`super::protocol::Metadata`] poll shows
    /// `archived_through` has reached the batch's `base_offset`. Any weaker
    /// profile is satisfied by the leader's own [`ProduceAck`] already in
    /// hand — see the module documentation.
    fn await_ack_profile(&mut self, ack: ProduceAck) -> Result<ProduceAck> {
        if !matches!(self.ack_profile, AckProfile::Quorum)
            || ack.archived_through() >= ack.base_offset()
        {
            return Ok(ack);
        }
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            if !self.retry_policy.may_retry(attempt) {
                return Err(Error::timeout(format!(
                    "produce to {}:{} reached base offset {} but archived_through stayed at {} \
                     after {attempt} metadata poll(s): the quorum acknowledgement this \
                     producer's ack profile requires never arrived within the retry budget",
                    self.stream,
                    self.partition,
                    ack.base_offset(),
                    ack.archived_through()
                )));
            }
            let backoff = self.retry_policy.backoff(attempt, &mut self.retry_rng);
            self.sleeper.sleep(backoff);

            let request = Request::Metadata(MetadataRequest {
                stream: self.stream.clone(),
                partition: self.partition,
            });
            match self.call(request)? {
                Response::Metadata(metadata)
                    if metadata.archived_through() >= ack.base_offset() =>
                {
                    return ProduceAck::new(
                        ack.stream(),
                        ack.partition(),
                        ack.base_offset(),
                        metadata.high_watermark(),
                        metadata.archived_through(),
                    );
                }
                Response::Metadata(_) => continue,
                Response::Refused(refusal) => {
                    return Err(describe_refusal(Route::Metadata, refusal));
                }
                other => return Err(wrong_route(Route::Metadata, &other)),
            }
        }
    }

    /// One call, through the breaker and the retry ladder. Never called for
    /// anything but a transport round trip: a [`Refusal`] is a successful
    /// call by this method's own accounting, because the broker answered.
    fn call(&mut self, request: Request) -> Result<Response> {
        call_with_resilience(
            self.transport.as_mut(),
            &mut self.breaker,
            &self.peer_key,
            &self.retry_policy,
            &mut self.retry_rng,
            self.sleeper.as_ref(),
            self.timeouts,
            request,
        )
    }
}

/// Shared by [`Producer`] and [`super::consumer::Consumer`]: admit through
/// the breaker, call the transport, record the outcome, and retry a
/// transport-level failure up to the policy's own attempt budget. A
/// [`Response`] — including [`Response::Refused`] — is never retried here:
/// the broker answered, which is success by this function's own accounting,
/// and what to do about a refusal is the caller's decision, made with the
/// refusal's own corrective fact in hand.
pub(crate) fn call_with_resilience(
    transport: &mut dyn FabricTransport,
    breaker: &mut CircuitBreaker,
    peer: &str,
    retry_policy: &RetryPolicy,
    retry_rng: &mut Xoshiro256,
    sleeper: &dyn Sleeper,
    timeouts: Timeouts,
    request: Request,
) -> Result<Response> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match breaker.admit(peer) {
            Decision::Refused(refusal) => return Err(breaker_refusal_error(refusal)),
            Decision::Admitted(permit) => match transport.call(request.clone(), timeouts) {
                Ok(response) => {
                    breaker.record(permit, Outcome::Success);
                    return Ok(response);
                }
                Err(error) => {
                    breaker.record(permit, Outcome::failed(&error));
                    if retry_policy.may_retry(attempt) {
                        let backoff = retry_policy.backoff(attempt, retry_rng);
                        sleeper.sleep(backoff);
                        continue;
                    }
                    return Err(error);
                }
            },
        }
    }
}

fn breaker_refusal_error(refusal: BreakerRefusal) -> Error {
    refusal.into()
}

/// An error naming what a [`Refusal`] itself already names, for a caller
/// that only reads `Result<_, qip_core::Error>`. See the module
/// documentation: this is never on a retry path.
pub(crate) fn describe_refusal(route: Route, refusal: Refusal) -> Error {
    match refusal {
        Refusal::Fenced => Error::denied(format!(
            "{route:?}: this producer epoch has been fenced by a newer producer and must not \
             retry; call init() again for a fresh epoch"
        )),
        Refusal::OutOfOrderSequence { expected } => Error::invalid(format!(
            "{route:?}: the batch's sequence is ahead of what this epoch has written; resend \
             from sequence {expected}"
        )),
        Refusal::SequenceConflict => Error::invalid(format!(
            "{route:?}: the batch's sequence matches one already accepted, but its payload \
             does not — this is a collision, not a retry"
        )),
        Refusal::SequenceBelowWindow => Error::invalid(format!(
            "{route:?}: the batch's sequence is behind the deduplication window; too old to \
             tell a retry from a replay"
        )),
        Refusal::SchemaRefused => Error::schema(format!(
            "{route:?}: the batch's schema is not compatible with the stream's registered schema"
        )),
        Refusal::AckTooWeak { class } => Error::denied(format!(
            "{route:?}: the acknowledgement profile is weaker than {} requires",
            class.as_str()
        )),
        Refusal::Quota { retry_after_ms } => Error::guard(format!(
            "{route:?}: this producer's quota is spent; wait {retry_after_ms}ms before trying \
             again"
        )),
        Refusal::Shed { class } => Error::guard(format!(
            "{route:?}: this record was shed under load ({})",
            class.as_str()
        )),
        Refusal::Isolated { operator, reason } => Error::denied(format!(
            "{route:?}: the partition is isolated by {operator}: {reason}"
        )),
        Refusal::DiskBudget { class } => Error::guard(format!(
            "{route:?}: the disk budget for {} is exhausted; new writes wait for the archive \
             to catch up",
            class.as_str()
        )),
        Refusal::AclDenied => Error::denied(format!(
            "{route:?}: the presented identity holds no grant on this stream"
        )),
        Refusal::KeyOutOfScope => Error::denied(format!(
            "{route:?}: the presented identity's grant is scoped to a different partition key"
        )),
    }
}

/// A successfully-decoded [`Response`] that named a route other than the one
/// this call was made on — not a network failure, a protocol bug, and
/// refused rather than trusted regardless (this crate's own
/// [`super::protocol::decode_response`] already checks this on the wire; this
/// is the belt for the case where a future in-process [`FabricTransport`]
/// skips it).
pub(crate) fn wrong_route(expected: Route, actual: &Response) -> Error {
    Error::invalid(format!(
        "a call made on {expected:?} answered with {:?} instead, which is a protocol error, not \
         a transport failure",
        actual.route()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breaker::BreakerPolicy;
    use crate::retry::RecordingSleeper;
    use qip_core::Timestamp;
    use qip_core::time::ManualClock;
    use qip_events::event_fabric::codec::{MessageType, PayloadCodec, Record};
    use std::sync::Mutex;

    /// A scripted [`FabricTransport`] answering one canned [`Response`] per
    /// call, in order, and refusing (a test bug, not a production path) once
    /// the script is exhausted.
    #[derive(Debug)]
    struct ScriptedTransport {
        script: Mutex<Vec<Response>>,
    }

    impl ScriptedTransport {
        fn new(script: Vec<Response>) -> Self {
            Self {
                script: Mutex::new(script),
            }
        }
    }

    impl FabricTransport for ScriptedTransport {
        fn call(&mut self, _request: Request, _timeouts: Timeouts) -> Result<Response> {
            let mut script = self.script.lock().unwrap_or_else(|p| p.into_inner());
            if script.is_empty() {
                return Err(Error::invalid(
                    "test bug: the scripted transport's script ran out of answers",
                ));
            }
            Ok(script.remove(0))
        }
    }

    fn one_record_batch() -> Batch {
        let event = sample_event();
        let record =
            Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes");
        Batch::new(
            MessageType::Data,
            1,
            1,
            PayloadCodec::CanonicalJson,
            vec![record],
        )
        .expect("one record makes a valid batch")
    }

    fn sample_event() -> qip_events::AnyEvent {
        use qip_core::{CorrelationId, EventId, Lineage};
        use qip_events::{Envelope, EventBody, Topic};
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        struct Tick {
            symbol: String,
        }
        impl EventBody for Tick {
            const TOPIC: Topic = Topic::MarketTick;
            const SCHEMA_VERSION: u32 = 1;
        }

        let lineage = Lineage {
            correlation_id: CorrelationId::from_string("COR00000000000000000000001"),
            causation_id: None,
            trace_id: None,
            producer: "producer-test".to_string(),
        };
        let occurred_at = Timestamp::from_civil(2026, 3, 1);
        let envelope = Envelope::new(
            EventId::from_string("EVT0000000000000000000001"),
            occurred_at,
            occurred_at,
            lineage,
            Tick {
                symbol: "SOLO".to_string(),
            },
        );
        envelope.erase().expect("Tick erases to AnyEvent")
    }

    fn producer(script: Vec<Response>) -> Producer {
        let config = ProducerConfig {
            transport: Box::new(ScriptedTransport::new(script)),
            stream: "orders".to_string(),
            partition: 0,
            producer_id: "cell-eu-1".to_string(),
            ack_profile: AckProfile::Quorum,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                ..RetryPolicy::default()
            },
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(ManualClock::new(Timestamp::from_secs(0))),
            sleeper: Arc::new(RecordingSleeper::new()),
            retry_seed: 7,
            breaker_seed: 7,
            timeouts: Timeouts::default(),
        };
        Producer::new(config).expect("a well-formed config builds a producer")
    }

    /// Asserts its own premise (the ack the mock answers with is genuinely
    /// short of the quorum condition — `archived_through` behind
    /// `base_offset`) before asserting that `send` polls metadata until it
    /// catches up and only then returns.
    ///
    /// Mutation: delete the `!matches!(self.ack_profile, AckProfile::Quorum)`
    /// short-circuit's *body* so `await_ack_profile` always returns `ack`
    /// immediately — fails, because the returned ack's `archived_through`
    /// (80, the stale produce-time value) no longer matches the metadata
    /// poll's caught-up value (100) this test asserts on.
    #[test]
    fn a_quorum_ack_profile_is_not_reported_until_a_metadata_poll_shows_the_batch_archived() {
        let init_epoch = 4;
        let produce_ack =
            ProduceAck::new("orders", 0, 100, 120, 80).expect("a coherent produce ack");
        assert!(
            produce_ack.archived_through() < produce_ack.base_offset(),
            "premise: the produce ack is not yet archived through its own base offset"
        );
        let still_behind = super::super::protocol::Metadata::new("orders", 0, init_epoch, 120, 80)
            .expect("a coherent metadata answer");
        let caught_up = super::super::protocol::Metadata::new("orders", 0, init_epoch, 120, 100)
            .expect("a coherent metadata answer");

        let mut producer = producer(vec![
            Response::ProducerInit(super::super::protocol::ProducerInitResponse {
                producer_epoch: init_epoch,
            }),
            Response::Produce(produce_ack),
            Response::Metadata(still_behind),
            Response::Metadata(caught_up),
        ]);
        producer.init().expect("init succeeds");

        let ack = producer
            .send(one_record_batch())
            .expect("send eventually reports once the quorum condition is met");
        assert_eq!(
            ack.archived_through(),
            100,
            "the acknowledged ack must carry the caught-up archived_through, not the stale \
             produce-time value"
        );
    }
}
