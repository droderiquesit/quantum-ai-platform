//! The typed consumer: fetches within a fixed credit, commits with its
//! generation, and can subscribe into a bounded channel without polling on
//! the caller's own thread (ADR 0100 §1; FABRIC-016, FABRIC-034, FABRIC-080).
//!
//! # Fetches within its credit
//!
//! FABRIC-080 requires credit/window flow control on every session.
//! [`ConsumerConfig::fetch_credit_bytes`] is that window here: every
//! [`super::protocol::FetchRequest`] this type builds carries exactly that
//! `max_bytes` ceiling, fixed at construction, never a caller-chosen value
//! per call and never unbounded. A broker that has more to say than the
//! credit allows answers less than it could, not more than was asked.
//!
//! # Resuming after a restart: N, then N+1
//!
//! FABRIC-016/RES-058: a consumer that committed offset `N` and was then
//! killed must resume at `N + 1`, not `N` — resuming at `N` would redeliver
//! the record the commit already said was processed. [`Consumer::resume`]
//! is the one place that arithmetic happens, with
//! [`u64::checked_add`] rather than a plain `+ 1`: a committed offset of
//! `u64::MAX` has no offset after it, and this refuses that case by name
//! instead of wrapping to zero and silently rewinding to the start of the
//! partition.
//!
//! # Subscribing without polling on the caller's thread
//!
//! [`Consumer::subscribe`] is FABRIC-034: it spawns one background thread
//! that calls [`Consumer::fetch`] in a loop and pushes what it finds into a
//! bounded channel, so the caller's own thread only ever blocks in
//! [`Subscription::recv`] — never in a fetch, never in a retry's backoff.
//! The channel is bounded because an unbounded one would let a slow caller
//! turn "the caller is behind" into "this process holds every record the
//! broker has ever produced since the last read"; a full channel instead
//! backs the *fetch thread* up against its own credit, which is exactly
//! FABRIC-080's flow control applied to this SDK's own internal boundary,
//! not only the wire.
//!
//! Dropping a [`Subscription`] drops its receiving end, which is what wakes a
//! fetch thread blocked trying to push into a full channel (a bounded
//! channel's sender only ever unblocks on room or on disconnection) — the
//! thread's next send then fails and it exits. This type does not join that
//! thread: doing so would make dropping a [`Subscription`] block the caller
//! for as long as the fetch thread's own retry ladder might still be
//! spending, which defeats the reason `subscribe` exists in the first place.
//!
//! # One batch decoded per fetch, honestly
//!
//! A [`super::protocol::FetchResponse`] may in principle carry more than one
//! encoded batch concatenated (see its own documentation). Splitting that
//! requires knowing how many bytes one encoded batch consumed, and
//! `qip_events::event_fabric::codec` does not expose that to a caller — only
//! [`qip_events::event_fabric::codec::Batch::decode`] itself walks a frame's
//! length. Rather than duplicate that framing logic here, against the
//! architecture's own rule that a batch is decoded in exactly one place,
//! [`Consumer::fetch`] decodes at most the first batch in the response and
//! advances past exactly the records it contained. A broker answering with
//! more than one batch per fetch is not yet fully consumed by this client;
//! this is a known limitation, not a silent truncation, because nothing here
//! claims to have consumed bytes it never decoded.

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc;

use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_core::hash::from_hex;
use qip_core::rng::Xoshiro256;
use qip_core::time::Clock;
use qip_events::event_fabric::codec::{Batch, DecodeOutcome};

use crate::breaker::{BreakerPolicy, BreakerState, CircuitBreaker};
use crate::retry::{RetryPolicy, Sleeper};

use super::producer::{call_with_resilience, describe_refusal, wrong_route};
use super::protocol::{
    FetchRequest, GroupCommitRequest, GroupJoinRequest, GroupLagRequest, Request, Response, Route,
};
use super::transport::{FabricTransport, Timeouts};

/// Everything [`Consumer::new`] needs. See [`super::producer::ProducerConfig`]
/// for why this is a struct rather than positional arguments.
pub struct ConsumerConfig {
    pub transport: Box<dyn FabricTransport + Send>,
    pub stream: String,
    pub partition: u32,
    pub group_id: String,
    pub retry_policy: RetryPolicy,
    pub breaker_policy: BreakerPolicy,
    pub clock: Arc<dyn Clock>,
    pub sleeper: Arc<dyn Sleeper>,
    pub retry_seed: u64,
    pub breaker_seed: u64,
    pub timeouts: Timeouts,
    /// FABRIC-080's credit window for this session: the `max_bytes` every
    /// fetch carries. Refused at zero — see [`Consumer::new`].
    pub fetch_credit_bytes: u32,
}

impl fmt::Debug for ConsumerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsumerConfig")
            .field("stream", &self.stream)
            .field("partition", &self.partition)
            .field("group_id", &self.group_id)
            .field("retry_policy", &self.retry_policy)
            .field("breaker_policy", &self.breaker_policy)
            .field("retry_seed", &self.retry_seed)
            .field("breaker_seed", &self.breaker_seed)
            .field("timeouts", &self.timeouts)
            .field("fetch_credit_bytes", &self.fetch_credit_bytes)
            .finish_non_exhaustive()
    }
}

/// A typed consumer for one `(stream, partition)` inside one group. See the
/// module documentation for the credit window, resuming and subscribing.
pub struct Consumer {
    transport: Box<dyn FabricTransport + Send>,
    stream: String,
    partition: u32,
    group_id: String,
    member_id: Option<String>,
    generation: Option<u64>,
    retry_policy: RetryPolicy,
    retry_rng: Xoshiro256,
    sleeper: Arc<dyn Sleeper>,
    breaker: CircuitBreaker,
    peer_key: String,
    timeouts: Timeouts,
    next_offset: u64,
    fetch_credit_bytes: u32,
}

impl fmt::Debug for Consumer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Consumer")
            .field("stream", &self.stream)
            .field("partition", &self.partition)
            .field("group_id", &self.group_id)
            .field("member_id", &self.member_id)
            .field("generation", &self.generation)
            .field("next_offset", &self.next_offset)
            .field("fetch_credit_bytes", &self.fetch_credit_bytes)
            .field("breaker_state", &self.breaker.state(&self.peer_key))
            .finish_non_exhaustive()
    }
}

/// One decoded batch from a [`Consumer::fetch`], with the watermark the
/// broker reported alongside it.
#[derive(Clone, Debug)]
pub struct FetchedBatch {
    pub batch: Batch,
    pub high_watermark: u64,
}

/// One message from a [`Subscription`]'s background fetch thread.
#[derive(Clone, Debug)]
pub enum SubscriptionEvent {
    /// A batch was fetched and this consumer's offset has already advanced
    /// past it.
    Delivered(FetchedBatch),
    /// The fetch thread hit an error it cannot retry past and has stopped.
    /// No further [`SubscriptionEvent`] follows this one.
    Failed(Error),
}

/// The receiving half of [`Consumer::subscribe`]. See the module
/// documentation for why dropping this does not join the fetch thread.
pub struct Subscription {
    receiver: mpsc::Receiver<SubscriptionEvent>,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription").finish_non_exhaustive()
    }
}

impl Subscription {
    /// Block for the next event, or `None` once the fetch thread has ended
    /// and every event it sent has already been received.
    pub fn recv(&self) -> Option<SubscriptionEvent> {
        self.receiver.recv().ok()
    }
}

impl Consumer {
    /// Refuses an empty stream or group id, and a zero fetch credit — a
    /// credit of zero would never admit a byte of any batch, which is a
    /// disabled consumer wearing a working one's shape (the same refusal
    /// [`crate::spool::DurableSpool::open`] gives a zero-capacity spool).
    pub fn new(config: ConsumerConfig) -> Result<Self> {
        if config.stream.trim().is_empty() {
            return Err(Error::invalid(
                "a consumer must name a non-empty stream to read from",
            ));
        }
        if config.group_id.trim().is_empty() {
            return Err(Error::invalid("a consumer must name a non-empty group id"));
        }
        if config.fetch_credit_bytes == 0 {
            return Err(Error::invalid(
                "a consumer with a zero-byte fetch credit can never fetch a byte of any batch; \
                 it is not a smaller credit, it is a disabled consumer",
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
            group_id: config.group_id,
            member_id: None,
            generation: None,
            retry_policy: config.retry_policy,
            retry_rng: Xoshiro256::seeded(config.retry_seed),
            sleeper: config.sleeper,
            breaker,
            peer_key,
            timeouts: config.timeouts,
            next_offset: 0,
            fetch_credit_bytes: config.fetch_credit_bytes,
        })
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn partition(&self) -> u32 {
        self.partition
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    pub fn member_id(&self) -> Option<&str> {
        self.member_id.as_deref()
    }

    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// The offset the next [`Self::fetch`] will ask for.
    pub fn next_offset(&self) -> u64 {
        self.next_offset
    }

    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.state(&self.peer_key)
    }

    /// Move this consumer's own idea of the next offset to fetch, without
    /// asking the broker anything. For a caller that already knows where to
    /// start (a fresh consumer reading from the beginning starts at the
    /// default of zero without ever calling this).
    pub fn seek(&mut self, offset: u64) {
        self.next_offset = offset;
    }

    /// Join this consumer's group, refusing an answer that does not assign
    /// this consumer's own partition — fetching a partition nothing assigned
    /// to this member is exactly the double-read two members of one group
    /// must never produce.
    pub fn join(&mut self) -> Result<()> {
        let request = Request::GroupJoin(GroupJoinRequest {
            group_id: self.group_id.clone(),
            stream: self.stream.clone(),
            member_id: self.member_id.clone(),
        });
        match self.call(request)? {
            Response::GroupJoin(joined) => {
                if !joined.assigned_partitions.contains(&self.partition) {
                    return Err(Error::denied(format!(
                        "group {} joined member {} at generation {} without assigning \
                         partition {} of {}: assigned {:?}",
                        self.group_id,
                        joined.member_id,
                        joined.generation,
                        self.partition,
                        self.stream,
                        joined.assigned_partitions
                    )));
                }
                self.member_id = Some(joined.member_id);
                self.generation = Some(joined.generation);
                Ok(())
            }
            Response::Refused(refusal) => Err(describe_refusal(Route::GroupJoin, refusal)),
            other => Err(wrong_route(Route::GroupJoin, &other)),
        }
    }

    /// Ask the broker where this group last committed on this partition, and
    /// resume fetching one past it (FABRIC-016). See the module
    /// documentation for why the arithmetic is checked.
    pub fn resume(&mut self) -> Result<u64> {
        let request = Request::GroupLag(GroupLagRequest {
            group_id: self.group_id.clone(),
            stream: self.stream.clone(),
            partition: self.partition,
        });
        match self.call(request)? {
            Response::GroupLag(lag) => {
                let resumed = lag.committed_offset().checked_add(1).ok_or_else(|| {
                    Error::invalid(format!(
                        "the committed offset {} for {}:{} in group {} is already u64::MAX; \
                         there is no offset after it to resume at",
                        lag.committed_offset(),
                        self.stream,
                        self.partition,
                        self.group_id
                    ))
                })?;
                self.next_offset = resumed;
                Ok(resumed)
            }
            Response::Refused(refusal) => Err(describe_refusal(Route::GroupLag, refusal)),
            other => Err(wrong_route(Route::GroupLag, &other)),
        }
    }

    /// Record this member's processed offset. Requires [`Self::join`] to
    /// have already assigned a member id and generation — a commit naming
    /// neither is not a smaller commit, it is one the broker cannot
    /// attribute to a group membership at all.
    pub fn commit(&mut self, offset: u64) -> Result<u64> {
        let member_id = self.member_id.clone().ok_or_else(|| {
            Error::invalid(
                "commit() was called before join(): a commit needs the member id join() assigns",
            )
        })?;
        let generation = self.generation.ok_or_else(|| {
            Error::invalid(
                "commit() was called before join(): a commit needs the generation join() assigns",
            )
        })?;
        let request = Request::GroupCommit(GroupCommitRequest {
            group_id: self.group_id.clone(),
            member_id,
            generation,
            stream: self.stream.clone(),
            partition: self.partition,
            offset,
        });
        match self.call(request)? {
            Response::GroupCommit(committed) => Ok(committed.committed_offset),
            Response::Refused(refusal) => Err(describe_refusal(Route::GroupCommit, refusal)),
            other => Err(wrong_route(Route::GroupCommit, &other)),
        }
    }

    /// Fetch within this consumer's fixed credit, at its own next offset.
    /// `Ok(None)` is a legitimate answer — the broker had nothing new — and
    /// leaves [`Self::next_offset`] unchanged. See the module documentation
    /// for why at most one batch is ever decoded from one response.
    pub fn fetch(&mut self) -> Result<Option<FetchedBatch>> {
        let request = Request::Fetch(FetchRequest {
            stream: self.stream.clone(),
            partition: self.partition,
            offset: self.next_offset,
            max_bytes: self.fetch_credit_bytes,
        });
        match self.call(request)? {
            Response::Fetch(fetched) => {
                let Some(batch) = decode_one_batch(fetched.batches())? else {
                    return Ok(None);
                };
                let records = u64::try_from(batch.records.len()).map_err(|_| {
                    Error::invalid("a fetched batch carries more records than a u64 can count")
                })?;
                self.next_offset = batch.base_offset.checked_add(records).ok_or_else(|| {
                    Error::invalid(format!(
                        "the next offset for {}:{} would overflow past base offset {}",
                        self.stream, self.partition, batch.base_offset
                    ))
                })?;
                Ok(Some(FetchedBatch {
                    batch,
                    high_watermark: fetched.high_watermark(),
                }))
            }
            Response::Refused(refusal) => Err(describe_refusal(Route::Fetch, refusal)),
            other => Err(wrong_route(Route::Fetch, &other)),
        }
    }

    /// FABRIC-034: subscribe without polling on the caller's own thread.
    /// Consumes `self` — the fetch loop owns it from here, and the only way
    /// back in is through the returned [`Subscription`]'s channel. Refuses a
    /// zero `channel_bound` (a channel that admits nothing delivers nothing)
    /// rather than silently treating it as one.
    pub fn subscribe(mut self, channel_bound: usize, idle_poll: Duration) -> Result<Subscription> {
        if channel_bound == 0 {
            return Err(Error::invalid(
                "a subscription with a zero-capacity channel can never deliver a batch",
            ));
        }
        let (sender, receiver) = mpsc::sync_channel(channel_bound);
        std::thread::spawn(move || {
            loop {
                match self.fetch() {
                    Ok(Some(fetched)) => {
                        if sender.send(SubscriptionEvent::Delivered(fetched)).is_err() {
                            // The `Subscription` was dropped: the receiver is
                            // gone, and nothing will ever read another
                            // event. Stopping here is what turns a slow
                            // caller's drop into this thread actually
                            // exiting, not a thread nothing will ever join
                            // spinning forever against a broker nobody reads
                            // from any more.
                            return;
                        }
                    }
                    Ok(None) => self.sleeper.sleep(idle_poll),
                    Err(error) => {
                        let _ = sender.send(SubscriptionEvent::Failed(error));
                        return;
                    }
                }
            }
        });
        Ok(Subscription { receiver })
    }

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

/// Decode the first batch in a fetch response's hex-encoded `batches` field.
/// `None` for an empty answer (no new data); `Err` for hex that does not
/// decode or a frame the codec itself calls torn — a broker must only ever
/// answer with whole batches over this protocol, so a torn one here is the
/// broker's bug, not an ordinary end-of-log condition.
fn decode_one_batch(hex: &str) -> Result<Option<Batch>> {
    let bytes = from_hex(hex)
        .ok_or_else(|| Error::schema("a fetch response's batches field is not valid hex"))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    match Batch::decode(&bytes)? {
        DecodeOutcome::Complete(batch) => Ok(Some(batch)),
        DecodeOutcome::Torn => Err(Error::schema(
            "a fetch response carried a torn batch frame; a broker must only ever return whole \
             batches over the wire, never a partial one",
        )),
    }
}
