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
use qip_api::ledger_views::{EVALUATED_AS_ROLE, GATE_NOTE, NO_PRODUCTS, NO_WALLET, POSTURE};
use qip_api::routes::{Api, ROUTES};
use qip_capital::ledger::{
    Eligibility, EligibilityDecision, EligibilityTerms, Jurisdiction, Mandate, MandateId,
    MandateTerms, PermittedFamilies, UserId,
};
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
use qip_kernel::config::{PlatformConfig, UserMandate};
use qip_kernel::platform::Platform;
use qip_lifecycle::trials::StrategyFamily;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::OperatorIdentity;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- fixtures ---------------------------------------------------------------

const ANALYST_TOKEN: &str = "analyst-token";
const VIEWER_TOKEN: &str = "viewer-token";
const MONITOR_TOKEN: &str = "monitor-token";
/// An operator whose credential was issued at the instant the rig's clock
/// holds, so the kernel's freshness rule admits it.
const OPERATOR_TOKEN: &str = "operator-token";
const OPERATOR_SUBJECT: &str = "operator@example.com";
/// An operator whose credential was issued an hour before the rig's clock.
/// The kernel refuses an eligibility decision on a credential older than
/// fifteen minutes, and without a credential that *is* older the refusal
/// would be a branch no test could reach.
const STALE_OPERATOR_TOKEN: &str = "stale-operator-token";
const CELL: &str = "cell-lon-1";
const INSTRUMENT: &str = "obj-AAA";

/// The four paths under test, as the route table spells them.
const TREASURY_PATHS: [&str; 4] = ["/ledger/users", "/wallet", "/corridors", "/transfer-gate"];

/// The role each path requires, as `ROUTES-LEDGER.md` states it.
///
/// `/ledger/users` is the one that carries a per-user datum — every user's
/// mandate, balances and inflow references — and the portal hands the
/// viewer role to anyone who completes self-registration, so it is the one
/// held above viewer. The other three describe the process, not a user.
fn required_role_of(path: &str) -> Role {
    match path {
        "/ledger/users" => Role::Analyst,
        "/wallet" | "/corridors" | "/transfer-gate" => Role::Viewer,
        other => panic!("{other} is not a treasury path"),
    }
}

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
            fixture_liquidity(),
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
    rig_with(PlatformConfig::default())
}

/// A user mandate under the desk's: `capital` under management, every
/// family, no floor, in the desk's currency.
fn enrolment(user: &str, capital: Decimal) -> Result<UserMandate> {
    Ok(UserMandate {
        user: UserId::new(user)?,
        id: MandateId::new(format!("mandate-{user}"))?,
        mandate: Mandate::new(MandateTerms {
            capital,
            currency: qip_core::Currency::USD,
            risk_tolerance: Decimal::ONE,
            permitted_families: PermittedFamilies::Any,
            liquidity_floor: Decimal::ZERO,
            exploration_share: Decimal::ZERO,
            jurisdiction: Jurisdiction::new("GB")?,
        })?,
    })
}

fn rig_with(config: PlatformConfig) -> Result<Rig> {
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
        Credential::from_token(
            "monitor@example.com",
            Role::Monitor,
            MONITOR_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            OPERATOR_SUBJECT,
            Role::Operator,
            OPERATOR_TOKEN.to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        ),
        Credential::from_token(
            "stale-operator@example.com",
            Role::Operator,
            STALE_OPERATOR_TOKEN.to_string(),
            start().saturating_sub(Duration::from_mins(60)),
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

    /// Read a path as an analyst, which every treasury route admits.
    fn get(&self, path: &str) -> Response {
        self.call(Method::Get, path, ANALYST_TOKEN)
    }

    /// `POST /ledger/users/{user}/eligibility` with `body`, as `token`.
    fn decide(&self, user: &str, token: &str, body: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        self.api.handle(&Request {
            method: Method::Post,
            path: format!("/api/v1/ledger/users/{user}/eligibility"),
            query: BTreeMap::new(),
            headers,
            body: body.as_bytes().to_vec(),
            peer: "127.0.0.1:1".to_string(),
        })
    }

    /// One user's row out of `GET /ledger/users`, read as an analyst.
    fn row(&self, user: &str) -> serde_json::Value {
        let (text, body) = body_of(self.get("/ledger/users"));
        body["users"]
            .as_array()
            .unwrap_or_else(|| panic!("users is not a list: {text}"))
            .iter()
            .find(|row| row["user_id"] == serde_json::json!(user))
            .unwrap_or_else(|| panic!("no row for {user}: {text}"))
            .clone()
    }
}

/// The body an operator sends to grant eligibility, in the shape
/// `ROUTES-LEDGER.md` writes out.
fn grant_body() -> String {
    serde_json::json!({
        "decision": "granted",
        "verified_at": start().to_rfc3339(),
        "can_invest": true,
        "jurisdiction": "GB",
        "expires_at": start().saturating_add(Duration::from_days(365)).to_rfc3339(),
        "reason": "identity verified against the passport on file",
    })
    .to_string()
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
fn every_treasury_route_answers_its_role_with_the_documented_keys_and_the_posture_literal()
-> Result<()> {
    // The failure this guards: a page built against ROUTES-LEDGER.md renders
    // blank because a key was renamed on one side, or a body reaches the
    // browser without the posture a reader must see.
    let rig = rig()?;
    for path in TREASURY_PATHS {
        // Premise: the path is in the route table at the documented role, as
        // a GET, so what is answered below is the route and not a 404.
        let route = ROUTES
            .iter()
            .find(|route| route.pattern == path)
            .unwrap_or_else(|| panic!("{path} is not in ROUTES"));
        assert_eq!(route.method, Method::Get, "{path}");
        assert_eq!(route.required_role, required_role_of(path), "{path}");
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
fn a_viewer_reads_the_process_level_views_but_not_every_users_ledger() -> Result<()> {
    // The failure this guards, found in review: the portal grants the viewer
    // role to anyone who completes self-registration on the public front
    // door, and `/ledger/users` carries every enrolled user's mandate,
    // balances and inflow references. At viewer, whoever could sign up could
    // read every user's capital. An analyst is desk staff; a viewer is not
    // necessarily anyone.
    let rig = rig()?;
    // Premise, both ways: the viewer token is a live credential that the
    // three process-level views admit, and the analyst token reads the
    // same three — so the 403 below is the role, not the token.
    for path in ["/wallet", "/corridors", "/transfer-gate"] {
        assert_eq!(
            rig.call(Method::Get, path, VIEWER_TOKEN).status,
            200,
            "a viewer was refused {path}, which carries no per-user datum"
        );
        assert_eq!(
            rig.call(Method::Get, path, ANALYST_TOKEN).status,
            200,
            "{path}"
        );
    }
    // Premise: the route serves the per-user body to an analyst, so the
    // refusal is not a route that answers nobody.
    let (text, body) = body_of(rig.call(Method::Get, "/ledger/users", ANALYST_TOKEN));
    assert_eq!(
        body["users"][0]["user_id"],
        serde_json::json!("desk"),
        "{text}"
    );
    assert!(
        body["users"][0]["mandate"]["capital"].is_string(),
        "the premise is a body carrying a user's capital: {text}"
    );

    let refused = rig.call(Method::Get, "/ledger/users", VIEWER_TOKEN);
    assert_eq!(
        refused.status, 403,
        "a viewer read every user's mandate and balances"
    );
    let (text, body) = body_of(refused);
    // The refusal names the role required and nothing about any user.
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("analyst")),
        "the refusal does not name the role required: {text}"
    );
    assert!(
        !text.contains("desk") && !text.contains("mandate") && !text.contains("capital"),
        "the refusal leaked a user datum: {text}"
    );
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

#[test]
fn a_fill_across_two_enrolled_users_is_served_as_each_users_share_and_none_of_the_desks()
-> Result<()> {
    // The failure this guards: the route listing the desk alone with the
    // whole fill while two users' mandates were enrolled, so the page could
    // not show whose capital the strategy was trading. Premise first: both
    // users are enrolled with capital at work at alpha, one to two, and the
    // desk has no book.
    let rig = rig_with(PlatformConfig::default().with_user_mandates(vec![
        enrolment("alice", dec!("1000"))?,
        enrolment("bob", dec!("1000"))?,
    ]))?;
    let alpha = StrategyId::new("alpha");
    rig.with_platform(|platform| -> Result<()> {
        // A user is fundable only once an operator has decided their
        // eligibility, through the same journaled path the API's operator
        // route takes; the fixture records that decision for each user and
        // asserts it stands before funding, so a refusal below would be the
        // route's and not the registry's.
        let operator = OperatorIdentity::verified("ops-carol", "oidc", start());
        for name in ["alice", "bob"] {
            let user = UserId::new(name)?;
            platform.decide_eligibility(
                &user,
                EligibilityDecision::Granted {
                    eligibility: Eligibility::new(EligibilityTerms {
                        verified_at: start(),
                        can_invest: true,
                        jurisdiction: Jurisdiction::new("GB")?,
                        expires_at: start().saturating_add(Duration::from_days(365)),
                    })?,
                },
                &operator,
                "identity verified against the passport on file",
                start(),
            )?;
            assert!(
                platform
                    .user_ledger()
                    .eligibility_of(&user, start())
                    .is_ok(),
                "the premise: {name} is eligible before funding"
            );
        }
        platform.fund_user(&UserId::new("alice")?, &alpha, dec!("100"), start())?;
        platform.fund_user(&UserId::new("bob")?, &alpha, dec!("200"), start())?;
        Ok(())
    })??;
    let (text, before) = body_of(rig.get("/ledger/users"));
    let users = before["users"].as_array().expect("a list");
    assert_eq!(users.len(), 3, "{text}");
    assert_eq!(
        users
            .iter()
            .map(|user| user["user_id"].clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("alice"),
            serde_json::json!("bob"),
            serde_json::json!("desk")
        ],
        "in user-id order: {text}"
    );
    assert_eq!(
        users[0]["balances"][0]["settled"],
        serde_json::json!("100"),
        "{text}"
    );
    assert_eq!(
        users[1]["balances"][0]["settled"],
        serde_json::json!("200"),
        "{text}"
    );
    assert_eq!(users[2]["balances"], serde_json::json!([]), "{text}");
    assert_eq!(before["fills_journalled"], serde_json::json!(0), "{text}");

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

    // A thousand, one to two, in nine decimals: the truncated unit lands
    // in the larger share, and the two sum to the fill exactly.
    let (text, body) = body_of(rig.get("/ledger/users"));
    assert_eq!(body["fills_journalled"], serde_json::json!(2), "{text}");
    let users = body["users"].as_array().expect("a list");
    let alice = &users[0]["balances"][0];
    assert_eq!(alice["strategy"], serde_json::json!("alpha"), "{text}");
    assert_eq!(
        alice["settled"],
        serde_json::json!("433.333333333"),
        "{text}"
    );
    assert_eq!(alice["entries"], serde_json::json!(2), "{text}");
    let bob = &users[1]["balances"][0];
    assert_eq!(bob["settled"], serde_json::json!("866.666666667"), "{text}");
    assert_eq!(bob["entries"], serde_json::json!(2), "{text}");
    assert_eq!(
        users[2]["balances"],
        serde_json::json!([]),
        "the desk was booked a share of a fill two users were entitled to: {text}"
    );
    Ok(())
}

// --- /wallet ----------------------------------------------------------------

#[test]
fn the_wallet_is_unassembled_until_a_statement_and_a_cycle_and_then_reconciles_the_desks_cash()
-> Result<()> {
    // The failure this guards, both ways: a wallet panel showing zero
    // holdings and a clean reconciliation for an account nobody has
    // observed; and, once one is observed, a panel reading a copy the API
    // kept rather than the wallet the kernel's journal assembled. Premise
    // first: nothing is assembled until a statement is handed in and a
    // cycle's LEARN stage assembles against it.
    let rig = rig()?;
    let initial_equity = rig.with_platform(|platform| platform.config().initial_equity)?;
    let (text, before) = body_of(rig.get("/wallet"));
    assert_eq!(before["assembled"], serde_json::json!(false), "{text}");
    assert_eq!(before["reason"], serde_json::json!(NO_WALLET), "{text}");
    assert_eq!(before["as_of"], serde_json::Value::Null, "{text}");
    assert_eq!(before["holdings"], serde_json::json!([]), "{text}");
    assert_eq!(
        before["reconciliation"],
        serde_json::json!({"outcomes": [], "halted_venue_assets": 0}),
        "{text}"
    );

    // Two statements: the desk's own cash at its venue, to the unit, and a
    // balance at a venue the ledger books nothing at — which is a break the
    // wallet exists to find, not a wallet that cannot break.
    let cycle_at = start().saturating_add(Duration::from_secs(60));
    rig.with_platform(|platform| -> Result<()> {
        platform.observe_statement(
            VenueId::new("simulated-venue"),
            "USD",
            initial_equity,
            dec!("1"),
            start(),
        )?;
        platform.observe_statement(
            VenueId::new("custodian-x"),
            "USD",
            dec!("250"),
            dec!("1"),
            start(),
        )?;
        // A statement alone assembles nothing; the cycle does.
        assert!(platform.fabric_state().wallet().is_none());
        platform.run_cycle(cycle_at);
        Ok(())
    })??;

    let (text, body) = body_of(rig.get("/wallet"));
    assert_eq!(body["assembled"], serde_json::json!(true), "{text}");
    assert_eq!(body["reason"], serde_json::Value::Null, "{text}");
    assert_eq!(
        body["as_of"],
        serde_json::json!(cycle_at.to_rfc3339()),
        "{text}"
    );
    let holdings = body["holdings"].as_array().expect("a list");
    assert_eq!(holdings.len(), 2, "{text}");
    // Venue-asset order: the custodian before the simulated venue.
    assert_eq!(
        holdings[0],
        serde_json::json!({
            "venue": "custodian-x",
            "asset": "USD",
            "observed_quantity": "250",
            "observed_at": start().to_rfc3339(),
            "provenance": "statement",
            "ledger_expected": null
        }),
        "{text}"
    );
    assert_eq!(
        holdings[1],
        serde_json::json!({
            "venue": "simulated-venue",
            "asset": "USD",
            "observed_quantity": initial_equity.to_string(),
            "observed_at": start().to_rfc3339(),
            "provenance": "statement",
            "ledger_expected": initial_equity.to_string()
        }),
        "{text}"
    );
    assert!(holdings[1]["observed_quantity"].is_string(), "{text}");
    let outcomes = body["reconciliation"]["outcomes"]
        .as_array()
        .expect("a list");
    assert_eq!(outcomes.len(), 2, "{text}");
    assert_eq!(outcomes[0]["outcome"], serde_json::json!("halt"), "{text}");
    assert_eq!(
        outcomes[0]["alert"]["cause"],
        serde_json::json!("unrecorded_by_ledger"),
        "{text}"
    );
    assert_eq!(
        outcomes[1],
        serde_json::json!({
            "outcome": "reconciled",
            "venue": "simulated-venue",
            "asset": "USD",
            "delta": "0"
        }),
        "{text}"
    );
    assert_eq!(
        body["reconciliation"]["halted_venue_assets"],
        serde_json::json!(1),
        "{text}"
    );
    Ok(())
}

// --- /corridors -------------------------------------------------------------

#[test]
fn the_corridor_and_destination_registries_are_held_and_empty_until_a_command_proposes_one()
-> Result<()> {
    // The failure this guards: a registry that exists and admits nothing
    // rendered as though no registry existed. Both are held from assembly
    // by the kernel's fabric journal; the records are whatever commands
    // through that journal proposed, and this process has proposed none.
    // A proposal needs a fabric command, which this crate cannot name, so
    // the populated shape is proven in the kernel's own `tests/ledger.rs`.
    let rig = rig()?;
    // Premise: the journal holds no record at all.
    assert_eq!(rig.with_platform(|platform| platform.fabric_records())?, 0);
    let (text, body) = body_of(rig.get("/corridors"));
    for key in ["corridors", "destinations"] {
        assert_eq!(body[key]["held"], serde_json::json!(true), "{key}: {text}");
        assert_eq!(
            body[key]["reason"],
            serde_json::Value::Null,
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

/// The failure this guards: a page listing a user with a mandate and
/// balances and no way to tell whether the next funding would be refused,
/// and why — the eligibility gate at the one place capital enters a book
/// was invisible from the surface an analyst reads. Premise first: one
/// user is decided eligible through the journaled operator path and the
/// other is not decided at all, and the ledger itself agrees before the
/// route is read.
#[test]
fn each_user_row_says_whether_the_ledger_would_fund_them_and_names_the_refusal() -> Result<()> {
    let rig = rig_with(PlatformConfig::default().with_user_mandates(vec![
        enrolment("alice", dec!("1000"))?,
        enrolment("bob", dec!("1000"))?,
    ]))?;
    rig.with_platform(|platform| -> Result<()> {
        let operator = OperatorIdentity::verified("ops-carol", "oidc", start());
        let alice = UserId::new("alice")?;
        platform.decide_eligibility(
            &alice,
            EligibilityDecision::Granted {
                eligibility: Eligibility::new(EligibilityTerms {
                    verified_at: start(),
                    can_invest: true,
                    jurisdiction: Jurisdiction::new("GB")?,
                    expires_at: start().saturating_add(Duration::from_days(365)),
                })?,
            },
            &operator,
            "identity verified against the passport on file",
            start(),
        )?;
        let ledger = platform.user_ledger();
        assert!(
            ledger.eligibility_of(&alice, start()).is_ok(),
            "the premise: alice is eligible"
        );
        assert!(
            ledger
                .eligibility_of(&UserId::new("bob")?, start())
                .is_err(),
            "the premise: nobody has decided bob"
        );
        Ok(())
    })??;
    let (text, body) = body_of(rig.get("/ledger/users"));
    let users = body["users"].as_array().expect("a list");
    let row = |id: &str| {
        users
            .iter()
            .find(|user| user["user_id"] == serde_json::json!(id))
            .unwrap_or_else(|| panic!("no row for {id}: {text}"))
            .clone()
    };
    let alice = row("alice");
    assert_eq!(
        alice["eligibility"]["eligible"],
        serde_json::json!(true),
        "{text}"
    );
    assert_eq!(
        alice["eligibility"]["can_invest"],
        serde_json::json!(true),
        "{text}"
    );
    assert_eq!(
        alice["eligibility"]["jurisdiction"],
        serde_json::json!("GB"),
        "{text}"
    );
    assert!(
        alice["eligibility"]["refused"].is_null(),
        "an eligible row carries a refusal: {text}"
    );
    let bob = row("bob");
    assert_eq!(
        bob["eligibility"]["eligible"],
        serde_json::json!(false),
        "{text}"
    );
    assert_eq!(
        bob["eligibility"]["refused"],
        serde_json::json!("unknown_user"),
        "{text}"
    );
    assert!(
        bob["eligibility"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("(unknown_user)") && reason.contains("bob")),
        "the refusal does not name the user and the token: {text}"
    );
    assert!(bob["eligibility"]["verified_at"].is_null(), "{text}");
    // The record has no withdrawal field, on the surface or underneath.
    assert!(
        !text.contains("can_withdraw\":true") && alice["eligibility"].get("can_withdraw").is_none(),
        "a withdrawal capability appeared on the eligibility record: {text}"
    );
    Ok(())
}

// --- POST /ledger/users/{user}/eligibility ------------------------------------

#[test]
fn an_operator_grants_eligibility_the_row_reads_eligible_and_a_revocation_takes_it_back()
-> Result<()> {
    // The failure this guards: the ledger refuses to fund a user until an
    // operator has decided their eligibility, and until this route existed
    // the only ways to take that decision were the deployment's committed
    // configuration and a test. A gate that nobody a running process can
    // name is able to open is a gate the desk works around, not one it uses.
    let rig = rig_with(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;

    // Premise: nobody has decided alice, and the ledger says so by name — so
    // the change below is this route's and not a state the rig started in.
    let before = rig.row("alice");
    assert_eq!(before["eligibility"]["eligible"], serde_json::json!(false));
    assert_eq!(
        before["eligibility"]["refused"],
        serde_json::json!("unknown_user"),
        "{before}"
    );

    let response = rig.decide("alice", OPERATOR_TOKEN, &grant_body());
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, decided) = body_of(response);
    assert_eq!(decided["posture"], serde_json::json!(POSTURE), "{text}");
    assert_eq!(
        decided["served_at"],
        serde_json::json!(start().to_rfc3339()),
        "{text}"
    );
    assert_eq!(
        decided["user"]["user_id"],
        serde_json::json!("alice"),
        "{text}"
    );
    assert_eq!(
        decided["user"]["eligibility"]["eligible"],
        serde_json::json!(true),
        "{text}"
    );
    assert_eq!(
        decided["user"]["eligibility"]["can_invest"],
        serde_json::json!(true),
        "{text}"
    );
    assert_eq!(
        decided["user"]["eligibility"]["jurisdiction"],
        serde_json::json!("GB"),
        "{text}"
    );
    assert_eq!(
        decided["user"]["eligibility"]["expires_at"],
        serde_json::json!(
            start()
                .saturating_add(Duration::from_days(365))
                .to_rfc3339()
        ),
        "{text}"
    );
    assert!(
        decided["user"]["eligibility"]["refused"].is_null(),
        "{text}"
    );
    // No withdrawal field arrived with the terms, on the surface or under it.
    assert!(
        !text.contains("can_withdraw\":true"),
        "a granted withdrawal reached the answer: {text}"
    );

    // The answer is the ledger's row, not the route's account of it: the
    // list agrees, key for key.
    assert_eq!(rig.row("alice"), decided["user"], "{text}");

    // And the platform acted on nothing the log does not hold: the registry
    // rebuilt from the event log alone is the one the ledger is using.
    rig.with_platform(|platform| -> Result<()> {
        assert_eq!(
            &platform.replay_eligibility()?,
            platform.user_ledger().eligibility(),
            "the eligibility the ledger holds is not the one the log replays"
        );
        assert!(
            platform
                .user_ledger()
                .eligibility()
                .record(&UserId::new("alice")?)
                .is_some_and(|record| record.by.subject() == OPERATOR_SUBJECT),
            "the decision names somebody other than the authenticated operator"
        );
        Ok(())
    })??;

    // A revocation takes it back, and the row says `revoked` rather than
    // reverting to `unknown_user`: "never verified" and "verified and then
    // revoked" are different answers and the registry keeps them apart.
    let revoked = rig.decide(
        "alice",
        OPERATOR_TOKEN,
        &serde_json::json!({
            "decision": "revoked",
            "reason": "the passport on file expired and has not been renewed",
        })
        .to_string(),
    );
    assert_eq!(
        revoked.status,
        200,
        "{}",
        String::from_utf8_lossy(&revoked.body)
    );
    let (text, revoked) = body_of(revoked);
    assert_eq!(
        revoked["user"]["eligibility"]["eligible"],
        serde_json::json!(false),
        "{text}"
    );
    assert_eq!(
        revoked["user"]["eligibility"]["refused"],
        serde_json::json!("revoked"),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_viewer_cannot_decide_a_users_eligibility_and_nothing_is_recorded() -> Result<()> {
    // The portal grants the viewer role to anyone who completes
    // self-registration on the public front door. A viewer who could decide
    // eligibility could decide their own, and the gate the ledger runs
    // before every funding would be a gate its subject holds the key to.
    let rig = rig_with(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;

    // Premise, both ways: the viewer's credential is live — it reads the
    // three process-level views — and the route exists at operator, which
    // the table says and the test above proves end to end.
    assert_eq!(rig.call(Method::Get, "/wallet", VIEWER_TOKEN).status, 200);
    let route = ROUTES
        .iter()
        .find(|route| {
            route.method == Method::Post && route.pattern == "/ledger/users/:user/eligibility"
        })
        .expect("the eligibility route is in the table");
    assert_eq!(route.required_role, Role::Operator);

    let response = rig.decide("alice", VIEWER_TOKEN, &grant_body());
    assert_eq!(
        response.status,
        403,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    // An analyst is refused too: this is not a role the read surface's own
    // authority reaches, however far up it goes.
    assert_eq!(
        rig.decide("alice", ANALYST_TOKEN, &grant_body()).status,
        403
    );

    // Nothing moved: the row still says nobody decided, and the log holds no
    // decision to replay.
    assert_eq!(
        rig.row("alice")["eligibility"]["refused"],
        serde_json::json!("unknown_user")
    );
    rig.with_platform(|platform| -> Result<()> {
        assert_eq!(
            platform.replay_eligibility()?.records().len(),
            0,
            "a refused request reached the event log"
        );
        Ok(())
    })??;
    Ok(())
}

#[test]
fn an_eligibility_body_naming_a_withdrawal_capability_is_refused_as_a_key_this_route_does_not_read()
-> Result<()> {
    // The failure this guards: blueprint §43.3 lists `can_withdraw` beside
    // `can_invest`, so a caller writing against the blueprint will send it.
    // A key silently ignored would let them go on believing this platform
    // has a withdrawal path; the refusal says in as many words that it does
    // not, and names the ADR that decided it.
    let rig = rig_with(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;
    let mut body: serde_json::Value = serde_json::from_str(&grant_body())?;
    body["can_withdraw"] = serde_json::json!(true);

    let response = rig.decide("alice", OPERATOR_TOKEN, &body.to_string());
    assert_eq!(
        response.status,
        400,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, refusal) = body_of(response);
    let reason = refusal["error"].as_str().unwrap_or_default();
    assert!(
        // `serde_json::Map` is a `BTreeMap`, so the object's keys are in
        // sorted order and `can_withdraw` is the second of the seven.
        reason.contains("key at position 2") && reason.contains("ADR 0021"),
        "the refusal does not locate the key and name the decision that refuses it: {text}"
    );
    // And it does not echo what the caller sent: a refusal that repeated the
    // key would publish whatever was written into a key name.
    assert!(
        !reason.contains("can_withdraw\""),
        "the refusal quoted the caller's key: {text}"
    );
    // Nothing was decided on the way to refusing it.
    assert_eq!(
        rig.row("alice")["eligibility"]["refused"],
        serde_json::json!("unknown_user")
    );

    // Every other field refusal names the field, and the terms the platform's
    // own type refuses are refused by it rather than by a copy of its rule
    // kept here.
    for (mutate, names) in [
        (
            serde_json::json!({"decision": "granted", "reason": "identity verified on file"}),
            "no `verified_at`",
        ),
        (
            serde_json::json!({"decision": "sideways", "reason": "identity verified on file"}),
            "`granted` or `revoked`",
        ),
    ] {
        let response = rig.decide("alice", OPERATOR_TOKEN, &mutate.to_string());
        assert_eq!(response.status, 400, "{mutate}");
        let (text, refusal) = body_of(response);
        assert!(
            refusal["error"]
                .as_str()
                .is_some_and(|reason| reason.contains(names)),
            "{text}"
        );
    }
    for (field, value, names) in [
        (
            "can_invest",
            serde_json::json!("true"),
            "`can_invest` must be a JSON boolean",
        ),
        (
            "verified_at",
            serde_json::json!("the ninth of October"),
            "`verified_at` is not an RFC 3339 instant",
        ),
        ("jurisdiction", serde_json::json!("GBR"), "ISO 3166"),
        (
            "expires_at",
            serde_json::json!(start().to_rfc3339()),
            "expiry must be after the verification",
        ),
    ] {
        let mut body: serde_json::Value = serde_json::from_str(&grant_body())?;
        body[field] = value;
        let response = rig.decide("alice", OPERATOR_TOKEN, &body.to_string());
        assert_eq!(response.status, 400, "{field}: {body}");
        let (text, refusal) = body_of(response);
        assert!(
            refusal["error"]
                .as_str()
                .is_some_and(|reason| reason.contains(names)),
            "{field}: {text}"
        );
    }

    // Premise, asserted last so the refusals above are about what changed:
    // the same body without the offending key is admitted.
    assert_eq!(
        rig.decide("alice", OPERATOR_TOKEN, &grant_body()).status,
        200
    );
    Ok(())
}

#[test]
fn an_operator_credential_older_than_the_kernel_accepts_decides_nothing() -> Result<()> {
    // The kernel holds an eligibility decision to a credential authenticated
    // within fifteen minutes, on the ground that a session token from this
    // morning is not evidence that anyone is at the keyboard now. The route
    // therefore dates the operator identity at the instant the *credential*
    // was issued: stamped with `now` instead, the identity would be fresh by
    // construction and the kernel's rule would be a control that cannot
    // fire.
    let rig = rig_with(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;

    // Premise: the stale token is a live operator credential — it is
    // recognised, unexpired and holds the role — so the refusal below is the
    // credential's age and not its authority.
    let route = ROUTES
        .iter()
        .find(|route| {
            route.method == Method::Post && route.pattern == "/ledger/users/:user/eligibility"
        })
        .expect("the eligibility route is in the table");
    assert_eq!(route.required_role, Role::Operator);
    assert_eq!(
        rig.call(Method::Get, "/ledger/users", STALE_OPERATOR_TOKEN)
            .status,
        200,
        "the stale token is not a working credential, so the 409 below would prove nothing"
    );

    let response = rig.decide("alice", STALE_OPERATOR_TOKEN, &grant_body());
    assert_eq!(
        response.status,
        409,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, refusal) = body_of(response);
    assert!(
        refusal["error"]
            .as_str()
            .is_some_and(|reason| reason.contains("re-authenticate")),
        "the refusal does not say what to do instead: {text}"
    );
    assert_eq!(
        rig.row("alice")["eligibility"]["refused"],
        serde_json::json!("unknown_user"),
        "a stale credential decided an eligibility"
    );

    // Premise on the other side: the fresh credential of the same role is
    // admitted, so the rule refuses an age and not every operator.
    assert_eq!(
        rig.decide("alice", OPERATOR_TOKEN, &grant_body()).status,
        200
    );
    Ok(())
}

#[test]
fn an_eligibility_decision_for_a_user_holding_no_mandate_is_refused_by_name() -> Result<()> {
    // Eligibility is a statement about a mandate holder — the ledger's own
    // first refusal says so. A decision recorded against a user nobody
    // enrolled would be a record `/ledger/users` never shows and nobody
    // would find again.
    let rig = rig_with(
        PlatformConfig::default().with_user_mandates(vec![enrolment("alice", dec!("1000"))?]),
    )?;
    // Premise: the registry holds alice and not mallory.
    rig.with_platform(|platform| -> Result<()> {
        let held = platform.user_ledger().mandates();
        assert!(held.contains_key(&UserId::new("alice")?));
        assert!(!held.contains_key(&UserId::new("mallory")?));
        Ok(())
    })??;

    let response = rig.decide("mallory", OPERATOR_TOKEN, &grant_body());
    assert_eq!(
        response.status,
        404,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let (text, refusal) = body_of(response);
    assert!(
        refusal["error"].as_str().is_some_and(
            |reason| reason.contains("mallory") && reason.contains("enrol the mandate")
        ),
        "the refusal does not name the user or what to do: {text}"
    );
    rig.with_platform(|platform| -> Result<()> {
        assert_eq!(platform.replay_eligibility()?.records().len(), 0);
        Ok(())
    })??;
    Ok(())
}

// --- POST /ledger/users/{user}/investment-requests -----------------------------

/// The strategy `register_family` deploys, spelled as the compiler holds it.
const REGISTERED_STRATEGY: &str = "AAA";
const FAMILY: &str = "ledger-route-tests";

/// A rig with alice enrolled, verified, the strategy registered under
/// [`FAMILY`] and that family cleared for sale where she is — everything an
/// investment request needs before the only thing left to answer it is the
/// mandate's own arithmetic.
fn rig_ready_for_requests(capital: Decimal) -> Result<Rig> {
    let rig =
        rig_with(PlatformConfig::default().with_user_mandates(vec![enrolment("alice", capital)?]))?;
    register_family(&rig, FAMILY)?;
    rig.with_platform(|platform| -> Result<()> {
        let operator = OperatorIdentity::verified("ops-carol", "oidc", start());
        platform.decide_eligibility(
            &UserId::new("alice")?,
            EligibilityDecision::Granted {
                eligibility: Eligibility::new(EligibilityTerms {
                    verified_at: start(),
                    can_invest: true,
                    jurisdiction: Jurisdiction::new("GB")?,
                    expires_at: start().saturating_add(Duration::from_days(365)),
                })?,
            },
            &operator,
            "identity verified against the passport on file",
            start(),
        )?;
        platform.offer_product(
            qip_capital::ledger::ProductEligibility::new(FAMILY)
                .eligible_in(Jurisdiction::new("GB")?),
            &operator,
            "cleared for retail distribution in GB by the compliance committee",
            start(),
        )
    })??;
    Ok(rig)
}

/// A request body in the shape `ROUTES-LEDGER.md` writes out.
fn request_body(family: &str, amount: &str) -> String {
    serde_json::json!({
        "strategy": REGISTERED_STRATEGY,
        "family": family,
        "currency": "USD",
        "amount": amount,
        "reason": "the client asked for this in writing on the dated instruction",
    })
    .to_string()
}

impl Rig {
    /// `POST /ledger/users/{user}/investment-requests` with `body`, as `token`.
    fn raise(&self, user: &str, token: &str, body: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        self.api.handle(&Request {
            method: Method::Post,
            path: format!("/api/v1/ledger/users/{user}/investment-requests"),
            query: BTreeMap::new(),
            headers,
            body: body.as_bytes().to_vec(),
            peer: "127.0.0.1:1".to_string(),
        })
    }
}

#[test]
fn an_operator_raises_an_investment_request_and_the_route_answers_the_limit_that_refused_it()
-> Result<()> {
    // The failure this closes: §40.9 gives `investment-api` one intent — an
    // investment request — and this API raised none at all, while the mandate
    // gate that would decide one (`UserLedger::admit`: eligibility,
    // entitlement, currency, investable capital net of what is at work, and
    // the share tolerated at one strategy) sat in the tree reachable from
    // nothing a deployed process ran.
    //
    // Premise first, because a route that refused everything would pass a
    // refusal assertion on its own: the same user and strategy admit a
    // request inside the mandate.
    let rig = rig_ready_for_requests(dec!("1000"))?;
    let admitted = rig.raise("alice", OPERATOR_TOKEN, &request_body(FAMILY, "400"));
    assert_eq!(admitted.status, 200);
    let (text, body) = body_of(admitted);
    assert_eq!(body["admitted"], serde_json::json!(true), "{text}");
    assert_eq!(body["posture"], serde_json::json!(POSTURE), "{text}");
    assert!(body["refused_limit"].is_null(), "{text}");
    assert_eq!(
        body["request"]["amount"],
        serde_json::json!("400"),
        "the answer echoes the ledger's own record of the request: {text}"
    );
    assert_eq!(body["request"]["user_id"], serde_json::json!("alice"));
    // The property that separates this from an order path, asserted rather
    // than assumed: an admitted request moved nothing.
    assert_eq!(
        body["funded"],
        serde_json::json!(false),
        "an admitted request claimed to have funded: {text}"
    );
    assert_eq!(
        rig.row("alice")["balances"],
        serde_json::json!([]),
        "an admitted request opened a book"
    );

    // The refusal, and the limit named as a value a page can group on rather
    // than a sentence it would have to parse.
    let refused = rig.raise("alice", OPERATOR_TOKEN, &request_body(FAMILY, "5000"));
    assert_eq!(
        refused.status, 200,
        "a refusal is a decision the platform took, not an error"
    );
    let (text, body) = body_of(refused);
    assert_eq!(body["admitted"], serde_json::json!(false), "{text}");
    assert_eq!(
        body["refused_limit"],
        serde_json::json!("InvestableCapital"),
        "{text}"
    );
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("investable")),
        "the refusal does not say what would have to change: {text}"
    );
    // The row the verdict was reached against travels with it, so the
    // figures behind the refusal are readable beside it.
    assert_eq!(
        body["user"]["user_id"],
        serde_json::json!("alice"),
        "{text}"
    );
    assert_eq!(
        body["user"]["mandate"]["investable"],
        serde_json::json!("1000"),
        "{text}"
    );
    assert_eq!(rig.row("alice")["balances"], serde_json::json!([]));
    Ok(())
}

#[test]
fn a_viewer_cannot_raise_an_investment_request_and_a_user_with_no_mandate_is_not_a_decision()
-> Result<()> {
    // Two refusals that must not be confused with a verdict. The first is the
    // route table's: raising a request writes an operator's name to the event
    // log beside a claim about a client's capital, so it is not read-role
    // work. The second is the difference between "the mandate refused this"
    // and "there is no mandate": the ledger would answer the second with a
    // 200 carrying `NoMandate`, which reads as a decision about a person the
    // platform has never heard of.
    let rig = rig_ready_for_requests(dec!("1000"))?;
    // Premise: the route table declares the authority, and the operator can
    // in fact raise one.
    let route = ROUTES
        .iter()
        .find(|route| route.pattern == "/ledger/users/:user/investment-requests")
        .expect("the route is in the table");
    assert_eq!(route.method, Method::Post);
    assert_eq!(route.required_role, Role::Operator);
    assert_eq!(
        rig.raise("alice", OPERATOR_TOKEN, &request_body(FAMILY, "100"))
            .status,
        200
    );

    for token in [VIEWER_TOKEN, ANALYST_TOKEN] {
        assert_eq!(
            rig.raise("alice", token, &request_body(FAMILY, "100"))
                .status,
            403,
            "a credential below the operator role raised a request"
        );
    }

    let unknown = rig.raise("nobody", OPERATOR_TOKEN, &request_body(FAMILY, "100"));
    assert_eq!(unknown.status, 404);
    let (text, body) = body_of(unknown);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("no mandate is registered")),
        "{text}"
    );
    Ok(())
}

#[test]
fn an_investment_request_amount_sent_as_a_number_or_carrying_a_funding_field_is_refused()
-> Result<()> {
    // Two bodies a caller sends when they have misunderstood what this route
    // is. A JSON number is money through a float, which is the failure
    // `Decimal` exists to prevent and which every other figure on this
    // surface avoids by being a string. A `fund` key is a caller who believes
    // this route places something; ignoring it would let them keep believing
    // it until they wondered why nothing traded.
    //
    // Premise: the same request with the amount as a string is decided, so
    // what follows is the screening and not a route that refuses everything.
    let rig = rig_ready_for_requests(dec!("1000"))?;
    assert_eq!(
        rig.raise("alice", OPERATOR_TOKEN, &request_body(FAMILY, "100"))
            .status,
        200
    );

    let numeric = serde_json::json!({
        "strategy": REGISTERED_STRATEGY,
        "family": FAMILY,
        "currency": "USD",
        "amount": 100.10,
        "reason": "the client asked for this in writing on the dated instruction",
    })
    .to_string();
    let response = rig.raise("alice", OPERATOR_TOKEN, &numeric);
    assert_eq!(response.status, 400);
    let (text, body) = body_of(response);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("never a number")),
        "{text}"
    );

    let funding = serde_json::json!({
        "strategy": REGISTERED_STRATEGY,
        "family": FAMILY,
        "currency": "USD",
        "amount": "100",
        "fund": true,
        "reason": "the client asked for this in writing on the dated instruction",
    })
    .to_string();
    let response = rig.raise("alice", OPERATOR_TOKEN, &funding);
    assert_eq!(response.status, 400);
    let (text, body) = body_of(response);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("funds nothing"),
        "the refusal does not say the route funds nothing: {text}"
    );
    assert!(
        !error.contains("true"),
        "the refusal quoted what the caller sent back at them: {text}"
    );

    // A family the factory did not register the strategy under is refused by
    // the kernel rather than re-labelled, and the refusal reaches the caller
    // as theirs to fix.
    let response = rig.raise(
        "alice",
        OPERATOR_TOKEN,
        &request_body("another-family", "100"),
    );
    assert_eq!(response.status, 400);
    let (text, body) = body_of(response);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains(&format!("registered under {FAMILY}"))),
        "{text}"
    );
    Ok(())
}
