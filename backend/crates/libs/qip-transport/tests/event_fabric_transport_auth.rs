//! The HTTP transport presents the identity it was built with, and a broker
//! checking with `auth::verify` admits it and refuses any other.
//!
//! Until the lead's review of SLICE-28, `HttpTransport` attached no identity
//! at all. The producer and consumer ran over a transport that no broker
//! would ever authenticate, and each packet's report said so as a limitation
//! rather than as a failure. These tests drive the real socket path, because
//! the property is what arrives at the far end, not what the struct holds.

use std::sync::Arc;

use qip_core::hash::sha256_hex;
use qip_transport::event_fabric::auth::{self, BearerToken, IdentityTable};
use qip_transport::event_fabric::protocol::{
    Metadata, MetadataRequest, Refusal, Request, Response, encode_response,
};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::server::{
    Handler, Request as ServerRequest, Response as ServerResponse, Server, ServerLimits,
};

const PRODUCER_TOKEN: &str = "sliceTestProducerTokenForTransportAuth0123456789";
const STRANGER_TOKEN: &str = "sliceTestStrangerTokenNobodyGrantedAnything987654";

/// A broker stand-in that answers metadata to a verified caller and
/// `AclDenied` to anyone else, deciding with the same `verify` the real
/// broker will use.
struct VerifyingFabric {
    identities: IdentityTable,
}

impl Handler for VerifyingFabric {
    fn handle(&self, request: &ServerRequest) -> ServerResponse {
        let response = match auth::verify(&self.identities, request.header(auth::HEADER)) {
            Ok(_) => Response::Metadata(
                Metadata::new("orders", 3, 7, 120, 80).expect("a coherent metadata answer"),
            ),
            Err(_) => Response::Refused(Refusal::AclDenied),
        };
        let body = encode_response(&response).expect("a response encodes to JSON");
        ServerResponse::json(200, String::from_utf8(body).expect("JSON is UTF-8"))
    }
}

fn metadata_request() -> Request {
    Request::Metadata(MetadataRequest {
        stream: "orders".to_string(),
        partition: 3,
    })
}

fn token(text: &str) -> BearerToken {
    BearerToken::new(text.to_string()).expect("a well-formed test token")
}

#[test]
fn the_http_transport_presents_its_token_and_only_a_granted_one_is_admitted() {
    let identities = IdentityTable::parse(&format!(
        "producer-a {}\n",
        sha256_hex(PRODUCER_TOKEN.as_bytes())
    ))
    .expect("an identities file holding one digest");

    // Premise: the broker stand-in really refuses a request that carries no
    // identity, so an admitted call below is the token's doing and not a
    // handler that admits everyone.
    assert!(
        auth::verify(&identities, None).is_err(),
        "the verifier admitted a request with no Authorization header"
    );

    let handler: Arc<dyn Handler> = Arc::new(VerifyingFabric { identities });
    let server = Server::bind("127.0.0.1:0", handler, ServerLimits::default())
        .expect("binds an ephemeral loopback port");
    let address = server.local_address().expect("the bound address");
    let served = std::thread::spawn(move || {
        for _ in 0..2 {
            let _ = server.serve_once();
        }
    });

    let mut granted = HttpTransport::new(format!("http://{address}"), token(PRODUCER_TOKEN));
    let answer = granted
        .call(metadata_request(), Timeouts::default())
        .expect("the granted producer's call is answered");
    assert!(
        matches!(answer, Response::Metadata(_)),
        "a transport built with a granted token was not admitted: {answer:?}"
    );

    let mut stranger = HttpTransport::new(format!("http://{address}"), token(STRANGER_TOKEN));
    let answer = stranger
        .call(metadata_request(), Timeouts::default())
        .expect("the stranger's call is answered, with a refusal");
    assert_eq!(
        answer,
        Response::Refused(Refusal::AclDenied),
        "a transport built with a token nobody granted was admitted"
    );

    served.join().expect("the serving thread does not panic");
}

#[test]
fn no_debug_rendering_of_a_transport_contains_its_token() {
    let transport = HttpTransport::new("http://127.0.0.1:1", token(PRODUCER_TOKEN));
    let rendered = format!("{transport:?}");
    assert!(
        rendered.contains("HttpTransport"),
        "premise: the rendering is of the transport: {rendered}"
    );
    assert!(
        !rendered.contains(PRODUCER_TOKEN),
        "a Debug rendering of the transport leaked its token: {rendered}"
    );
}
