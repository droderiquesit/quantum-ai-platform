//! The research read surface: predictions, correlation, backtests, and the
//! two routes that answer honestly that nothing produces them.
//!
//! Every test here feeds the platform first and asserts the premise before
//! reading the route, because a route that serves an empty collection passes
//! every assertion about shape whether or not the platform ever held the fact.
//! The failure each guards is the one the portal was built around: a page
//! rendering a placeholder because no route served the data, or worse, a
//! number the platform never measured.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::http::{Handler, Method, Request, Response};
use qip_api::routes::{Api, CORRELATION_MINIMUM_CLOSES};
use qip_api::stream::StreamKind;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_events::topic::Topic;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::central::StrategyCandidate;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, LeakageAudit, StrategyEvidence,
};
use qip_lifecycle::trials::StrategyFamily;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// --- fixtures ---------------------------------------------------------------

const VIEWER_TOKEN: &str = "viewer-token";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB", "ZZZ"] {
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

/// The kernel's own learning fixture's limits: wide enough that a thesis is
/// sized rather than refused, which is what makes a prediction exist.
fn limits() -> LimitSet {
    LimitSet::new("research-test")
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
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "viewer@example.com",
        Role::Viewer,
        VIEWER_TOKEN.to_string(),
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
    fn with_platform<T>(&self, f: impl FnOnce(&mut Platform) -> T) -> Result<T> {
        let mut platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        Ok(f(&mut platform))
    }

    fn get(&self, path: &str) -> Response {
        let mut headers = BTreeMap::new();
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {VIEWER_TOKEN}"),
        );
        self.api.handle(&Request {
            method: Method::Get,
            path: path.to_string(),
            query: BTreeMap::new(),
            headers,
            body: Vec::new(),
            peer: "127.0.0.1:1".to_string(),
        })
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

/// Bars whose closes follow `closes` exactly, one a day, ending at `start()`.
fn bars_with_closes(symbol: &str, closes: &[f64]) -> Vec<SensedRecord> {
    let count = closes.len();
    closes
        .iter()
        .enumerate()
        .map(|(i, close)| {
            let open = if i == 0 { *close } else { closes[i - 1] };
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, *close)
        })
        .collect()
}

// --- predictions ------------------------------------------------------------

#[test]
fn predictions_are_served_per_instrument_in_key_order_with_the_claim_the_platform_wrote_down()
-> Result<()> {
    // The failure this guards: the portal's predictions page rendered a
    // placeholder because no route served the REASON stage's claims, while
    // the fastbrain banner counted them in the tens. Two instruments are fed
    // so the grouping and its order are both exercised; a single instrument
    // would pass a body that ignored the key.
    let rig = rig()?;
    let (before_text, before) = body_of(rig.get("/api/v1/predictions"));
    assert_eq!(before["held"], serde_json::json!(0), "{before_text}");
    assert_eq!(before["open"], serde_json::json!(0), "{before_text}");
    assert_eq!(
        before["calibration"]["available"],
        serde_json::json!(false),
        "{before_text}"
    );

    // REASON convenes on the head of the queue, one opportunity a cycle, and
    // the head stays until it expires (five days), so each instrument gets
    // its own cycle and the second runs after the first's opportunity has
    // lapsed. The later instrument sorts first, so a body that listed claims
    // in the order they were made would fail the ordering assertion below.
    let second_cycle = start().saturating_add(Duration::from_days(6));
    rig.with_platform(|platform| {
        platform.observe(jumping_bars("ZZZ", 120));
        platform.run_cycle(start());
        platform.observe(jumping_bars("AAA", 120));
        platform.run_cycle(second_cycle)
    })?;
    // Premise: the cycle wrote at least one claim, and it is open. Without
    // this every assertion below could pass against an empty body.
    let (open_count, subjects) = rig.with_platform(|platform| {
        let subjects: Vec<String> = platform
            .predictions()
            .iter()
            .filter_map(|p| p.claim.as_ref().map(|c| c.subject.clone()))
            .collect();
        (
            platform
                .predictions()
                .iter()
                .filter(|p| p.is_open())
                .count(),
            subjects,
        )
    })?;
    assert!(open_count >= 1, "the cycle wrote no claim");
    assert!(
        subjects.contains(&"obj-AAA".to_string()) && subjects.contains(&"obj-ZZZ".to_string()),
        "both instruments must carry a claim for the ordering to be tested: {subjects:?}"
    );

    let (text, body) = body_of(rig.get("/api/v1/predictions"));
    assert_eq!(body["as_of_cycle"], serde_json::json!(2), "{text}");
    assert_eq!(body["open"], serde_json::json!(open_count), "{text}");
    assert_eq!(
        body["window"],
        serde_json::json!(Platform::prediction_window()),
        "{text}"
    );
    let instruments = body["instruments"]
        .as_object()
        .expect("instruments is an object keyed by instrument");
    assert!(instruments.contains_key("obj-AAA"), "{text}");
    assert!(instruments.contains_key("obj-ZZZ"), "{text}");
    // Key order in the body as written, not as a parser re-sorted it. A
    // replay that reorders is not a replay.
    let aaa = text.find("\"obj-AAA\":{").expect("AAA is a key");
    let zzz = text.find("\"obj-ZZZ\":{").expect("ZZZ is a key");
    assert!(aaa < zzz, "instruments are not served in key order: {text}");

    let first = &instruments["obj-AAA"]["predictions"][0];
    assert_eq!(first["state"], serde_json::json!("open"), "{text}");
    assert!(
        matches!(first["direction"].as_str(), Some("up" | "down")),
        "a claim with a written direction is served without one: {text}"
    );
    let confidence = first["confidence"].as_f64().expect("a confidence");
    assert!((0.0..=1.0).contains(&confidence), "{text}");
    assert!(
        first["horizon_seconds"].as_i64().unwrap_or(0) > 0,
        "the horizon is not served as the gap between made_at and resolves_at: {text}"
    );
    assert!(first["scored_at"].is_null(), "{text}");
    assert!(
        first["metric"]
            .as_str()
            .is_some_and(|metric| metric.ends_with(":obj-AAA")),
        "the metric does not name the instrument's own series: {text}"
    );

    // Resolve the claim the way the kernel's learning test does, and the
    // route must show the verdict and the calibration LEARN produced.
    let horizon = rig
        .with_platform(|platform| {
            platform
                .predictions()
                .iter()
                .find(|p| p.is_open() && p.claim.as_ref().is_some_and(|c| c.subject == "obj-AAA"))
                .map(|p| p.proposition.resolves_at)
        })?
        .expect("the AAA claim just written is open");
    let swings: Vec<SensedRecord> = (0..20)
        .map(|i| {
            let (open, close) = if i % 2 == 0 {
                (100.0, 150.0)
            } else {
                (150.0, 100.0)
            };
            let at = horizon.saturating_sub(Duration::from_mins((20 - i) * 60));
            bar("AAA", at, open, close)
        })
        .collect();
    rig.with_platform(|platform| {
        platform.observe(swings);
        platform.run_cycle(horizon.saturating_add(Duration::from_mins(1)))
    })?;
    let resolved = rig.with_platform(|platform| {
        platform
            .predictions()
            .iter()
            .filter(|p| !p.is_open())
            .count()
    })?;
    assert!(resolved >= 1, "the second cycle settled nothing");
    let calibrated = rig.with_platform(|platform| platform.calibration().is_some())?;
    assert!(calibrated, "LEARN computed no calibration");

    let (text, body) = body_of(rig.get("/api/v1/predictions"));
    assert_eq!(body["resolved"], serde_json::json!(resolved), "{text}");
    let settled: Vec<&str> = body["instruments"]
        .as_object()
        .expect("instruments")
        .values()
        .flat_map(|entry| entry["predictions"].as_array().expect("a list"))
        .filter_map(|p| p["state"].as_str())
        .filter(|state| *state != "open")
        .collect();
    assert_eq!(settled.len(), resolved, "{text}");
    assert!(
        settled
            .iter()
            .all(|state| matches!(*state, "held" | "failed")),
        "a settled claim is served in a state that is neither held nor failed: {settled:?}"
    );
    assert_eq!(
        body["calibration"]["available"],
        serde_json::json!(true),
        "{text}"
    );
    assert!(
        body["calibration"]["report"]["evaluated"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "{text}"
    );
    assert!(
        body["calibration"]["report"]["brier_score"].is_number(),
        "{text}"
    );
    Ok(())
}

// --- correlation ------------------------------------------------------------

/// Closes whose simple returns are exactly the negation of `reference`'s,
/// so the Pearson coefficient between them is -1 by construction.
fn mirrored_closes(reference: &[f64]) -> Vec<f64> {
    let mut closes = vec![100.0];
    for pair in reference.windows(2) {
        let r = (pair[1] - pair[0]) / pair[0];
        let last = *closes.last().expect("seeded");
        closes.push(last * (1.0 - r));
    }
    closes
}

#[test]
fn correlation_is_served_over_the_tape_with_its_window_and_refuses_what_it_cannot_estimate()
-> Result<()> {
    // The failure this guards: a correlation matrix rendered from nothing, or
    // one carrying `NaN` where a series had no variance — which JSON cannot
    // hold and a chart plots as zero. The body must state the window and the
    // minimum so the coefficient is reproducible, exclude what falls below
    // the minimum by name, and write `null` with a reason where the statistic
    // is undefined.
    let rig = rig()?;
    let (text, before) = body_of(rig.get("/api/v1/correlation"));
    assert_eq!(before["available"], serde_json::json!(false), "{text}");
    assert_eq!(
        before["minimum_closes"],
        serde_json::json!(CORRELATION_MINIMUM_CLOSES),
        "{text}"
    );
    assert_eq!(
        before["instruments_observed"],
        serde_json::json!([]),
        "{text}"
    );

    let reference: Vec<f64> = (0..60)
        .map(|i| 100.0 + (i as f64 * 1.3).sin() * 4.0 + i as f64 * 0.1)
        .collect();
    let mirrored = mirrored_closes(&reference);
    let short: Vec<f64> = reference[..10].to_vec();
    let flat = vec![100.0; 60];
    rig.with_platform(|platform| {
        platform.observe(bars_with_closes("AAA", &reference));
        platform.observe(bars_with_closes("BBB", &mirrored));
        platform.observe(bars_with_closes("CCC", &short));
        platform.observe(bars_with_closes("DDD", &flat));
        platform.run_cycle(start())
    })?;
    // Premise: the tape holds what was fed, at the lengths the assertions
    // depend on.
    let lengths: BTreeMap<String, usize> = rig.with_platform(|platform| {
        platform
            .price_history()
            .iter()
            .map(|(k, v)| (k.clone(), v.len()))
            .collect()
    })?;
    assert_eq!(lengths.get("obj-AAA"), Some(&60), "{lengths:?}");
    assert_eq!(lengths.get("obj-BBB"), Some(&60), "{lengths:?}");
    assert_eq!(lengths.get("obj-CCC"), Some(&10), "{lengths:?}");
    assert_eq!(lengths.get("obj-DDD"), Some(&60), "{lengths:?}");
    // The minimum sits between the short series and the long ones, or the
    // exclusion below would be testing nothing.
    assert!(
        lengths["obj-CCC"] < CORRELATION_MINIMUM_CLOSES
            && CORRELATION_MINIMUM_CLOSES <= lengths["obj-AAA"],
        "the fixture's lengths do not straddle the minimum: {lengths:?}"
    );

    let (text, body) = body_of(rig.get("/api/v1/correlation"));
    assert!(!text.contains("NaN"), "the body carries NaN: {text}");
    assert_eq!(body["available"], serde_json::json!(true), "{text}");
    assert_eq!(body["as_of_cycle"], serde_json::json!(1), "{text}");
    assert_eq!(body["window_closes"], serde_json::json!(60), "{text}");
    assert_eq!(body["window_returns"], serde_json::json!(59), "{text}");
    assert_eq!(
        body["minimum_closes"],
        serde_json::json!(CORRELATION_MINIMUM_CLOSES),
        "{text}"
    );
    assert_eq!(
        body["instruments"],
        serde_json::json!(["obj-AAA", "obj-BBB", "obj-DDD"]),
        "{text}"
    );

    let matrix = &body["matrix"];
    // Exactly one, as a number: not 0.9999999999999998, and not null.
    assert_eq!(matrix["obj-AAA"]["obj-AAA"].as_f64(), Some(1.0), "{text}");
    assert_eq!(matrix["obj-BBB"]["obj-BBB"].as_f64(), Some(1.0), "{text}");
    let ab = matrix["obj-AAA"]["obj-BBB"]
        .as_f64()
        .expect("a coefficient");
    assert!(
        (ab + 1.0).abs() < 1e-9,
        "returns built to be exact mirrors correlate at {ab}, not -1: {text}"
    );
    assert_eq!(
        matrix["obj-AAA"]["obj-BBB"], matrix["obj-BBB"]["obj-AAA"],
        "the matrix is not symmetric: {text}"
    );
    // A flat series has no variance: the coefficient is undefined, written
    // as null on every pair that touches it — including its own diagonal —
    // and named once under `undefined`.
    assert!(matrix["obj-AAA"]["obj-DDD"].is_null(), "{text}");
    assert!(matrix["obj-DDD"]["obj-DDD"].is_null(), "{text}");
    let undefined = body["undefined"].as_array().expect("a list");
    assert!(
        undefined.iter().any(|pair| {
            pair["a"] == serde_json::json!("obj-AAA")
                && pair["b"] == serde_json::json!("obj-DDD")
                && pair["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("zero return variance"))
        }),
        "the undefined pair is not named with its reason: {text}"
    );

    let excluded = body["excluded"].as_array().expect("a list");
    assert_eq!(excluded.len(), 1, "{text}");
    assert_eq!(
        excluded[0]["instrument"],
        serde_json::json!("obj-CCC"),
        "{text}"
    );
    assert_eq!(excluded[0]["closes"], serde_json::json!(10), "{text}");
    assert!(
        excluded[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(&CORRELATION_MINIMUM_CLOSES.to_string())),
        "{text}"
    );
    assert!(
        matrix.get("obj-CCC").is_none(),
        "an instrument below the minimum reached the matrix: {text}"
    );
    Ok(())
}

// --- backtests --------------------------------------------------------------

fn compile(id: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = object(id);
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

fn good_returns(seed: u64, n: usize, drift: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n)
        .map(|_| {
            let u = rng.next_f64() + rng.next_f64() - 1.0;
            drift + u * 0.01
        })
        .collect()
}

fn holdout_evidence(observations: usize, trials: usize) -> Result<StrategyEvidence> {
    let (folds, label_horizon, embargo) = (5, 10, 5);
    let splits = PurgedSplit::new(folds, label_horizon, embargo)?.split(observations)?;
    let holdout = HoldoutEvidence {
        holdout_returns: good_returns(1, observations, 0.0018),
        in_sample_folds: (0..5).map(|f| good_returns(10 + f, 80, 0.0020)).collect(),
        out_of_sample_folds: (0..5).map(|f| good_returns(20 + f, 80, 0.0018)).collect(),
        trials,
        periods_per_year: 252.0,
        cross_validation: CrossValidationRun {
            folds,
            label_horizon,
            embargo,
            observations,
            purged: splits.iter().map(|s| s.purged).sum(),
            embargoed: splits.iter().map(|s| s.embargoed).sum(),
        },
        leakage: LeakageAudit {
            timings: (0..8)
                .map(|i| FeatureTiming {
                    feature: format!("feature-{i}"),
                    known_at: start(),
                    used_at: start().saturating_add(Duration::from_hours(1)),
                })
                .collect(),
            restated_without_snapshots: Vec::new(),
        },
    };
    Ok(StrategyEvidence::new().with_holdout(holdout))
}

#[test]
fn backtests_serve_the_holdout_evidence_the_gate_findings_and_the_band_the_ledger_recorded()
-> Result<()> {
    // The failure this guards: the portal's backtests page rendered a
    // placeholder while the deepbrain banner counted trials on the ledger.
    // The route serves what the ledger holds — evidence, findings, band — and
    // says in words that no equity curve and no numeric deflated Sharpe are
    // kept, rather than drawing either.
    let rig = rig()?;
    let (text, before) = body_of(rig.get("/api/v1/backtests"));
    assert_eq!(before["strategies"], serde_json::json!([]), "{text}");
    assert_eq!(
        before["trial_book"]["attached"],
        serde_json::json!(true),
        "{text}"
    );
    assert_eq!(
        before["trial_book"]["durable"],
        serde_json::json!(false),
        "{text}"
    );
    assert_eq!(
        before["equity_curve"]["available"],
        serde_json::json!(false),
        "{text}"
    );
    assert_eq!(
        before["deflated_sharpe"]["available"],
        serde_json::json!(false),
        "{text}"
    );

    let id = StrategyId::new("AAA");
    let (observations, trials) = (400, 12);
    rig.with_platform(|platform| -> Result<()> {
        let (compiled, program) = compile("AAA")?;
        let candidate = StrategyCandidate::new(
            compiled,
            program,
            StrategyFamily::new("research-tests")?,
            "london-1",
            VenueId::new("XNYS"),
            start(),
        )?
        .with_evidence(holdout_evidence(observations, trials)?);
        let factory = platform.central_mut().factory_mut();
        factory.register(candidate)?;
        factory.promote(&id, None, "the holdout gate passed", start())?;
        Ok(())
    })??;
    // Premise: the ledger holds a holdout admission with a band.
    let (stage, band_present) = rig.with_platform(|platform| {
        let factory = platform.central().factory();
        (
            factory.stage_of(&id),
            factory.ledger().holdout_band(&id).is_some(),
        )
    })?;
    assert_eq!(stage, qip_contracts::gate::GateStage::Holdout);
    assert!(band_present, "the holdout gate admitted without a band");

    let (text, body) = body_of(rig.get("/api/v1/backtests"));
    let strategies = body["strategies"].as_array().expect("a list");
    assert_eq!(strategies.len(), 1, "{text}");
    let strategy = &strategies[0];
    assert_eq!(strategy["strategy"], serde_json::json!("AAA"), "{text}");
    assert_eq!(
        strategy["family"],
        serde_json::json!("research-tests"),
        "{text}"
    );
    assert_eq!(strategy["stage"], serde_json::json!("holdout"), "{text}");

    let holdout = &strategy["holdout"];
    assert_eq!(holdout["submitted"], serde_json::json!(true), "{text}");
    assert_eq!(
        holdout["observations"],
        serde_json::json!(observations),
        "{text}"
    );
    assert_eq!(
        holdout["trials_this_run"],
        serde_json::json!(trials),
        "{text}"
    );
    assert_eq!(holdout["periods_per_year"].as_f64(), Some(252.0), "{text}");
    assert_eq!(
        holdout["cross_validation"]["folds"],
        serde_json::json!(5),
        "{text}"
    );

    // The gate charged the book and did not write the account back onto the
    // factory's evidence, so the evidence-side account is honestly absent
    // and the family's lifetime count is served from the book beside it.
    let account = &strategy["trial_account"];
    assert_eq!(account["on_evidence"], serde_json::json!(false), "{text}");
    assert!(
        account["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("family_lifetime_trials")),
        "{text}"
    );
    assert_eq!(
        strategy["family_lifetime_trials"],
        serde_json::json!(trials),
        "{text}"
    );

    let band = &strategy["holdout_band"];
    assert_eq!(band["present"], serde_json::json!(true), "{text}");
    let sharpe = band["sharpe"].as_f64().expect("the holdout Sharpe");
    let lower = band["lower"].as_f64().expect("a lower bound");
    let upper = band["upper"].as_f64().expect("an upper bound");
    assert!(lower < sharpe && sharpe < upper, "{text}");
    assert_eq!(
        band["observations"],
        serde_json::json!(observations),
        "{text}"
    );
    assert_eq!(band["trials"], serde_json::json!(trials), "{text}");

    let moves = strategy["ledger"].as_array().expect("a list");
    assert_eq!(moves.len(), 1, "{text}");
    assert_eq!(moves[0]["from"], serde_json::json!("candidate"), "{text}");
    assert_eq!(moves[0]["to"], serde_json::json!("holdout"), "{text}");
    assert_eq!(
        moves[0]["gate"]["passed"],
        serde_json::json!(true),
        "{text}"
    );
    let findings = moves[0]["gate"]["findings"].as_array().expect("a list");
    assert!(
        findings.iter().any(|finding| {
            finding["check"] == serde_json::json!("deflated_sharpe_above_selection")
                && finding["passed"] == serde_json::json!(true)
                && finding["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("Sharpe"))
        }),
        "the deflated Sharpe finding the gate recorded is not served: {text}"
    );

    let families = body["trial_book"]["families"].as_array().expect("a list");
    assert!(
        families.iter().any(|family| {
            family["family"] == serde_json::json!("research-tests")
                && family["lifetime_trials"] == serde_json::json!(trials)
        }),
        "the family's lifetime trial count is not served: {text}"
    );
    Ok(())
}

// --- what nothing produces --------------------------------------------------

#[test]
fn regimes_and_news_answer_that_nothing_produces_them_and_name_the_declared_stream_topic()
-> Result<()> {
    // The failure this guards: a portal page declaring "no regime data" as a
    // frontend guess, when the platform can say — and should — that no
    // classifier runs and that the `regime.changed` topic its stream declares
    // has no publisher.
    let rig = rig()?;
    let (text, regimes) = body_of(rig.get("/api/v1/regimes"));
    assert_eq!(regimes["available"], serde_json::json!(false), "{text}");
    assert!(
        regimes["reason"].as_str().is_some_and(
            |reason| reason.contains("regime.changed") && reason.contains("UNDERSTAND")
        ),
        "the reason does not name the topic and what would produce it: {text}"
    );
    let topic = &regimes["stream_topic"];
    assert_eq!(
        topic["name"],
        serde_json::json!(Topic::RegimeChanged.name()),
        "{text}"
    );
    assert_eq!(topic["published"], serde_json::json!(false), "{text}");
    assert_eq!(
        topic["declared_on"],
        serde_json::json!("/api/v1/stream/signals"),
        "{text}"
    );
    // The statement and the stream must agree: the topic the route says is
    // declared is the topic the stream actually filters on.
    assert!(
        StreamKind::Signals.topics().contains(&Topic::RegimeChanged),
        "the route names a topic the signals stream does not declare"
    );

    let (text, news) = body_of(rig.get("/api/v1/news"));
    assert_eq!(news["available"], serde_json::json!(false), "{text}");
    assert!(
        news["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no narrative adapter")),
        "{text}"
    );
    Ok(())
}
