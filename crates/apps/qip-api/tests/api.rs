//! Tests for the HTTP surface.
//!
//! The security-relevant properties: that limits are enforced while reading
//! rather than after, that a path cannot escape its prefix, that token
//! comparison does not leak, and that the authorisation table actually gates
//! what it claims to.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, Principal, RateLimiter, Role};
use qip_api::http::{
    Handler, Method, Request, Response, Server, ServerLimits, normalise_path, percent_decode,
};
use qip_api::json;
use qip_api::routes::{Api, ROUTES};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ManualClock};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

// --- path handling ----------------------------------------------------------

#[test]
fn a_path_cannot_escape_its_prefix() {
    // Resolving `..` correctly is possible and getting it subtly wrong is the
    // classic traversal bug, so it is refused outright.
    for attempt in [
        "/api/../../etc/passwd",
        "/api/v1/../secret",
        "/..",
        "/a/b/../../..",
        // Percent-encoded, which is how the naive check gets bypassed.
        "/api/%2e%2e/secret",
        "/api/%2E%2E%2Fsecret",
    ] {
        assert!(normalise_path(attempt).is_none(), "{attempt} was accepted");
    }
}

#[test]
fn a_path_with_a_control_character_is_refused() {
    // A null byte is how a decoded path smuggles a separator past a later
    // check.
    assert!(normalise_path("/api/v1/health%00.txt").is_none());
    assert!(normalise_path("/api/v1/he%0Aalth").is_none());
}

#[test]
fn a_malformed_escape_is_refused_rather_than_passed_through() {
    assert!(percent_decode("%zz").is_none());
    assert!(percent_decode("%2").is_none());
    assert_eq!(percent_decode("%2F").as_deref(), Some("/"));
    assert_eq!(percent_decode("plain").as_deref(), Some("plain"));
}

#[test]
fn repeated_separators_collapse_so_routing_is_unambiguous() {
    assert_eq!(
        normalise_path("/api//v1///health").as_deref(),
        Some("/api/v1/health")
    );
    assert_eq!(normalise_path("/health").as_deref(), Some("/health"));
    assert!(normalise_path("relative").is_none());
}

// --- authentication ---------------------------------------------------------

fn credentials() -> Vec<Credential> {
    vec![
        Credential::from_token(
            "monitor@example.com",
            Role::Monitor,
            "monitor-token".to_string(),
            now(),
            now().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "viewer@example.com",
            Role::Viewer,
            "viewer-token".to_string(),
            now(),
            now().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "operator@example.com",
            Role::Operator,
            "operator-token".to_string(),
            now(),
            now().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "expired@example.com",
            Role::Operator,
            "expired-token".to_string(),
            now().saturating_sub(Duration::from_days(60)),
            now().saturating_sub(Duration::from_days(1)),
        ),
    ]
}

#[test]
fn a_credential_stores_only_the_hash_of_its_token() {
    // A configuration file or a memory dump containing this struct must not
    // contain a usable token.
    let credential = credentials().into_iter().next().unwrap();
    let encoded = serde_json::to_string(&credential).unwrap();
    assert!(
        !encoded.contains("monitor-token"),
        "the token survived serialisation: {encoded}"
    );
    assert_eq!(credential.token_hash.len(), 64, "a SHA-256 hex digest");
}

#[test]
fn a_valid_token_authenticates_to_its_role() -> Result<()> {
    let authenticator = Authenticator::new(credentials());
    let principal = authenticator.authenticate(Some("Bearer viewer-token"), now())?;
    assert_eq!(principal.subject, "viewer@example.com");
    assert_eq!(principal.role, Role::Viewer);
    Ok(())
}

#[test]
fn an_unrecognised_token_is_refused_without_saying_why() {
    // Telling a caller that a token is nearly right is a gift.
    let authenticator = Authenticator::new(credentials());
    let error = authenticator
        .authenticate(Some("Bearer wrong-token"), now())
        .unwrap_err();
    assert_eq!(error.message(), "the credential was not recognised");
}

#[test]
fn an_expired_credential_is_refused() {
    let authenticator = Authenticator::new(credentials());
    let error = authenticator
        .authenticate(Some("Bearer expired-token"), now())
        .unwrap_err();
    assert!(error.message().contains("expired"), "{}", error.message());
}

#[test]
fn a_missing_or_malformed_credential_is_refused() {
    let authenticator = Authenticator::new(credentials());
    assert!(authenticator.authenticate(None, now()).is_err());
    assert!(
        authenticator
            .authenticate(Some("Basic dXNlcjpwYXNz"), now())
            .unwrap_err()
            .message()
            .contains("bearer token")
    );
}

#[test]
fn roles_are_ordered_so_authority_accumulates() {
    assert!(Role::Operator.includes(Role::Viewer));
    assert!(Role::Operator.includes(Role::Monitor));
    assert!(!Role::Viewer.includes(Role::Operator));
    assert!(!Role::Monitor.includes(Role::Viewer));
    for role in [
        Role::Monitor,
        Role::Viewer,
        Role::Analyst,
        Role::Approver,
        Role::Operator,
    ] {
        assert_eq!(Role::parse(role.as_str()).unwrap(), role);
        assert!(role.includes(role), "a role includes itself");
    }
}

#[test]
fn a_principal_below_the_required_role_is_refused_without_naming_its_own() {
    // An unauthorised caller learning the shape of the hierarchy is a small
    // leak with no upside.
    let principal = Principal {
        subject: "viewer@example.com".to_string(),
        role: Role::Viewer,
        issued_at: now(),
    };
    let error = principal.require(Role::Operator).unwrap_err();
    assert!(error.message().contains("operator"));
    assert!(!error.message().contains("viewer"));
}

// --- rate limiting ----------------------------------------------------------

#[test]
fn the_rate_limiter_refuses_past_its_maximum_and_resets_after_the_window() {
    let limiter = RateLimiter::new(Duration::from_secs(60), 3);
    for i in 0..3 {
        assert!(limiter.permit("caller", now()), "request {i} was refused");
    }
    assert!(
        !limiter.permit("caller", now()),
        "the fourth must be refused"
    );

    // A different caller is unaffected: limiting per subject rather than per
    // address means one caller cannot lock out the rest.
    assert!(limiter.permit("someone-else", now()));

    // And the window resets.
    assert!(limiter.permit("caller", now().saturating_add(Duration::from_secs(61))));
}

// --- the route table --------------------------------------------------------

#[test]
fn every_route_declares_a_role_and_a_summary() {
    // The table is what a security review reads instead of the handlers.
    assert!(!ROUTES.is_empty());
    for route in ROUTES {
        assert!(route.pattern.starts_with('/'), "{}", route.pattern);
        assert!(
            !route.summary.trim().is_empty(),
            "{} has no summary",
            route.pattern
        );
    }
}

#[test]
fn mutating_routes_require_more_than_read_authority() {
    // The rule the table exists to make checkable.
    for route in ROUTES.iter().filter(|route| route.method.is_mutating()) {
        assert!(
            route.required_role.includes(Role::Analyst),
            "{} {} is mutating but only needs {}",
            route.method.as_str(),
            route.pattern,
            route.required_role.as_str()
        );
    }
}

#[test]
fn the_kill_switch_requires_an_operator_in_both_directions() {
    // Tripping and clearing are both operator actions through the API, even
    // though the switch itself lets any component trip it: a caller reaching
    // the platform over HTTP is not a component, it is a caller.
    for method in [Method::Post, Method::Delete] {
        let route = ROUTES
            .iter()
            .find(|route| route.method == method && route.pattern == "/kill-switch")
            .expect("both directions exist");
        assert_eq!(route.required_role, Role::Operator);
    }
}

#[test]
fn no_route_can_change_the_autonomy_level() {
    // Changing the autonomy level is an authenticated operator action with a
    // second approver, which a bearer token cannot establish. There is
    // deliberately no endpoint for it.
    assert!(
        !ROUTES
            .iter()
            .any(|route| route.pattern.contains("autonomy") && route.method.is_mutating()),
        "the API must not expose a way to raise autonomy"
    );
}

#[test]
fn routes_resolve_only_under_the_version_prefix() {
    assert!(Api::route_for(Method::Get, "/api/v1/health").is_some());
    assert!(Api::route_for(Method::Get, "/health").is_none());
    assert!(Api::route_for(Method::Get, "/api/v2/health").is_none());
    assert!(Api::route_for(Method::Post, "/api/v1/health").is_none());
}

// --- JSON encoding ----------------------------------------------------------

#[test]
fn json_strings_escape_what_would_break_out_of_them() {
    assert_eq!(json::string("a\"b"), "\"a\\\"b\"");
    assert_eq!(json::string("a\\b"), "\"a\\\\b\"");
    assert_eq!(json::string("a\nb"), "\"a\\nb\"");
    // A control character escaped numerically, since passing it through
    // produces JSON a strict parser rejects.
    assert_eq!(json::string("a\u{0001}b"), "\"a\\u0001b\"");
}

#[test]
fn json_numbers_that_are_not_json_numbers_become_null() {
    assert_eq!(json::number(1.5), "1.5");
    assert_eq!(json::number(f64::NAN), "null");
    assert_eq!(json::number(f64::INFINITY), "null");
}

// --- the server -------------------------------------------------------------

/// A handler that echoes what it was given, so the parsing can be checked.
#[derive(Debug)]
struct Echo;

impl Handler for Echo {
    fn handle(&self, request: &Request) -> Response {
        Response::json(
            200,
            format!(
                "{{\"method\":{},\"path\":{},\"body_len\":{},\"headers\":{}}}",
                json::string(request.method.as_str()),
                json::string(&request.path),
                request.body.len(),
                request.headers.len()
            ),
        )
    }
}

fn send(address: &str, raw: &str) -> String {
    let mut stream = TcpStream::connect(address).expect("connects");
    stream.write_all(raw.as_bytes()).expect("writes");
    stream.flush().expect("flushes");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn serve_one(limits: ServerLimits, raw: &str) -> String {
    let server =
        Server::bind("127.0.0.1:0", Arc::new(Echo), limits).expect("binds an ephemeral port");
    let address = server.local_address().expect("has an address");
    let handle = std::thread::spawn(move || {
        let _ = server.serve_once();
    });
    let response = send(&address, raw);
    let _ = handle.join();
    response
}

#[test]
fn a_well_formed_request_is_parsed() {
    let response = serve_one(
        ServerLimits::default(),
        "GET /api/v1/health?x=1 HTTP/1.1\r\nhost: localhost\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains("\"path\":\"/api/v1/health\""),
        "{response}"
    );
}

#[test]
fn every_response_carries_the_security_headers() {
    let response = serve_one(
        ServerLimits::default(),
        "GET /api/v1/health HTTP/1.1\r\nhost: localhost\r\n\r\n",
    );
    for header in [
        "content-security-policy",
        "x-content-type-options: nosniff",
        "x-frame-options: DENY",
        "referrer-policy: no-referrer",
        "strict-transport-security",
    ] {
        assert!(response.contains(header), "missing {header}: {response}");
    }
    // The policy forbids script entirely, which is what makes the web UI's
    // zero-JavaScript decision enforceable rather than aspirational.
    assert!(response.contains("default-src 'none'"), "{response}");
    assert!(!response.contains("script-src"), "{response}");
}

#[test]
fn an_oversized_body_is_refused_before_it_is_buffered() {
    let limits = ServerLimits {
        max_body: 64,
        ..ServerLimits::default()
    };
    // The declared length alone is enough to refuse: the allocation never
    // happens.
    let response = serve_one(
        limits,
        "POST /api/v1/cycle HTTP/1.1\r\nhost: localhost\r\ncontent-length: 100000000\r\n\r\n",
    );
    assert!(
        response.starts_with("HTTP/1.1 413"),
        "an unbounded declared length must be refused: {response}"
    );
}

#[test]
fn an_oversized_request_line_is_refused() {
    let limits = ServerLimits {
        max_request_line: 128,
        ..ServerLimits::default()
    };
    let long = "a".repeat(500);
    let response = serve_one(
        limits,
        &format!("GET /api/v1/{long} HTTP/1.1\r\nhost: localhost\r\n\r\n"),
    );
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
}

#[test]
fn too_many_headers_are_refused() {
    // An unbounded header list is an unbounded allocation.
    let limits = ServerLimits {
        max_headers: 4,
        ..ServerLimits::default()
    };
    let headers: String = (0..50)
        .map(|i| format!("x-filler-{i}: value\r\n"))
        .collect();
    let response = serve_one(
        limits,
        &format!("GET /api/v1/health HTTP/1.1\r\n{headers}\r\n"),
    );
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
}

#[test]
fn a_malformed_request_is_refused_rather_than_guessed_at() {
    for raw in [
        "NOTAMETHOD / HTTP/1.1\r\n\r\n",
        "GET\r\n\r\n",
        "GET / HTTP/9.9\r\n\r\n",
        "GET ../etc/passwd HTTP/1.1\r\n\r\n",
    ] {
        let response = serve_one(ServerLimits::default(), raw);
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "{raw:?} produced {response}"
        );
    }
}

#[test]
fn a_header_value_cannot_inject_a_second_response() {
    // CR or LF in a header value would let a caller inject headers or a whole
    // second response. The encoder is what strips it.
    let server = Server::bind(
        "127.0.0.1:0",
        Arc::new(|_: &Request| {
            Response::json(200, "{}").with_header("x-test", "value\r\nx-injected: yes")
        }),
        ServerLimits::default(),
    )
    .expect("binds");
    let address = server.local_address().unwrap();
    let handle = std::thread::spawn(move || {
        let _ = server.serve_once();
    });
    let raw = send(&address, "GET /api/v1/health HTTP/1.1\r\nhost: x\r\n\r\n");
    let _ = handle.join();

    assert!(
        !raw.contains("\r\nx-injected:"),
        "a header injection survived: {raw}"
    );
    assert!(
        raw.contains("x-test: valuex-injected: yes"),
        "the value should survive with its line breaks removed: {raw}"
    );
}

#[test]
fn a_head_request_returns_the_headers_without_the_body() {
    let response = serve_one(
        ServerLimits::default(),
        "HEAD /api/v1/health HTTP/1.1\r\nhost: localhost\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let (_, body) = response
        .split_once("\r\n\r\n")
        .expect("has a body separator");
    assert!(
        body.is_empty(),
        "a HEAD response must have no body: {body:?}"
    );
    // But the length is still declared, so a client knows what a GET would
    // return.
    assert!(response.contains("content-length:"), "{response}");
}

// --- the API, end to end ----------------------------------------------------

fn api() -> Result<Arc<Api>> {
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::universe::Universe;
    use qip_kernel::{Platform, PlatformConfig};
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(now()));
    let context = Context::new(clock.clone(), config.seed);

    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            qip_core::ObjectId::from_string("obj-AAA"),
            "AAA",
            InstrumentType::CommonStock,
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(qip_core::Decimal::from_int(100))
        .provenance(Provenance::synthetic("test", now()))
        .build(now())?,
    )?;

    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;

    Ok(Arc::new(Api::new(
        Arc::new(std::sync::Mutex::new(platform)),
        Arc::new(Authenticator::new(credentials())),
        Arc::new(RateLimiter::new(Duration::from_secs(60), 1000)),
        clock,
    )))
}

fn request(method: Method, path: &str, token: Option<&str>) -> Request {
    let mut headers = BTreeMap::new();
    if let Some(token) = token {
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
    }
    Request {
        method,
        path: path.to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    }
}

fn get(api: &Api, path: &str, token: Option<&str>) -> Response {
    api.handle(&request(Method::Get, path, token))
}

#[test]
fn discovery_is_unauthenticated_because_a_client_needs_it_first() -> Result<()> {
    let api = api()?;
    let response = get(&api, "/api/v1", None);
    assert_eq!(response.status, 200);
    let body = String::from_utf8(response.body).unwrap();
    assert!(body.contains("\"version\":\"v1\""), "{body}");
    Ok(())
}

#[test]
fn an_unauthenticated_request_to_a_real_route_is_refused() -> Result<()> {
    let api = api()?;
    let response = get(&api, "/api/v1/health", None);
    assert_eq!(response.status, 401);
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name == "www-authenticate" && value == "Bearer")
    );
    Ok(())
}

#[test]
fn a_monitor_can_check_health_but_not_read_the_portfolio() -> Result<()> {
    let api = api()?;
    assert_eq!(
        get(&api, "/api/v1/health", Some("monitor-token")).status,
        200
    );
    let refused = get(&api, "/api/v1/portfolio", Some("monitor-token"));
    assert_eq!(refused.status, 403);
    Ok(())
}

#[test]
fn a_viewer_cannot_halt_the_platform() -> Result<()> {
    // The authorisation table, enforced.
    let api = api()?;
    let response = api.handle(&request(
        Method::Post,
        "/api/v1/kill-switch",
        Some("viewer-token"),
    ));
    assert_eq!(response.status, 403);
    Ok(())
}

#[test]
fn an_operator_can_halt_and_clear_the_platform() -> Result<()> {
    let api = api()?;
    let halt = api.handle(&request(
        Method::Post,
        "/api/v1/kill-switch",
        Some("operator-token"),
    ));
    assert_eq!(halt.status, 200);

    let status = get(&api, "/api/v1/health", Some("monitor-token"));
    let body = String::from_utf8(status.body).unwrap();
    assert!(body.contains("\"halted\":true"), "{body}");

    let clear = api.handle(&request(
        Method::Delete,
        "/api/v1/kill-switch",
        Some("operator-token"),
    ));
    assert_eq!(clear.status, 200);

    let body = String::from_utf8(get(&api, "/api/v1/health", Some("monitor-token")).body).unwrap();
    assert!(body.contains("\"halted\":false"), "{body}");
    Ok(())
}

#[test]
fn the_health_endpoint_reports_that_the_platform_is_not_live_capable() -> Result<()> {
    // The question an operator and a health check both ask.
    let api = api()?;
    let body = String::from_utf8(get(&api, "/api/v1/health", Some("monitor-token")).body).unwrap();
    assert!(body.contains("\"live_capable\":false"), "{body}");
    assert!(body.contains("\"autonomy\":\"paper_trading\""), "{body}");
    Ok(())
}

#[test]
fn an_unknown_path_is_a_404_and_a_wrong_method_is_a_405() -> Result<()> {
    let api = api()?;
    assert_eq!(
        get(&api, "/api/v1/nonexistent", Some("viewer-token")).status,
        404
    );
    let wrong_method = api.handle(&request(
        Method::Delete,
        "/api/v1/health",
        Some("operator-token"),
    ));
    assert_eq!(wrong_method.status, 405);
    Ok(())
}

#[test]
fn the_agents_endpoint_lists_the_whole_organisation() -> Result<()> {
    let api = api()?;
    let body = String::from_utf8(get(&api, "/api/v1/agents", Some("viewer-token")).body).unwrap();
    assert!(body.contains("macro-analyst"), "{body}");
    assert!(body.contains("risk-control"), "{body}");
    assert_eq!(
        body.matches("\"id\":").count(),
        18,
        "all eighteen agents are listed"
    );
    Ok(())
}

#[test]
fn running_a_cycle_through_the_api_traverses_every_stage() -> Result<()> {
    let api = api()?;
    let response = api.handle(&request(
        Method::Post,
        "/api/v1/cycle",
        Some("operator-token"),
    ));
    assert_eq!(response.status, 202);
    let body = String::from_utf8(response.body).unwrap();
    assert!(body.contains("\"traversed_every_stage\":true"), "{body}");
    Ok(())
}
