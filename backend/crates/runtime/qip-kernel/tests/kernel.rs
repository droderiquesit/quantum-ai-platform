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
use qip_observability::metrics::{Snapshot, labels, names};
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

// --- what the loop records about itself -------------------------------------
//
// Until these existed, nothing in the platform wrote to `Telemetry` at all.
// Every process constructed one, every process handed it to the kernel, and
// the kernel never called it — so `/metrics` served an empty surface and the
// four Cloud Monitoring alert policies were gated off behind
// `workload_metrics_exist = false` because no descriptor by their names had
// ever been ingested. These tests are what keeps that from happening again
// quietly: each asserts a specific fact reached a specific series, so deleting
// an emission fails a test rather than leaving a dashboard that is merely
// blank, which reads as a quiet platform rather than a blind one.

fn recorded(platform: &Platform) -> Snapshot {
    platform.telemetry().metrics.snapshot()
}

#[test]
fn a_cycle_records_one_run_and_one_timed_outcome_for_every_stage_it_ran() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;

    // The premise: the surface is empty before a cycle runs. Without this the
    // assertions below would pass against a registry pre-loaded by assembly,
    // and would be asserting that `Platform::new` records rather than that the
    // cycle does.
    assert_eq!(
        recorded(&platform).counter_total(names::CYCLES_RUN),
        0,
        "the registry is not empty before the first cycle; these assertions would not be \
         about the cycle"
    );

    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "every stage must have run, or the per-stage counts below prove nothing:\n{}",
        report.summarise()
    );

    let snapshot = recorded(&platform);
    assert_eq!(snapshot.counter_total(names::CYCLES_RUN), 1);
    assert_eq!(
        snapshot.counter_total(names::STAGE_RUNS),
        8,
        "one outcome per stage, labelled; a total of eight is what says the loop reported \
         each stage rather than the cycle once"
    );

    for stage in Stage::all() {
        let by_stage = labels([("stage", stage.as_str())]);
        assert_eq!(
            snapshot.counter(
                names::STAGE_RUNS,
                &labels([("ran", "true"), ("stage", stage.as_str())])
            ),
            1,
            "{} ran but was not counted as having run",
            stage.as_str()
        );
        // The duration is asserted as an observation having been made, not as a
        // value: the clock these tests inject is manual and does not advance,
        // so every stage genuinely took zero. A count of one per stage is the
        // fact under test — `StageOutcome::with_elapsed` existed and was never
        // called, so every stage claimed zero because nothing timed it rather
        // than because nothing elapsed.
        let histogram = snapshot
            .histogram(names::STAGE_DURATION_MS, &by_stage)
            .unwrap_or_else(|| panic!("{} recorded no duration at all", stage.as_str()));
        assert_eq!(
            histogram.count,
            1,
            "{} was timed twice or not at all",
            stage.as_str()
        );
    }

    assert_eq!(
        snapshot
            .histogram(names::CYCLE_DURATION_MS, &labels([]))
            .map(|h| h.count),
        Some(1)
    );
    Ok(())
}

#[test]
fn the_length_of_the_event_log_and_the_journal_write_are_recorded_as_the_cycle_seals_itself()
-> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let report = platform.run_cycle(start());

    // The premise: the cycle really did journal itself, so the counter below is
    // counting a write that happened rather than agreeing with an absence.
    assert!(
        report.events_logged > 0,
        "the cycle logged nothing; there is no journal write to count"
    );

    let snapshot = recorded(&platform);
    assert_eq!(
        snapshot.gauge(names::EVENT_LOG_ENTRIES, &labels([])),
        Some(report.events_logged as f64),
        "the gauge must be the log's own length, not a number computed beside it"
    );
    assert_eq!(
        snapshot.counter(names::EVENTS_PUBLISHED, &labels([("topic", "cycle")])),
        1
    );
    assert_eq!(
        snapshot.counter_total(names::JOURNAL_FAILURES),
        0,
        "nothing failed to journal, so the failure counter must not have been touched"
    );

    // A second cycle moves the gauge, which is what distinguishes a gauge that
    // is set from one that was written once at assembly.
    let second = platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    assert!(second.events_logged > report.events_logged);
    assert_eq!(
        recorded(&platform).gauge(names::EVENT_LOG_ENTRIES, &labels([])),
        Some(second.events_logged as f64)
    );
    Ok(())
}

#[test]
fn an_order_refused_by_a_control_is_counted_against_that_control_and_not_another() -> Result<()> {
    // Two orders refused by two different gates. One counter with a `control`
    // label is only useful if the label distinguishes them; a single
    // `orders_refused` total would say the controls fired twice and leave an
    // operator to guess which.
    let mut platform = platform(PlatformConfig::default())?;

    let untraceable = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-1",
        Vec::new(),
        start(),
    );
    let error = platform
        .submit_order(untraceable, start())
        .expect_err("an order tracing to no hypothesis must be refused");
    assert!(
        error.message().contains("nobody can explain"),
        "{}",
        error.message()
    );

    // 20,000 at 100 is 2m against 10m of equity: 20% in one name against a 10%
    // cap, so this one is refused by the pre-trade risk check rather than by
    // validation.
    let oversized = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("20000"),
        dec!("100"),
        "prop-2",
        vec!["hyp-1".to_string()],
        start(),
    );
    let error = platform
        .submit_order(oversized, start())
        .expect_err("an order breaching a limit must be refused");
    assert!(
        error.message().contains("risk refused"),
        "{}",
        error.message()
    );

    let snapshot = recorded(&platform);
    assert_eq!(
        snapshot.counter(
            names::ORDERS_REFUSED,
            &labels([("control", "order-validation")])
        ),
        1
    );
    assert_eq!(
        snapshot.counter(
            names::ORDERS_REFUSED,
            &labels([("control", "pre-trade-risk")])
        ),
        1
    );
    assert_eq!(
        snapshot.counter_total(names::ORDERS_SUBMITTED),
        0,
        "nothing reached a venue, so nothing may be counted as submitted"
    );
    Ok(())
}

#[test]
fn an_accepted_order_is_counted_where_it_reached_a_venue_and_its_fills_are_not_live() -> Result<()>
{
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

    // The premise: a fill really happened. An assertion that the live-fill
    // counter is zero proves nothing on a platform that filled nothing at all,
    // which is exactly the shape of test this rule exists to forbid.
    assert!(
        !platform.orders().fills().is_empty(),
        "no fill occurred; the live-fill assertion below would hold vacuously"
    );

    let snapshot = recorded(&platform);
    assert_eq!(snapshot.counter_total(names::ORDERS_SUBMITTED), 1);
    assert_eq!(
        snapshot.counter_total(names::ORDERS_FILLED),
        platform.orders().fills().len() as u64
    );
    assert_eq!(
        snapshot.counter_total(names::LIVE_FILLS),
        0,
        "a paper platform filled against a simulated venue and must record no live fill"
    );
    Ok(())
}

#[test]
fn the_reason_stage_records_where_the_router_put_the_decision_and_whether_the_panel_convened()
-> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    // The premise: there was something to reason about. On an empty queue
    // REASON returns before it routes anything, and the assertions below would
    // be about a stage that never ran.
    let reason = report.stage(Stage::Reason).expect("REASON reports");
    assert!(
        !reason.detail.contains("nothing in the queue"),
        "the queue was empty, so no routing decision was made: {}",
        reason.detail
    );

    let snapshot = recorded(&platform);
    assert_eq!(
        snapshot.counter_total(names::REASON_ROUTINGS),
        1,
        "exactly one routing decision per cycle that reasoned"
    );

    // And it agrees with what the stage itself said happened. These are two
    // independently produced accounts of one decision — a sentence for an
    // operator and a counter for a dashboard — and a counter that disagreed
    // with the report beside it would be the more believed of the two because
    // it is the one on a screen.
    let expected = if reason.detail.contains("the panel was not convened") {
        "declined"
    } else {
        "convened"
    };
    let outcomes: Vec<&str> = snapshot
        .series
        .iter()
        .filter(|series| series.name == names::REASON_ROUTINGS)
        .filter_map(|series| series.labels.get("outcome").map(String::as_str))
        .collect();
    assert_eq!(
        outcomes,
        vec![expected],
        "the counter and the stage report disagree about the same decision: {}",
        reason.detail
    );

    // And the rung is labelled with a name from the ladder rather than left
    // off. A routing counter that cannot say where the decision was placed
    // records that a decision happened, which nobody was in doubt about.
    let tiers: Vec<&str> = snapshot
        .series
        .iter()
        .filter(|series| series.name == names::REASON_ROUTINGS)
        .filter_map(|series| series.labels.get("tier").map(String::as_str))
        .collect();
    assert_eq!(tiers.len(), 1, "the routing series carries no tier label");
    assert_ne!(
        tiers[0], "none",
        "the router placed the decision somewhere, but the label says it did not"
    );
    Ok(())
}

#[test]
fn the_compute_a_cycle_was_charged_is_read_off_the_ledger_rather_than_recomputed() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());

    // The premise: the cycle was charged something. A gauge asserted equal to a
    // ledger total that is itself zero would pass however the gauge was set.
    let charged = platform.last_cycle_cost();
    assert!(
        charged.is_positive(),
        "the cycle was charged nothing; there is no bill to check the gauge against"
    );

    let snapshot = recorded(&platform);
    assert!(
        qip_core::testing::approx_eq(
            snapshot
                .gauge(names::CYCLE_COMPUTE_COST, &labels([]))
                .expect("the cycle cost was recorded"),
            charged.to_f64(),
            1e-9,
        ),
        "the gauge is not the ledger's own total"
    );
    assert!(
        qip_core::testing::approx_eq(
            snapshot
                .gauge(names::COMPUTE_SPEND, &labels([]))
                .expect("the running total was recorded"),
            platform.compute_spend().to_f64(),
            1e-9,
        ),
        "the running total on the gauge is not the platform's own"
    );

    // A second cycle must move the running total past one cycle's charge, or
    // the gauge is the per-cycle number under a second name.
    platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    let after = recorded(&platform)
        .gauge(names::COMPUTE_SPEND, &labels([]))
        .expect("the running total is still recorded");
    assert!(
        after > charged.to_f64(),
        "the running total did not accumulate across cycles: {after} against {charged}"
    );
    Ok(())
}

#[test]
fn the_kill_switch_gauge_the_alert_policy_queries_falls_back_when_the_halt_is_cleared() -> Result<()>
{
    // `qip_kill_switch_tripped` is queried by a Cloud Monitoring policy as
    // `max(...) > 0`. A gauge only written when something is wrong stays lit
    // after the halt clears, so both directions are the property under test.
    let mut platform = platform(PlatformConfig::default())?;
    platform.run_cycle(start());
    assert_eq!(
        recorded(&platform).gauge(names::KILL_SWITCH_TRIPPED, &labels([])),
        Some(0.0),
        "a platform that is not halted must report zero, not report nothing"
    );

    platform.autonomy_mut().kill_switch_mut().trip_global(
        start(),
        "operator",
        "a halt raised so the gauge has something to report".to_string(),
    );
    // The premise: the switch really is tripped, or the gauge below could read
    // one for any reason at all.
    assert!(
        platform.autonomy().kill_switch().is_globally_tripped(),
        "the kill switch did not trip"
    );
    platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    assert_eq!(
        recorded(&platform).gauge(names::KILL_SWITCH_TRIPPED, &labels([])),
        Some(1.0)
    );

    let cleared_at = start().saturating_add(Duration::from_mins(10));
    platform.autonomy_mut().kill_switch_mut().clear_global(
        &OperatorIdentity::verified("operator-a", "desk", cleared_at),
        cleared_at,
    )?;
    assert!(
        !platform.autonomy().kill_switch().is_globally_tripped(),
        "the halt did not clear"
    );
    platform.run_cycle(start().saturating_add(Duration::from_mins(15)));
    assert_eq!(
        recorded(&platform).gauge(names::KILL_SWITCH_TRIPPED, &labels([])),
        Some(0.0),
        "the gauge stayed lit after the halt cleared; the alert would never resolve"
    );
    Ok(())
}

#[test]
fn every_pass_of_the_risk_monitor_is_recorded_with_the_breach_count_it_saw() -> Result<()> {
    // The monitor runs whether or not there is anything to trade, and
    // `qip_limit_breaches` is queried by a Cloud Monitoring policy as
    // `max(...) > 0`. Both halves matter: a pass that is not counted makes a
    // silent monitor indistinguishable from a busy one, and a gauge written
    // only when something is wrong never falls back, so the alert stays lit
    // after the breach clears.
    let mut platform = platform(PlatformConfig::default())?;
    platform.run_cycle(start());

    let snapshot = recorded(&platform);
    assert_eq!(
        snapshot.counter_total(names::RISK_EVALUATIONS),
        1,
        "the ACT stage ran, so the monitor observed the book exactly once"
    );
    assert_eq!(
        snapshot.gauge(names::LIMIT_BREACHES, &labels([])),
        Some(0.0),
        "a book inside its limits must report zero breaches, not report nothing; \
         `max() > 0` over a series that stopped reporting sees no halt rather than a halt"
    );

    // A second cycle is a second pass. Without this the counter could be set
    // once at assembly and every assertion above would still hold.
    platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    assert_eq!(
        recorded(&platform).counter_total(names::RISK_EVALUATIONS),
        2
    );
    Ok(())
}

#[test]
fn the_panel_records_the_agents_that_ran_and_records_no_denial_it_did_not_have() -> Result<()> {
    // `qip_permission_denials_total` is the second of the four names the alert
    // policies query. Asserting it is zero is only worth anything if agents
    // actually ran — a panel that never convened has no permission to violate,
    // and a test that could not tell those apart would pass forever.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    let reason = report.stage(Stage::Reason).expect("REASON reports");
    assert!(
        !reason.detail.contains("nothing in the queue"),
        "the panel did not convene, so no agent ran and the denial count is vacuous: {}",
        reason.detail
    );

    let snapshot = recorded(&platform);
    assert!(
        snapshot.counter_total(names::AGENT_RUNS) > 0,
        "the panel convened but no agent run was recorded"
    );
    assert_eq!(
        snapshot.counter_total(names::PERMISSION_DENIALS),
        0,
        "an agent on the shipped roster reached past its manifest, or a run was miscounted \
         as a denial"
    );

    // The problems the stage reported reach the counter too, and they must
    // agree: the sentence an operator reads and the number on a dashboard are
    // two accounts of the same cycle, and the number is the more believed.
    let problems: usize = report
        .stages
        .iter()
        .map(|outcome| outcome.problems.len())
        .sum();
    assert_eq!(
        snapshot.counter_total(names::STAGE_PROBLEMS),
        problems as u64,
        "the problem counter and the cycle report disagree about the same cycle"
    );
    Ok(())
}
