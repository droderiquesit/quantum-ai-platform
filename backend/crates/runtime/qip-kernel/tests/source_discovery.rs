//! Discovery starts from what the platform is wrong about or blind to and
//! has no source for (DATA-031).
//!
//! The target generator was proven in `qip-data-finder` on hand-written
//! observations. These drive the real ones: theses scored through
//! `Platform::learn_from` — the call the LEARN stage makes — a universe the
//! world model has or has not seen a bar for, and a source the finder
//! actually registered.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ObjectId, dec};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::discovery_targets::TargetReason;
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_events::Topic;
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::source_discovery::DISCOVERY_SPIKE_MULTIPLE;
use qip_learning_engine::{Outcome, ThesisClaim};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform(universe: Universe) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

/// A thesis about `subject` expecting +100bp, and what happened instead.
fn scored(id: &str, subject: &str, realised_bps: f64) -> (ThesisClaim, Outcome) {
    (
        ThesisClaim {
            hypothesis_id: id.to_string(),
            class: "momentum".to_string(),
            subject: subject.to_string(),
            formed_at: start(),
            resolves_at: start().saturating_add(Duration::from_secs(60)),
            direction: 1.0,
            expected_move_bps: 100.0,
            confidence: 0.6,
            falsifiers: vec![],
            contributors: vec![],
        },
        Outcome {
            hypothesis_id: id.to_string(),
            observed_at: start().saturating_add(Duration::from_secs(61)),
            realised_move_bps: realised_bps,
            realised_pnl: 0.0,
            falsifiers_triggered: vec![],
            mechanism_confirmed: None,
        },
    )
}

const COVER_URL: &str = "https://cover.example/quotes";

/// A candidate declaring `instrument`, licensed for what the platform's
/// finder assesses against.
fn covering(instrument: &str) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new("cover", "cover feed", "Example Data Ltd")?,
        SourceEndpoint::parse(
            COVER_URL,
            AccessMechanism::Rest {
                auth: AuthRequirement::None,
                incremental_parameter: None,
                page_size: 100,
            },
        )?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::UsEast],
            [instrument.to_string()],
            UpdateFrequency::Minutely,
        )?,
        LicensingPosture::declared(SourceLicense::new(
            "vendor-terms-2026",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
        SourceCost::free(Currency::USD),
        SourceRegion::UsEast,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        start(),
    )
}

fn probe() -> InMemoryProbe {
    InMemoryProbe::new()
        .with_robots(
            "cover.example",
            RobotsFetch::Served {
                body: "User-agent: *\nAllow: /\n".to_string(),
                latency: Duration::from_millis(12),
            },
        )
        .with_head(
            COVER_URL,
            HeadResponse {
                status: 200,
                content_type: Some("application/json".to_string()),
                content_length: Some(64),
                last_modified: Some(start()),
                latency: Duration::from_millis(40),
            },
        )
        .with_sample(
            COVER_URL,
            PayloadSample {
                body: r#"{"symbol":"X","bid":10.25,"ask":10.27}"#.to_string(),
                media_type: "application/json".to_string(),
                payload_at: Some(start()),
                latency: Duration::from_millis(55),
            },
        )
}

#[test]
fn a_forecast_error_spike_on_an_entity_no_source_covers_becomes_a_discovery_target_naming_it()
-> Result<()> {
    // An empty universe, so nothing here is a world-model gap and every
    // target below is a forecast error's doing.
    let mut platform = platform(Universe::new())?;
    let now = start().saturating_add(Duration::from_secs(120));
    assert!(platform.discovery_targets(now)?.is_empty());

    // Four theses that landed 10bp from where they said, and two that landed
    // 500bp away — one on an entity nothing covers, one on an entity a
    // source is about to be registered for.
    let (claims, outcomes): (Vec<_>, Vec<_>) = [
        scored("h1", "ent-quiet-1", 110.0),
        scored("h2", "ent-quiet-2", 110.0),
        scored("h3", "ent-quiet-3", 110.0),
        scored("h4", "ent-quiet-4", 110.0),
        scored("h5", "ent-northwind", 600.0),
        scored("h6", "ent-covered", 600.0),
    ]
    .into_iter()
    .unzip();
    platform.learn_from(&claims, &outcomes, now)?;

    // The premise: the window holds the six, each naming its entity, and
    // both large misses really are spikes against the platform's usual
    // error — so when one of them stops being a target below, it is the
    // cover that removed it and not the arithmetic.
    assert_eq!(platform.evaluations().len(), 6);
    let errors = platform.forecast_errors();
    let ratio = |entity: &str| {
        errors
            .iter()
            .find(|error| error.entity == entity)
            .map(|error| error.error / error.baseline)
    };
    assert!(ratio("ent-northwind").is_some_and(|r| r >= DISCOVERY_SPIKE_MULTIPLE));
    assert!(ratio("ent-covered").is_some_and(|r| r >= DISCOVERY_SPIKE_MULTIPLE));
    assert!(ratio("ent-quiet-1").is_some_and(|r| r < 1.0));

    // A pass names what it set out to find before anything registered: both.
    let mut probe = probe();
    let assessment = platform.assess_sources(vec![covering("ent-covered")?], &mut probe, now)?;
    assert_eq!(assessment.registered(), 1, "{:?}", assessment.decisions);
    let named: Vec<&str> = assessment
        .targets
        .iter()
        .map(|target| target.entity.as_str())
        .collect();
    assert_eq!(named, ["ent-covered", "ent-northwind"]);

    // With the source registered, only the entity nothing covers remains.
    let targets = platform.discovery_targets(now)?;
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].entity, "ent-northwind");
    assert!(
        matches!(targets[0].reason, TargetReason::ForecastErrorSpike { ratio } if ratio >= DISCOVERY_SPIKE_MULTIPLE),
        "the target is not there for its forecast error: {:?}",
        targets[0].reason
    );
    Ok(())
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

#[test]
fn a_universe_entity_the_world_model_has_no_close_for_is_a_gap_until_a_bar_arrives() -> Result<()> {
    let mut platform = platform(universe())?;
    let gaps = |platform: &Platform| -> Vec<String> {
        platform
            .knowledge_gaps(start())
            .into_iter()
            .map(|gap| gap.entity)
            .collect()
    };
    // The premise: both instruments are in the universe and neither has been
    // observed, so both are gaps and both are targets for that reason.
    assert_eq!(gaps(&platform), ["obj-AAA", "obj-BBB"]);
    let targets = platform.discovery_targets(start())?;
    assert_eq!(targets.len(), 2);
    assert!(
        targets
            .iter()
            .all(|target| target.reason == TargetReason::KnowledgeGap)
    );

    // A bar that closed two days ago: knowable now.
    let absorbed = platform.observe(vec![SensedRecord::Bar(Box::new(Bar {
        object_id: object("AAA"),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: start().saturating_sub(Duration::from_days(3)),
        open: dec!("100"),
        high: dec!("101"),
        low: dec!("99"),
        close: dec!("100.5"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))]);
    assert_eq!(absorbed, 1, "the bar was not absorbed");
    assert_eq!(gaps(&platform), ["obj-BBB"]);
    Ok(())
}
