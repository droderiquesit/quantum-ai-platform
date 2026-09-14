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
fn no_venue_the_platform_did_not_withdraw_can_be_reinstated() -> Result<()> {
    // A good body, a good operator, and a venue nothing withdrew: not found,
    // because a reinstatement signs a withdrawal *the platform made*. The
    // distinction from a 400 matters — the request was well formed; the thing
    // it names does not exist — and the refusal is what stops this route
    // being a way to name a venue into the platform's vocabulary.
    let rig = rig()?;
    let response = rig.call(Method::Post, SIGN_PATH, OPERATOR_TOKEN, GOOD_BODY);
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
            .is_some_and(|error| error.contains("is not withdrawn")),
        "the refusal does not say the venue was never withdrawn: {text}"
    );

    // And an unrelated name is refused the same way, rather than being
    // treated as a venue with no withdrawal to sign.
    let response = rig.call(
        Method::Post,
        "/venues/XZZZ/reinstatements",
        OPERATOR_TOKEN,
        GOOD_BODY,
    );
    assert_eq!(response.status, 404);
    Ok(())
}

#[test]
fn one_operator_signing_twice_is_one_person_and_two_operators_put_the_venue_back() -> Result<()> {
    // The security review's finding, now reachable. Every dual-signature
    // control on this platform rests on `OperatorIdentity::subject()` being
    // durable per human rather than per session: the kernel's countersignature
    // check compares *subjects*, so a subject sourced from anything
    // session- or request-scoped — a token id, a request id — would let one
    // person countersign their own reinstatement by opening a second session,
    // and the check would not notice. Until this route existed there was
    // nothing to exercise that against.
    //
    // Here it is exercised as an operator would hit it: two separate HTTP
    // requests, on the same credential, are two sessions and must be refused
    // as one person; a second credential with a different subject is a second
    // person and completes the reinstatement.
    let rig = rig()?;
    rig.withdraw_the_desk_venue()?;

    // The first signature is held, and the venue stays withdrawn.
    let first = rig.call(Method::Post, SIGN_PATH, OPERATOR_TOKEN, GOOD_BODY);
    assert_eq!(
        first.status,
        200,
        "{}",
        String::from_utf8_lossy(&first.body)
    );
    let (text, entry) = body_of(first);
    assert_eq!(entry["outcome"], "awaiting_countersignature", "{text}");
    assert_eq!(entry["approver"], "operator-one@example.com", "{text}");
    assert!(entry["second_approver"].is_null(), "{text}");
    let (_, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(
        view["withdrawn"].as_array().map(Vec::len),
        Some(1),
        "a single signature reinstated the venue"
    );
    assert_eq!(view["withdrawn"][0]["awaiting_countersignature"], true);

    // The same credential again: a second request, a second session, and the
    // same person. Refused, and the venue is still withdrawn.
    let same_again = rig.call(Method::Post, SIGN_PATH, OPERATOR_TOKEN, GOOD_BODY);
    assert_eq!(
        same_again.status,
        409,
        "a second session of one operator countersigned its own reinstatement: {}",
        String::from_utf8_lossy(&same_again.body)
    );
    let (text, body) = body_of(same_again);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("a second session is not a second person")),
        "{text}"
    );
    let (_, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(
        view["withdrawn"].as_array().map(Vec::len),
        Some(1),
        "the refused countersignature reinstated the venue anyway"
    );

    // A different subject is a different person, so the pair completes and
    // the venue comes back at both seams.
    let second = rig.call(Method::Post, SIGN_PATH, SECOND_OPERATOR_TOKEN, GOOD_BODY);
    assert_eq!(
        second.status,
        200,
        "{}",
        String::from_utf8_lossy(&second.body)
    );
    let (text, entry) = body_of(second);
    assert_eq!(entry["outcome"], "reinstated", "{text}");
    assert_eq!(entry["approver"], "operator-one@example.com", "{text}");
    assert_eq!(
        entry["second_approver"], "operator-two@example.com",
        "{text}"
    );

    let (text, view) = body_of(rig.call(Method::Get, LIST_PATH, VIEWER_TOKEN, ""));
    assert_eq!(
        view["withdrawn"].as_array().map(Vec::len),
        Some(0),
        "two signatures did not put the venue back: {text}"
    );
    // The withdrawal record stays on the log: a venue put back is not a
    // venue that was never withdrawn, and the difference is the whole reason
    // the record exists.
    assert_eq!(view["withdrawals_recorded"], 1, "{text}");

    // And the desk trades there again — the seam the withdrawal actually
    // closed, checked rather than inferred from the list.
    let mut platform = rig.platform.lock().expect("the platform lock");
    let order = platform.order_from(
        object("AAA"),
        qip_execution_engine::order::Side::Buy,
        dec!("10"),
        dec!("100"),
        "prop-after-reinstatement",
        vec!["hyp-after-reinstatement".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;
    Ok(())
}
