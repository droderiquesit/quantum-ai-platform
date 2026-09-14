//! The venue-withdrawal surface: `GET /venues/withdrawals` and
//! `POST /venues/{venue}/reinstatements` (ADR 0062).
//!
//! ADR 0062 shipped `Platform::reinstate_venue` and recorded that no HTTP
//! route exposed it, which left a fail-closed control — the platform can
//! withdraw its only venue on its own evidence — recoverable only by someone
//! with direct access to the kernel. What this file guards is the shape of
//! the signature route rather than the arithmetic behind the withdrawal,
//! which `qip-kernel` proves: that the approver is the session and the body
//! cannot carry one, that a venue the platform did not withdraw is not found,
//! and that two sessions of one operator are still one operator at the route
//! and not only in a kernel unit test.
//!
//! Every test asserts its premise first — the routes exist at the roles the
//! table declares, the withdrawal actually happened — so a refusal below is
//! about the thing under test and not about a route that never answered.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::{Api, ROUTES};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Two operator credentials with *different* subjects, because the property
/// under test is that two subjects are two people and one subject is one
/// person however many sessions it opens.
const OPERATOR_TOKEN: &str = "operator-token-for-the-venue-suite";
const SECOND_OPERATOR_TOKEN: &str = "second-operator-token-for-the-venue-suite";
const VIEWER_TOKEN: &str = "viewer-token-for-the-venue-suite";

const LIST_PATH: &str = "/venues/withdrawals";
const SIGN_PATH: &str = "/venues/simulated-venue/reinstatements";
const SIGN_PATTERN: &str = "/venues/:venue/reinstatements";

const GOOD_BODY: &str =
    r#"{"rationale": "the venue's lot grid was re-read and the desk's sizing corrected"}"#;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// A liquid listed name, stated rather than inherited:
/// [`LiquidityProfile`] has no `Default`, because the controls that veto on
/// liquidity read exactly the two figures a default would have invented.
fn fixture_liquidity() -> LiquidityProfile {
    LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

/// One listing that states no lot of its own, so it carries `qip-financial`'s
/// builder default of one whole lot — which is what makes an order for ten
/// and a half shares infeasible at the desk's venue.
fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn bar(at: Timestamp, level: f64) -> SensedRecord {
    let price = Decimal::from_f64(level).expect("a price");
    SensedRecord::Bar(Box::new(Bar {
        object_id: object("AAA"),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price,
        high: price,
        low: price,
        close: price,
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Some(price),
        quality: DataQuality::default(),
    }))
}

/// A flat tape, so the platform has a price for the instrument and nothing
/// else in the cycle has a reason to act.
fn flat_tape(days: usize) -> Vec<SensedRecord> {
    (0..days)
        .map(|day| {
            bar(
                start().saturating_sub(Duration::from_days((days - day) as i64)),
                100.0,
            )
        })
        .collect()
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
}

fn rig() -> Result<Rig> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let platform = Platform::new(
        config.clone(),
        Context::new(clock.clone(), config.seed),
        Telemetry::silent(),
        universe(),
        LimitSet::conservative_default(),
    )?;
    let platform = Arc::new(Mutex::new(platform));
    let credentials = vec![
        Credential::from_token(
            "operator-one@example.com",
            Role::Operator,
            OPERATOR_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "operator-two@example.com",
            Role::Operator,
            SECOND_OPERATOR_TOKEN.to_string(),
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
    ];
    Ok(Rig {
        api: Api::new(
            platform.clone(),
            Arc::new(Authenticator::new(credentials)),
            Arc::new(RateLimiter::new(Duration::from_secs(60), 1000)),
            clock,
        ),
        platform,
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

    /// Drive the platform to withdraw its own venue, the only way a venue
    /// ever becomes withdrawn: ten feasibility refusals at one venue, then
    /// the LEARN stage's review.
    ///
    /// Through the platform directly rather than through a route, because
    /// there is no route that withdraws a venue and there must not be — the
    /// withdrawal is the platform's finding about its own evidence, and a
    /// caller who could ask for one could deny the desk its venue.
    fn withdraw_the_desk_venue(&self) -> Result<()> {
        let mut platform = self.platform.lock().expect("the platform lock");
        platform.observe(flat_tape(30));
        for n in 0..10 {
            let order = platform.order_from(
                object("AAA"),
                qip_execution_engine::order::Side::Buy,
                dec!("10.5"),
                dec!("100"),
                &format!("prop-off-lot-{n}"),
                vec![format!("hyp-off-lot-{n}")],
                start(),
            );
            let error = platform
                .submit_order(order, start())
                .expect_err("ten and a half shares of a one-lot listing reached the venue");
            assert!(
                error.message().contains("infeasible"),
                "the premise failed: refused for another reason than the lot grid: {}",
                error.message()
            );
        }
        platform.run_cycle(start());
        assert_eq!(
            platform.withdrawn_venues().iter().collect::<Vec<_>>(),
            vec!["simulated-venue"],
            "the premise failed: ten lot refusals did not withdraw the desk venue"
        );
        Ok(())
    }
}

fn body_of(response: Response) -> (String, serde_json::Value) {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    let value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
    (text, value)
}

#[test]
fn a_viewer_reads_the_withdrawn_venues_with_the_cluster_each_was_withdrawn_on() -> Result<()> {
    // The premise for everything below: the surface exists, at the roles the
    // table declares, and a platform that has withdrawn nothing says so as an
    // observed zero rather than as an error.
    let rig = rig()?;
    let list = ROUTES
        .iter()
        .find(|route| route.method == Method::Get && route.pattern == LIST_PATH)
        .expect("the list route is in the table");
    assert_eq!(list.required_role, Role::Viewer);
    let sign = ROUTES
        .iter()
        .find(|route| route.method == Method::Post && route.pattern == SIGN_PATTERN)
        .expect("the signature route is in the table");
    assert_eq!(sign.required_role, Role::Operator);

    let (_, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(view["withdrawn"].as_array().map(Vec::len), Some(0));
    assert_eq!(view["withdrawals_recorded"], 0);

    // And once the platform has withdrawn one, the row carries the evidence
    // the review made the finding on. A list that named the venue and not the
    // cluster would tell an operator that something happened and give them no
    // way to judge whether it should have.
    rig.withdraw_the_desk_venue()?;
    let (text, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(view["withdrawals_recorded"], 1, "{text}");
    let rows = view["withdrawn"].as_array().expect("an array");
    assert_eq!(rows.len(), 1, "{text}");
    assert_eq!(rows[0]["venue"], "simulated-venue");
    assert_eq!(rows[0]["awaiting_countersignature"], false);
    assert_eq!(rows[0]["withdrawal"]["constraint"], "feasibility_lot");
    assert_eq!(rows[0]["withdrawal"]["count"], 10);
    assert_eq!(rows[0]["withdrawal"]["sample"], 10);
    // The route a signature goes to, served beside the list rather than left
    // to a runbook the reader may not have open.
    assert_eq!(
        view["reinstatement_path"],
        "/api/v1/venues/:venue/reinstatements"
    );

    // A viewer cannot sign: the table says operator and the router enforces
    // it before the body is read.
    let refused = rig.call(Method::Post, SIGN_PATH, VIEWER_TOKEN, GOOD_BODY);
    assert_eq!(refused.status, 403);
    Ok(())
}

#[test]
fn the_approver_of_a_reinstatement_is_the_session_and_never_the_body() -> Result<()> {
    // The failure this prevents: a body that names its approver turns an
    // approval into a claim to have been approved, and the record that was
    // the control becomes the alibi. The route refuses the key by position —
    // never by echoing it — and does so before the kernel sees the request,
    // so nothing is journaled.
    let rig = rig()?;
    rig.withdraw_the_desk_venue()?;

    let with_approver = r#"{"rationale": "the grid was re-read", "approver": "someone-else"}"#;
    let response = rig.call(Method::Post, SIGN_PATH, OPERATOR_TOKEN, with_approver);
    assert_eq!(response.status, 400);
    let (text, body) = body_of(response);
    let error = body["error"].as_str().expect("an error string");
    // By position and never by name: `serde_json` sorts an object's keys, so
    // the position is the sorted one, and what matters is that the key is
    // located without being repeated.
    assert!(
        error.contains("key at position 1"),
        "the refusal does not name the offending key by position: {text}"
    );
    assert!(
        error.contains("approver cannot be sent"),
        "the refusal does not say the approver is the session's: {text}"
    );
    assert!(
        !text.contains("someone-else"),
        "the refusal echoed the value the caller sent: {text}"
    );

    // And a venue in the body is refused too, because the venue is the path
    // segment and must be one the platform itself withdrew.
    let with_venue = r#"{"rationale": "the grid was re-read", "venue": "XZZZ"}"#;
    let response = rig.call(Method::Post, SIGN_PATH, OPERATOR_TOKEN, with_venue);
    assert_eq!(response.status, 400);
    let (text, _) = body_of(response);
    assert!(
        !text.contains("XZZZ"),
        "the refusal echoed the body: {text}"
    );

    // Nothing reached the kernel: no signature stands against the venue.
    let (_, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(view["withdrawn"][0]["awaiting_countersignature"], false);
    Ok(())
}

#[test]
fn a_reinstatement_is_refused_before_the_kernel_is_asked_because_nothing_attests_a_person()
-> Result<()> {
    // This test used to be called
    // `no_venue_the_platform_did_not_withdraw_can_be_reinstated` and asserted
    // the kernel's 404 for a venue nothing withdrew. The old assertion was
    // true of the kernel and is no longer reachable over HTTP, and the reason
    // is the point of this change: the route now refuses before the kernel is
    // consulted, because it cannot date an operator identity. A standing
    // bearer token mounted from Secret Manager attests possession and never
    // presence, and the kernel's fifteen-minute window is entitled to a number
    // that means something. The kernel's own not-found remains proven where it
    // always was, against the kernel:
    // `qip-kernel/tests/learning.rs::reinstatement_needs_two_different_fresh_
    // operators_and_is_journaled_at_each_signature`.
    let rig = rig()?;

    // Premise: the route is reachable at the operator role and the credential
    // authenticates, so what follows is this gate and not a 401 or a 403 from
    // the role check. A viewer on the same path answers 403 too, which is why
    // the premise is asserted with a *readable* route rather than by trusting
    // the status code alone.
    let (premise, _) = body_of(rig.call(Method::Get, LIST_PATH, OPERATOR_TOKEN, ""));
    assert!(
        premise.contains("withdrawals_recorded"),
        "the operator credential does not authenticate, so the refusal below proves nothing:          {premise}"
    );

    let response = rig.call(Method::Post, SIGN_PATH, OPERATOR_TOKEN, GOOD_BODY);
    assert_eq!(
        response.status,
        403,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, body) = body_of(response);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("a standing bearer token cannot carry it"),
        "the refusal does not say why no instant is available: {text}"
    );
    assert!(
        error.contains("per-request proof of recency"),
        "a refusal must name what would be required instead: {text}"
    );

    // And the refusal does not echo the caller's path segment. The old
    // not-found refusal was careful about this and the new one inherits the
    // care: a venue name a caller invented, repeated back, is a reflection
    // this route has no reason to offer.
    let response = rig.call(
        Method::Post,
        "/venues/XZZZ/reinstatements",
        OPERATOR_TOKEN,
        GOOD_BODY,
    );
    assert_eq!(response.status, 403);
    let (text, _) = body_of(response);
    assert!(
        !text.contains("XZZZ"),
        "the refusal echoed the path: {text}"
    );
    Ok(())
}

#[test]
fn no_pair_of_operators_can_put_a_venue_back_while_the_credential_attests_nobody() -> Result<()> {
    // This test used to be
    // `one_operator_signing_twice_is_one_person_and_two_operators_put_the_venue_
    // _back` and drove the whole dual-signature flow over HTTP: first
    // signature held, same credential refused as one person, second credential
    // completes, venue trades again. Every one of those assertions was true of
    // the kernel and *none* of them was true of a deployment, for two
    // independent reasons that this fixture hid.
    //
    // The first is the one this change fixes. The route dated the operator
    // identity at the credential record's minting instant, which for a
    // standing secret is when the process started; the fixture minted its
    // credentials at `start()` and drove at `start()`, so the kernel's
    // fifteen-minute window computed an age of zero and passed. In the shipped
    // binary that same window measured the pod's uptime: any copy of the token,
    // however old, passed for fifteen minutes after a restart, and the operator
    // actually at the keyboard was refused for ever after. The fixture proved
    // a property of the fixture.
    //
    // The second is why refusing costs nothing that worked. This rig holds two
    // operator credentials with *different* subjects. A deployment holds one:
    // the composition root mints `format!("{}@env", role.as_str())`, so both
    // humans sharing `QIP_TOKEN_OPERATOR` present `operator@env` and the
    // kernel refuses the countersignature as one person signing twice. ADR
    // 0062 Amendment B records that; ADR 0065 records this.
    //
    // So what is asserted here is the refusal, from both subjects, with the
    // venue still withdrawn and no signature held. The kernel's countersignature
    // arithmetic — held first signature, refused same subject, completed pair —
    // is proven against the kernel in
    // `qip-kernel/tests/learning.rs::reinstatement_needs_two_different_fresh_
    // operators_and_is_journaled_at_each_signature`, which is where it can be
    // proven honestly.
    let rig = rig()?;
    rig.withdraw_the_desk_venue()?;

    // Premise: there is a withdrawal to sign. Without this the refusals below
    // would be indistinguishable from a route with nothing to act on.
    let (text, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(
        view["withdrawn"].as_array().map(Vec::len),
        Some(1),
        "{text}"
    );
    assert_eq!(view["withdrawn"][0]["awaiting_countersignature"], false);

    for (label, token) in [("first", OPERATOR_TOKEN), ("second", SECOND_OPERATOR_TOKEN)] {
        let response = rig.call(Method::Post, SIGN_PATH, token, GOOD_BODY);
        assert_eq!(
            response.status,
            403,
            "the {label} operator signed on a credential that attests nobody: {}",
            String::from_utf8_lossy(&response.body)
        );
        let (text, body) = body_of(response);
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|error| error.contains("a standing bearer token cannot carry it")),
            "the {label} refusal is not this gate: {text}"
        );
    }

    // Nothing was held and nothing moved: no first signature stands, so a
    // later change that made the second signature complete a pair the platform
    // never recorded would fail here rather than in production.
    let (text, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(
        view["withdrawn"].as_array().map(Vec::len),
        Some(1),
        "{text}"
    );
    assert_eq!(
        view["withdrawn"][0]["awaiting_countersignature"], false,
        "a refused signature was held anyway: {text}"
    );

    // And the platform's own set still holds it, read off the platform rather
    // than off the view, so a list that had merely stopped rendering the row
    // would not pass for a venue still withdrawn.
    let platform = rig.platform.lock().expect("the platform lock");
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"],
        "two refused signatures put the venue back anyway"
    );
    Ok(())
}

#[test]
fn a_reinstatement_path_served_to_an_operator_is_a_path_the_router_matches() -> Result<()> {
    // The failure this prevents, named in `venue_views.rs`'s own comment and
    // not prevented by it: "a path served to an operator and a path the router
    // matches that disagreed would send the person holding the second
    // signature to a 404 in the middle of recovering a venue". The comment
    // claimed one constant was read by the view and by the route table. It was
    // not — the table had its own literal, the handler arm a third, this file a
    // fourth, and the `/api/v1` half was hand-typed rather than taken from
    // `VERSION_PREFIX` — so a rename would have produced exactly that 404 with
    // the comment still asserting it could not.
    //
    // Asserted by *driving* the served string rather than comparing it to a
    // copy. A comparison against a constant this file also imports would pass
    // for ever if both moved together, which is the trap the testing rules
    // name.
    let rig = rig()?;
    let (text, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    let served = view["reinstatement_path"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Premise: something was served, and it is a full path rather than a bare
    // pattern. A blank string would satisfy every `strip_prefix` below.
    assert!(
        served.starts_with("/api/v1/") && served.len() > "/api/v1/".len(),
        "no usable reinstatement path was served: {text}"
    );

    // The router's own table names it, under the prefix the router strips.
    let pattern = served
        .strip_prefix(qip_api::routes::VERSION_PREFIX)
        .expect("the served path sits under the version prefix");
    assert!(
        ROUTES
            .iter()
            .any(|route| route.method == Method::Post && route.pattern == pattern),
        "the path served to an operator, {served}, is not a POST route the table declares"
    );

    // And the router actually matches it, with the parameter filled in as an
    // operator would fill it.
    //
    // Asserted on the router's *own* refusal body rather than on a status
    // code, and the distinction is not pedantry: an unmatched path and a venue
    // the platform never withdrew are both 404s, so a status-code assertion
    // would pass or fail for either reason and tell the reader nothing about
    // which. `{"error":"no such route"}` is the one answer only the router
    // produces.
    const UNMATCHED: &str = "no such route";
    let concrete = pattern.replace(":venue", "simulated-venue");
    let response = rig.call(Method::Post, &concrete, OPERATOR_TOKEN, GOOD_BODY);
    let served_answer = String::from_utf8_lossy(&response.body).into_owned();
    assert!(
        !served_answer.contains(UNMATCHED),
        "the path the list serves is a path the router does not match: {served_answer}"
    );

    // The other half, without which the line above would pass against a
    // router that matched everything: a path this table does not declare is
    // refused as unmatched, by the same call through the same rig.
    let nonsense = concrete.replace("reinstatements", "reinstatement");
    let missing = rig.call(Method::Post, &nonsense, OPERATOR_TOKEN, GOOD_BODY);
    assert_eq!(missing.status, 404);
    assert!(
        String::from_utf8_lossy(&missing.body).contains(UNMATCHED),
        "a path the table does not declare was not refused as unmatched, so the assertion above \
         proves nothing: {}",
        String::from_utf8_lossy(&missing.body)
    );
    Ok(())
}
