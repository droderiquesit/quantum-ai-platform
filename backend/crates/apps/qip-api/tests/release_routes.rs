//! `GET /release/:plane` — one plane's release standing, served to the Kargo
//! promotion out of a stage (ADR 0504, R12-OBS-9156).
//!
//! The requirement's bar is "a spent budget refuses a release for that plane
//! only". Each half is asserted against a platform driven into the state, not
//! against a fixture that asserts it: the netting-ratio objective (plane
//! `edge`) is spent by ingesting the same twenty thin cell reports the
//! kernel's own `self_observation` test uses, through
//! `Platform::ingest_cell_report`, which is the seam production reaches.
//!
//! The premise of every test is asserted first. A route that answered 200 to
//! everything would pass "a healthy plane proceeds" whether or not it read
//! anything, so each test that expects a 200 also drives or checks a state in
//! which the same route answers something else.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::{Api, ROUTES};
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ManualClock, ObjectId, dec};
use qip_financial::universe::Universe;
use qip_kernel::central::CellReport;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const MONITOR_TOKEN: &str = "monitor-token";
const CELL: &str = "cell-lon-1";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
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
        LimitSet::new("release-route-test"),
    )?;
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "promotion@example.com",
        Role::Monitor,
        MONITOR_TOKEN.to_string(),
        start(),
        start().saturating_add(Duration::from_days(30)),
    )]));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    Ok(Rig {
        api: Api::new(platform.clone(), authenticator, rate_limiter, clock),
        platform,
    })
}

impl Rig {
    fn get(&self, path: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {MONITOR_TOKEN}"),
        );
        self.api.handle(&Request {
            method: Method::Get,
            path: format!("/api/v1{path}"),
            query: BTreeMap::new(),
            headers,
            body: Vec::new(),
            peer: "127.0.0.1:1".to_string(),
        })
    }

    /// Spend the `edge` plane's netting-ratio budget: twenty observations,
    /// every one a miss, is past `SloStatus::is_page_worthy`'s bar.
    fn spend_the_edge_budget(&self) -> Result<()> {
        let mut platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        for _ in 0..20 {
            platform.ingest_cell_report(thin_report(), start())?;
        }
        Ok(())
    }
}

/// Gross 120 over a net of 100: 1.2, under §49.1's 1.5 netting floor.
fn thin_report() -> CellReport {
    let contributors = ["100", "20", "-20"]
        .iter()
        .enumerate()
        .map(|(index, size)| qip_contracts::intent::Contributor {
            strategy: StrategyId::new(format!("contributor-{index}")),
            signed_size: qip_core::Decimal::parse(size).unwrap_or(qip_core::Decimal::ZERO),
            inputs: Vec::new(),
        })
        .collect();
    CellReport::new(CELL, start()).with_orders(vec![DeltaOrder {
        order_id: "ord-thin".to_string(),
        strategy: StrategyId::new("objectives-strategy"),
        object_id: ObjectId::from_string("AAA"),
        venue: VenueId::new("XNYS"),
        side: BookSide::Bid,
        quantity: dec!("100"),
        price: dec!("100"),
        simulated: true,
        contributors,
    }])
}

fn body_of(response: &Response) -> serde_json::Value {
    let text = String::from_utf8(response.body.clone()).expect("a UTF-8 body");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

#[test]
fn the_release_route_is_declared_read_only_in_the_table() {
    let route = ROUTES
        .iter()
        .find(|route| route.pattern == "/release/:plane")
        .expect("ROUTES declares the release-standing route");
    assert_eq!(
        route.method,
        Method::Get,
        "a standing is read, never written"
    );
    assert_eq!(route.required_role, Role::Monitor);
}

/// The requirement's bar, both halves, on one platform.
#[test]
fn a_spent_budget_refuses_its_own_planes_release_and_no_other_planes() -> Result<()> {
    let rig = rig()?;

    let before = rig.get("/release/edge");
    assert_eq!(
        before.status, 200,
        "premise: a platform that measured nothing refuses nothing"
    );
    assert_eq!(body_of(&before)["decision"], "proceed");
    assert_eq!(
        body_of(&before)["observed_objectives"],
        0,
        "premise: and says it rests on no observation"
    );

    rig.spend_the_edge_budget()?;

    let edge = rig.get("/release/edge");
    assert_eq!(
        edge.status, 409,
        "a spent budget refuses its plane's release"
    );
    let body = body_of(&edge);
    assert_eq!(body["decision"], "refused");
    assert_eq!(body["plane"], "edge");
    let reason = body["reason"].as_str().expect("a refusal carries a reason");
    assert!(
        reason.contains("netting-ratio"),
        "the refusal names the objective whose budget is gone: {reason}"
    );
    assert!(
        body["observed_objectives"].as_u64().unwrap_or(0) >= 1,
        "the refusal rests on an observation: {body}"
    );

    // "That plane only": every other declared plane still proceeds.
    for plane in ["kernel", "edge-node", "strategy", "portfolio"] {
        let other = rig.get(&format!("/release/{plane}"));
        assert_eq!(
            other.status,
            200,
            "`{plane}` is not refused by `edge`'s budget: {:?}",
            body_of(&other)
        );
        assert_eq!(body_of(&other)["decision"], "proceed");
    }
    Ok(())
}

/// An undeclared plane is refused, not waved through. `release_gate` alone
/// answers `Proceed` for a name it has no objective on, so a promotion step
/// with a typo in its plane would read a green light for ever.
#[test]
fn a_plane_no_objective_is_declared_on_is_refused_and_not_waved_through() -> Result<()> {
    let rig = rig()?;
    rig.spend_the_edge_budget()?;
    assert_eq!(
        rig.get("/release/edge").status,
        409,
        "premise: the real plane is refused"
    );
    let typo = rig.get("/release/edges");
    assert_eq!(typo.status, 404, "an undeclared plane has no standing");
    let error = body_of(&typo)["error"]
        .as_str()
        .expect("the refusal says why")
        .to_string();
    assert!(
        error.contains("`edges`") && error.contains("edge-node"),
        "the refusal names the plane asked for and the planes that exist: {error}"
    );
    Ok(())
}

/// The route requires a credential: a promotion step that could read a
/// standing anonymously could also be fed one by anybody who can answer on
/// its address.
#[test]
fn the_release_route_refuses_an_unauthenticated_caller() -> Result<()> {
    let rig = rig()?;
    assert_eq!(
        rig.get("/release/edge").status,
        200,
        "premise: the authenticated caller is served"
    );
    let anonymous = rig.api.handle(&Request {
        method: Method::Get,
        path: "/api/v1/release/edge".to_string(),
        query: BTreeMap::new(),
        headers: BTreeMap::new(),
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    });
    assert_eq!(anonymous.status, 401);
    Ok(())
}
