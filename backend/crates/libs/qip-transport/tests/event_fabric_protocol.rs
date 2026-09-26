//! `qip_transport::event_fabric::{protocol, transport}` — ADR 0100 §1.
//!
//! Proves three things a hand-rolled wire protocol can get wrong silently:
//! that every route's request and response actually survive a trip through
//! JSON (a struct that compiles is not a struct that round-trips), that a
//! body over a route's own size ceiling is refused before it is ever handed
//! to a parser, and that this crate's production transport
//! ([`HttpTransport`]) answers a real socket exactly as an in-process call to
//! the same handler would — the property the whole swap seam
//! ([`FabricTransport`]) depends on: a future QUIC-with-mTLS implementation
//! is only a safe swap if nothing downstream can tell it apart from this one
//! by the *shape* of what comes back.

use std::collections::BTreeMap;
use std::sync::Arc;

use qip_events::event_fabric::policy::QosClass;
use qip_transport::event_fabric::protocol::{
    AdminIsolateRequest, AdminIsolateResponse, AdminReleaseRequest, AdminReleaseResponse,
    Direction, FetchRequest, FetchResponse, GroupCommitRequest, GroupCommitResponse,
    GroupJoinRequest, GroupJoinResponse, GroupLagRequest, GroupLagResponse, Metadata,
    MetadataRequest, ProduceAck, ProduceRequest, ProducerInitRequest, ProducerInitResponse,
    Refusal, Request, Response, Route, decode_request, decode_response, encode_request,
    encode_response,
};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::server::{
    Handler, Method as ServerMethod, Request as ServerRequest, Response as ServerResponse, Server,
    ServerLimits,
};

/// One concrete, well-formed request per route — the same values every test
/// below builds from, so a mismatch between two tests' fixtures is never the
/// reason one of them fails.
fn sample_request(route: Route) -> Request {
    match route {
        Route::Metadata => Request::Metadata(MetadataRequest {
            stream: "orders".to_string(),
            partition: 3,
        }),
        Route::ProducerInit => Request::ProducerInit(ProducerInitRequest {
            stream: "orders".to_string(),
            partition: 3,
            producer_id: "cell-eu-1".to_string(),
        }),
        Route::Produce => Request::Produce(
            ProduceRequest::new("orders", 3, "deadbeef".to_string())
                .expect("a well-formed hex batch is accepted"),
        ),
        Route::Fetch => Request::Fetch(FetchRequest {
            stream: "orders".to_string(),
            partition: 3,
            offset: 40,
            max_bytes: 1024,
        }),
        Route::GroupJoin => Request::GroupJoin(GroupJoinRequest {
            group_id: "risk-desk".to_string(),
            stream: "orders".to_string(),
            member_id: None,
        }),
        Route::GroupCommit => Request::GroupCommit(GroupCommitRequest {
            group_id: "risk-desk".to_string(),
            member_id: "member-1".to_string(),
            generation: 2,
            stream: "orders".to_string(),
            partition: 3,
            offset: 41,
        }),
        Route::GroupLag => Request::GroupLag(GroupLagRequest {
            group_id: "risk-desk".to_string(),
            stream: "orders".to_string(),
            partition: 3,
        }),
        Route::AdminIsolate => Request::AdminIsolate(AdminIsolateRequest {
            stream: "orders".to_string(),
            partition: 3,
            operator: "on-call".to_string(),
            reason: "reconciliation break".to_string(),
        }),
        Route::AdminRelease => Request::AdminRelease(AdminReleaseRequest {
            stream: "orders".to_string(),
            partition: 3,
            operator: "on-call".to_string(),
        }),
    }
}

/// One concrete, coherent success answer per route, mirroring
/// [`sample_request`].
fn sample_success_response(route: Route) -> Response {
    match route {
        Route::Metadata => Response::Metadata(
            Metadata::new("orders", 3, 7, 120, 80).expect("a coherent metadata answer"),
        ),
        Route::ProducerInit => Response::ProducerInit(ProducerInitResponse { producer_epoch: 4 }),
        Route::Produce => Response::Produce(
            ProduceAck::new("orders", 3, 100, 120, 80).expect("a coherent produce acknowledgement"),
        ),
        Route::Fetch => Response::Fetch(
            FetchResponse::new("orders", 3, 120, 80, "deadbeef".to_string())
                .expect("a coherent fetch response"),
        ),
        Route::GroupJoin => Response::GroupJoin(GroupJoinResponse {
            member_id: "member-1".to_string(),
            generation: 2,
            assigned_partitions: vec![0, 1, 2],
        }),
        Route::GroupCommit => Response::GroupCommit(GroupCommitResponse {
            committed_offset: 41,
        }),
        Route::GroupLag => {
            Response::GroupLag(GroupLagResponse::new(41, 120).expect("a coherent lag answer"))
        }
        Route::AdminIsolate => Response::AdminIsolate(AdminIsolateResponse {
            isolated_at_offset: 100,
        }),
        Route::AdminRelease => Response::AdminRelease(AdminReleaseResponse {
            released_at_offset: 101,
        }),
    }
}

/// Every one of the twelve refusal codes, each carrying the corrective fact
/// it claims to.
fn every_refusal() -> Vec<Refusal> {
    vec![
        Refusal::Fenced,
        Refusal::OutOfOrderSequence { expected: 42 },
        Refusal::SequenceConflict,
        Refusal::SequenceBelowWindow,
        Refusal::SchemaRefused,
        Refusal::AckTooWeak {
            class: QosClass::P1Outcomes,
        },
        Refusal::Quota {
            retry_after_ms: 250,
        },
        Refusal::Shed {
            class: QosClass::P2MarketJournal,
        },
        Refusal::Isolated {
            operator: "on-call".to_string(),
            reason: "reconciliation break".to_string(),
        },
        Refusal::DiskBudget {
            class: QosClass::P3Research,
        },
        Refusal::AclDenied,
        Refusal::KeyOutOfScope,
    ]
}

/// Asserts its own premise first (nine routes, twelve refusals — the counts
/// this test's name promises to exercise), then that every route's request
/// and response, and every refusal code, survive `encode` and `decode`
/// unchanged.
///
/// Mutation: drop `expected` from `Refusal::OutOfOrderSequence` — fails,
/// because [`every_refusal`] can no longer construct that variant.
#[test]
fn every_request_and_response_round_trips_and_every_refusal_names_what_to_do_instead() {
    assert_eq!(Route::ALL.len(), 9, "premise: nine routes to cover");

    for route in Route::ALL {
        let request = sample_request(route);
        assert_eq!(
            request.route(),
            route,
            "premise: the sample request for {route:?} names its own route"
        );
        let bytes = encode_request(&request).expect("a sample request encodes to JSON");
        let decoded = decode_request(route, &bytes)
            .unwrap_or_else(|error| panic!("{route:?} request failed to decode: {error}"));
        assert_eq!(decoded, request, "{route:?} request did not round-trip");

        let response = sample_success_response(route);
        assert_eq!(
            response.route(),
            Some(route),
            "premise: the sample response for {route:?} names its own route"
        );
        let bytes = encode_response(&response).expect("a sample response encodes to JSON");
        let decoded = decode_response(route, &bytes)
            .unwrap_or_else(|error| panic!("{route:?} response failed to decode: {error}"));
        assert_eq!(decoded, response, "{route:?} response did not round-trip");
    }

    let refusals = every_refusal();
    assert_eq!(refusals.len(), 12, "premise: twelve refusal codes to cover");
    for refusal in refusals {
        // A refusal answers any route; Metadata's control-plane limit is
        // ample for all twelve, none of which carries a batch.
        let response = Response::Refused(refusal.clone());
        let bytes = encode_response(&response).expect("a refusal response encodes to JSON");
        let decoded = decode_response(Route::Metadata, &bytes)
            .unwrap_or_else(|error| panic!("{refusal:?} refusal failed to decode: {error}"));
        assert_eq!(decoded, response, "{refusal:?} refusal did not round-trip");
    }
}

/// Mutation: in `From<Metadata> for MetadataWire` and
/// `From<ProduceAck> for ProduceAckWire`, hard-code `archived_through: 0`
/// instead of copying the real value — fails, because the round trip below
/// then decodes a different value than it encoded. A test that only read the
/// in-memory `Metadata`/`ProduceAck` back through their own getters would not
/// catch this: the getters see the value before it is ever handed to serde.
#[test]
fn a_produce_acknowledgement_and_a_metadata_answer_both_carry_archived_through_and_the_high_watermark()
 {
    let ack =
        ProduceAck::new("orders", 3, 100, 120, 80).expect("a coherent produce acknowledgement");
    assert_eq!(
        ack.high_watermark(),
        120,
        "premise: the sample ack carries a watermark"
    );
    assert_eq!(
        ack.archived_through(),
        80,
        "premise: the sample ack carries an archive point"
    );

    let metadata = Metadata::new("orders", 3, 7, 120, 80).expect("a coherent metadata answer");
    assert_eq!(
        metadata.high_watermark(),
        120,
        "premise: the sample metadata answer carries a watermark"
    );
    assert_eq!(
        metadata.archived_through(),
        80,
        "premise: the sample metadata answer carries an archive point"
    );

    let ack_response = Response::Produce(ack);
    let ack_bytes = encode_response(&ack_response).expect("a produce ack encodes to JSON");
    let ack_decoded =
        decode_response(Route::Produce, &ack_bytes).expect("a produce ack decodes from JSON");
    let Response::Produce(decoded_ack) = ack_decoded else {
        panic!("decoding a produce response did not answer with a produce ack");
    };
    assert_eq!(decoded_ack.high_watermark(), 120);
    assert_eq!(
        decoded_ack.archived_through(),
        80,
        "the wire form of a produce ack must carry archived_through, not drop it"
    );

    let metadata_response = Response::Metadata(metadata);
    let metadata_bytes =
        encode_response(&metadata_response).expect("a metadata answer encodes to JSON");
    let metadata_decoded = decode_response(Route::Metadata, &metadata_bytes)
        .expect("a metadata answer decodes from JSON");
    let Response::Metadata(decoded_metadata) = metadata_decoded else {
        panic!("decoding a metadata response did not answer with metadata");
    };
    assert_eq!(decoded_metadata.high_watermark(), 120);
    assert_eq!(
        decoded_metadata.archived_through(),
        80,
        "the wire form of a metadata answer must carry archived_through, not drop it"
    );
}

/// Mutation: swap `decode_request`'s order so `serde_json::from_slice` runs
/// before the size check — fails, because the oversized, non-JSON probe body
/// then produces a JSON parse error (`Error::Schema`, no mention of
/// "exceeds") instead of the size refusal this test asserts on.
#[test]
fn a_body_over_the_route_limit_is_refused_before_it_is_parsed() {
    let route = Route::Metadata;
    let limit = route.max_body_len(Direction::Request);

    // Not valid JSON at any length: if the parser ever sees this, it fails
    // for a different reason than the one this test checks for, which is
    // exactly how the "parse then check" mutation is caught.
    let oversized = vec![b'x'; limit + 1];
    assert!(
        oversized.len() > limit,
        "premise: the probe body is actually over {route:?}'s request limit of {limit} bytes"
    );

    let error = decode_request(route, &oversized)
        .expect_err("a body over the route limit must be refused, not parsed");
    assert_eq!(
        error.code(),
        "invalid",
        "the refusal must be the size ceiling firing, not whatever a JSON parser makes of \
         {} bytes of 'x': {error}",
        oversized.len()
    );
    assert!(
        error.message().contains("exceeds"),
        "the refusal must name why: {error}"
    );
}

/// A handler that answers every event-fabric route with [`sample_success_response`],
/// after decoding the request through the same [`Route`]-derived size check
/// and route-agreement rule every caller of this protocol goes through. Both
/// transports below are driven against one instance of it, so any difference
/// in what they return is a difference the transport introduced, not the
/// handler.
#[derive(Debug)]
struct ScriptedFabric;

impl Handler for ScriptedFabric {
    fn handle(&self, request: &ServerRequest) -> ServerResponse {
        let Some(route) = Route::from_path(&request.path) else {
            return ServerResponse::json(404, r#"{"error":"no such event-fabric route"}"#);
        };
        if decode_request(route, &request.body).is_err() {
            return ServerResponse::json(400, r#"{"error":"the request could not be decoded"}"#);
        }
        let response = sample_success_response(route);
        let body = encode_response(&response).expect("a canned response encodes to JSON");
        ServerResponse::json(
            200,
            String::from_utf8(body).expect("JSON is always valid UTF-8"),
        )
    }
}

/// A [`FabricTransport`] that calls a [`Handler`] directly, with no socket in
/// between. Exists only in this test, to give [`HttpTransport`] something to
/// be compared against: the same handler, the same request, no network.
struct InMemoryTransport(Arc<dyn Handler>);

impl FabricTransport for InMemoryTransport {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> qip_core::error::Result<Response> {
        let route = request.route();
        let body = encode_request(&request)?;
        let server_request = ServerRequest {
            method: ServerMethod::Post,
            path: route.path().to_string(),
            query: BTreeMap::new(),
            headers: BTreeMap::new(),
            body,
            peer: "in-memory".to_string(),
        };
        let response = self.0.handle(&server_request);
        decode_response(route, &response.body)
    }
}

/// Mutation: in `HttpTransport::call`, after decoding the response, force a
/// decoded [`Response::Metadata`]'s `archived_through` to `0` — fails,
/// because the in-memory transport still reports the handler's real value
/// (80) and the equality assertion below catches the disagreement.
#[test]
fn the_http_transport_and_an_in_memory_transport_answer_one_request_with_identical_typed_responses()
{
    let handler: Arc<dyn Handler> = Arc::new(ScriptedFabric);
    let server = Server::bind("127.0.0.1:0", handler.clone(), ServerLimits::default())
        .expect("binds an ephemeral loopback port");
    let address = server
        .local_address()
        .expect("a bound listener reports its own address");
    let served = std::thread::spawn(move || {
        let _ = server.serve_once();
    });

    let request = sample_request(Route::Metadata);

    let mut in_memory = InMemoryTransport(handler);
    let in_memory_response = in_memory
        .call(request.clone(), Timeouts::default())
        .expect("the in-memory transport answers the scripted handler");

    let mut http = HttpTransport::new(format!("http://{address}"));
    let http_response = http
        .call(request, Timeouts::default())
        .expect("the http transport answers the scripted handler over a real socket");

    served
        .join()
        .expect("the thread serving one connection does not panic");

    assert_eq!(
        http_response, in_memory_response,
        "the http transport and the in-memory transport must answer one request with \
         identical typed responses from the same scripted handler"
    );
}
