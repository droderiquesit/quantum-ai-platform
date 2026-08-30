//! The three peers the demonstration talks to, each on a loopback port it
//! chose for itself.
//!
//! # Why these are servers and not doubles of the client
//!
//! Every adapter in this walk is interesting exactly where it meets a socket.
//! A stubbed HTTP client would show that a decoder was called; it would not
//! show that a request was framed, a credential put in a header, a body read to
//! its declared length and a connection closed. So the demonstration binds real
//! listeners and lets the adapters connect to them.
//!
//! # Which server, and why not the one in `tests/`
//!
//! `qip-market-ingestion`, `qip-brokers` and `qip-edge-node` each carry a
//! loopback HTTP harness under `tests/`. None of them is reachable from a
//! binary — an integration test's `mod server` is compiled into that test
//! binary and nothing else — so the choice was between copying three hundred
//! lines of request parsing into this crate and using a server the workspace
//! already ships.
//!
//! This uses the one it ships: [`qip_api::http::Server`], the platform's own
//! HTTP/1.1 server, which `qip-cli` already depended on. It is not a test
//! harness and does not pretend to be one — it has no scripted truncation, no
//! deliberate stall, no oversized body — and that is the right trade here. What
//! this demonstration shows is a live path working; what a peer having a bad
//! day does to each adapter is proven, case by case, by that adapter's own
//! suite, and re-staging it in an operator command would be theatre.
//!
//! `qip_transport::mesh`'s own documentation points at the same seam: its
//! endpoint answers "in terms of a status and a body and whatever is serving
//! HTTP writes them", precisely so a caller can put it behind this server.
//! [`MeshPeer`] is that sentence, composed.
//!
//! # What these peers are
//!
//! Scripts. The vendor serves what [`crate::demo::script`] says. The venue
//! fills forty units of whatever it is sent and never anything else: it is not
//! a matching engine, has no book, and its acknowledgement is computed from the
//! request only so that it is evidence the order arrived rather than a canned
//! string that would look the same if nothing had. **Every fill in this run is
//! fabricated here.**
//!
//! # Stopping them
//!
//! [`Loopback`] holds a shutdown flag its accept loop checks between
//! connections, and setting it is all a drop can honestly do: the loop is
//! blocked in `accept` and will not look at the flag until something connects.
//! The demonstration therefore does not wait for these threads — it returns
//! from `main`, which ends the process and every thread in it. That is why the
//! run cannot hang on a peer that is idle by design.

use qip_api::http::{Handler, Method, Request, Response, Server, ServerLimits};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_transport::{MeshEndpoint, Method as WireMethod};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

/// Limits for a peer that only ever talks to this process.
///
/// Tight in time so that a request nobody answers fails the run in
/// milliseconds, and tight in size because the largest body here is a hundred
/// and twenty bars.
fn limits() -> ServerLimits {
    ServerLimits {
        max_body: 1024 * 1024,
        read_timeout: StdDuration::from_secs(2),
        write_timeout: StdDuration::from_secs(2),
        max_concurrent: 8,
        ..ServerLimits::default()
    }
}

/// A peer on an ephemeral loopback port.
///
/// Port zero asks the operating system for a free one, so several of these run
/// beside each other — and beside every test binary on the machine — without a
/// hard-coded number that fails on whichever host already uses it.
#[derive(Debug)]
pub struct Loopback {
    role: String,
    url: String,
    server: Arc<Server>,
    shutdown: Arc<AtomicBool>,
}

impl Loopback {
    /// Bind a port, start serving on a thread, and hand back the address.
    ///
    /// The address is read before the server is handed to the thread, so a
    /// caller always has a URL to configure an adapter with by the time this
    /// returns — there is no window in which the peer exists and nobody can
    /// name it.
    pub fn spawn(role: &str, handler: Arc<dyn Handler>) -> Result<Self> {
        let server = Arc::new(Server::bind("127.0.0.1:0", handler, limits())?);
        let url = format!("http://{}", server.local_address()?);
        let shutdown = server.shutdown_handle();
        let serving = Arc::clone(&server);
        let name = format!("qip-demo-{role}");
        std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                // A serving error is the listener going away, which is what
                // shutdown looks like from inside the loop. There is nobody to
                // report it to: the demonstration's own failure will be that a
                // request did not get an answer, and that is the error worth
                // showing.
                let _ = serving.serve();
            })
            .map_err(|error| {
                Error::io(format!("cannot start the {role} peer's thread: {error}"))
            })?;
        Ok(Self {
            role: role.to_string(),
            url,
            server,
            shutdown,
        })
    }

    /// What this peer is playing.
    pub fn role(&self) -> &str {
        &self.role
    }

    /// `http://127.0.0.1:port`, the value an adapter is configured with.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// How many connections this peer has finished serving.
    ///
    /// The demonstration prints it rather than asserting on it, but it is the
    /// number that separates "the adapter answered from memory" from "the
    /// adapter opened a socket", and an operator watching a live path should
    /// have it in front of them.
    pub fn served(&self) -> u64 {
        self.server.request_count()
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        // Read the module documentation: this is a request, not a join. The
        // accept loop notices between connections and the process does not
        // wait for it.
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// A per-path hit counter, so a peer can say what it was asked for.
#[derive(Debug, Default)]
struct Tally {
    hits: Mutex<BTreeMap<String, u64>>,
}

impl Tally {
    fn record(&self, path: &str) -> u64 {
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = hits.entry(path.to_string()).or_default();
        *entry = entry.saturating_add(1);
        *entry
    }

    fn hits(&self, path: &str) -> u64 {
        self.hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(path)
            .copied()
            .unwrap_or_default()
    }
}

/// The paths the vendor answers on. Named once so the adapter configuration and
/// the router cannot drift apart.
pub const MARKET_DATA_PATH: &str = "/v1/market-data";
pub const NARRATIVE_PATH: &str = "/v1/narrative";
pub const DEPTH_SNAPSHOT_PATH: &str = "/v1/depth/snapshot";
pub const DEPTH_UPDATES_PATH: &str = "/v1/depth/updates";
pub const ALTERNATIVE_PATH: &str = "/v1/alternative";

/// The paths the venue answers on.
pub const HEALTH_PATH: &str = "/v1/health";
pub const ORDERS_PATH: &str = "/v1/orders";

/// A data vendor: four feeds, one script.
///
/// Everything it serves is a function of the instant the demonstration started,
/// held here so that each of the four adapters is answered consistently — a
/// book stamped after the bars it sits beside would be a vendor contradicting
/// itself, and none of these decoders would catch it.
#[derive(Debug)]
pub struct VendorDouble {
    start: Timestamp,
    tally: Tally,
}

impl VendorDouble {
    pub fn new(start: Timestamp) -> Self {
        Self {
            start,
            tally: Tally::default(),
        }
    }

    /// How many requests reached one feed.
    pub fn hits(&self, path: &str) -> u64 {
        self.tally.hits(path)
    }
}

impl Handler for VendorDouble {
    fn handle(&self, request: &Request) -> Response {
        let path = request.path.as_str();
        let hit = self.tally.record(path);
        match path {
            MARKET_DATA_PATH => Response::json(200, super::script::market_data(self.start)),
            NARRATIVE_PATH => Response::json(200, super::script::narrative(self.start)),
            DEPTH_SNAPSHOT_PATH => Response::json(200, super::script::depth_snapshot(self.start)),
            // Only the first poll carries increments. See the script: replaying
            // them would be a vendor re-sending sequence numbers the book has
            // already applied.
            DEPTH_UPDATES_PATH if hit == 1 => {
                Response::json(200, super::script::depth_updates(self.start))
            }
            DEPTH_UPDATES_PATH => Response::json(200, super::script::NO_DEPTH_UPDATES),
            ALTERNATIVE_PATH => Response::json(200, super::script::alternative(self.start)),
            other => Response::json(
                404,
                format!(r#"{{"error":"this vendor serves no feed at {other}"}}"#),
            ),
        }
    }
}

/// A venue: a health probe, and an order collection that partially fills.
///
/// The acknowledgement is computed from the request body rather than scripted,
/// which is the only way it can be evidence. A hard-coded answer would come
/// back identically whether or not the adapter had put anything on the wire,
/// and "the order reached the venue" is the single claim this half of the
/// demonstration exists to make.
#[derive(Debug, Default)]
pub struct VenueDouble {
    tally: Tally,
    /// The last acknowledgement for each order, so a later query answers with
    /// what the venue already said instead of inventing a second story.
    acknowledged: Mutex<BTreeMap<String, String>>,
}

/// Units this venue fills, of any order, ever.
///
/// Fixed and small so the shortfall is visible: the point of the fill is that
/// the demonstration then has to reconcile a partial, not that the arithmetic
/// is interesting.
pub const FILLED_UNITS: &str = "40";

impl VenueDouble {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hits(&self, path: &str) -> u64 {
        self.tally.hits(path)
    }

    fn acknowledge(&self, body: &[u8]) -> Response {
        let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(body) else {
            return Response::json(400, r#"{"error":"the submit was not JSON"}"#);
        };
        let field = |name: &str| {
            parsed
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let id = field("client_order_id");
        if id.is_empty() {
            return Response::json(400, r#"{"error":"the submit named no client order id"}"#);
        }
        let acknowledgement = format!(
            r#"{{"client_order_id":"{id}","venue_order_id":"VEN-1","state":"partially_filled",
                 "instrument":"{instrument}","side":"{side}","quantity":"{quantity}",
                 "filled":"{FILLED_UNITS}",
                 "fills":[{{"fill_id":"F-{id}","quantity":"{FILLED_UNITS}","price":"100.02",
                            "costs":"0.35","at":"{submitted}"}}]}}"#,
            instrument = field("instrument"),
            side = field("side"),
            quantity = field("quantity"),
            submitted = field("submitted_at"),
        );
        self.acknowledged
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, acknowledgement.clone());
        Response::json(200, acknowledgement)
    }

    fn recall(&self, order_id: Option<&str>) -> Response {
        let Some(order_id) = order_id else {
            return Response::json(400, r#"{"error":"a query must name a client order id"}"#);
        };
        match self
            .acknowledged
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(order_id)
        {
            Some(known) => Response::json(200, known.clone()),
            None => Response::json(
                404,
                format!(r#"{{"error":"this venue has no order {order_id}"}}"#),
            ),
        }
    }
}

impl Handler for VenueDouble {
    fn handle(&self, request: &Request) -> Response {
        self.tally.record(request.path.as_str());
        match (request.method, request.path.as_str()) {
            (Method::Get, HEALTH_PATH) => Response::json(200, "{}"),
            (Method::Post, ORDERS_PATH) => self.acknowledge(&request.body),
            (Method::Get, ORDERS_PATH) => self.recall(request.query_param("client_order_id")),
            (method, path) => Response::json(
                404,
                format!(
                    r#"{{"error":"this venue has no {method} {path}"}}"#,
                    method = method.as_str()
                ),
            ),
        }
    }
}

/// The central plane's mesh endpoint, behind an HTTP server.
///
/// The endpoint is `qip-transport`'s own — the same one a deployed central
/// plane serves — so what this peer accepts, deduplicates, refuses and hands
/// back is the platform's behaviour and not a re-implementation of it. All this
/// type adds is the translation between two `Method` enums and the fact that
/// the request target is what the endpoint routes on.
#[derive(Debug)]
pub struct MeshPeer {
    endpoint: MeshEndpoint,
}

impl MeshPeer {
    pub fn new(endpoint: MeshEndpoint) -> Self {
        Self { endpoint }
    }
}

impl Handler for MeshPeer {
    fn handle(&self, request: &Request) -> Response {
        let Some(method) = WireMethod::parse(request.method.as_str()) else {
            return Response::json(405, r#"{"error":"the mesh endpoint knows no such method"}"#);
        };
        let answer = self.endpoint.handle(method, &request.path, &request.body);
        Response::new(
            answer.status,
            qip_transport::EndpointResponse::CONTENT_TYPE,
            answer.body,
        )
    }
}
