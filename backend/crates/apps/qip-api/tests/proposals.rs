//! `GET /proposals` — the decision record behind each proposal.
//!
//! What this file guards is that the route reports what the DECIDE stage
//! actually recorded, rather than a summary of it. The failure it exists to
//! prevent has already happened: the route served a status *word* and dropped
//! the reason beside it, so a console could say a proposal was `vetoed` and
//! could not say by which control or why. Blueprint §40.2's "why not the
//! obvious trade?" was scored against the console for an answer the platform
//! had already written down and the wire threw away.
//!
//! Every test asserts its premise first — that a cycle ran and left a
//! proposal to read — so a missing field below is a statement about the route
//! and not about a platform that proposed nothing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

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
const PATH: &str = "/proposals";

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

    /// Feed the loop a real tape, then run it until it stages a proposal that
    /// actually has legs.
    ///
    /// The premise every test here needs, and it is not a formality: the
    /// first version of this file ran a cycle against an empty universe, got
    /// a proposal with zero legs, and every per-leg assertion below would
    /// have passed over an empty list. A platform that observed nothing has
    /// nothing to explain.
    fn proposals_after_a_cycle(&self) -> Result<Vec<serde_json::Value>> {
        {
            let mut platform = self
                .platform
                .lock()
                .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
            platform.observe(tape("AAA"));
        }
        for _ in 0..6 {
            let ran = self.call(Method::Post, "/cycle", ANALYST_TOKEN);
            assert_eq!(ran.status, 202, "the cycle did not run: {ran:?}");
            let body = body_of(self.call(Method::Get, PATH, VIEWER_TOKEN));
            let staged = body["proposals"]
                .as_array()
                .cloned()
                .unwrap_or_else(Vec::new);
            let with_legs = staged.iter().any(|proposal| {
                !proposal["leg_detail"]
                    .as_array()
                    .is_none_or(|l| l.is_empty())
            });
            if with_legs {
                return Ok(staged);
            }
        }
        panic!("six cycles staged no proposal with a leg; these tests would prove nothing");
    }
}

fn body_of(response: Response) -> serde_json::Value {
    assert_eq!(response.status, 200, "unexpected status");
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

#[test]
fn the_proposals_route_is_a_viewer_read() -> Result<()> {
    // The premise for every test below: the route is in the table, at the
    // role the tests call it with.
    let route = ROUTES
        .iter()
        .find(|route| route.method == Method::Get && route.pattern == PATH)
        .expect("the proposals route is in the table");
    assert_eq!(route.required_role, Role::Viewer);
    Ok(())
}

#[test]
fn a_proposal_reports_the_weights_the_sizing_moved_between_rather_than_a_leg_count() -> Result<()> {
    let rig = rig()?;
    let staged = rig.proposals_after_a_cycle()?;
    let first = staged
        .iter()
        .find(|proposal| {
            !proposal["leg_detail"]
                .as_array()
                .is_none_or(|l| l.is_empty())
        })
        .ok_or_else(|| Error::not_found("a proposal with legs"))?;

    // The premise: the proposal has legs. A proposal with none would satisfy
    // every assertion below vacuously.
    let legs = first["leg_detail"]
        .as_array()
        .expect("leg_detail is an array");
    assert!(
        !legs.is_empty(),
        "the proposal staged no legs; this test proves nothing"
    );

    for leg in legs {
        // The sizing question the blueprint asks is "why this size", and the
        // answer is a movement, not a destination: a target weight alone
        // cannot say whether the platform added to a position or cut it.
        assert!(
            leg["current_weight"].is_number(),
            "no current weight on {leg}"
        );
        assert!(
            leg["target_weight"].is_number(),
            "no target weight on {leg}"
        );
        assert!(
            leg["weight_change"].is_number(),
            "no weight change on {leg}"
        );
        // Money is exact and crosses the wire as a string. A float here would
        // be a cent lost in every consumer that parsed it.
        assert!(
            leg["reference_price"].is_string(),
            "the reference price is not a string on {leg}"
        );
        assert!(
            leg["quantity"].is_string(),
            "the quantity is not a string on {leg}"
        );
        assert!(leg["instrument"].is_string(), "no instrument on {leg}");
    }
    Ok(())
}

#[test]
fn a_proposal_carries_the_decision_record_and_not_only_the_status_word() -> Result<()> {
    let rig = rig()?;
    let staged = rig.proposals_after_a_cycle()?;
    let first = &staged[0];

    // The premise: the status word is still served, so the assertions below
    // are about what was added beside it rather than about a renamed field.
    assert!(
        first["status"].is_string(),
        "the status word was dropped: {first}"
    );

    // The decision detail. A draft has no instant and nobody to name, and
    // serialises as its discriminant alone; every other status carries the
    // instant it was decided at, which is what makes a refusal reviewable.
    let decision = &first["decision"];
    assert!(
        decision["status"].is_string(),
        "the decision carries no discriminant: {decision}"
    );
    assert_eq!(
        decision["status"], first["status"],
        "the decision's status disagrees with the proposal's: {first}"
    );
    if decision["status"] != "draft" {
        assert!(
            decision["at"].is_string(),
            "a decided proposal does not say when: {decision}"
        );
    }

    // Equity is money. It is the denominator every weight above is a fraction
    // of, so a consumer that parsed it as a float would be sizing against a
    // rounded book.
    assert!(
        first["equity"].is_string(),
        "equity is not a string: {first}"
    );
    // The reasoning the DECIDE stage recorded and the route used to drop.
    assert!(first["compromises"].is_array(), "no compromises on {first}");
    assert!(
        first["checks_passed"].is_array(),
        "no checks_passed on {first}"
    );
    assert!(first["as_of"].is_string(), "no as_of on {first}");
    assert!(
        first["estimated_cost_bps"].is_number(),
        "no estimated cost on {first}"
    );
    Ok(())
}
