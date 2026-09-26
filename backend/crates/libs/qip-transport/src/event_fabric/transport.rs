//! The `FabricTransport` swap seam. See ADR 0100 §1 and "Alternatives
//! rejected": fidelity-first's proposal named a transport seam so that C2's
//! eventual move to QUIC with mTLS "reshapes the producer, consumer, drain,
//! control consumer and grant command" not at all — every one of those talks
//! to [`FabricTransport`], never to a socket. This is the seam and its only
//! production implementation, [`HttpTransport`], which speaks
//! [`super::protocol`]'s typed requests and responses over this crate's own
//! blocking HTTP/1.1 client.
//!
//! # What this seam is not
//!
//! It carries [`super::protocol::Request`] and [`super::protocol::Response`]
//! and nothing else: no venue, no placer, no order type crosses it. Paper
//! layer 3 — what may place, amend or cancel an order — is untouched by this
//! file, and the swap ADR 0100 names is a transport swap, not a widening of
//! what a cell may do.
//!
//! # Identity is a property of the transport
//!
//! ADR 0100 §7's bearer-token identity is attached here, once per call, from
//! the [`BearerToken`] an [`HttpTransport`] is built with. It is not a
//! parameter of [`FabricTransport::call`]. Who is calling is a fact about the
//! connection a composition root opened, not about each request; a trait
//! that took a token per call would let one producer send under two
//! identities. There is no constructor without a token, because the
//! broker's `verify` refuses a request that carries none. A transport that
//! could be built unauthenticated would only fail later, at the far end,
//! with every produce refused and nothing near the caller saying why.
//!
//! This section said the opposite until the lead's review of SLICE-28. It
//! found that no packet in the slice owned attaching the token: SLICE-22
//! deferred it to the SDK, and the SDK (SLICE-28) could not reach the
//! request this seam builds. The producer and consumer therefore ran over
//! an HTTP transport that no broker would ever authenticate.

use std::time::Duration;

use qip_core::error::{Error, Result};

use crate::http::{ClientLimits, HttpClient, HttpRequest, Method};

use super::auth::{self, BearerToken};
use super::protocol::{Direction, Request, Response, decode_response, encode_request};

/// Explicit connect, read and write timeouts for one [`FabricTransport::call`].
///
/// Named separately from [`crate::http::ClientLimits`] rather than reusing it
/// directly: a caller of this seam thinks in terms of one call's deadlines,
/// not the byte-count ceilings `ClientLimits` also carries, and
/// [`HttpTransport`] is the one place those two have to meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
    pub write: Duration,
}

impl Timeouts {
    pub const fn new(connect: Duration, read: Duration, write: Duration) -> Self {
        Self {
            connect,
            read,
            write,
        }
    }
}

impl Default for Timeouts {
    /// Chosen for mesh traffic inside one VPC, matching
    /// [`crate::http::ClientLimits`]'s own defaults: a peer a few
    /// milliseconds away, and a failure that should be visible in seconds.
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(2),
            read: Duration::from_secs(15),
            write: Duration::from_secs(30),
        }
    }
}

/// The seam C2 (QUIC with mTLS) swaps. Blocking request/response over
/// [`super::protocol`]'s typed messages, every call carrying its own explicit
/// timeouts rather than a transport-wide default a caller cannot see.
///
/// `&mut self` rather than `&self`: nothing this build's only implementation
/// does needs mutation, but a future implementation holding a live connection
/// (C2's QUIC stream, reused rather than reopened per call) does, and taking
/// `&mut self` now means that implementation is not a breaking change to this
/// trait.
pub trait FabricTransport {
    fn call(&mut self, request: Request, timeouts: Timeouts) -> Result<Response>;
}

/// The production [`FabricTransport`]: one HTTP/1.1 request per call, over
/// this crate's own [`HttpClient`], to a fabric endpoint's own scheme and
/// authority.
///
/// Not `Clone`: [`BearerToken`] is deliberately not, so a token is not copied
/// into places nobody chose. `Debug` is safe; the token's own `Debug` is
/// redacted.
#[derive(Debug)]
pub struct HttpTransport {
    /// Scheme and authority only — `http://host:port`, no path. Each call
    /// appends the request's own `Route::path`.
    base_url: String,
    /// Sent as `Authorization: Bearer <token>` on every call.
    identity: BearerToken,
}

impl HttpTransport {
    /// `base_url` is the fabric endpoint's scheme and authority, with no
    /// trailing slash and no path — `http://fabricd.internal:7100`, not
    /// `.../v1/event-fabric`. Every call appends the request's own route.
    /// `identity` is the token every call presents; the broker checks it
    /// against its identities file and refuses anything else.
    pub fn new(base_url: impl Into<String>, identity: BearerToken) -> Self {
        Self {
            base_url: base_url.into(),
            identity,
        }
    }
}

impl FabricTransport for HttpTransport {
    fn call(&mut self, request: Request, timeouts: Timeouts) -> Result<Response> {
        let route = request.route();
        let url = format!("{}{}", self.base_url, route.path());
        let body = encode_request(&request)?;

        let http_request = HttpRequest::json(Method::Post, &url, body)
            .map_err(|error| {
                let message = format!("cannot build the event-fabric {route:?} request: {error}");
                Error::from(error).relabelled(message)
            })?
            .with_header(auth::HEADER, &self.identity.header_value());

        let limits = ClientLimits {
            connect_timeout: timeouts.connect,
            read_timeout: timeouts.read,
            write_timeout: timeouts.write,
            // Bounded to what this specific route's answer may legitimately
            // be, not the client's generic default: a byzantine or
            // misconfigured broker answering a control-plane call with
            // megabytes of body is refused while reading, before it is
            // buffered.
            max_body: route.max_body_len(Direction::Response),
            ..ClientLimits::default()
        };

        let response = HttpClient::new(limits)
            .send(&http_request)
            .map_err(|error| {
                let message = format!("the event-fabric {route:?} call to {url} failed: {error}");
                Error::from(error).relabelled(message)
            })?;

        if !response.is_success() {
            return Err(Error::io(format!(
                "the event-fabric {route:?} call to {url} answered with status {}: {}",
                response.status,
                response.body_excerpt()
            )));
        }

        decode_response(route, &response.body)
    }
}
