//! Stream grants and the broker's protocol handler. See ADR 0100 §1 and §7.
//!
//! [`Service`] is what turns the broker library into something a process
//! can serve: it takes the three things a request arrives with — an
//! `Authorization` header, a path and a body — and answers with a status and
//! a body, having authenticated the caller, authorised the request against
//! the catalogue's grants, and only then touched the [`Broker`].
//!
//! Until this module existed the client SDK and the broker had each been
//! proven alone, the SDK against scripted transports and the broker by
//! direct calls, and no code path joined them. Joining them is what showed
//! that they disagreed about five facts — what a producer's sequence counts,
//! whether it carries over a restart, what a consumer's offset counts,
//! whether `archived_through` includes the offset it names, and whether a
//! cell's identity can be written in an identities file — none of which
//! either side's own tests could see. `qip-fabricd`'s `tests/broker.rs`
//! lists them and holds each one.
//!
//! # The order is the point
//!
//! 1. **Route.** A path that is not one of the protocol's nine is a 404.
//! 2. **Authenticate, before the body is parsed.** The route's body limit is
//!    an allocation an unauthenticated caller could otherwise make us do.
//! 3. **Decode**, through the protocol's own bounded decoder.
//! 4. **Authorise** against the grants (FABRIC-018), per request, from a
//!    grant list that can be replaced while the process runs. A request the
//!    grants do not cover is answered with [`Refusal::AclDenied`] or
//!    [`Refusal::KeyOutOfScope`] and never reaches the broker.
//! 5. **Act**, and answer a refusal the broker names as that [`Refusal`].
//!
//! # Fabric metadata
//!
//! The stream declarations, their schemas and the grants all come from one
//! [`Catalogue`]: [`Service::apply_catalogue`] declares every stream on the
//! broker under its declared policy, registers each stream's batch schema,
//! and replaces the grant list. It is called at start-up and may be called
//! again with a newer catalogue; a catalogue that would change an existing
//! stream's policy is refused by the broker and the grants in force stay in
//! force.
//!
//! # What is not here
//!
//! No consumer-group membership: `groups/join` assigns a caller every
//! partition its consume grant covers and holds no lease, so two members of
//! one group on one partition are not kept apart (FABRIC-033). No transport
//! security: the bearer token travels over plaintext TCP, and
//! `qip_transport::event_fabric::auth` says what that does and does not buy.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use qip_core::error::{Error, Result};
use qip_events::event_fabric::catalogue::{Catalogue, Grant, Permission};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, MAX_BATCH_LEN};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass, StreamPolicy};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_transport::event_fabric::auth::{self, IdentityTable};
use qip_transport::event_fabric::protocol::{
    AdminIsolateResponse, AdminReleaseResponse, GroupCommitResponse, GroupJoinResponse,
    GroupLagResponse, ProducerInitResponse, Refusal, Request, Response, Route, decode_request,
    encode_response,
};

use super::acl::{self, Scope};
use super::broker::Broker;

/// What the handler did with one request, for whoever records metrics.
///
/// The handler lives in a service crate and the recorder in the binary that
/// serves it, so the facts cross that seam as data. Every label here is
/// bounded: a route from the protocol's closed set, a stream from the
/// catalogue, a class from an enum, an outcome and a reason from the
/// literals in this file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub route: Option<Route>,
    pub stream: Option<String>,
    pub class: Option<QosClass>,
    pub partition: Option<u32>,
    /// `success`, `refused`, `error`, `unauthenticated` or `unknown_route`.
    pub outcome: &'static str,
    /// For a refusal, which one; for an error, its class.
    pub reason: Option<&'static str>,
    pub high_watermark: Option<u64>,
    pub archived_through: Option<u64>,
    /// For a group route, the group and how far it trails the watermark.
    pub group: Option<String>,
    pub lag: Option<u64>,
}

/// One answered request: an HTTP status, a body, and what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handled {
    pub status: u16,
    pub body: Vec<u8>,
    pub observation: Observation,
}

/// The label a refusal is counted under. One literal per variant, so the
/// series' cardinality is the protocol's own closed set.
pub const fn refusal_reason(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::Fenced => "fenced",
        Refusal::OutOfOrderSequence { .. } => "out_of_order_sequence",
        Refusal::SequenceConflict => "sequence_conflict",
        Refusal::SequenceBelowWindow => "sequence_below_window",
        Refusal::SchemaRefused => "schema_refused",
        Refusal::AckTooWeak { .. } => "ack_too_weak",
        Refusal::Quota { .. } => "quota",
        Refusal::Shed { .. } => "shed",
        Refusal::Isolated { .. } => "isolated",
        Refusal::DiskBudget { .. } => "disk_budget",
        Refusal::AclDenied => "acl_denied",
        Refusal::KeyOutOfScope => "key_out_of_scope",
    }
}

/// The HTTP status an error that is not a protocol [`Refusal`] is answered
/// with. A refusal is a 200 carrying `Response::Refused`: the broker
/// answered. These are the cases where it could not.
const fn status_of(error: &Error) -> u16 {
    match error {
        Error::Invalid(_) | Error::Schema(_) | Error::Numeric(_) => 400,
        Error::Denied(_) => 403,
        Error::NotFound(_) => 404,
        Error::Guard(_) => 429,
        Error::Unavailable(_) | Error::Timeout(_) => 503,
        Error::Io(_) => 500,
    }
}

fn error_body(error: &Error) -> Vec<u8> {
    serde_json::json!({ "error": error.to_string() })
        .to_string()
        .into_bytes()
}

/// The shape a stream's batch schema is registered under: one field per
/// topic the catalogue admits to the stream. Removing a topic from a stream
/// without bumping the batch schema version is then the "field absent from
/// the new shape" case the broker's compatibility check already refuses,
/// and admitting a new topic is the additive case it allows.
fn stream_shape(topics: &[qip_events::topic::Topic]) -> Shape {
    Shape::Object(
        topics
            .iter()
            .map(|topic| (topic.name().to_string(), Shape::Number))
            .collect(),
    )
}

/// The event-fabric broker's protocol handler. See the module documentation.
pub struct Service {
    broker: Arc<Broker>,
    identities: IdentityTable,
    /// Replaced whole by [`Service::apply_catalogue`], never edited in
    /// place, so a request authorises against one consistent grant list.
    grants: RwLock<Arc<Vec<Grant>>>,
    classes: RwLock<BTreeMap<String, QosClass>>,
    partitions_per_stream: u32,
    segments: SegmentConfigFor,
}

/// How a stream's partitions open their segment logs, given the stream's
/// policy. Supplied by the composition root, which owns the clock and the
/// roll size; this crate reads no configuration.
pub type SegmentConfigFor = Arc<dyn Fn(&StreamPolicy) -> SegmentLogConfig + Send + Sync>;

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("broker", &self.broker)
            .field("identities", &self.identities)
            .field("partitions_per_stream", &self.partitions_per_stream)
            .finish_non_exhaustive()
    }
}

impl Service {
    /// Build a handler over `broker`, declaring `catalogue`'s streams on it.
    ///
    /// `partitions_per_stream` is how many partitions every stream is
    /// declared with. It is a parameter and has no default because the
    /// catalogue does not carry it, and a count chosen silently here would
    /// decide which partition every cell's key routes to.
    pub fn new(
        broker: Arc<Broker>,
        identities: IdentityTable,
        catalogue: &Catalogue,
        partitions_per_stream: u32,
        segments: SegmentConfigFor,
    ) -> Result<Self> {
        if partitions_per_stream == 0 {
            return Err(Error::invalid(
                "every stream needs at least one partition; state how many each is declared with",
            ));
        }
        let service = Self {
            broker,
            identities,
            grants: RwLock::new(Arc::new(Vec::new())),
            classes: RwLock::new(BTreeMap::new()),
            partitions_per_stream,
            segments,
        };
        service.apply_catalogue(catalogue)?;
        Ok(service)
    }

    /// The broker this handler serves.
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    /// Declare every stream in `catalogue`, register its batch schema, and
    /// replace the grants in force with the catalogue's (FABRIC-018: a grant
    /// added to the catalogue is honoured without a restart).
    ///
    /// The grants are replaced last and only if every declaration was
    /// accepted. A catalogue the broker refuses — one that changes a
    /// declared stream's policy, or removes a topic from a stream without a
    /// schema version bump — therefore changes nothing about who may do
    /// what.
    pub fn apply_catalogue(&self, catalogue: &Catalogue) -> Result<()> {
        let mut classes = BTreeMap::new();
        for (name, declaration) in catalogue.streams() {
            let policy = &declaration.policy;
            self.broker.declare_stream(
                name,
                self.partitions_per_stream,
                policy.clone(),
                (self.segments)(policy),
            )?;
            self.broker.register_schema(
                name,
                policy.qos_class().batch_schema_id(),
                BATCH_SCHEMA_VERSION,
                stream_shape(&declaration.topics),
            )?;
            classes.insert(name.clone(), policy.qos_class());
        }
        *self.classes.write().unwrap_or_else(|e| e.into_inner()) = classes;
        *self.grants.write().unwrap_or_else(|e| e.into_inner()) =
            Arc::new(catalogue.grants().to_vec());
        Ok(())
    }

    /// Answer one request. Never panics and never returns an error: every
    /// failure is a status and a body, because the caller is a listener that
    /// has to write something back.
    pub fn handle(&self, authorization: Option<&str>, path: &str, body: &[u8]) -> Handled {
        let mut seen = Observation::default();
        let Some(route) = Route::from_path(path) else {
            seen.outcome = "unknown_route";
            return Handled {
                status: 404,
                body: error_body(&Error::not_found(
                    "no event-fabric route is served at this path; the routes are under \
                     /v1/event-fabric",
                )),
                observation: seen,
            };
        };
        seen.route = Some(route);
        let identity = match auth::verify(&self.identities, authorization) {
            Ok(identity) => identity,
            Err(error) => {
                seen.outcome = "unauthenticated";
                return Handled {
                    status: 401,
                    body: error_body(&error),
                    observation: seen,
                };
            }
        };
        let answered = decode_request(route, body)
            .and_then(|request| self.answer(identity.as_str(), request, &mut seen))
            .and_then(|response| {
                if let Response::Refused(refusal) = &response {
                    seen.outcome = "refused";
                    seen.reason = Some(refusal_reason(refusal));
                } else {
                    seen.outcome = "success";
                }
                encode_response(&response)
            });
        match answered {
            Ok(body) => Handled {
                status: 200,
                body,
                observation: seen,
            },
            Err(error) => {
                seen.outcome = "error";
                seen.reason = Some(error.code());
                Handled {
                    status: status_of(&error),
                    body: error_body(&error),
                    observation: seen,
                }
            }
        }
    }

    /// Whether `identity` may exercise one of `permissions` on `partition`
    /// of `stream`: `None` if it may, the refusal if it may not.
    ///
    /// A stream the broker never declared answers `AclDenied`, the same as a
    /// declared stream the identity holds no grant on, so a caller without a
    /// grant cannot use this to learn which streams exist.
    fn refuse_unless_granted(
        &self,
        identity: &str,
        stream: &str,
        partition: u32,
        permissions: &[Permission],
        seen: &mut Observation,
    ) -> Result<Option<Refusal>> {
        seen.stream = Some(stream.to_string());
        seen.partition = Some(partition);
        Ok(self
            .scope(identity, stream, permissions, seen)?
            .admits(partition)
            .err())
    }

    fn scope(
        &self,
        identity: &str,
        stream: &str,
        permissions: &[Permission],
        seen: &mut Observation,
    ) -> Result<Scope> {
        seen.stream = Some(stream.to_string());
        seen.class = self
            .classes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(stream)
            .copied();
        let Ok(partition_count) = self.broker.partition_count(stream) else {
            return Ok(Scope::None);
        };
        let grants = self
            .grants
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        acl::scope(&grants, identity, stream, permissions, partition_count)
    }

    fn answer(&self, identity: &str, request: Request, seen: &mut Observation) -> Result<Response> {
        const ANY_GRANT: [Permission; 3] =
            [Permission::Produce, Permission::Consume, Permission::Admin];
        match request {
            Request::Metadata(r) => {
                if let Some(refusal) =
                    self.refuse_unless_granted(identity, &r.stream, r.partition, &ANY_GRANT, seen)?
                {
                    return Ok(Response::Refused(refusal));
                }
                let metadata = self.broker.metadata(&r.stream, r.partition)?;
                seen.high_watermark = Some(metadata.high_watermark());
                seen.archived_through = Some(metadata.archived_through());
                Ok(Response::Metadata(metadata))
            }
            Request::ProducerInit(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    &r.stream,
                    r.partition,
                    &[Permission::Produce],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                let producer_epoch =
                    self.broker
                        .init_producer(&r.stream, r.partition, &r.producer_id)?;
                let next_sequence =
                    self.broker
                        .next_sequence(&r.stream, r.partition, &r.producer_id)?;
                Ok(Response::ProducerInit(ProducerInitResponse {
                    producer_epoch,
                    next_sequence,
                }))
            }
            Request::Produce(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    r.stream(),
                    r.partition(),
                    &[Permission::Produce],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                let bytes = qip_core::hash::from_hex(r.batch()).ok_or_else(|| {
                    Error::invalid("the produce request's batch is not hexadecimal")
                })?;
                let batch = match Batch::decode(&bytes)? {
                    DecodeOutcome::Complete(batch) => batch,
                    DecodeOutcome::Torn => {
                        return Err(Error::invalid(
                            "the produce request's batch is truncated; resend the whole batch",
                        ));
                    }
                };
                match self.broker.produce_to(r.stream(), r.partition(), batch)? {
                    Ok(ack) => {
                        seen.high_watermark = Some(ack.high_watermark());
                        seen.archived_through = Some(ack.archived_through());
                        Ok(Response::Produce(ack))
                    }
                    Err(refused) => Ok(Response::Refused(refused.refusal)),
                }
            }
            Request::Fetch(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    &r.stream,
                    r.partition,
                    &[Permission::Consume],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                // The response's own body limit is two hex digits per byte of
                // one maximal batch; a caller asking for more is given that
                // much, not an answer its own decoder would refuse.
                let max_bytes = r.max_bytes.min(MAX_BATCH_LEN as u32);
                let fetched = self
                    .broker
                    .fetch(&r.stream, r.partition, r.offset, max_bytes)?;
                seen.high_watermark = Some(fetched.high_watermark());
                seen.archived_through = Some(fetched.archived_through());
                Ok(Response::Fetch(fetched))
            }
            Request::GroupJoin(r) => {
                let scope = self.scope(identity, &r.stream, &[Permission::Consume], seen)?;
                if scope == Scope::None {
                    return Ok(Response::Refused(Refusal::AclDenied));
                }
                seen.group = Some(r.group_id.clone());
                let partition_count = self.broker.partition_count(&r.stream)?;
                Ok(Response::GroupJoin(GroupJoinResponse {
                    member_id: r.member_id.unwrap_or_else(|| identity.to_string()),
                    // The broker's leader epoch: the one number here that
                    // changes when the broker a member joined is no longer
                    // the one it is talking to.
                    generation: self.broker.leader_epoch(),
                    assigned_partitions: scope.partitions(partition_count),
                }))
            }
            Request::GroupCommit(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    &r.stream,
                    r.partition,
                    &[Permission::Consume],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                seen.group = Some(r.group_id.clone());
                let committed_offset =
                    self.broker
                        .commit_offset(&r.group_id, &r.stream, r.partition, r.offset)?;
                let high_watermark = self
                    .broker
                    .metadata(&r.stream, r.partition)?
                    .high_watermark();
                seen.high_watermark = Some(high_watermark);
                seen.lag = Some(high_watermark.saturating_sub(committed_offset.saturating_add(1)));
                Ok(Response::GroupCommit(GroupCommitResponse {
                    committed_offset,
                }))
            }
            Request::GroupLag(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    &r.stream,
                    r.partition,
                    &[Permission::Consume, Permission::Admin],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                seen.group = Some(r.group_id.clone());
                let committed = self
                    .broker
                    .committed_offset(&r.group_id, &r.stream, r.partition)?
                    .ok_or_else(|| {
                        Error::not_found(format!(
                            "group '{}' has never committed on {}:{}; it has no checkpoint to \
                             resume from, so seek to the offset it should start at",
                            r.group_id, r.stream, r.partition
                        ))
                    })?;
                let high_watermark = self
                    .broker
                    .metadata(&r.stream, r.partition)?
                    .high_watermark();
                seen.high_watermark = Some(high_watermark);
                seen.lag = Some(high_watermark.saturating_sub(committed.saturating_add(1)));
                Ok(Response::GroupLag(GroupLagResponse::new(
                    committed,
                    high_watermark,
                )?))
            }
            Request::AdminIsolate(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    &r.stream,
                    r.partition,
                    &[Permission::Admin],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                refuse_a_borrowed_name(identity, &r.operator)?;
                let isolated_at_offset =
                    self.broker
                        .isolate(&r.stream, r.partition, identity, &r.reason)?;
                Ok(Response::AdminIsolate(AdminIsolateResponse {
                    isolated_at_offset,
                }))
            }
            Request::AdminRelease(r) => {
                if let Some(refusal) = self.refuse_unless_granted(
                    identity,
                    &r.stream,
                    r.partition,
                    &[Permission::Admin],
                    seen,
                )? {
                    return Ok(Response::Refused(refusal));
                }
                refuse_a_borrowed_name(identity, &r.operator)?;
                let released_at_offset = self.broker.release(&r.stream, r.partition, identity)?;
                Ok(Response::AdminRelease(AdminReleaseResponse {
                    released_at_offset,
                }))
            }
        }
    }
}

/// FABRIC-028 requires an isolation to be "an attributable operator action".
/// The request carries an `operator` field anyone could fill in; the
/// attribution that means something is the identity whose token was
/// verified. A request naming a different operator is refused rather than
/// recorded under either name.
fn refuse_a_borrowed_name(identity: &str, claimed: &str) -> Result<()> {
    if identity == claimed {
        return Ok(());
    }
    Err(Error::denied(format!(
        "this request names operator '{claimed}' but was authenticated as '{identity}'; an \
         operator action is recorded under the identity that presented the token, so name \
         that one"
    )))
}
