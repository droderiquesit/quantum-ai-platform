//! The event-fabric wire protocol: one typed request and response per route.
//!
//! ADR 0100 §1 places the client SDK and protocol here, beside the HTTP/1.1
//! client this crate already owns. This module is the protocol half: the
//! closed set of routes under `/v1/event-fabric`, a `Request`/`Response` pair
//! per route, and the twelve refusal codes a broker can answer with instead of
//! a success. [`super::transport`] is the seam that carries these types over a
//! wire; nothing here opens a socket.
//!
//! # Routes
//!
//! Nine routes, each with its own request and response payload:
//! `metadata`, `producers/init`, `produce`, `fetch`, `groups/join`,
//! `groups/commit`, `groups/lag`, `admin/isolate`, `admin/release`. Health and
//! metrics are deliberately absent — they are served on the separate health
//! listener `qip-edge-node` and the two brains already use, not on this
//! protocol, and adding them here would give an operator probe the same
//! refusal machinery a producer's own traffic goes through.
//!
//! # Refusals name what to do instead
//!
//! [`Refusal`] is the closed set of reasons a well-formed, well-routed request
//! can still be turned down. Every variant that can name a corrective fact
//! does: [`Refusal::OutOfOrderSequence`] names the sequence to resend from,
//! [`Refusal::Quota`] names how long to wait, [`Refusal::Isolated`] names who
//! isolated the partition and why. A refusal with no such fact — `Fenced`,
//! `SequenceConflict`, `SequenceBelowWindow`, `SchemaRefused`, `AclDenied`,
//! `KeyOutOfScope` — is one the name alone answers: there is nothing to retry
//! with, only a reason to stop.
//!
//! # Batch bodies pass through verbatim
//!
//! [`ProduceRequest::batch`] and [`FetchResponse::batches`] carry
//! [`qip_events::event_fabric::codec::Batch::encode`]'s own wire bytes,
//! hex-encoded so they fit inside a JSON string, and this module never
//! decodes them. ADR 0100 §1 fixes the batch codec as "one on-disk format,
//! used by the spool, the wire and broker segments" — a second place that
//! parses or rebuilds a batch's header would be a second definition of that
//! format, and the two would drift the first time either changed. This
//! protocol layer only checks that the hex is well-formed (an even number of
//! hex digits); the framing CRCs the codec itself carries are what actually
//! catch a corrupt or truncated batch, and they are checked when whatever
//! reads the decoded bytes calls [`qip_events::event_fabric::codec::Batch::decode`],
//! not here.
//!
//! # What this module does not validate
//!
//! Every request and response type below round-trips through JSON without
//! that JSON ever going unbounded — see [`Route::max_body_len`] — and the
//! handful of fields with a real cross-field invariant ([`Metadata`],
//! [`ProduceRequest`], [`ProduceAck`], [`FetchResponse`], [`GroupLagResponse`])
//! refuse a value that contradicts itself. Deliberately absent: a check that
//! `stream`, `group_id`, `member_id`, `producer_id` or `operator` is
//! non-empty. Those are exactly the checks
//! `qip_events::event_fabric::envelope::FabricEnvelope` already applies to the
//! identity fields it owns; the broker this protocol addresses
//! (`qip-streaming::event_fabric`, SLICE-27/30/35) is unbuilt, and adding
//! that discipline here, ahead of the ACL and catalogue lookups that are the
//! actual authority on whether a name is valid, would be a second opinion
//! this crate cannot keep in sync with the first. An empty name is refused
//! there, by failing to match any declared stream or grant, not here.

use serde::{Deserialize, Serialize};

use qip_core::error::{Error, Result};
use qip_core::hash::from_hex;
use qip_events::event_fabric::codec::MAX_BATCH_LEN;
use qip_events::event_fabric::policy::QosClass;

/// Which way a body travels, since a route's request and its response can
/// have very different size ceilings — see [`Route::max_body_len`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Request,
    Response,
}

/// The largest control-plane body accepted: metadata, producer, group and
/// admin exchanges are a handful of names and integers. An unbounded limit
/// here is an unbounded allocation on a body nothing has authenticated yet —
/// [`decode_request`] and [`decode_response`] check this before calling
/// `serde_json::from_slice`, not after.
pub const CONTROL_MAX_BODY: usize = 64 * 1024;

/// Slack added to [`MAX_BATCH_LEN`] for [`BATCH_MAX_BODY`]: room for the
/// surrounding JSON object and its field names, not for a second batch.
const BATCH_JSON_OVERHEAD: usize = 4 * 1024;

/// The largest body that can carry a batch. Hex encoding doubles every byte,
/// so a batch at [`MAX_BATCH_LEN`] costs twice that in JSON text before the
/// object wrapped around it is counted. Shared by [`Route::Produce`]'s
/// request and [`Route::Fetch`]'s response — the only two directions that
/// carry raw batch bytes rather than control-plane integers.
pub const BATCH_MAX_BODY: usize = 2 * MAX_BATCH_LEN + BATCH_JSON_OVERHEAD;

/// One route under `/v1/event-fabric`. See the module documentation for the
/// full list and for what is deliberately not a route here (health, metrics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Route {
    Metadata,
    ProducerInit,
    Produce,
    Fetch,
    GroupJoin,
    GroupCommit,
    GroupLag,
    AdminIsolate,
    AdminRelease,
}

impl Route {
    /// Every route, in the module documentation's own order. Used so a test
    /// (or a future router) can walk the whole surface rather than a list
    /// that has to be kept in step with the enum by hand.
    pub const ALL: [Route; 9] = [
        Route::Metadata,
        Route::ProducerInit,
        Route::Produce,
        Route::Fetch,
        Route::GroupJoin,
        Route::GroupCommit,
        Route::GroupLag,
        Route::AdminIsolate,
        Route::AdminRelease,
    ];

    /// The path this route is served at, under `/v1/event-fabric`.
    ///
    /// Every route is `POST`: this is a request/response control protocol
    /// with a body on every call, not a resource CRUD API, and a mixture of
    /// `GET`/`POST` would buy nothing here while costing a second table to
    /// keep in step with this one.
    pub const fn path(&self) -> &'static str {
        match self {
            Route::Metadata => "/v1/event-fabric/metadata",
            Route::ProducerInit => "/v1/event-fabric/producers/init",
            Route::Produce => "/v1/event-fabric/produce",
            Route::Fetch => "/v1/event-fabric/fetch",
            Route::GroupJoin => "/v1/event-fabric/groups/join",
            Route::GroupCommit => "/v1/event-fabric/groups/commit",
            Route::GroupLag => "/v1/event-fabric/groups/lag",
            Route::AdminIsolate => "/v1/event-fabric/admin/isolate",
            Route::AdminRelease => "/v1/event-fabric/admin/release",
        }
    }

    /// The route served at `path`, or `None` for anything else — a 404, not a
    /// guess at which route a caller meant.
    pub fn from_path(path: &str) -> Option<Route> {
        Route::ALL.into_iter().find(|route| route.path() == path)
    }

    /// The largest body this route accepts in `direction`, checked before any
    /// parsing happens. See [`BATCH_MAX_BODY`] and [`CONTROL_MAX_BODY`] for
    /// why produce's request and fetch's response are the two exceptions.
    pub const fn max_body_len(&self, direction: Direction) -> usize {
        match (self, direction) {
            (Route::Produce, Direction::Request) => BATCH_MAX_BODY,
            (Route::Fetch, Direction::Response) => BATCH_MAX_BODY,
            _ => CONTROL_MAX_BODY,
        }
    }
}

/// Refuse `value` unless it is an even-length hex string [`from_hex`] can
/// decode, naming `field`. Never echoes `value`: on a corrupt or truncated
/// batch this is the caller's own multi-megabyte payload, and a message that
/// quoted it back would be unreadable rather than useful.
fn require_hex(value: String, field: &str) -> Result<String> {
    if from_hex(&value).is_none() {
        return Err(Error::invalid(format!(
            "{field} is not an even-length hexadecimal string"
        )));
    }
    Ok(value)
}

/// Refuse an `archived_through` ahead of its `high_watermark`.
///
/// ADR 0100 §3 defines the watermark as the last **fsynced** offset and
/// archival as something that only happens to a batch already durable — a
/// response claiming `archived_through > high_watermark` is not a fact about
/// the partition, it is a peer contradicting its own definitions, and is
/// refused rather than trusted.
fn require_archive_within_watermark(archived_through: u64, high_watermark: u64) -> Result<()> {
    if archived_through > high_watermark {
        return Err(Error::invalid(format!(
            "archived_through ({archived_through}) is ahead of the high watermark \
             ({high_watermark}), which ADR 0100 §3 makes impossible: nothing is archived \
             before it is fsynced"
        )));
    }
    Ok(())
}

// --- metadata -----------------------------------------------------------

/// `metadata` request: describe one stream's partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataRequest {
    pub stream: String,
    pub partition: u32,
}

/// `metadata` response. Carries `archived_through` and `high_watermark`
/// alongside the leader epoch — ADR 0100 §3's producer-retained durability
/// depends on every metadata answer reporting the same release signal a
/// produce acknowledgement does ([`ProduceAck`]), so a producer's spool can
/// release what has been archived without waiting on its own next produce
/// call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "MetadataWire", try_from = "MetadataWire")]
pub struct Metadata {
    stream: String,
    partition: u32,
    leader_epoch: u64,
    high_watermark: u64,
    archived_through: u64,
}

impl Metadata {
    pub fn new(
        stream: impl Into<String>,
        partition: u32,
        leader_epoch: u64,
        high_watermark: u64,
        archived_through: u64,
    ) -> Result<Self> {
        require_archive_within_watermark(archived_through, high_watermark)?;
        Ok(Self {
            stream: stream.into(),
            partition,
            leader_epoch,
            high_watermark,
            archived_through,
        })
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn partition(&self) -> u32 {
        self.partition
    }
    pub fn leader_epoch(&self) -> u64 {
        self.leader_epoch
    }
    pub fn high_watermark(&self) -> u64 {
        self.high_watermark
    }
    pub fn archived_through(&self) -> u64 {
        self.archived_through
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MetadataWire {
    stream: String,
    partition: u32,
    leader_epoch: u64,
    high_watermark: u64,
    archived_through: u64,
}

impl From<Metadata> for MetadataWire {
    fn from(value: Metadata) -> Self {
        Self {
            stream: value.stream,
            partition: value.partition,
            leader_epoch: value.leader_epoch,
            high_watermark: value.high_watermark,
            archived_through: value.archived_through,
        }
    }
}

impl TryFrom<MetadataWire> for Metadata {
    type Error = Error;

    fn try_from(wire: MetadataWire) -> Result<Self> {
        Metadata::new(
            wire.stream,
            wire.partition,
            wire.leader_epoch,
            wire.high_watermark,
            wire.archived_through,
        )
    }
}

// --- producers/init -------------------------------------------------------

/// `producers/init` request: establish (or re-establish, fencing the old one)
/// a producer's epoch on one partition, per ADR 0100 §4.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerInitRequest {
    pub stream: String,
    pub partition: u32,
    pub producer_id: String,
}

/// `producers/init` response: the epoch now assigned, which fences any
/// earlier one for the same `producer_id` on the same partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerInitResponse {
    pub producer_epoch: u64,
}

// --- produce ---------------------------------------------------------------

/// `produce` request: a batch for one partition, already stamped by the
/// drain (producer id, epoch and base sequence — see
/// [`qip_events::event_fabric::codec::stamp_drain`]) and carried whole. This
/// route names only where the batch goes; everything about what is in it is
/// inside the batch's own header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "ProduceRequestWire", try_from = "ProduceRequestWire")]
pub struct ProduceRequest {
    stream: String,
    partition: u32,
    batch: String,
}

impl ProduceRequest {
    /// `batch` is `qip_events::event_fabric::codec::Batch::encode`'s bytes,
    /// hex-encoded. Refused if empty (a produce call with nothing in it) or
    /// not well-formed hex; never decoded as a batch here — see the module
    /// documentation's "batch bodies pass through verbatim".
    pub fn new(stream: impl Into<String>, partition: u32, batch: String) -> Result<Self> {
        if batch.is_empty() {
            return Err(Error::invalid(
                "a produce request must carry a non-empty batch",
            ));
        }
        let batch = require_hex(batch, "batch")?;
        Ok(Self {
            stream: stream.into(),
            partition,
            batch,
        })
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn partition(&self) -> u32 {
        self.partition
    }
    pub fn batch(&self) -> &str {
        &self.batch
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProduceRequestWire {
    stream: String,
    partition: u32,
    batch: String,
}

impl From<ProduceRequest> for ProduceRequestWire {
    fn from(value: ProduceRequest) -> Self {
        Self {
            stream: value.stream,
            partition: value.partition,
            batch: value.batch,
        }
    }
}

impl TryFrom<ProduceRequestWire> for ProduceRequest {
    type Error = Error;

    fn try_from(wire: ProduceRequestWire) -> Result<Self> {
        ProduceRequest::new(wire.stream, wire.partition, wire.batch)
    }
}

/// `produce` response: the offset assigned to the batch, and the same
/// release signal [`Metadata`] carries. See the module documentation for why
/// `archived_through` is mandatory here rather than a later, separate poll.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "ProduceAckWire", try_from = "ProduceAckWire")]
pub struct ProduceAck {
    stream: String,
    partition: u32,
    base_offset: u64,
    high_watermark: u64,
    archived_through: u64,
}

impl ProduceAck {
    pub fn new(
        stream: impl Into<String>,
        partition: u32,
        base_offset: u64,
        high_watermark: u64,
        archived_through: u64,
    ) -> Result<Self> {
        require_archive_within_watermark(archived_through, high_watermark)?;
        Ok(Self {
            stream: stream.into(),
            partition,
            base_offset,
            high_watermark,
            archived_through,
        })
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn partition(&self) -> u32 {
        self.partition
    }
    pub fn base_offset(&self) -> u64 {
        self.base_offset
    }
    pub fn high_watermark(&self) -> u64 {
        self.high_watermark
    }
    pub fn archived_through(&self) -> u64 {
        self.archived_through
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProduceAckWire {
    stream: String,
    partition: u32,
    base_offset: u64,
    high_watermark: u64,
    archived_through: u64,
}

impl From<ProduceAck> for ProduceAckWire {
    fn from(value: ProduceAck) -> Self {
        Self {
            stream: value.stream,
            partition: value.partition,
            base_offset: value.base_offset,
            high_watermark: value.high_watermark,
            archived_through: value.archived_through,
        }
    }
}

impl TryFrom<ProduceAckWire> for ProduceAck {
    type Error = Error;

    fn try_from(wire: ProduceAckWire) -> Result<Self> {
        ProduceAck::new(
            wire.stream,
            wire.partition,
            wire.base_offset,
            wire.high_watermark,
            wire.archived_through,
        )
    }
}

// --- fetch -------------------------------------------------------------

/// `fetch` request: read from one partition starting at `offset`, bounded by
/// `max_bytes` — the caller's own ceiling, checked against
/// [`Route::max_body_len`] on the answer, not on this request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchRequest {
    pub stream: String,
    pub partition: u32,
    pub offset: u64,
    pub max_bytes: u32,
}

/// `fetch` response: zero or more batches starting at the request's offset,
/// carried whole (see the module documentation), plus the same release
/// signal every other answer carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "FetchResponseWire", try_from = "FetchResponseWire")]
pub struct FetchResponse {
    stream: String,
    partition: u32,
    high_watermark: u64,
    archived_through: u64,
    batches: String,
}

impl FetchResponse {
    /// `batches` is zero or more `Batch::encode`d frames concatenated —
    /// empty is a legitimate answer (the caller is already at the high
    /// watermark) — hex-encoded, never decoded here.
    pub fn new(
        stream: impl Into<String>,
        partition: u32,
        high_watermark: u64,
        archived_through: u64,
        batches: String,
    ) -> Result<Self> {
        require_archive_within_watermark(archived_through, high_watermark)?;
        let batches = require_hex(batches, "batches")?;
        Ok(Self {
            stream: stream.into(),
            partition,
            high_watermark,
            archived_through,
            batches,
        })
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn partition(&self) -> u32 {
        self.partition
    }
    pub fn high_watermark(&self) -> u64 {
        self.high_watermark
    }
    pub fn archived_through(&self) -> u64 {
        self.archived_through
    }
    pub fn batches(&self) -> &str {
        &self.batches
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FetchResponseWire {
    stream: String,
    partition: u32,
    high_watermark: u64,
    archived_through: u64,
    batches: String,
}

impl From<FetchResponse> for FetchResponseWire {
    fn from(value: FetchResponse) -> Self {
        Self {
            stream: value.stream,
            partition: value.partition,
            high_watermark: value.high_watermark,
            archived_through: value.archived_through,
            batches: value.batches,
        }
    }
}

impl TryFrom<FetchResponseWire> for FetchResponse {
    type Error = Error;

    fn try_from(wire: FetchResponseWire) -> Result<Self> {
        FetchResponse::new(
            wire.stream,
            wire.partition,
            wire.high_watermark,
            wire.archived_through,
            wire.batches,
        )
    }
}

// --- groups/join -----------------------------------------------------------

/// `groups/join` request. `member_id` is `None` on a first join; the broker
/// assigns one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupJoinRequest {
    pub group_id: String,
    pub stream: String,
    pub member_id: Option<String>,
}

/// `groups/join` response: the member's id, the generation it joined, and the
/// partitions it now owns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupJoinResponse {
    pub member_id: String,
    pub generation: u64,
    pub assigned_partitions: Vec<u32>,
}

// --- groups/commit -----------------------------------------------------

/// `groups/commit` request: record a member's processed offset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCommitRequest {
    pub group_id: String,
    pub member_id: String,
    pub generation: u64,
    pub stream: String,
    pub partition: u32,
    pub offset: u64,
}

/// `groups/commit` response: the offset now committed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCommitResponse {
    pub committed_offset: u64,
}

// --- groups/lag ------------------------------------------------------------

/// `groups/lag` request: how far behind is this group on one partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupLagRequest {
    pub group_id: String,
    pub stream: String,
    pub partition: u32,
}

/// `groups/lag` response. `lag` is never carried on the wire: it is
/// `high_watermark - committed_offset`, computed with checked arithmetic on
/// decode rather than sent as a third number a peer could make disagree with
/// the first two. A `committed_offset` ahead of `high_watermark` — a group
/// that committed past what the partition holds — is refused rather than
/// wrapped into a very large lag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "GroupLagResponseWire", try_from = "GroupLagResponseWire")]
pub struct GroupLagResponse {
    committed_offset: u64,
    high_watermark: u64,
    lag: u64,
}

impl GroupLagResponse {
    pub fn new(committed_offset: u64, high_watermark: u64) -> Result<Self> {
        let lag = high_watermark
            .checked_sub(committed_offset)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "committed offset {committed_offset} is ahead of the high watermark \
                 {high_watermark}: a group cannot have committed past what the partition holds"
                ))
            })?;
        Ok(Self {
            committed_offset,
            high_watermark,
            lag,
        })
    }

    pub fn committed_offset(&self) -> u64 {
        self.committed_offset
    }
    pub fn high_watermark(&self) -> u64 {
        self.high_watermark
    }
    pub fn lag(&self) -> u64 {
        self.lag
    }
}

/// The wire form carries only the two independent facts; `lag` is derived on
/// every decode rather than trusted from a third field, so there is exactly
/// one place a lag value is computed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct GroupLagResponseWire {
    committed_offset: u64,
    high_watermark: u64,
}

impl From<GroupLagResponse> for GroupLagResponseWire {
    fn from(value: GroupLagResponse) -> Self {
        Self {
            committed_offset: value.committed_offset,
            high_watermark: value.high_watermark,
        }
    }
}

impl TryFrom<GroupLagResponseWire> for GroupLagResponse {
    type Error = Error;

    fn try_from(wire: GroupLagResponseWire) -> Result<Self> {
        GroupLagResponse::new(wire.committed_offset, wire.high_watermark)
    }
}

// --- admin/isolate -------------------------------------------------------

/// `admin/isolate` request: an operator parking a partition, naming why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminIsolateRequest {
    pub stream: String,
    pub partition: u32,
    pub operator: String,
    pub reason: String,
}

/// `admin/isolate` response: the offset the partition was isolated at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminIsolateResponse {
    pub isolated_at_offset: u64,
}

// --- admin/release -------------------------------------------------------

/// `admin/release` request: an operator releasing a previously isolated
/// partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminReleaseRequest {
    pub stream: String,
    pub partition: u32,
    pub operator: String,
}

/// `admin/release` response: the offset the partition resumed accepting
/// writes at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminReleaseResponse {
    pub released_at_offset: u64,
}

// --- refusals ----------------------------------------------------------

/// The closed set of reasons a well-formed, correctly-routed request can
/// still be turned down. See the module documentation for which variants name
/// a corrective fact and which do not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum Refusal {
    /// A newer producer epoch has taken over this partition; this one is
    /// dead and must not retry.
    Fenced,
    /// The batch's `base_sequence` is ahead of what this producer epoch has
    /// written so far. `expected` is the next sequence the broker will
    /// accept — the corrective fact a producer resends from.
    OutOfOrderSequence { expected: u64 },
    /// The batch's `base_sequence` matches one already accepted, but its
    /// payload hash does not: not a retry of the same batch, a collision.
    SequenceConflict,
    /// The batch's `base_sequence` is behind the deduplication window (ADR
    /// 0100 §4); too old to tell a retry from a replay.
    SequenceBelowWindow,
    /// The batch's declared schema does not satisfy
    /// `qip_events::event_fabric::schema_id::check_compatible` against the
    /// stream's registered schema.
    SchemaRefused,
    /// The request named an [`QosClass::ack_floor`] weaker than the stream's
    /// own class requires.
    AckTooWeak { class: QosClass },
    /// The producer's quota is spent. `retry_after_ms` names how long to
    /// wait before trying again.
    Quota { retry_after_ms: u64 },
    /// The class's overload policy allows shedding, and this record was
    /// shed under load.
    Shed { class: QosClass },
    /// The partition is isolated by an operator. `operator` and `reason`
    /// name who and why, from the [`AdminIsolateRequest`] that isolated it.
    Isolated { operator: String, reason: String },
    /// The class's disk budget is exhausted; new writes to it are refused
    /// until the archive catches up.
    DiskBudget { class: QosClass },
    /// The presented identity holds no grant on this stream at all.
    AclDenied,
    /// The presented identity holds a grant on this stream, but scoped to a
    /// different partition key than the one this request names.
    KeyOutOfScope,
}

// --- request/response envelopes -----------------------------------------

/// One typed request, tagged by [`Route`] on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "route", content = "payload", rename_all = "snake_case")]
pub enum Request {
    Metadata(MetadataRequest),
    ProducerInit(ProducerInitRequest),
    Produce(ProduceRequest),
    Fetch(FetchRequest),
    GroupJoin(GroupJoinRequest),
    GroupCommit(GroupCommitRequest),
    GroupLag(GroupLagRequest),
    AdminIsolate(AdminIsolateRequest),
    AdminRelease(AdminReleaseRequest),
}

impl Request {
    /// The route this request belongs on. Used to check a decoded body's own
    /// claim against the path it arrived on — see [`decode_request`].
    pub const fn route(&self) -> Route {
        match self {
            Request::Metadata(_) => Route::Metadata,
            Request::ProducerInit(_) => Route::ProducerInit,
            Request::Produce(_) => Route::Produce,
            Request::Fetch(_) => Route::Fetch,
            Request::GroupJoin(_) => Route::GroupJoin,
            Request::GroupCommit(_) => Route::GroupCommit,
            Request::GroupLag(_) => Route::GroupLag,
            Request::AdminIsolate(_) => Route::AdminIsolate,
            Request::AdminRelease(_) => Route::AdminRelease,
        }
    }
}

/// One typed response: a route's success payload, or [`Refusal`], which
/// every route can answer with (`route: "refused"` on the wire).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "route", content = "payload", rename_all = "snake_case")]
pub enum Response {
    Metadata(Metadata),
    ProducerInit(ProducerInitResponse),
    Produce(ProduceAck),
    Fetch(FetchResponse),
    GroupJoin(GroupJoinResponse),
    GroupCommit(GroupCommitResponse),
    GroupLag(GroupLagResponse),
    AdminIsolate(AdminIsolateResponse),
    AdminRelease(AdminReleaseResponse),
    Refused(Refusal),
}

impl Response {
    /// The route this response answers, or `None` for [`Response::Refused`]:
    /// a refusal is a legitimate answer on any route, so it names none of
    /// its own.
    pub const fn route(&self) -> Option<Route> {
        match self {
            Response::Metadata(_) => Some(Route::Metadata),
            Response::ProducerInit(_) => Some(Route::ProducerInit),
            Response::Produce(_) => Some(Route::Produce),
            Response::Fetch(_) => Some(Route::Fetch),
            Response::GroupJoin(_) => Some(Route::GroupJoin),
            Response::GroupCommit(_) => Some(Route::GroupCommit),
            Response::GroupLag(_) => Some(Route::GroupLag),
            Response::AdminIsolate(_) => Some(Route::AdminIsolate),
            Response::AdminRelease(_) => Some(Route::AdminRelease),
            Response::Refused(_) => None,
        }
    }
}

/// Serialise `request` to the JSON bytes [`decode_request`] reads back.
pub fn encode_request(request: &Request) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(request)?)
}

/// Decode a request body received on `route`.
///
/// The body's length is checked against [`Route::max_body_len`] **before**
/// `serde_json::from_slice` is called — a request declaring a route that
/// disagrees with the path it arrived on, or a body over that route's limit,
/// must never reach the parser: parsing an oversized document is itself the
/// allocation this check exists to refuse before it happens.
pub fn decode_request(route: Route, body: &[u8]) -> Result<Request> {
    check_body_len(route, Direction::Request, body.len())?;
    let request: Request = serde_json::from_slice(body)?;
    if request.route() != route {
        return Err(Error::invalid(format!(
            "a request to the {route:?} path named the {:?} route in its body",
            request.route()
        )));
    }
    Ok(request)
}

/// Serialise `response` to the JSON bytes [`decode_response`] reads back.
pub fn encode_response(response: &Response) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(response)?)
}

/// Decode a response body received for a call made on `route`. See
/// [`decode_request`] for why the size check runs first.
///
/// A [`Response::Refused`] answers any route and is accepted unconditionally;
/// any other variant must name `route` itself, or it is refused rather than
/// handed to a caller expecting one route's shape and holding another's.
pub fn decode_response(route: Route, body: &[u8]) -> Result<Response> {
    check_body_len(route, Direction::Response, body.len())?;
    let response: Response = serde_json::from_slice(body)?;
    match response.route() {
        None => Ok(response),
        Some(actual) if actual == route => Ok(response),
        Some(actual) => Err(Error::invalid(format!(
            "a response to a {route:?} call named the {actual:?} route instead"
        ))),
    }
}

fn check_body_len(route: Route, direction: Direction, len: usize) -> Result<()> {
    let limit = route.max_body_len(direction);
    if len > limit {
        return Err(Error::invalid(format!(
            "a {route:?} {direction:?} body of {len} bytes exceeds this route's \
             {limit}-byte limit"
        )));
    }
    Ok(())
}
