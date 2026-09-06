//! The cognition read surface: `/cognition/self-model` and
//! `/cognition/precedents`, against the contract in `ROUTES-COGNITION.md`.
//!
//! Every test asserts its premise before the property. A route that serves
//! an empty list passes every shape assertion whether or not the platform
//! ever held the fact, so where a row is asserted the platform is driven to
//! produce one first — a thesis is graded through `Platform::learn_from`, a
//! cycle is run over a tape with a jump in it — and the state before is
//! checked to be the other one.
//!
//! The property that matters most is the engine's own: no accuracy is served
//! for a component below the minimum sample. The number a page would use to
//! explain a `null` is pinned to the count at which the engine actually
//! starts reporting, by driving outcomes in one at a time and watching the
//! body flip.
//!
//! Neither this crate nor its tests depend on `qip-learning-engine`, so the
//! claims and outcomes handed to `learn_from` are built by `serde_json` with
//! their types inferred from the kernel's signature. That is deliberate: the
//! API's boundary is the kernel, and a fixture that reached past it would be
//! proving a route against a type the route cannot see.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::{Api, ROUTES};
use qip_api::self_model_views::MINIMUM_SAMPLE;
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

// --- fixtures ---------------------------------------------------------------

const VIEWER_TOKEN: &str = "viewer-token";
const MONITOR_TOKEN: &str = "monitor-token";

/// The two paths under test, as the route table spells them.
const COGNITION_PATHS: [&str; 2] = ["/cognition/self-model", "/cognition/precedents"];

/// The keys `ROUTES-COGNITION.md` promises at the top level of each body.
fn documented_keys(path: &str) -> &'static [&'static str] {
    match path {
        "/cognition/self-model" => &["components", "minimum_sample"],
        "/cognition/precedents" => &["precedents"],
        other => panic!("{other} is not a cognition path"),
    }
}

/// The hypothesis class every graded thesis below carries, and so the id of
/// the detector the engine charges.
const DETECTOR: &str = "price_dislocation";

/// An analyst on the default roster, named in each claim's contributors so
/// a second component of a different kind is charged beside the detector.
const ANALYST: &str = "macro-analyst";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    for symbol in ["AAA", "ZZZ"] {
        universe.insert(
            FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())?,
        )?;
    }
    Ok(universe)
}

/// The research fixture's limits: wide enough that a thesis is sized rather
/// than refused, which is what makes REASON convene and record a precedent.
fn limits() -> LimitSet {
    LimitSet::new("self-model-routes-test")
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

    /// Grade one thesis of class [`DETECTOR`] with [`ANALYST`] among its
    /// contributors, `correct` or not, through the only road into the
    /// self-model: an evaluation the learning engine produced. Returns how
    /// many evaluations were graded, which is the premise every caller
    /// asserts.
    fn grade(&self, n: usize, correct: bool) -> Result<usize> {
        let resolves_at = start().saturating_add(Duration::from_days(5));
        let id = format!("hyp-{n}");
        let claim = serde_json::json!({
            "hypothesis_id": id,
            "class": DETECTOR,
            "subject": "obj-AAA",
            "formed_at": start(),
            "resolves_at": resolves_at,
            "direction": 1.0,
            "expected_move_bps": 200.0,
            "confidence": 0.7,
            "falsifiers": ["it reverts"],
            "contributors": [format!("run-{ANALYST}-{n}")],
        });
        let outcome = serde_json::json!({
            "hypothesis_id": id,
            "observed_at": resolves_at,
            "realised_move_bps": if correct { 180.0 } else { -180.0 },
            "realised_pnl": 0.0,
            "falsifiers_triggered": [],
            "mechanism_confirmed": null,
        });
        // The types are the kernel's parameter types, inferred rather than
        // named: see the module comment.
        let claims =
            vec![serde_json::from_value(claim).map_err(|error| Error::invalid(error.to_string()))?];
        let outcomes = vec![
            serde_json::from_value(outcome).map_err(|error| Error::invalid(error.to_string()))?,
        ];
        self.with_platform(|platform| {
            platform
                .learn_from(&claims, &outcomes, resolves_at)
                .map(|learned| learned.evaluations.len())
        })?
    }
}

fn body_of(response: Response) -> (String, serde_json::Value) {
    let text = String::from_utf8(response.body).expect("a UTF-8 body");
    let value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
    (text, value)
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

/// A price series with a jump partway through, so the detectors have
/// something real to find — the shape the kernel's own learning test feeds.
fn jumping_bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

// --- the surface as a whole -------------------------------------------------

#[test]
fn every_cognition_route_is_a_viewer_get_that_answers_json_with_the_documented_keys() -> Result<()>
{
    // The failure this guards: a page built against ROUTES-COGNITION.md
    // renders blank because a key was renamed on one side, or because the
    // route was declared in the table and never wired to a handler, which
    // the table alone cannot tell from a route that works.
    let rig = rig()?;
    for path in COGNITION_PATHS {
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
        assert!(
            response.headers.iter().any(
                |(name, value)| name == "content-type" && value.starts_with("application/json")
            ),
            "{path} did not answer JSON: {:?}",
            response.headers
        );
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
    }
    Ok(())
}

#[test]
fn an_empty_platform_serves_empty_lists_and_still_states_the_minimum_sample() -> Result<()> {
    // The failure this guards: a page reading `minimum_sample` to explain a
    // `null` accuracy finding no such key on a fresh platform, because the
    // body only carried it once a component existed. Premise first: the
    // platform has graded nothing and recorded no precedent, so an empty
    // list is the truth rather than a route that ignores its source.
    let rig = rig()?;
    let (graded, recorded) =
        rig.with_platform(|platform| (platform.self_model().len(), platform.precedents().len()))?;
    assert_eq!(graded, 0, "the premise is an empty self-model");
    assert_eq!(recorded, 0, "the premise is no precedent");

    let (text, body) = body_of(rig.get("/cognition/self-model"));
    assert_eq!(body["components"], serde_json::json!([]), "{text}");
    assert_eq!(
        body["minimum_sample"],
        serde_json::json!(MINIMUM_SAMPLE),
        "{text}"
    );
    assert!(
        body["minimum_sample"].as_u64().unwrap_or(0) > 1,
        "a minimum of one or none would make every graded component calibrated: {text}"
    );

    let (text, body) = body_of(rig.get("/cognition/precedents"));
    assert_eq!(body["precedents"], serde_json::json!([]), "{text}");
    Ok(())
}

#[test]
fn a_monitor_credential_is_refused_and_no_method_but_get_reaches_a_cognition_path() -> Result<()> {
    // The failure this guards: a scrape credential, which holds no
    // portfolio authority, reading what the platform thinks of its own
    // analysts; and a route that could re-grade or forget a component
    // appearing under a cognition path.
    let rig = rig()?;
    // Premise: the same monitor token reaches the one route a monitor may
    // read, so the 403 below is about the role and not the token.
    assert_eq!(rig.call(Method::Get, "/health", MONITOR_TOKEN).status, 200);
    for path in COGNITION_PATHS {
        // Premise: GET is admitted at the viewer role.
        assert_eq!(rig.get(path).status, 200, "{path}");
        let response = rig.call(Method::Get, path, MONITOR_TOKEN);
        assert_eq!(response.status, 403, "{path}");
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
        if COGNITION_PATHS.contains(&route.pattern) {
            assert_eq!(route.method, Method::Get, "{}", route.pattern);
        }
    }
    Ok(())
}

// --- /cognition/self-model --------------------------------------------------

#[test]
fn a_component_below_the_minimum_sample_reports_no_accuracy_and_is_not_calibrated() -> Result<()> {
    // The failure this guards is the engine's own: a component graded once
    // served with an accuracy, which a page would render as a measured hit
    // rate. One graded thesis charges the detector and the analyst; both
    // rows must carry the count and `null`, and be ordered by kind then key
    // so two reads render identically.
    let rig = rig()?;
    assert!(
        rig.with_platform(|platform| platform.self_model().is_empty())?,
        "the premise is an empty self-model"
    );
    let graded = rig.grade(1, true)?;
    assert_eq!(graded, 1, "the premise is one graded thesis");
    let charged: Vec<String> = rig.with_platform(|platform| {
        platform
            .self_model()
            .iter()
            .map(|(key, _)| key.to_string())
            .collect()
    })?;
    assert_eq!(
        charged,
        vec![format!("detector:{DETECTOR}"), format!("analyst:{ANALYST}")],
        "the premise is a detector and an analyst, each charged once"
    );
    let (text, body) = body_of(rig.get("/cognition/self-model"));
    // Premise on the served figure, not the constant: one outcome is below
    // the minimum this body states, or the `null` below proves nothing.
    assert!(
        body["minimum_sample"].as_u64().is_some_and(|min| min > 1),
        "one outcome must be below the served minimum: {text}"
    );
    assert_eq!(
        body["components"],
        serde_json::json!([
            {
                "kind": "analyst",
                "key": ANALYST,
                "samples": 1,
                "accuracy": null,
                "calibrated": false
            },
            {
                "kind": "detector",
                "key": DETECTOR,
                "samples": 1,
                "accuracy": null,
                "calibrated": false
            }
        ]),
        "{text}"
    );
    // Lexicographic by kind, as written, not as a parser re-sorted it.
    let analyst = text.find(r#""kind":"analyst""#).expect("an analyst row");
    let detector = text.find(r#""kind":"detector""#).expect("a detector row");
    assert!(
        analyst < detector,
        "components are not served in (kind, key) order: {text}"
    );
    Ok(())
}

#[test]
fn the_minimum_sample_the_body_states_is_the_count_at_which_the_engine_starts_reporting()
-> Result<()> {
    // The failure this guards: the body's `minimum_sample` drifting from the
    // engine's, so a page explains a `null` with the wrong number, or shows
    // an accuracy beside a count the page says is too small. The route
    // cannot name the engine's constant, so this pins it by behaviour: one
    // outcome at a time, the body must say uncalibrated up to the minimum
    // less one and calibrated at exactly the minimum, with the accuracy the
    // engine's stated formula gives.
    let rig = rig()?;
    let minimum = MINIMUM_SAMPLE;
    assert!(minimum >= 2, "a minimum below two has no 'one short' state");

    let hits = 1usize;
    for n in 1..=minimum {
        // One hit, then misses: (1 + 2) / (n + 4) is the engine's formula.
        assert_eq!(
            rig.grade(n, n <= hits)?,
            1,
            "the premise is one graded thesis"
        );
        let (text, body) = body_of(rig.get("/cognition/self-model"));
        let components = body["components"].as_array().expect("a list");
        assert_eq!(components.len(), 2, "{text}");
        for row in components {
            assert_eq!(row["samples"], serde_json::json!(n), "{text}");
            if n < minimum {
                assert_eq!(row["calibrated"], serde_json::json!(false), "{n}: {text}");
                assert_eq!(row["accuracy"], serde_json::Value::Null, "{n}: {text}");
            } else {
                assert_eq!(row["calibrated"], serde_json::json!(true), "{n}: {text}");
                // Statistics as the engine's own text: (h + k/2) / (n + k)
                // with k = 4 pseudo-counts.
                let expected = (hits as f64 + 2.0) / (n as f64 + 4.0);
                assert_eq!(
                    row["accuracy"],
                    serde_json::json!(expected.to_string()),
                    "{n}: {text}"
                );
            }
        }
        assert_eq!(body["minimum_sample"], serde_json::json!(minimum), "{text}");
    }
    Ok(())
}

// --- /cognition/precedents --------------------------------------------------

#[test]
fn a_precedent_reason_recorded_is_served_as_the_kernel_holds_it_and_in_its_order() -> Result<()> {
    // The failure this guards: a precedents page rendering a placeholder
    // because the route served nothing the REASON stage wrote, or reordering
    // what it did write — a replay that reorders is not a replay. Two
    // instruments over two cycles give two records, so the order is
    // exercised; one would pass a body that ignored it.
    let rig = rig()?;
    let (text, before) = body_of(rig.get("/cognition/precedents"));
    assert_eq!(before["precedents"], serde_json::json!([]), "{text}");

    // REASON convenes on the head of the queue, one opportunity a cycle, and
    // the head stays until it expires (five days), so each instrument gets
    // its own cycle after the first's opportunity has lapsed.
    let second_cycle = start().saturating_add(Duration::from_days(6));
    rig.with_platform(|platform| {
        platform.observe(jumping_bars("ZZZ", 120));
        platform.run_cycle(start());
        platform.observe(jumping_bars("AAA", 120));
        platform.run_cycle(second_cycle)
    })?;
    // Premise: the cycles recorded at least two precedents, and the kernel's
    // own serialisation of them is what the body is compared against.
    let (held, ids) =
        rig.with_platform(|platform| -> Result<(serde_json::Value, Vec<String>)> {
            let ids = platform
                .precedents()
                .iter()
                .map(|precedent| precedent.hypothesis_id.clone())
                .collect();
            let held = serde_json::to_value(platform.precedents())
                .map_err(|error| Error::invalid(error.to_string()))?;
            Ok((held, ids))
        })??;
    assert!(
        ids.len() >= 2,
        "the cycles recorded fewer than two precedents: {ids:?}"
    );
    assert!(
        ids.windows(2).all(|pair| pair[0] != pair[1]),
        "two adjacent precedents share a hypothesis id, so order is untestable: {ids:?}"
    );

    let (text, body) = body_of(rig.get("/cognition/precedents"));
    assert_eq!(body["precedents"], held, "{text}");
    // Order as written, not as a parser re-sorted it: the first recorded
    // id precedes the last in the text.
    let first = text
        .find(&format!(r#""hypothesis_id":"{}""#, ids[0]))
        .expect("the first precedent is in the body");
    let last = text
        .find(&format!(r#""hypothesis_id":"{}""#, ids[ids.len() - 1]))
        .expect("the last precedent is in the body");
    assert!(
        first < last,
        "precedents are not served in the kernel's order: {text}"
    );
    // The digest fields the contract names, on every record.
    for precedent in body["precedents"].as_array().expect("a list") {
        for key in ["nearest", "resolved", "agreeing", "agreement"] {
            assert!(
                precedent["digest"].get(key).is_some(),
                "a precedent's digest lacks {key}: {text}"
            );
        }
    }
    Ok(())
}
