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
use qip_api::cells::CellRegistry;
use qip_api::console::Console;
use qip_api::http::{
    Handler, Method, Request, Response, Server, ServerLimits, normalise_path, percent_decode,
};
use qip_api::json;
use qip_api::routes::{Api, DISCOVERY_PATH, OPENAPI_PATH, ROUTES};
use qip_api::web::{Router, Web};
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
// --- the brute-force budget -------------------------------------------------
//
// These guard a defect that was real in this file, not a hypothetical one: the
// failure counter used to be keyed by subject, a subject was only known after
// a hash match, and so guessing random tokens incremented nothing. The threat
// model documented the hole. The property below is the one that was missing —
// unattributable failures are counted, and counted together.

/// The refusal a spent budget produces, matched as a whole phrase.
///
/// `contains("unrecognised")` would also be true of "the credential was not
/// recognised" under a careless edit, which is exactly the substring trap this
/// suite has been bitten by before.
const BUDGET_SPENT: &str = "too many unrecognised credentials have been presented";

#[test]
fn repeated_unrecognised_tokens_reach_the_budget_and_are_then_refused_as_a_flood() {
    let authenticator = Authenticator::new(credentials());
    let threshold = authenticator.lockout_threshold();
    assert!(threshold >= 2, "a threshold of {threshold} proves nothing");

    // The premise: every attempt short of the threshold is an ordinary
    // refusal, so the assertion below is about the threshold and not about the
    // authenticator refusing everything from the start.
    for attempt in 1..threshold {
        let error = authenticator
            .authenticate(Some(&format!("Bearer guess-{attempt}")), now())
            .unwrap_err();
        assert_eq!(
            error.message(),
            "the credential was not recognised",
            "attempt {attempt} of {threshold} was refused early"
        );
        assert_eq!(authenticator.unrecognised_attempts(now()), attempt);
    }

    let error = authenticator
        .authenticate(Some("Bearer guess-at-the-threshold"), now())
        .unwrap_err();
    assert!(
        error.message().contains(BUDGET_SPENT),
        "the budget did not fire: {}",
        error.message()
    );
    assert_eq!(authenticator.unrecognised_attempts(now()), threshold);
}

#[test]
fn a_valid_credential_authenticates_while_another_callers_budget_is_spent() -> Result<()> {
    // The asymmetry is the design. A budget that also refused valid tokens
    // would let any anonymous caller lock the desk out of the halt and
    // kill-switch routes with ten wrong guesses.
    let authenticator = Authenticator::new(credentials());
    for attempt in 0..authenticator.lockout_threshold() + 5 {
        assert!(
            authenticator
                .authenticate(Some(&format!("Bearer guess-{attempt}")), now())
                .is_err()
        );
    }
    assert_eq!(
        authenticator.unrecognised_attempts(now()),
        authenticator.lockout_threshold(),
        "the premise: the budget is spent before the operator calls"
    );

    let principal = authenticator.authenticate(Some("Bearer operator-token"), now())?;
    assert_eq!(principal.subject, "operator@example.com");
    assert_eq!(principal.role, Role::Operator);
    Ok(())
}

#[test]
fn distinct_random_tokens_share_one_budget_and_leave_no_state_behind() {
    // Keying the counter on the presented token would make every guess its own
    // first offence — the control would never fire — and would let an
    // anonymous caller allocate a map entry per guess. Both failures show up
    // here: five thousand distinct tokens must still be refused as a flood,
    // and the recorded count must stay at the threshold rather than climbing
    // with the traffic.
    let authenticator = Authenticator::new(credentials());
    let threshold = authenticator.lockout_threshold();
    let mut flooded = 0_u32;
    for attempt in 0..5_000_u32 {
        // Distinct and spread across the token space rather than sequential,
        // so a per-token map could not coincidentally collide them together.
        let token = qip_core::hash::to_hex(&qip_core::hash::sha256(
            format!("random-guess-{attempt}").as_bytes(),
        ));
        let error = authenticator
            .authenticate(Some(&format!("Bearer {token}")), now())
            .unwrap_err();
        if error.message().contains(BUDGET_SPENT) {
            flooded += 1;
        }
    }
    assert_eq!(
        flooded,
        5_000 - (threshold - 1),
        "every attempt from the threshold onwards must be refused as a flood"
    );
    assert_eq!(
        authenticator.unrecognised_attempts(now()),
        threshold,
        "the counter grew with the traffic instead of saturating"
    );
}

#[test]
fn a_successful_authentication_does_not_clear_the_budget() -> Result<()> {
    // Clearing on success is the wrong answer and was worth writing down: an
    // attacker holding any low-privilege token could reset the budget between
    // guesses, and an ordinary monitoring poll every second would reset it
    // forever, so the control could never fire. Only time clears it.
    let authenticator = Authenticator::new(credentials());
    for attempt in 0..3 {
        assert!(
            authenticator
                .authenticate(Some(&format!("Bearer guess-{attempt}")), now())
                .is_err()
        );
    }
    assert_eq!(
        authenticator.unrecognised_attempts(now()),
        3,
        "the premise: three failures are on the books"
    );

    authenticator.authenticate(Some("Bearer monitor-token"), now())?;
    assert_eq!(
        authenticator.unrecognised_attempts(now()),
        3,
        "a successful authentication wiped the guessing pressure"
    );
    Ok(())
}

#[test]
fn the_budget_starts_again_in_the_next_window() {
    // A refusal that never lifts is an outage. The window is what keeps a
    // brute-force burst from permanently refusing a caller who mistypes a
    // token afterwards.
    let authenticator = Authenticator::new(credentials());
    for attempt in 0..authenticator.lockout_threshold() {
        assert!(
            authenticator
                .authenticate(Some(&format!("Bearer guess-{attempt}")), now())
                .is_err()
        );
    }
    assert!(
        authenticator
            .authenticate(Some("Bearer another-guess"), now())
            .unwrap_err()
            .message()
            .contains(BUDGET_SPENT),
        "the premise: the budget is spent in this window"
    );

    let later = now().saturating_add(Duration::from_mins(1));
    assert_eq!(authenticator.unrecognised_attempts(later), 0);
    let error = authenticator
        .authenticate(Some("Bearer another-guess"), later)
        .unwrap_err();
    assert_eq!(error.message(), "the credential was not recognised");
}

#[test]
fn an_expired_credential_does_not_spend_the_unrecognised_budget() {
    // The two failure paths are not the same fact. An expired credential is
    // attributable and its holder has already lost access; counting it here
    // would let one stale poller keep the budget permanently spent, which is
    // an attacker disarming the control by looking like a tired client.
    let authenticator = Authenticator::new(credentials());
    for _ in 0..authenticator.lockout_threshold() + 5 {
        let error = authenticator
            .authenticate(Some("Bearer expired-token"), now())
            .unwrap_err();
        assert!(error.message().contains("expired"), "{}", error.message());
        assert!(
            !error.message().contains(BUDGET_SPENT),
            "an expired credential was treated as a guess: {}",
            error.message()
        );
    }
    assert_eq!(authenticator.unrecognised_attempts(now()), 0);
}

#[test]
fn only_a_caller_holding_a_real_token_can_tell_the_two_refusals_apart() {
    // An attacker who can distinguish "unrecognised" from "expired" learns
    // which tokens exist. The expired refusal names its subject on purpose,
    // but reaching it requires presenting a token that matched a stored hash —
    // possession of the secret itself. Nothing on the unattributable path may
    // name a subject or an expiry.
    let authenticator = Authenticator::new(credentials());
    for attempt in 0..authenticator.lockout_threshold() + 2 {
        let message = authenticator
            .authenticate(Some(&format!("Bearer guess-{attempt}")), now())
            .unwrap_err()
            .message()
            .to_string();
        for leak in ["expired", "@example.com", "monitor", "operator", "viewer"] {
            assert!(
                !message.contains(leak),
                "the refusal for an unrecognised token leaked {leak}: {message}"
            );
        }
    }
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

/// The whole server, assembled the way `main` assembles it.
///
/// One platform, one clock, one cell registry: the API and the console read
/// the same state, which is the property that keeps a page and the JSON behind
/// it from disagreeing.
struct Assembled {
    api: Arc<Api>,
    console: Arc<Console>,
    web: Arc<Web>,
    cells: Arc<CellRegistry>,
    clock: Arc<ManualClock>,
    /// The platform the API and the interface share, for a test to drive a
    /// fact into before asserting a page shows it.
    platform: Arc<std::sync::Mutex<qip_kernel::Platform>>,
}

fn assemble() -> Result<Assembled> {
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

    let platform = Arc::new(std::sync::Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(credentials()));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    let cells = Arc::new(CellRegistry::default());

    // Wired the way `main.rs` wires it: the API records each cycle where the
    // interface reads it. A fixture that skipped this would pass every test
    // while the deployed page stayed empty.
    let web = Arc::new(Web::new(
        platform.clone(),
        authenticator.clone(),
        rate_limiter.clone(),
        clock.clone(),
    ));

    Ok(Assembled {
        api: Arc::new(
            Api::new(
                platform.clone(),
                authenticator.clone(),
                rate_limiter.clone(),
                clock.clone(),
            )
            .with_cells(cells.clone())
            .with_cycle_overview(web.cycle_overview()),
        ),
        console: Arc::new(Console::new(
            platform.clone(),
            cells.clone(),
            authenticator,
            rate_limiter,
            clock.clone(),
        )),
        web,
        cells,
        clock,
        platform,
    })
}

fn api() -> Result<Arc<Api>> {
    Ok(assemble()?.api)
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

/// The overview page reads its stages from a store only the cycle route
/// writes. Until this was wired, nothing wrote it: the store was private to
/// the interface, the interface never ran a cycle, and the stage overview
/// rendered empty for the life of every process — which an operator cannot
/// tell from "no cycle has run". Driven through the router, as a browser
/// would, so the seam under test is the one that is deployed.
#[test]
fn a_cycle_run_through_the_router_reaches_the_operator_interfaces_stage_overview() -> Result<()> {
    let assembled = assemble()?;
    let overview = assembled.web.cycle_overview();
    // Premise: nothing has been recorded before a cycle runs.
    assert!(
        overview.rows().is_empty(),
        "the overview held stages before any cycle ran"
    );

    let router = Router::new(assembled.api.clone(), assembled.web.clone());
    let response = router.handle(&request(
        Method::Post,
        "/api/v1/cycle",
        Some("operator-token"),
    ));
    assert_eq!(response.status, 202);

    let rows = overview.rows();
    assert!(
        !rows.is_empty(),
        "the cycle ran and the operator interface's stage overview is still empty"
    );
    let stages: Vec<&str> = rows.iter().map(|row| row.stage.as_str()).collect();
    // Exact tokens, not substrings: every stage of the loop, in cycle order.
    assert_eq!(
        stages,
        [
            "sense",
            "understand",
            "discover",
            "reason",
            "simulate",
            "decide",
            "act",
            "learn"
        ]
    );
    Ok(())
}

// --- the console's read surface, as JSON ------------------------------------

/// Parse a response body, failing loudly if it is not JSON.
///
/// Every body this API returns is assembled by hand from `crate::json`, so
/// "does it parse at all" is a real question and the tests below ask it of
/// everything they read.
fn body_of(response: Response) -> serde_json::Value {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

/// Whether any number appears anywhere in a value.
fn contains_a_number(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Number(_) => true,
        serde_json::Value::Array(items) => items.iter().any(contains_a_number),
        serde_json::Value::Object(fields) => fields.values().any(contains_a_number),
        _ => false,
    }
}

#[test]
fn a_surface_with_nothing_behind_it_names_the_reason_and_returns_no_number() -> Result<()> {
    // The rule the whole crate is built around. A client that plots whatever
    // it is given must not be given a zero the platform never observed, so
    // these bodies carry no number at all — only what is missing and why.
    let api = api()?;
    for path in [
        "/api/v1/markets",
        "/api/v1/assets",
        "/api/v1/arbitrage",
        "/api/v1/pnl",
        "/api/v1/data-sources",
        "/api/v1/training",
        "/api/v1/regimes",
        "/api/v1/news",
    ] {
        let response = get(&api, path, Some("viewer-token"));
        assert_eq!(response.status, 200, "{path}");
        let body = body_of(response);
        assert_eq!(body["available"], serde_json::json!(false), "{path}");
        let reason = body["reason"].as_str().unwrap_or_default();
        assert!(
            reason.len() > 40,
            "{path} gives no usable reason: {reason:?}"
        );
        assert!(
            !contains_a_number(&body),
            "{path} returned a number nothing reported: {body}"
        );
    }
    Ok(())
}

#[test]
fn regions_reports_that_no_cell_has_reported_rather_than_an_empty_book() -> Result<()> {
    // An empty aggregate and a silent feed are the same JSON if the endpoint
    // renders the aggregate, and they are opposite readings.
    let api = api()?;
    let body = body_of(get(&api, "/api/v1/regions", Some("viewer-token")));
    assert_eq!(body["available"], serde_json::json!(false), "{body}");
    assert!(
        body["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("no edge cell has reported"),
        "{body}"
    );
    Ok(())
}

#[test]
fn a_cell_that_reported_is_shown_with_its_age_and_goes_stale_on_the_clock() -> Result<()> {
    let assembled = assemble()?;
    assembled
        .cells
        .record(&qip_kernel::CellReport::new("eu-west", now()));

    let body = body_of(get(&assembled.api, "/api/v1/regions", Some("viewer-token")));
    let cell = &body["cells"][0];
    assert_eq!(cell["cell"], serde_json::json!("eu-west"), "{body}");
    assert_eq!(cell["stale"], serde_json::json!(false), "{body}");
    assert_eq!(cell["age"], serde_json::json!("0s"), "{body}");

    // The registry's freshness bound is a minute, and the point of recording
    // arrival times at all is that a book stops being presented as current.
    assembled.clock.advance(Duration::from_secs(120));
    let body = body_of(get(&assembled.api, "/api/v1/regions", Some("viewer-token")));
    assert_eq!(body["cells"][0]["stale"], serde_json::json!(true), "{body}");
    assert_eq!(
        body["cells"][0]["age"],
        serde_json::json!("2m 0s"),
        "{body}"
    );
    Ok(())
}

#[test]
fn risk_reports_no_exposure_rather_than_zero_exposure_when_no_cell_has_reported() -> Result<()> {
    // Zero gross exposure is what a flat book looks like. It is also what a
    // platform nothing is reporting to looks like, and only one of those is a
    // reason to relax.
    let api = api()?;
    let body = body_of(get(&api, "/api/v1/risk", Some("viewer-token")));
    assert_eq!(
        body["exposure"]["available"],
        serde_json::json!(false),
        "{body}"
    );
    assert!(!contains_a_number(&body["exposure"]), "{body}");
    // And no empty findings list either: "no concentration breach" and "nobody
    // has looked" are the same empty array and opposite readings.
    assert_eq!(
        body["concentrations"]["available"],
        serde_json::json!(false),
        "{body}"
    );
    // The two risk figures this process cannot measure are named rather than
    // omitted: a client that found no limits would otherwise read it as a
    // platform with no limits.
    assert_eq!(
        body["limit_utilisation"]["available"],
        serde_json::json!(false)
    );
    assert_eq!(body["tail_risk"]["available"], serde_json::json!(false));
    // What it does know.
    assert_eq!(body["kill_switch"]["halted"], serde_json::json!(false));
    Ok(())
}

#[test]
fn models_reports_what_the_agents_spent_without_inventing_a_roster() -> Result<()> {
    let api = api()?;
    let body = body_of(get(&api, "/api/v1/models", Some("viewer-token")));
    assert_eq!(
        body["registry"]["available"],
        serde_json::json!(false),
        "{body}"
    );
    // Spend is an observation this process holds: the audit trail is its own,
    // so a zero here is a measured zero rather than a missing feed.
    assert_eq!(
        body["observed_use"]["agent_runs"],
        serde_json::json!(0),
        "{body}"
    );
    assert_eq!(
        body["observed_use"]["cost_micros"],
        serde_json::json!(0),
        "{body}"
    );
    Ok(())
}

#[test]
fn system_reports_the_event_log_chain_and_where_it_broke() -> Result<()> {
    let api = api()?;
    let body = body_of(get(&api, "/api/v1/system", Some("viewer-token")));
    assert_eq!(body["chain_intact"], serde_json::json!(true), "{body}");
    // Null rather than a sentinel: zero is a record number.
    assert!(body["chain_broken_at"].is_null(), "{body}");
    assert_eq!(
        body["autonomy"],
        serde_json::json!("paper_trading"),
        "{body}"
    );
    Ok(())
}

#[test]
fn quantum_shows_no_job_at_all_rather_than_a_result_without_its_classical_run() -> Result<()> {
    let api = api()?;
    let body = body_of(get(&api, "/api/v1/quantum", Some("viewer-token")));
    assert_eq!(
        body["jobs"]["available"],
        serde_json::json!(false),
        "{body}"
    );
    assert_eq!(
        body["routing"]["classical_baseline"],
        serde_json::json!("always"),
        "{body}"
    );
    Ok(())
}

#[test]
fn every_console_route_answers_a_viewer_with_json_and_refuses_a_monitor() -> Result<()> {
    // The console's read surface is portfolio data, and a monitoring token
    // holds no portfolio authority. The table says so; this checks the table
    // is enforced rather than merely written down.
    let api = api()?;
    for route in ROUTES
        .iter()
        .filter(|route| route.required_role == Role::Viewer)
    {
        let path = format!("/api/v1{}", route.pattern);
        let response = get(&api, &path, Some("viewer-token"));
        assert_eq!(response.status, 200, "{path}");
        let _ = body_of(response);
        assert_eq!(
            get(&api, &path, Some("monitor-token")).status,
            403,
            "{path} is readable by a monitoring token"
        );
    }
    Ok(())
}

// --- the generated OpenAPI document -----------------------------------------

#[test]
fn the_openapi_document_is_unauthenticated_valid_json_and_declares_its_version() -> Result<()> {
    let api = api()?;
    let response = api.handle(&request(Method::Get, OPENAPI_PATH, None));
    assert_eq!(response.status, 200);
    let document = body_of(response);
    assert!(
        document["openapi"]
            .as_str()
            .is_some_and(|version| version.starts_with("3.1")),
        "{document}"
    );
    // The two endpoints served ahead of the table are in it, and are the only
    // operations declaring no security.
    assert!(document["paths"][DISCOVERY_PATH].is_object(), "{document}");
    assert!(document["paths"][OPENAPI_PATH].is_object(), "{document}");
    Ok(())
}

#[test]
fn the_openapi_document_describes_every_route_and_the_authority_it_requires() {
    // The document is generated from the route table, so this is really a
    // check that nothing was lost on the way: a path a security review reads
    // in the document must require what the table says it requires.
    let document: serde_json::Value =
        serde_json::from_str(&qip_api::document()).expect("valid JSON");
    let paths = &document["paths"];
    for route in ROUTES {
        let path = format!("/api/v1{}", route.pattern);
        let method = route.method.as_str().to_ascii_lowercase();
        let operation = &paths[&path][&method];
        assert!(
            operation.is_object(),
            "{} {path} is not in the document",
            route.method.as_str()
        );
        assert_eq!(
            operation["x-required-role"],
            serde_json::json!(route.required_role.as_str()),
            "{path} states the wrong authority"
        );
        assert_eq!(
            operation["summary"],
            serde_json::json!(route.summary),
            "{path}"
        );
        // The success status comes from the table too, so a route answering
        // 202 is not documented as answering 200.
        assert!(
            operation["responses"][route.success.to_string()].is_object(),
            "{path} does not document its {} response",
            route.success
        );
        assert!(
            operation["responses"]["403"].is_object(),
            "{path} does not document the refusal its role check produces"
        );
    }
}

#[test]
fn the_openapi_document_declares_nothing_the_router_does_not_serve() {
    // The other direction, and the one that catches a document drifting into
    // describing an endpoint that was removed.
    let document: serde_json::Value =
        serde_json::from_str(&qip_api::document()).expect("valid JSON");
    let paths = document["paths"].as_object().expect("a paths object");

    let mut identifiers: Vec<String> = Vec::new();
    for (path, operations) in paths {
        if path == DISCOVERY_PATH || path == OPENAPI_PATH {
            continue;
        }
        let suffix = path
            .strip_prefix("/api/v1")
            .unwrap_or_else(|| panic!("{path} is not under the version prefix"));
        for (method, operation) in operations.as_object().expect("operations") {
            if method == "parameters" {
                continue;
            }
            assert!(
                ROUTES.iter().any(|route| {
                    route.pattern == suffix && route.method.as_str().to_ascii_lowercase() == *method
                }),
                "the document declares {method} {path}, which is not a route"
            );
            identifiers.push(
                operation["operationId"]
                    .as_str()
                    .expect("an operation id")
                    .to_string(),
            );
        }
    }
    let mut unique = identifiers.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        identifiers.len(),
        "two operations share an id, which no generated client can express"
    );
}

#[test]
fn the_openapi_document_carries_no_credential() {
    // It declares that a bearer token is required. It must not contain one,
    // and no token exists in this crate to leak.
    let document = qip_api::document();
    assert!(document.contains(r#""scheme":"bearer""#), "{document}");
    for token in ["monitor-token", "viewer-token", "operator-token"] {
        assert!(!document.contains(token), "{document}");
    }
}

// --- the operator console ---------------------------------------------------

#[test]
fn the_console_is_served_under_console_and_is_not_a_lower_bar_than_the_api() -> Result<()> {
    let assembled = assemble()?;
    let router = Router::new(assembled.api.clone(), assembled.web.clone())
        .with_console(assembled.console.clone());

    let page = router.handle(&request(Method::Get, "/console", Some("viewer-token")));
    assert_eq!(page.status, 200);
    let html = String::from_utf8(page.body).expect("UTF-8");
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    // A panel with nothing behind it says so on the page as well as in the
    // JSON, and in the same words.
    assert!(html.contains("No data."), "{html}");

    assert_eq!(
        router
            .handle(&request(Method::Get, "/console", Some("monitor-token")))
            .status,
        403,
        "a monitoring token must not read the console"
    );
    assert_eq!(
        router
            .handle(&request(Method::Get, "/console", None))
            .status,
        401
    );
    Ok(())
}

#[test]
fn a_router_with_no_console_refuses_the_console_paths_rather_than_answering_them() -> Result<()> {
    // Answering with the surfaces' overview page would be worse than a
    // refusal: an operator would read a page that is not the console and not
    // notice.
    let assembled = assemble()?;
    let router = Router::new(assembled.api.clone(), assembled.web.clone());
    let response = router.handle(&request(Method::Get, "/console/risk", Some("viewer-token")));
    assert_eq!(response.status, 503);
    Ok(())
}

#[test]
fn the_console_can_trip_the_kill_switch_and_has_no_path_that_clears_one() -> Result<()> {
    let assembled = assemble()?;
    let router = Router::new(assembled.api.clone(), assembled.web.clone())
        .with_console(assembled.console.clone());

    let tripped = router.handle(&request(
        Method::Post,
        "/console/risk/kill-switch",
        Some("viewer-token"),
    ));
    // See Other, so a refresh after the POST does not trip it again.
    assert_eq!(tripped.status, 303);
    let body = body_of(get(&assembled.api, "/api/v1/health", Some("monitor-token")));
    assert_eq!(body["halted"], serde_json::json!(true), "{body}");

    // And there is no console path that lifts it. Clearing requires an
    // operator identity verified minutes ago, which a page cannot establish.
    for path in [
        "/console/risk/kill-switch/clear",
        "/console/risk/clear",
        "/console/risk/resume",
    ] {
        let response = router.handle(&request(Method::Post, path, Some("operator-token")));
        assert_eq!(response.status, 404, "{path} did something");
    }
    let body = body_of(get(&assembled.api, "/api/v1/health", Some("monitor-token")));
    assert_eq!(body["halted"], serde_json::json!(true), "{body}");
    Ok(())
}

// --- the operator interface reads platform facts through the router ---------

/// Fetch one surface through the router as a viewer, asserting it rendered.
fn page(router: &Router, path: &str) -> String {
    let response = router.handle(&request(Method::Get, path, Some("viewer-token")));
    assert_eq!(response.status, 200, "{path}");
    String::from_utf8(response.body).expect("a UTF-8 page")
}

#[test]
fn the_execution_page_renders_the_settlement_the_platform_counted_and_not_a_zero_it_did_not()
-> Result<()> {
    use qip_contracts::message::BookSide;
    use qip_contracts::signal::StrategyId;
    use qip_contracts::venue::VenueId;
    use qip_kernel::central::{BreakDirection, CellReport, ReconciliationBreak};
    use qip_observability::metrics::{labels, names};

    let assembled = assemble()?;
    let router = Router::new(assembled.api.clone(), assembled.web.clone());

    // Premise: the platform has counted nothing and no cell has reported.
    {
        let platform = assembled.platform.lock().expect("the platform lock");
        let snapshot = platform.telemetry().metrics.snapshot();
        assert!(
            !snapshot
                .series
                .iter()
                .any(|series| series.name == names::CENTRAL_ORDERS_SENT),
            "the premise is a platform that has registered no sent order"
        );
    }
    assert!(assembled.cells.is_empty());

    let before = page(&router, "/execution");
    assert!(
        before.contains(r#"data-panel="Cells" data-state="absent""#),
        "{before}"
    );
    for key in [
        "central_orders_sent",
        "central_breaks_unsent_fill",
        "central_cell_halts_reconciliation",
    ] {
        assert!(
            before.contains(&format!(
                r#"data-fact="{key}" data-state="not-recorded">not recorded<"#
            )),
            "{key} was not rendered as not recorded: {before}"
        );
        assert!(
            !before.contains(&format!(r#"data-fact="{key}" data-state="recorded">0<"#)),
            "{key} rendered a zero the platform never counted"
        );
    }

    // One report: an order the venue accepted, and a break the cell shipped.
    // The plane registers the first and halts the cell on the second.
    let report = CellReport::new("eu-west", now())
        .with_orders(vec![qip_mesh::delta::DeltaOrder {
            order_id: "eu-west-1".to_string(),
            strategy: StrategyId::new("strat-1"),
            object_id: qip_core::ObjectId::from_string("obj-AAA"),
            venue: VenueId::new("XNYS"),
            side: BookSide::Ask,
            quantity: qip_core::Decimal::from_int(10),
            price: qip_core::Decimal::from_int(100),
            simulated: true,
            contributors: Vec::new(),
        }])
        .with_break(ReconciliationBreak {
            instrument: "obj-AAA".to_string(),
            cell_quantity: qip_core::Decimal::from_int(10),
            external_quantity: qip_core::Decimal::from_int(4),
            detail: "the venue confirms less than the cell holds".to_string(),
            origin: Default::default(),
        });
    assembled.cells.record(&report);
    let ingestion = {
        let mut platform = assembled.platform.lock().expect("the platform lock");
        platform.ingest_cell_report(report, now())?
    };
    assert_eq!(ingestion.settlement.orders_sent, 1, "{ingestion:?}");
    assert!(ingestion.halted.is_some(), "{ingestion:?}");

    // Premise for the rendering: the platform now holds the counted facts.
    {
        let platform = assembled.platform.lock().expect("the platform lock");
        let snapshot = platform.telemetry().metrics.snapshot();
        assert_eq!(snapshot.counter_total(names::CENTRAL_ORDERS_SENT), 1);
        assert_eq!(
            snapshot.counter(
                names::CENTRAL_RECONCILIATION_BREAKS,
                &labels([("direction", BreakDirection::CellOverVenue.as_str())])
            ),
            1
        );
        assert!(platform.autonomy().kill_switch().is_halted("eu-west"));
    }

    let after = page(&router, "/execution");
    assert!(
        after.contains(r#"data-fact="central_orders_sent" data-state="recorded">1<"#),
        "{after}"
    );
    assert!(
        after.contains(r#"data-fact="central_breaks_cell_over_venue" data-state="recorded">1<"#),
        "{after}"
    );
    assert!(
        after.contains(r#"data-fact="central_cell_halts_reconciliation" data-state="recorded">1<"#),
        "{after}"
    );
    // A direction nothing incremented is still not recorded, beside one that
    // was: the page distinguishes the arms per series, not per page.
    assert!(
        after.contains(r#"data-fact="central_breaks_unsent_fill" data-state="not-recorded">"#),
        "{after}"
    );
    // The cell's row: reported, halted by the centre's own scope, and its
    // per-cell settlement said to be not recorded rather than zero.
    assert!(
        after.contains(r#"data-panel="Cells" data-state="current""#),
        "{after}"
    );
    assert!(
        after.contains(r#"data-fact="cell.eu-west.halted_by_centre"><span class="pill bad">yes<"#),
        "{after}"
    );
    assert!(
        after.contains(r#"data-fact="cell.eu-west.policy_halt_flag"><span class="pill good">no<"#),
        "the centre's global switch is not tripped, so the policy flag it ships is no: {after}"
    );
    assert!(
        after.contains(
            r#"data-fact="cell.eu-west.orders_sent" data-state="not-recorded">not recorded<"#
        ),
        "{after}"
    );
    assert!(
        !after.contains(r#"data-fact="cell.eu-west.orders_sent" data-state="recorded">0<"#),
        "{after}"
    );
    // No mesh is served here, so the cell's own halted flag — which travels
    // only on its delta — is not recorded, and the polled flag never is.
    assert!(
        after
            .contains(r#"data-fact="cell.eu-west.cell_reports_halted" data-state="not-recorded">"#),
        "{after}"
    );
    assert!(
        after.contains(r#"data-fact="cell.eu-west.polled_halt_flag" data-state="not-recorded">"#),
        "{after}"
    );
    assert!(after.contains(">PAPER TRADING<"), "{after}");
    Ok(())
}

#[test]
fn a_router_with_a_mesh_lends_it_to_the_page_and_the_page_says_no_delta_was_decoded() -> Result<()>
{
    use qip_api::mesh::{CellAddress, MeshBackbone, MeshSettings};

    let assembled = assemble()?;
    let settings = MeshSettings {
        cells: vec![CellAddress {
            cell: "eu-west".to_string(),
            address: "127.0.0.1:0".to_string(),
        }],
        inbox_capacity: 8,
        spool_capacity: 8,
    };
    let mesh = MeshBackbone::open(
        &settings,
        Arc::new(qip_storage::kv::MemoryKeyValueStore::new()),
        assembled.clock.clone() as Arc<dyn qip_core::Clock>,
        None,
    )?;
    // Premise: the mesh has decoded no delta.
    assert!(mesh.status().standings.is_empty());
    let api = Arc::new(
        Api::new(
            assembled.platform.clone(),
            Arc::new(Authenticator::new(credentials())),
            Arc::new(RateLimiter::new(Duration::from_secs(60), 1000)),
            assembled.clock.clone(),
        )
        .with_cells(assembled.cells.clone())
        .with_mesh(Arc::new(std::sync::Mutex::new(mesh))),
    );
    assembled
        .cells
        .record(&qip_kernel::CellReport::new("eu-west", now()));
    let router = Router::new(api, assembled.web.clone());

    let page = page(&router, "/execution");
    assert!(
        page.contains(r#"data-fact="cell.eu-west.cell_reports_halted" data-state="not-recorded">not recorded<small class="muted"> — the mesh has decoded no delta from this cell"#),
        "{page}"
    );
    Ok(())
}

#[test]
fn the_governance_page_renders_the_whitelist_the_platform_journaled_and_no_slot_it_did_not()
-> Result<()> {
    use qip_events::Topic;

    let assembled = assemble()?;
    let router = Router::new(assembled.api.clone(), assembled.web.clone());

    // Premise: nothing has been journaled under the policy topic.
    {
        let platform = assembled.platform.lock().expect("the platform lock");
        assert!(
            platform
                .event_log()
                .by_topic(Topic::PolicyDistributed)
                .is_empty()
        );
    }
    let before = page(&router, "/governance");
    assert!(
        before.contains(r#"data-panel="Last payload per cell" data-state="absent""#),
        "{before}"
    );
    assert!(
        !before.contains(r#"data-fact="policy.eu-west.whitelist""#),
        "{before}"
    );

    // The platform issues and journals one cell's whitelist — what the cycle
    // route does at the shipping seam.
    let issue = {
        let mut platform = assembled.platform.lock().expect("the platform lock");
        let issue = platform.issue_cycle_whitelist("eu-west", now())?;
        assert_eq!(
            platform
                .event_log()
                .by_topic(Topic::PolicyDistributed)
                .len(),
            1
        );
        issue
    };

    let after = page(&router, "/governance");
    assert!(
        after.contains(&format!(
            r#"data-fact="policy.eu-west.whitelist">{}<"#,
            issue.describe()
        )),
        "the line the platform journaled is not the line on the page: {after}"
    );
    assert!(
        after.contains(&format!(
            r#"data-fact="policy.eu-west.cycle_whitelist" data-state="recorded">produced at {}<"#,
            now().to_rfc3339()
        )),
        "{after}"
    );
    // The other eleven slots are assembled at the shipping seam and not
    // journaled; the page says so rather than claiming they were produced.
    for slot in [
        "trained_models",
        "capital_grants",
        "risk_envelope",
        "adversary_profiles",
    ] {
        assert!(
            after.contains(&format!(
                r#"data-fact="policy.eu-west.{slot}" data-state="not-recorded">not recorded<"#
            )),
            "{slot}: {after}"
        );
    }
    assert!(
        after.contains(r#"data-fact="policy.eu-west.sequence" data-state="not-recorded">"#),
        "{after}"
    );
    assert!(after.contains(">PAPER TRADING<"), "{after}");
    Ok(())
}

#[test]
fn the_overview_renders_the_instruments_the_platform_found_unfit_and_says_what_it_cannot_attest()
-> Result<()> {
    let assembled = assemble()?;
    let router = Router::new(assembled.api.clone(), assembled.web.clone());

    // Premise: the fixture's one instrument is synthetic, and the platform
    // said so at assembly.
    let excluded = {
        let platform = assembled.platform.lock().expect("the platform lock");
        platform.universe_not_decision_grade().to_vec()
    };
    assert_eq!(excluded.len(), 1, "{excluded:?}");
    assert_eq!(excluded[0].0, "obj-AAA");
    assert_eq!(
        excluded[0].1,
        "licensing class Synthetic is not production-eligible"
    );

    let page = page(&router, "/");
    assert!(
        page.contains(r#"data-fact="universe.not_decision_grade" data-state="recorded">1<"#),
        "{page}"
    );
    assert!(
        page.contains(
            r#"<td class="mono">obj-AAA</td><td>licensing class Synthetic is not production-eligible</td>"#
        ),
        "{page}"
    );
    // The catalogue's identity is not readable from the platform, and the
    // page says so instead of printing a version it did not read.
    for key in [
        "universe.version",
        "universe.sha256",
        "universe.instruments",
    ] {
        assert!(
            page.contains(&format!(
                r#"data-fact="{key}" data-state="not-recorded">not recorded<"#
            )),
            "{key}: {page}"
        );
    }
    assert!(page.contains(">PAPER TRADING<"), "{page}");
    Ok(())
}
