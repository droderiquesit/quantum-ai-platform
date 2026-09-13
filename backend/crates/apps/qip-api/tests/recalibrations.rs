//! The recalibration surface: `GET /risk/recalibrations` and
//! `POST /risk/recalibrations/{rule}/approvals` (ADR 0061).
//!
//! What this file guards is the shape of the signature route rather than the
//! arithmetic behind it, which `qip-kernel` proves: that the approver is the
//! session and the body cannot carry one, that the bound is the platform's
//! own proposal and the body cannot carry one either, and that a rule the
//! platform has proposed nothing about cannot be approved into anything.
//! Every test asserts its premise first — the route exists at the role the
//! table says, the viewer can read — so a refusal below is about the thing
//! under test and not about a route that never answered.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::{Api, ROUTES};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ManualClock};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const OPERATOR_TOKEN: &str = "operator-token";
const VIEWER_TOKEN: &str = "viewer-token";
const LIST_PATH: &str = "/risk/recalibrations";
const APPROVE_PATH: &str = "/risk/recalibrations/order-notional/approvals";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

struct Rig {
    api: Api,
}

fn rig() -> Result<Rig> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![
        Credential::from_token(
            "operator@example.com",
            Role::Operator,
            OPERATOR_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "viewer@example.com",
            Role::Viewer,
            VIEWER_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
    ]));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    Ok(Rig {
        api: Api::new(platform, authenticator, rate_limiter, clock),
    })
}

impl Rig {
    fn call(&self, method: Method, path: &str, token: &str, body: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        self.api.handle(&Request {
            method,
            path: format!("/api/v1{path}"),
            query: BTreeMap::new(),
            headers,
            body: body.as_bytes().to_vec(),
            peer: "127.0.0.1:1".to_string(),
        })
    }
}

fn body_of(response: Response) -> (String, serde_json::Value) {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    let value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
    (text, value)
}

const GOOD_BODY: &str = r#"{"rationale": "the regret evidence was reviewed against the mandate"}"#;

#[test]
fn a_viewer_reads_the_open_proposals_and_the_running_set_by_name() -> Result<()> {
    // The premise for the two refusals below: the surface exists, at the
    // roles the table says, and a fresh platform has proposed nothing.
    let rig = rig()?;
    let list = ROUTES
        .iter()
        .find(|route| route.method == Method::Get && route.pattern == LIST_PATH)
        .expect("the list route is in the table");
    assert_eq!(list.required_role, Role::Viewer);
    let approve = ROUTES
        .iter()
        .find(|route| {
            route.method == Method::Post && route.pattern == "/risk/recalibrations/:rule/approvals"
        })
        .expect("the approval route is in the table");
    assert_eq!(approve.required_role, Role::Operator);

    let response = rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, "");
    assert_eq!(response.status, 200);
    let (_, view) = body_of(response);
    assert_eq!(view["limits"], "conservative-paper");
    assert_eq!(view["open"].as_array().map(Vec::len), Some(0));
    assert_eq!(view["history"].as_array().map(Vec::len), Some(0));

    // And a viewer cannot sign: the table says operator and the router
    // enforces it before the body is read.
    let refused = rig.call(Method::Post, APPROVE_PATH, VIEWER_TOKEN, GOOD_BODY);
    assert_eq!(refused.status, 403);
    Ok(())
}

#[test]
fn the_approver_of_a_recalibration_is_the_session_and_never_the_body() -> Result<()> {
    // The failure this prevents: a body that names its approver turns an
    // approval into a claim to have been approved. The route refuses the
    // key by position — never by echoing it — and does so before the kernel
    // sees the request, so nothing is journaled.
    let rig = rig()?;
    let with_approver =
        r#"{"rationale": "the regret evidence was reviewed", "approver": "someone-else"}"#;
    let response = rig.call(Method::Post, APPROVE_PATH, OPERATOR_TOKEN, with_approver);
    assert_eq!(response.status, 400);
    let (text, body) = body_of(response);
    let error = body["error"].as_str().expect("an error string");
    // By position and never by name: `serde_json` sorts an object's keys,
    // so the position is the sorted one, and what matters is that the key
    // is located without being repeated.
    assert!(
        error.contains("key at position 1"),
        "the refusal does not name the offending key by position: {text}"
    );
    assert!(
        error.contains("neither the approver"),
        "the refusal does not say the approver is the session's: {text}"
    );
    assert!(
        !text.contains("someone-else"),
        "the refusal echoed the value the caller sent: {text}"
    );

    // The same for a bound: the platform proposes it, a caller cannot.
    let with_bound =
        r#"{"rationale": "the regret evidence was reviewed", "proposed_bound": 999999}"#;
    let response = rig.call(Method::Post, APPROVE_PATH, OPERATOR_TOKEN, with_bound);
    assert_eq!(response.status, 400);
    let (text, body) = body_of(response);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("key at position 1")),
        "{text}"
    );
    assert!(
        !text.contains("999999"),
        "the refusal echoed the bound the caller sent: {text}"
    );
    Ok(())
}

#[test]
fn no_recalibration_the_platform_did_not_propose_can_be_approved() -> Result<()> {
    // A good body, a good operator, and a rule the platform has generated
    // no proposal for: not found, because a proposal is generated from
    // regret evidence and cannot be supplied by a caller. The distinction
    // from a 400 matters — the request was well formed; the thing it names
    // does not exist.
    let rig = rig()?;
    let response = rig.call(Method::Post, APPROVE_PATH, OPERATOR_TOKEN, GOOD_BODY);
    assert_eq!(
        response.status,
        404,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, body) = body_of(response);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("proposed no recalibration")
                && error.contains("cannot be supplied")),
        "the refusal does not say a proposal cannot be supplied: {text}"
    );

    // Nothing was journaled: the list still holds no history.
    let (_, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(view["history"].as_array().map(Vec::len), Some(0));
    assert_eq!(view["open"].as_array().map(Vec::len), Some(0));
    Ok(())
}
