//! The treasury read surface: `/ledger/users`, `/wallet`, `/corridors` and
//! `/transfer-gate`, against the contract in `ROUTES-LEDGER.md`.
//!
//! Every test asserts its premise before the property. A route that serves
//! an empty list passes every shape assertion whether or not the platform
//! ever held the fact, so where a row is asserted the platform is driven to
//! produce one first — a fill is ingested, a strategy is registered, the
//! switch is tripped — and the state before is checked to be the other one.
//!
//! The property that matters most is the one the ADR fixes: no body ever
//! carries a granted withdrawal, and no method but `GET` reaches any of the
//! four paths. The first is read from the type's own serialisation, so the
//! day someone adds the arm ADR 0021 refuses, the flag flips and the test
//! fires; the second is the boundary `api_boundary.rs` pins from the other
//! side.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::ledger_views::{
    EVALUATED_AS_ROLE, GATE_NOTE, NO_CORRIDOR_REGISTRY, NO_DESTINATION_ALLOWLIST, NO_PRODUCTS,
    NO_WALLET, POSTURE,
};
use qip_api::routes::{Api, ROUTES};
use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::{FillRecord, FillShare};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::central::{CellReport, StrategyCandidate};
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_lifecycle::trials::StrategyFamily;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// --- fixtures ---------------------------------------------------------------

const VIEWER_TOKEN: &str = "viewer-token";
const MONITOR_TOKEN: &str = "monitor-token";
const CELL: &str = "cell-lon-1";
const INSTRUMENT: &str = "obj-AAA";

/// The four paths under test, as the route table spells them.
const TREASURY_PATHS: [&str; 4] = ["/ledger/users", "/wallet", "/corridors", "/transfer-gate"];

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(INSTRUMENT),
            "AAA",
            InstrumentType::CommonStock,
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn limits() -> LimitSet {
    LimitSet::new("ledger-routes-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
}

fn rig() -> Result<Rig> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(config, context, Telemetry::silent(), universe()?, limits())?;
    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![
        Credential::from_token(
            "viewer@example.com",
            Role::Viewer,
            VIEWER_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "monitor@example.com",
            Role::Monitor,
            MONITOR_TOKEN.to_string(),
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
    fn with_platform<T>(&self, f: impl FnOnce(&mut Platform) -> T) -> Result<T> {
        let mut platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        Ok(f(&mut platform))
    }

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

    fn get(&self, path: &str) -> Response {
        self.call(Method::Get, path, VIEWER_TOKEN)
    }
}

fn body_of(response: Response) -> (String, serde_json::Value) {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    let value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
    (text, value)
}

/// A compiled strategy the factory will accept, so a family is registered
/// and an entitlement has a product to be evaluated against.
fn compile(id: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = ObjectId::from_string(INSTRUMENT);
    let pressure =
        qip_contracts::feature::FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(StrategyId::new(id), subject, Duration::from_millis(250))
        .with_rule(Rule::new(
            "enter",
            qip_contracts::signal::SignalKind::Enter,
            Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
            Expr::Exact(Decimal::from_int(100)),
            Expr::Statistic(0.62),
            500,
        ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn register_family(rig: &Rig, family: &str) -> Result<()> {
    rig.with_platform(|platform| -> Result<()> {
        let (compiled, program) = compile("AAA")?;
        let candidate = StrategyCandidate::new(
            compiled,
            program,
            StrategyFamily::new(family)?,
            "london-1",
            VenueId::new("XNYS"),
            start(),
        )?;
        platform.central_mut().factory_mut().register(candidate)?;
        Ok(())
    })?
}

/// One order sent and filled whole for `alpha`, as a cell reports it — the
/// kernel's own ledger fixture, because the only road into a user's book is
/// a report the centre accepted.
fn report(order_id: &str, side: BookSide, quantity: Decimal, price: Decimal) -> CellReport {
    let strategy = StrategyId::new("alpha");
    let order = DeltaOrder {
        order_id: order_id.to_string(),
        strategy: strategy.clone(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        side,
        quantity,
        price,
        simulated: true,
        contributors: vec![Contributor {
            strategy: strategy.clone(),
            signed_size: quantity,
            inputs: vec![("alpha-feature".to_string(), 1)],
        }],
    };
    let fill = FillRecord {
        order_id: order_id.to_string(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: VenueId::new("XNYS"),
        side,
        quantity,
        price,
        simulated: true,
        at: start(),
        shares: vec![FillShare { strategy, quantity }],
    };
    CellReport::new(CELL, start())
        .with_orders(vec![order])
        .with_fills(vec![fill])
}

/// The keys `ROUTES-LEDGER.md` promises at the top level of each body.
fn documented_keys(path: &str) -> &'static [&'static str] {
    match path {
        "/ledger/users" => &[
            "posture",
            "served_at",
            "evaluated_as_role",
            "products",
            "fills_journalled",
            "users",
        ],
        "/wallet" => &[
            "posture",
            "served_at",
            "assembled",
            "reason",
            "as_of",
            "holdings",
            "reconciliation",
        ],
        "/corridors" => &["posture", "served_at", "corridors", "destinations"],
        "/transfer-gate" => &[
            "posture",
            "served_at",
            "checks",
            "last_assessment",
            "kill_switch",
            "executes",
            "note",
        ],
        other => panic!("{other} is not a treasury path"),
    }
}

// --- the surface as a whole -------------------------------------------------

#[test]
fn every_treasury_route_answers_a_viewer_with_the_documented_keys_and_the_posture_literal()
-> Result<()> {
    // The failure this guards: a page built against ROUTES-LEDGER.md renders
    // blank because a key was renamed on one side, or a body reaches the
    // browser without the posture a viewer must see.
    let rig = rig()?;
    for path in TREASURY_PATHS {
        // Premise: the path is in the route table at the viewer role, as a
        // GET, so what is answered below is the route and not a 404.
        let route = ROUTES
            .iter()
            .find(|route| route.pattern == path)
            .unwrap_or_else(|| panic!("{path} is not in ROUTES"));
        assert_eq!(route.method, Method::Get, "{path}");
        assert_eq!(route.required_role, Role::Viewer, "{path}");
        assert_eq!(route.success, 200, "{path}");

        let response = rig.get(path);
        assert_eq!(response.status, 200, "{path}");
        let (text, body) = body_of(response);
        let object = body
            .as_object()
            .unwrap_or_else(|| panic!("{path} did not answer an object: {text}"));
        for key in documented_keys(path) {
            assert!(
                object.contains_key(*key),
                "{path} lacks the documented key {key}: {text}"
            );
        }
        // The literal, exactly, and first: a page renders what it is handed.
        assert_eq!(
            body["posture"],
            serde_json::json!(POSTURE),
            "{path}: {text}"
        );
        assert_eq!(POSTURE, "PAPER TRADING");
        assert!(
            text.starts_with(r#"{"posture":"PAPER TRADING""#),
            "{path} does not lead with the posture: {text}"
        );
        // ISO 8601, with the zone, at the instant the rig's clock holds.
        assert_eq!(
            body["served_at"],
            serde_json::json!(start().to_rfc3339()),
            "{path}: {text}"
        );
    }
    Ok(())
}

#[test]
fn no_method_but_get_reaches_any_treasury_path() -> Result<()> {
    // The failure this guards: a route that could submit, approve or move
    // something appearing under a treasury path. The table refuses by
    // method before authentication, so a viewer and a monitor read the same
    // 405; and the boundary suite pins the mutating set to the three that
    // already exist, so this checks the same fact from the server's side.
    let rig = rig()?;
    for path in TREASURY_PATHS {
        // Premise: GET is admitted, so the 405 below is about the method.
        assert_eq!(rig.get(path).status, 200, "{path}");
        for method in [Method::Post, Method::Put, Method::Delete] {
            let response = rig.call(method, path, VIEWER_TOKEN);
            assert_eq!(response.status, 405, "{method:?} {path}");
            let (text, body) = body_of(response);
            assert_eq!(
                body["error"],
                serde_json::json!("that method is not allowed here"),
                "{method:?} {path}: {text}"
            );
        }
    }
    // And the table agrees: nothing under these paths is anything but GET.
    for route in ROUTES {
        if TREASURY_PATHS.contains(&route.pattern) {
            assert_eq!(route.method, Method::Get, "{}", route.pattern);
        }
    }
    Ok(())
}

#[test]
fn a_monitor_credential_is_below_the_viewer_role_and_is_refused() -> Result<()> {
    // The failure this guards: a scrape credential, which holds no
    // portfolio authority, reading a user's mandate and balances.
    let rig = rig()?;
    // Premise: the same token reaches the one route a monitor may read.
    assert_eq!(rig.call(Method::Get, "/health", MONITOR_TOKEN).status, 200);
    for path in TREASURY_PATHS {
        assert_eq!(
            rig.call(Method::Get, path, MONITOR_TOKEN).status,
            403,
            "{path}"
        );
    }
    Ok(())
}

// --- /ledger/users ----------------------------------------------------------

#[test]
fn the_desk_is_enrolled_with_its_mandate_and_no_balance_until_a_fill_is_booked() -> Result<()> {
    // The failure this guards: a balance row rendered at zero for a book
    // that does not exist, indistinguishable from a book that holds zero.
    let rig = rig()?;
    let initial_equity = rig.with_platform(|platform| platform.config().initial_equity)?;
    let (text, body) = body_of(rig.get("/ledger/users"));
    let users = body["users"].as_array().expect("a list");
    assert_eq!(users.len(), 1, "{text}");
    let desk = &users[0];
    assert_eq!(desk["user_id"], serde_json::json!("desk"), "{text}");
    let mandate = &desk["mandate"];
    // Money as the platform's own decimal text, not a JSON number.
    assert_eq!(
        mandate["capital"],
        serde_json::json!(initial_equity.to_string()),
        "{text}"
    );
    assert!(mandate["capital"].is_string(), "{text}");
    assert_eq!(mandate["currency"], serde_json::json!("USD"), "{text}");
    assert_eq!(mandate["liquidity_floor"], serde_json::json!("0"), "{text}");
    assert_eq!(
        mandate["exploration_share"],
        serde_json::json!("0"),
        "{text}"
    );
    assert_eq!(mandate["jurisdiction"], serde_json::json!("ZZ"), "{text}");
    assert_eq!(
        mandate["investable"],
        serde_json::json!(initial_equity.to_string()),
        "{text}"
    );
    assert_eq!(
        mandate["permitted_families"],
        serde_json::json!({"any": true, "families": []}),
        "{text}"
    );
    assert_eq!(desk["balances"], serde_json::json!([]), "{text}");
    assert_eq!(body["fills_journalled"], serde_json::json!(0), "{text}");
    Ok(())
}

#[test]
fn a_fill_the_centre_settles_appears_as_the_desks_balance_with_inflows_kept_apart() -> Result<()> {
    // The failure this guards: the route serving a shape with nothing
    // behind it. A buy at 50 and a sell at 60 realise a thousand, which a
    // route reading a ledger that booked nothing would not show.
    let rig = rig()?;
    let (_, before) = body_of(rig.get("/ledger/users"));
    assert_eq!(before["users"][0]["balances"], serde_json::json!([]));

    let settled = rig.with_platform(|platform| -> Result<usize> {
        let bought = platform.ingest_cell_report(
            report("ord-1", BookSide::Ask, dec!("100"), dec!("50")),
            start(),
        )?;
        let sold = platform.ingest_cell_report(
            report("ord-2", BookSide::Bid, dec!("100"), dec!("60")),
            start(),
        )?;
        Ok(bought.settlement.fills_settled + sold.settlement.fills_settled)
    })??;
    assert_eq!(settled, 2, "the premise is two settled fills");

    let (text, body) = body_of(rig.get("/ledger/users"));
    assert_eq!(body["fills_journalled"], serde_json::json!(2), "{text}");
    let balances = body["users"][0]["balances"].as_array().expect("a list");
    assert_eq!(balances.len(), 1, "{text}");
    let row = &balances[0];
    assert_eq!(row["strategy"], serde_json::json!("alpha"), "{text}");
    assert_eq!(row["currency"], serde_json::json!("USD"), "{text}");
    assert_eq!(row["settled"], serde_json::json!("1000"), "{text}");
    assert_eq!(row["reserved"], serde_json::json!("0"), "{text}");
    assert_eq!(row["available"], serde_json::json!("1000"), "{text}");
    // Expected inflows are a separate figure and a separate list, never
    // folded into `available`.
    assert_eq!(
        row["expected_inflows_total"],
        serde_json::json!("0"),
        "{text}"
    );
    assert_eq!(row["expected_inflows"], serde_json::json!([]), "{text}");
    assert_eq!(row["entries"], serde_json::json!(2), "{text}");
    assert_eq!(
        row["last_entry_at"],
        serde_json::json!(start().to_rfc3339()),
        "{text}"
    );
    Ok(())
}

#[test]
fn an_entitlement_is_evaluated_per_registered_family_and_withdrawal_is_never_granted() -> Result<()>
{
    // The failure this guards is the one ADR 0021 names: a body carrying a
    // granted withdrawal. The flag is read from the type's serialisation,
    // not written, so adding the refused arm would flip it here. Premise
    // first: with no family registered there is no product to evaluate
    // against, and the body says so rather than inventing one.
    let rig = rig()?;
    let (text, before) = body_of(rig.get("/ledger/users"));
    assert_eq!(before["products"], serde_json::json!([]), "{text}");
    assert_eq!(
        before["evaluated_as_role"],
        serde_json::json!(EVALUATED_AS_ROLE),
        "{text}"
    );
    assert_eq!(
        before["users"][0]["entitlements"],
        serde_json::json!([]),
        "{text}"
    );
    assert_eq!(
        before["users"][0]["entitlements_note"],
        serde_json::json!(NO_PRODUCTS),
        "{text}"
    );

    register_family(&rig, "ledger-route-tests")?;

    let (text, body) = body_of(rig.get("/ledger/users"));
    assert_eq!(
        body["products"],
        serde_json::json!(["ledger-route-tests"]),
        "{text}"
    );
    let entitlements = body["users"][0]["entitlements"].as_array().expect("a list");
    assert_eq!(entitlements.len(), 1, "{text}");
    assert_eq!(
        body["users"][0]["entitlements_note"],
        serde_json::Value::Null
    );
    let entitlement = &entitlements[0];
    assert_eq!(
        entitlement["family"],
        serde_json::json!("ledger-route-tests"),
        "{text}"
    );
    assert_eq!(entitlement["role"], serde_json::json!("viewer"), "{text}");
    assert_eq!(
        entitlement["evaluated_at"],
        serde_json::json!(start().to_rfc3339()),
        "{text}"
    );
    // Viewing follows from the mandate.
    assert_eq!(
        entitlement["can_view"]["granted"],
        serde_json::json!(true),
        "{text}"
    );
    assert!(
        entitlement["can_view"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("holds a mandate")),
        "{text}"
    );
    // Investing is refused on the role, which is the first input checked.
    assert_eq!(
        entitlement["can_invest"]["granted"],
        serde_json::json!(false),
        "{text}"
    );
    assert!(
        entitlement["can_invest"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("viewer role")),
        "{text}"
    );
    // Withdrawal: refused, naming the ADR, with no other shape possible.
    assert_eq!(
        entitlement["can_withdraw"]["granted"],
        serde_json::json!(false),
        "{text}"
    );
    assert!(
        entitlement["can_withdraw"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("ADR 0021")),
        "{text}"
    );
    // And nowhere in the body — not under any key — does a granted
    // withdrawal appear. Scanned over the text so a second place the
    // capability might be rendered is caught too.
    assert!(
        !text.contains(r#""can_withdraw":{"granted":true"#),
        "a granted withdrawal reached a body: {text}"
    );
    Ok(())
}

// --- /wallet ----------------------------------------------------------------

#[test]
fn the_wallet_reports_that_none_is_assembled_and_fabricates_no_holding() -> Result<()> {
    // The failure this guards: a wallet panel showing zero holdings and a
    // clean reconciliation for an account nobody has observed.
    let rig = rig()?;
    let (text, body) = body_of(rig.get("/wallet"));
    assert_eq!(body["assembled"], serde_json::json!(false), "{text}");
    assert_eq!(body["reason"], serde_json::json!(NO_WALLET), "{text}");
    assert_eq!(body["as_of"], serde_json::Value::Null, "{text}");
    assert_eq!(body["holdings"], serde_json::json!([]), "{text}");
    assert_eq!(
        body["reconciliation"],
        serde_json::json!({"outcomes": [], "halted_venue_assets": 0}),
        "{text}"
    );
    Ok(())
}

// --- /corridors -------------------------------------------------------------

#[test]
fn the_corridor_and_destination_registries_are_reported_as_not_held() -> Result<()> {
    // The failure this guards: an empty registry — which admits nothing and
    // is a real state — rendered where the truth is that no registry exists.
    let rig = rig()?;
    let (text, body) = body_of(rig.get("/corridors"));
    for (key, reason) in [
        ("corridors", NO_CORRIDOR_REGISTRY),
        ("destinations", NO_DESTINATION_ALLOWLIST),
    ] {
        assert_eq!(body[key]["held"], serde_json::json!(false), "{key}: {text}");
        assert_eq!(
            body[key]["reason"],
            serde_json::json!(reason),
            "{key}: {text}"
        );
        assert_eq!(body[key]["records"], serde_json::json!([]), "{key}: {text}");
    }
    Ok(())
}

// --- /transfer-gate ---------------------------------------------------------

#[test]
fn the_transfer_gate_lists_the_seven_checks_in_order_with_no_assessment_and_the_switch()
-> Result<()> {
    // The failure this guards: a page listing checks the gate does not run,
    // or a last assessment for a gate nothing has ever called. The roster is
    // read through the kernel from the fabric's own list, so this pins the
    // names and the order §37.3 fixes.
    let rig = rig()?;
    let (text, body) = body_of(rig.get("/transfer-gate"));
    let checks = body["checks"].as_array().expect("a list");
    let expected = [
        ("corridor_authority", true),
        ("caps", false),
        ("minimum_interval", false),
        ("stated_purpose", false),
        ("source_balance", false),
        ("velocity_and_anomaly", true),
        ("kill_switch", false),
    ];
    assert_eq!(checks.len(), expected.len(), "{text}");
    for (index, (name, alerts)) in expected.into_iter().enumerate() {
        assert_eq!(
            checks[index],
            serde_json::json!({"order": index + 1, "name": name, "alerts": alerts}),
            "{text}"
        );
    }
    assert_eq!(body["last_assessment"], serde_json::Value::Null, "{text}");
    assert_eq!(body["executes"], serde_json::json!(false), "{text}");
    assert_eq!(body["note"], serde_json::json!(GATE_NOTE), "{text}");
    assert_eq!(
        body["kill_switch"],
        serde_json::json!({
            "halted": false,
            "halted_scopes": [],
            "tripped_by": null,
            "reason": null,
            "tripped_at": null
        }),
        "{text}"
    );

    // Trip the platform's switch and the gate's view follows: it is the same
    // fact `/risk` serves, not a copy.
    rig.with_platform(|platform| {
        platform.autonomy_mut().kill_switch_mut().trip_global(
            start(),
            "ledger-routes-test",
            "the seventh check reads this",
        );
    })?;
    let (text, body) = body_of(rig.get("/transfer-gate"));
    assert_eq!(
        body["kill_switch"],
        serde_json::json!({
            "halted": true,
            "halted_scopes": [],
            "tripped_by": "ledger-routes-test",
            "reason": "the seventh check reads this",
            "tripped_at": start().to_rfc3339()
        }),
        "{text}"
    );
    // Still no assessment: a tripped switch is state the gate would read,
    // not an assessment it made.
    assert_eq!(body["last_assessment"], serde_json::Value::Null, "{text}");
    Ok(())
}
