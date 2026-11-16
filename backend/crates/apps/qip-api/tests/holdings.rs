//! `GET /portfolio/holdings` — each held position's valuation confidence,
//! investment horizon and expected liquidity (R12-ASSET-4067).
//!
//! The record existed in the kernel and nothing served it, so the desk could
//! not see why any position was held. The premise every test asserts first is
//! that the book holds a position, so an empty list below is a statement
//! about the route and not about a book that holds nothing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::{Api, ROUTES};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The liquidity every fixture here states, because nothing states it for
/// them. `LiquidityProfile` has no `Default` on purpose: `MinLiquidity` and
/// `MaxDaysToLiquidate` are controls whose job is to veto, and a fixture may
/// state its own premise but may not inherit one nobody wrote down.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB", "ZZZ"] {
        universe.insert(
            FinancialObject::builder(
                object(symbol),
                symbol,
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())?,
        )?;
    }
    Ok(universe)
}

/// Limits wide enough that a thesis is sized rather than refused — the
/// kernel's own learning fixture's. A book that refused everything would
/// stage a proposal with no legs, and the leg assertions below would pass
/// vacuously.
fn limits() -> LimitSet {
    LimitSet::new("proposals-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

const TAPE: usize = 120;
const DROP: f64 = -0.09;

/// The kernel's own valuation fixture's tape: quiet noise, then a sharp move
/// on the last bar.
///
/// The move has to be on the *last* bar rather than partway through. A jump
/// the tape has since recovered from is history; a jump the tape ends on is a
/// thesis, and only a thesis is sized into a leg.
fn tape(symbol: &str) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let drop = if i == TAPE - 1 { DROP } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

const ANALYST_TOKEN: &str = "analyst-token";
const VIEWER_TOKEN: &str = "viewer-token";
const PATH: &str = "/portfolio/holdings";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
}

fn rig() -> Result<Rig> {
    // The review floor the kernel's own fixtures use. `ReviewPolicy`'s
    // default floor is tuned for a platform with a history; over a 120-bar
    // synthetic tape nothing survives it, every cycle records "no thesis
    // cleared the action bar", and a route test would then assert against a
    // proposal with no legs — which is how the first draft of this file
    // passed while proving nothing.
    let config = {
        let mut config = PlatformConfig::default();
        // The field is set rather than the struct rebuilt because
        // `ReviewPolicy` is re-exported privately by the kernel's config
        // module, and naming it here would mean a dependency on
        // `qip-reasoning-engine` that this crate has no other reason to hold.
        config.review.minimum_surviving_confidence = 0.10;
        config
    };
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(config, context, Telemetry::silent(), universe()?, limits())?;
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![
        Credential::from_token(
            "analyst@example.com",
            Role::Analyst,
            ANALYST_TOKEN.to_string(),
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
        api: Api::new(platform.clone(), authenticator, rate_limiter, clock),
        platform,
    })
}

impl Rig {
    fn call(&self, method: Method, path: &str, token: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        self.api.handle(&Request {
            method,
            path: format!("/api/v1{path}"),
            query: BTreeMap::new(),
            headers,
            body: Vec::new(),
            peer: "127.0.0.1:1".to_string(),
        })
    }
}

fn body_of(response: Response) -> serde_json::Value {
    assert_eq!(response.status, 200, "unexpected status");
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

#[test]
fn the_holdings_route_is_a_viewer_read() {
    let route = ROUTES
        .iter()
        .find(|route| route.method == Method::Get && route.pattern == PATH)
        .expect("the holdings route is in the table");
    assert_eq!(route.required_role, Role::Viewer);
}

/// The route serves, for each held position, the three figures the kernel's
/// own `position_holding` states, and serves them to a viewer.
///
/// Mutation: in `portfolio_holdings` (qip-api `routes.rs`) drop the
/// `horizon_secs` field — the field assertion fails; return an empty list —
/// the premise assertion fails.
#[test]
fn a_held_position_is_served_with_its_confidence_horizon_and_expected_liquidity() -> Result<()> {
    let rig = rig()?;
    rig.platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?
        .observe(tape("AAA"));
    let mut served = Vec::new();
    for _ in 0..8 {
        let ran = rig.call(Method::Post, "/cycle", ANALYST_TOKEN);
        assert_eq!(ran.status, 202, "the cycle did not run: {ran:?}");
        let body = body_of(rig.call(Method::Get, PATH, VIEWER_TOKEN));
        served = body["holdings"].as_array().cloned().unwrap_or_default();
        if !served.is_empty() {
            break;
        }
    }
    assert!(
        !served.is_empty(),
        "premise: the book holds a position after the cycles, so an empty list proves nothing"
    );
    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    for holding in &served {
        let object = holding["object"].as_str().expect("an object id");
        assert_eq!(holding["stated"], true, "{holding}");
        // Money-like confidence crosses the wire exact, as a string.
        let kernel = platform.position_holding(object, start())?;
        assert_eq!(
            holding["valuation_confidence"].as_str(),
            Some(kernel.valuation_confidence().to_string().as_str()),
            "{holding}"
        );
        assert_eq!(holding["hypothesis"].as_str(), Some(kernel.hypothesis()));
        assert_eq!(
            holding["horizon_secs"].as_i64(),
            Some(kernel.horizon().as_nanos() / 1_000_000_000),
            "{holding}"
        );
        assert!(holding["expected_exit_days"].is_number(), "{holding}");
    }
    Ok(())
}

#[test]
fn the_holdings_route_refuses_a_caller_with_no_credential() -> Result<()> {
    let rig = rig()?;
    let response = rig.call(Method::Get, PATH, "not-a-token");
    assert_eq!(response.status, 401, "{response:?}");
    Ok(())
}
