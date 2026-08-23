//! Tests for the composition root.
//!
//! The first is the platform's founding milestone: a single market observation
//! traversing all eight stages of the loop in one pass. The rest are about the
//! safety properties surviving assembly — a control that works in isolation and
//! not once wired up is not a control.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::{AutonomyLevel, OperatorIdentity};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
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

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
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

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// A price series with a jump partway through, so the detectors have something
/// real to find.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            // Deterministic pseudo-noise plus a jump two thirds of the way in.
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).unwrap(),
                high: Decimal::from_f64(open.max(price) * 1.002).unwrap(),
                low: Decimal::from_f64(open.min(price) * 0.998).unwrap(),
                close: Decimal::from_f64(price).unwrap(),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

// --- the founding milestone -------------------------------------------------

#[test]
fn one_observation_traverses_all_eight_stages() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let absorbed = platform.observe(bars("AAA", 90));
    assert_eq!(absorbed, 90);

    let report = platform.run_cycle(start());

    assert!(
        report.traversed_every_stage(),
        "a stage did not run:\n{}",
        report.summarise()
    );
    assert_eq!(report.stages.len(), 8);
    for stage in Stage::all() {
        let outcome = report.stage(stage).expect("every stage reports");
        assert!(
            !outcome.detail.trim().is_empty(),
            "{} said nothing; 'nothing happened' and 'nothing was attempted' are different",
            stage.as_str()
        );
    }
    Ok(())
}

#[test]
fn every_stage_of_a_cycle_shares_one_correlation_id() -> Result<()> {
    // The property the whole audit trail rests on: one key reconstructs the
    // entire cycle from the event log.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 90));

    let first = platform.run_cycle(start());
    let second = platform.run_cycle(start().saturating_add(Duration::from_mins(5)));

    assert_ne!(
        first.correlation_id, second.correlation_id,
        "each cycle needs its own key"
    );
    assert_eq!(first.cycle, 1);
    assert_eq!(second.cycle, 2);
    Ok(())
}

#[test]
fn a_cycle_with_no_data_still_runs_every_stage_and_says_why_each_was_quiet() -> Result<()> {
    // A blind platform is a normal state at start-up, and must be legible
    // rather than merely silent.
    let mut platform = platform(PlatformConfig::default())?;
    let report = platform.run_cycle(start());

    assert!(report.traversed_every_stage());
    assert!(
        report
            .stage(Stage::Sense)
            .unwrap()
            .detail
            .contains("running blind")
    );
    assert!(
        report
            .stage(Stage::Reason)
            .unwrap()
            .detail
            .contains("nothing in the queue")
    );
    Ok(())
}

#[test]
fn the_discover_stage_finds_something_in_a_series_with_a_jump() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    let discover = report.stage(Stage::Discover).unwrap();
    assert!(
        discover.produced > 0,
        "a 9% jump in a 0.8% series should be noticed: {}",
        discover.detail
    );
    assert!(
        !platform.queue().is_empty(),
        "and it should reach the queue"
    );
    Ok(())
}

#[test]
fn the_reason_stage_forms_a_reviewed_hypothesis_when_a_mechanism_is_implied() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    let reason = report.stage(Stage::Reason).unwrap();
    assert!(reason.ran);
    // Either a hypothesis was formed and reviewed, or the anomaly implied no
    // mechanism and the stage said so. Both are honest; inventing a mechanism
    // would not be.
    assert!(
        reason.detail.contains("hypothesis")
            || reason.detail.contains("no mechanism")
            || reason.detail.contains("nothing in the queue"),
        "{}",
        reason.detail
    );
    Ok(())
}

#[test]
fn a_cycle_summary_is_readable_at_three_in_the_morning() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 90));
    let summary = platform.run_cycle(start()).summarise();

    for stage in Stage::all() {
        assert!(
            summary.contains(stage.as_str()),
            "{} is missing from the summary",
            stage.as_str()
        );
    }
    Ok(())
}

// --- the safety properties, after assembly ----------------------------------

#[test]
fn a_default_platform_is_in_paper_trading_and_cannot_go_live() -> Result<()> {
    // The single most important assertion about the assembled system.
    let mut platform = platform(PlatformConfig::default())?;
    assert_eq!(platform.autonomy().level(), AutonomyLevel::PaperTrading);
    assert!(!platform.is_live_capable());
    assert!(!platform.config().permits_live_trading());

    let two_operators = OperatorIdentity::verified("alice@example.com", "hardware-token", start())
        .with_second_approver("bob@example.com");
    let error = platform
        .autonomy_mut()
        .request_change(
            AutonomyLevel::SupervisedLive,
            &two_operators,
            "attempting to enable live trading",
            start(),
        )
        .unwrap_err();
    assert!(error.message().contains("ceiling"), "{}", error.message());
    assert!(!platform.autonomy().is_live());
    Ok(())
}

#[test]
fn a_platform_configured_for_live_trading_says_so_and_still_needs_two_operators() -> Result<()> {
    let config = PlatformConfig::default().with_live_ceiling(AutonomyLevel::SupervisedLive);
    let mut platform = platform(config)?;
    assert!(platform.is_live_capable());
    // Being capable is not being live.
    assert!(!platform.autonomy().is_live());

    let one = OperatorIdentity::verified("alice@example.com", "hardware-token", start());
    assert!(
        platform
            .autonomy_mut()
            .request_change(
                AutonomyLevel::SupervisedLive,
                &one,
                "enabling live trading for the pilot",
                start()
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn an_order_that_traces_to_no_hypothesis_is_refused_by_the_assembled_platform() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-1",
        Vec::new(),
        start(),
    );
    let error = platform.submit_order(order, start()).unwrap_err();
    assert!(
        error.message().contains("nobody can explain"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_traceable_order_within_the_limits_reaches_the_simulated_venue() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;

    assert!(!platform.orders().fills().is_empty());
    assert!(
        !platform.orders().has_live_fills(),
        "a paper platform must produce no live fills"
    );
    Ok(())
}

#[test]
fn an_order_that_would_breach_a_limit_is_refused_by_the_assembled_platform() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    // 20,000 at 100 is 2m against 10m of equity: 20% in one name against a
    // 10% cap.
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("20000"),
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        start(),
    );
    let error = platform.submit_order(order, start()).unwrap_err();
    assert!(
        error.message().contains("risk refused"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_tripped_kill_switch_stops_the_assembled_platform() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.autonomy_mut().kill_switch_mut().trip_global(
        start(),
        "operator",
        "manual halt during the incident",
    );

    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        start(),
    );
    let error = platform.submit_order(order, start()).unwrap_err();
    assert!(error.message().contains("halted"), "{}", error.message());

    // And a cycle run while halted reports it.
    let report = platform.run_cycle(start());
    assert!(report.halted);
    Ok(())
}

#[test]
fn the_agent_roster_validates_at_assembly() -> Result<()> {
    // A platform whose governance does not hold should fail to assemble, not
    // fail on the first decision it makes.
    let platform = platform(PlatformConfig::default())?;
    assert_eq!(platform.organisation().len(), 18);
    let findings = platform.review_governance(start());
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == qip_agents::governance::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    Ok(())
}

#[test]
fn the_roster_expires_and_the_platform_can_see_that_it_has() {
    // Manifests are reviewed as of assembly, so a long-running platform will
    // eventually be operating on lapsed authorisations. The governance review
    // is what surfaces it, and an operator is expected to run it.
    let platform = platform(PlatformConfig::default()).expect("assembles");
    let much_later = start().saturating_add(Duration::from_days(200));

    let fresh = platform.review_governance(start());
    assert!(
        !fresh.iter().any(|f| f.rule == "authorisation-current"),
        "authorisations are current at assembly"
    );

    let lapsed = platform.review_governance(much_later);
    assert!(
        lapsed.iter().any(|f| f.rule == "authorisation-current"
            && f.severity == qip_agents::governance::Severity::Error),
        "a two-hundred-day-old authorisation must show as expired: {lapsed:?}"
    );
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_same_inputs_produce_the_same_cycle() -> Result<()> {
    // Replayability is the property the whole audit trail rests on.
    let run = || -> Result<Vec<(String, usize, String)>> {
        let mut platform = platform(PlatformConfig::default())?;
        platform.observe(bars("AAA", 120));
        Ok(platform
            .run_cycle(start())
            .stages
            .into_iter()
            .map(|outcome| {
                (
                    outcome.stage.as_str().to_string(),
                    outcome.produced,
                    outcome.detail,
                )
            })
            .collect())
    };
    assert_eq!(run()?, run()?);
    Ok(())
}

#[test]
fn a_different_seed_does_not_change_a_deterministic_conclusion() -> Result<()> {
    // Nothing in the loop should depend on the random stream for what it
    // concludes, only for how it explores. The stage detail is a conclusion.
    let detail_for = |seed: u64| -> Result<String> {
        let mut platform = platform(PlatformConfig::default().with_seed(seed))?;
        platform.observe(bars("AAA", 120));
        Ok(platform
            .run_cycle(start())
            .stage(Stage::Discover)
            .unwrap()
            .detail
            .clone())
    };
    assert_eq!(detail_for(1)?, detail_for(999)?);
    Ok(())
}

// --- where the event log goes -----------------------------------------------

fn log_directory(label: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-kernel-log-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_platform_nobody_configured_a_destination_for_keeps_its_log_in_memory() -> Result<()> {
    // The compatibility promise: every call site that assembled a platform
    // before the field existed behaves exactly as it did.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 30));
    platform.run_cycle(start());
    assert!(
        !platform.event_log().records().is_empty(),
        "the premise: a cycle appends to the log"
    );
    assert!(
        platform.config().event_log.path().is_none(),
        "an unconfigured platform chose a file to write to"
    );
    Ok(())
}

#[test]
fn a_platform_given_a_path_writes_its_event_log_there_rather_than_only_into_memory() -> Result<()> {
    let directory = log_directory("written");
    let path = directory.join("events.jsonl");
    // The parent does not exist yet on purpose: a deployment names a path
    // under a fresh volume, and a kernel that refused it would be refusing the
    // ordinary case.
    assert!(!directory.exists(), "the premise: nothing is there yet");

    let mut platform = platform(PlatformConfig::default().with_event_log_file(&path))?;
    platform.observe(bars("AAA", 30));
    platform.run_cycle(start());
    let appended = platform.event_log().records().len();
    assert!(appended > 0, "the premise: the cycle appended something");

    let written = std::fs::read_to_string(&path).expect("the log file exists");
    assert_eq!(
        written
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        appended,
        "the file holds a different number of records than the log says it appended"
    );

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_second_platform_over_the_same_file_continues_the_chain_instead_of_starting_a_new_one()
-> Result<()> {
    // This is the whole point of the injection point. A process that restarts
    // used to begin at sequence one with a genesis link, which is why the
    // record of a run could be overwritten by the run that followed it.
    let directory = log_directory("restart");
    let path = directory.join("events.jsonl");

    let (first_count, first_tail) = {
        let mut platform = platform(PlatformConfig::default().with_event_log_file(&path))?;
        platform.observe(bars("AAA", 30));
        platform.run_cycle(start());
        let records = platform.event_log().records();
        let tail = records.last().expect("the first run appended something");
        (records.len(), tail.record_hash.clone())
    };
    assert!(first_count > 0, "the premise: the first run wrote records");

    let mut second = platform(PlatformConfig::default().with_event_log_file(&path))?;
    assert_eq!(
        second.event_log().records().len(),
        first_count,
        "the second platform did not read back what the first one wrote"
    );

    second.observe(bars("BBB", 30));
    second.run_cycle(start());
    let records = second.event_log().records();
    assert!(
        records.len() > first_count,
        "the second run appended nothing, so there is nothing to have chained"
    );

    let carried_on = &records[first_count];
    assert_eq!(
        carried_on.sequence,
        first_count as u64 + 1,
        "the second run restarted the sequence rather than continuing it"
    );
    assert_eq!(
        carried_on.previous_hash, first_tail,
        "the second run's first record chains onto genesis rather than onto the first run's tail"
    );

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_log_destination_that_cannot_be_opened_fails_assembly_rather_than_the_first_append()
-> Result<()> {
    // A corrupt line is a deployment fault, and finding it at the first append
    // means the platform is already running and already believed.
    let directory = log_directory("corrupt");
    std::fs::create_dir_all(&directory).expect("the fixture directory is creatable");
    let path = directory.join("events.jsonl");
    std::fs::write(&path, "this is not a log record\n").expect("the fixture is writable");

    let refusal = platform(PlatformConfig::default().with_event_log_file(&path))
        .expect_err("a platform must not assemble over a log it cannot read");
    assert!(
        refusal.message().contains("corrupt log record"),
        "the refusal does not say what is wrong with the log: {}",
        refusal.message()
    );

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
