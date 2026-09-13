//! The operator interface's risk page renders the limits the platform
//! actually runs under (ADR 0061).
//!
//! The page used to call `LimitSet::conservative_default()` directly and
//! ignore the platform it was handed. That was harmless while every process
//! ran the shipped set, and became a lie the day a limits file could be
//! mounted: a desk that had deployed a signed recalibration would read the
//! old bound on the page and the new one in the refusals.

// The workspace denies `panic_in_result_fn` for production code; in a test
// the assertion is the deliverable and `?` keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request};
use qip_api::web::Web;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ManualClock};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const VIEWER_TOKEN: &str = "viewer-token";
const DESK_LIMIT: &str = "desk-gross-leverage-cap";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn web_running_under(limits: LimitSet) -> Result<Web> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        limits,
    )?;
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "viewer@example.com",
        Role::Viewer,
        VIEWER_TOKEN.to_string(),
        start(),
        start().saturating_add(Duration::from_days(30)),
    )]));
    let rate_limiter = Arc::new(RateLimiter::new(Duration::from_secs(60), 1000));
    Ok(Web::new(
        Arc::new(Mutex::new(platform)),
        authenticator,
        rate_limiter,
        clock,
    ))
}

fn risk_page(web: &Web) -> String {
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        format!("Bearer {VIEWER_TOKEN}"),
    );
    let response = web.handle(&Request {
        method: Method::Get,
        path: "/risk".to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    });
    assert_eq!(response.status, 200, "the risk page did not render");
    String::from_utf8(response.body).expect("UTF-8")
}

#[test]
fn the_risk_page_lists_the_limits_the_platform_actually_runs_under() -> Result<()> {
    // The premise: a platform on the shipped set renders the shipped names,
    // so the assertion below is about which set is read and not about a
    // page that lists nothing.
    let shipped = web_running_under(LimitSet::conservative_default())?;
    let page = risk_page(&shipped);
    assert!(
        page.contains("order-notional"),
        "the shipped set's first limit is not on the page: {page}"
    );
    assert!(!page.contains(DESK_LIMIT));

    // A desk running under its own set sees its own names and not the
    // shipped ones.
    let desk = web_running_under(
        LimitSet::new("desk-set").with(
            Limit::new(DESK_LIMIT, LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("the desk's own gross cap"),
        ),
    )?;
    let page = risk_page(&desk);
    assert!(
        page.contains(DESK_LIMIT),
        "the page does not list the limit the platform runs under: {page}"
    );
    assert!(
        !page.contains("order-notional"),
        "the page lists a shipped limit the platform does not run under: {page}"
    );
    Ok(())
}
