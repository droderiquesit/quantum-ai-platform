//! The feature-name contract, held at the seam where it can break.
//!
//! The analysts in `qip-investment-agents` read the feature store by name and
//! key; the absorb arms in `Platform::observe` write it by name and key. The
//! two are in different crates and, for the whole life of every deployed
//! binary before this test, agreed on nothing: the macro analyst read
//! `policy_rate@global` while the macro arm wrote `macro_level@US.CPI.YOY`,
//! and the alt-data analyst read `web_traffic_index` while the kernel wrote
//! `alt/web-traffic/web_traffic_index`. Each crate's own tests passed. Only
//! a test that drives the real arms and then reads with the real analysts'
//! names can see both halves, which is why it lives here and not beside
//! either.
//!
//! Every read is either written by an arm or declared unwritten in
//! `qip_world_model::vocabulary::UNWRITTEN` with the record kind that would
//! write it. The test refuses both a read that is neither, and a declaration
//! an arm has since made false.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::intelligence::{AlternativeDataPoint, MacroObservation};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_investment_agents::vocabulary::reads;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_world_model::vocabulary::{AltMetric, MacroSeries, SubjectKind, names, unwritten};

const ECONOMY: &str = "US";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-AAA")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(object(), "AAA", InstrumentType::CommonStock)
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .geography(ECONOMY)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        LimitSet::conservative_default(),
    )
}

fn release(series: MacroSeries, value: f64) -> SensedRecord {
    let reference = start().saturating_sub(Duration::from_days(20));
    SensedRecord::Macro(Box::new(MacroObservation {
        series_id: series.series_id(ECONOMY),
        region: ECONOMY.to_string(),
        value,
        unit: "percent".to_string(),
        reference_date: reference,
        consensus: Some(value),
        previous: None,
        is_revision: false,
        provenance: Provenance::new("statistics-office", reference, start()),
        quality: DataQuality::clean(),
    }))
}

fn reading(dataset: &str, metric: &str, value: f64) -> SensedRecord {
    let observed = start().saturating_sub(Duration::from_days(2));
    SensedRecord::AlternativeData(Box::new(AlternativeDataPoint {
        dataset: dataset.to_string(),
        subject_id: object().as_str().to_string(),
        metric: metric.to_string(),
        value,
        unit: "index".to_string(),
        observed_at: observed,
        lead_days: 14.0,
        proxy_correlation: 0.6,
        proxies_for: Some(names::REVENUE.to_string()),
        provenance: Provenance::new("alt-vendor", observed, start()),
        quality: DataQuality::clean(),
    }))
}

fn bar() -> SensedRecord {
    let close_at = start().saturating_sub(Duration::from_hours(1));
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: "XNYS".to_string(),
        interval: Interval::Hour,
        open_time: close_at.saturating_sub(Duration::from_hours(1)),
        open: dec!("100"),
        high: dec!("101"),
        low: dec!("99"),
        close: dec!("100.5"),
        volume: dec!("1000"),
        vwap: None,
        trade_count: 10,
        quality: DataQuality::clean(),
    }))
}

/// One record of every kind that feeds a feature an analyst reads, keyed the
/// way the analyst will read it.
fn one_of_everything() -> Vec<SensedRecord> {
    let mut records = vec![bar()];
    records.extend(
        MacroSeries::ALL
            .into_iter()
            .map(|series| release(series, 2.5)),
    );
    records.extend(
        AltMetric::ALL
            .into_iter()
            .map(|metric| reading(metric.dataset(), metric.feature(), 100.0)),
    );
    records
}

#[test]
fn every_feature_an_analyst_reads_is_written_by_an_absorb_arm_or_declared_unwritten() -> Result<()>
{
    // The premise: there are reads to check, from more than one analyst.
    let reads = reads();
    assert!(
        reads.len() >= 6,
        "premise: only {} analyst reads are declared",
        reads.len()
    );
    let analysts: std::collections::BTreeSet<&str> = reads.iter().map(|r| r.agent).collect();
    assert!(
        analysts.len() >= 4,
        "premise: only {analysts:?} read the feature store"
    );

    let mut platform = platform()?;
    let records = one_of_everything();
    let expected = records.len();
    let absorbed = platform.observe(records);
    assert_eq!(absorbed, expected, "every record was absorbed");

    let world = platform.world();
    let far_future = start().saturating_add(Duration::from_days(365));
    let mut orphans = Vec::new();
    let mut false_declarations = Vec::new();
    let mut written = Vec::new();
    for read in &reads {
        let subject = match read.read.keyed_by {
            SubjectKind::Economy => ECONOMY.to_string(),
            SubjectKind::Instrument => object().as_str().to_string(),
            SubjectKind::Entity => "ent-aaa".to_string(),
        };
        let has_series = !world
            .features()
            .history(read.read.name, &subject, far_future)
            .is_empty();
        let label = format!(
            "{} reads {}@{}",
            read.agent,
            read.read.name,
            read.read.keyed_by.as_str()
        );
        match (unwritten(read.read), has_series) {
            (None, false) => orphans.push(label),
            (Some(_), true) => false_declarations.push(label),
            (None, true) => written.push(label),
            (Some(_), false) => {}
        }
    }

    assert!(
        orphans.is_empty(),
        "an analyst reads a feature no absorb arm writes and no declaration admits: {orphans:?}"
    );
    assert!(
        false_declarations.is_empty(),
        "declared unwritten, but an arm writes it — remove the declaration: {false_declarations:?}"
    );
    // The contract is not vacuous: the macro and alternative-data reads are
    // the ones the arms now feed, and both kinds of key were exercised.
    assert!(
        written.iter().any(|w| w.ends_with("@economy")),
        "no economy-keyed read was written: {written:?}"
    );
    assert!(
        written.iter().any(|w| w.ends_with("@instrument")),
        "no instrument-keyed read was written: {written:?}"
    );
    Ok(())
}

#[test]
fn an_alternative_data_series_carries_its_dataset_as_provenance_not_as_a_path() -> Result<()> {
    let mut platform = platform()?;
    let metric = AltMetric::WebTrafficIndex;
    let absorbed = platform.observe(vec![reading(metric.dataset(), metric.feature(), 100.0)]);
    assert_eq!(absorbed, 1);

    let world = platform.world();
    let definition = world
        .features()
        .definition(metric.feature())
        .unwrap_or_else(|| panic!("{} is not defined after a reading landed", metric.feature()));
    assert_eq!(
        definition.producer,
        metric.dataset(),
        "the series does not name the dataset it was read from"
    );
    assert!(
        world
            .features()
            .definition(&format!("alt/{}/{}", metric.dataset(), metric.feature()))
            .is_none(),
        "the reading still lands under the path-prefixed name nothing reads"
    );
    Ok(())
}

#[test]
fn a_vocabulary_metric_from_another_dataset_is_refused_and_reported_not_stored() -> Result<()> {
    // The licence is per dataset and the analyst checks it by dataset. A
    // reading of `web_traffic_index` from a dataset the vocabulary does not
    // hold it under would be stored under a name a licence for `web-traffic`
    // admits, which is a licensing bypass wearing the shape of a feature.
    let mut platform = platform()?;
    let metric = AltMetric::WebTrafficIndex;
    let absorbed = platform.observe(vec![reading("scraped-web", metric.feature(), 100.0)]);
    assert_eq!(absorbed, 0, "the refused reading was counted as absorbed");
    assert!(
        platform
            .world()
            .features()
            .history(metric.feature(), object().as_str(), Timestamp::MAX)
            .is_empty(),
        "the refused reading was stored under the licensed name"
    );

    // Refused visibly: the LEARN stage of the next cycle carries the reason.
    let report = platform.run_cycle(start());
    let learn = report
        .stage(Stage::Learn)
        .expect("LEARN reports every cycle");
    assert!(
        learn
            .problems
            .iter()
            .any(|p| p.contains("scraped-web") && p.contains(metric.dataset())),
        "the refusal was swallowed: {:?}",
        learn.problems
    );
    Ok(())
}
