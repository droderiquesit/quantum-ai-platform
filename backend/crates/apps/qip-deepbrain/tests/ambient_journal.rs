//! The ambient mesh's journal, reached from the deep brain's run loop
//! (R12-AMBIENT-1005/1026/1140/1141/1144/7025/3027/3065): the research
//! sweep's signals and the attention router's answers leave a hash-chained
//! record on a schedule nobody requested, a NOW surprise's wakes name the
//! specialist they woke, and an anomaly scan's signals are on the record
//! with the routing decision each received.
//!
//! Each test asserts its premise first, so an empty journal cannot pass for
//! a quiet mesh.

use qip_contracts::ambient::{Disposition, SignalClass};
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::ambient_journal::{
    AMBIENT_ORIGIN, AmbientJournal, AmbientRecord, JournalDestination, records,
};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::research::{PERSIST_SWEEPS, ResearchConfig, ResearchLoop};
use qip_deepbrain::status::NodeStatus;
use qip_events::EventLog;
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::{EventLogDestination, Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("the platform assembles: {e}"))
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn bar(symbol: &str, at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap_or_else(|| panic!("a price"));
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(symbol.to_string()),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price(close),
        high: price(close * 1.001),
        low: price(close * 0.999),
        close: price(close),
        volume: Decimal::from_int(1_000_000),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

fn journal() -> AmbientJournal {
    AmbientJournal::open(&JournalDestination::InMemory)
        .unwrap_or_else(|e| panic!("the journal opens: {e}"))
}

fn read(log: &EventLog) -> Vec<AmbientRecord> {
    assert!(log.verify_chain().is_ok(), "the journal's chain verifies");
    records(log).unwrap_or_else(|e| panic!("the journal decodes: {e}"))
}

/// Run the node for two cycles with the research loop at `every` and the
/// journal attached; return what the journal holds.
fn run_node(every: u64) -> Vec<AmbientRecord> {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
    let config = PlatformConfig::default();
    let context = Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(2),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(start()).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let coverage = BTreeMap::from([("Equity".to_string(), set(&["obj-A", "obj-B"]))]);
    let mut research = ResearchLoop::new(
        ResearchConfig {
            every_cycles: every,
            ..ResearchConfig::default()
        },
        coverage,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut ambient = journal();
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        None,
        Loops {
            desk: None,
            research: Some(&mut research),
            ambient: Some(&mut ambient),
            ..Loops::default()
        },
        |outcome| {
            assert!(
                outcome.side_failures.is_empty(),
                "{:?}",
                outcome.side_failures
            );
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    read(ambient.log())
}

/// R12-AMBIENT-1005/7025: the run loop, with nobody asking, journals every
/// sweep's signals and the attention events the router answered them with,
/// each attention event caused by its signal; a loop switched off journals
/// nothing. Mutation: drop the `journal.record` call in `node::run_with`, or
/// the `round.signals.push` in `ResearchLoop::sweep_over` — the journal is
/// empty.
#[test]
fn the_run_loop_journals_each_sweeps_signals_and_attention_unprompted_and_a_disabled_loop_none() {
    let journaled = run_node(1);
    let signals: Vec<(u64, SignalClass)> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { cycle, signal } => Some((*cycle, signal.class())),
            _ => None,
        })
        .collect();
    let cycles: BTreeSet<u64> = signals.iter().map(|(c, _)| *c).collect();
    assert_eq!(
        cycles,
        BTreeSet::from([1, 2]),
        "a signal journaled on each scheduled sweep: {journaled:?}"
    );
    let signal_ids: BTreeSet<String> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { signal, .. } => Some(signal.id().to_string()),
            _ => None,
        })
        .collect();
    let answered: Vec<&str> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Attention { event, .. } => Some(event.signal_id.as_str()),
            _ => None,
        })
        .collect();
    assert!(!answered.is_empty(), "the router answered: {journaled:?}");
    assert!(
        answered.iter().all(|id| signal_ids.contains(*id)),
        "every attention event answers a journaled signal"
    );

    assert!(
        run_node(0).is_empty(),
        "a loop that does not run journals nothing"
    );
}

/// The attention record's lineage names the signal record that caused it.
/// Mutation: in `AmbientJournal::record`, use the root lineage for every
/// attention event — no record carries a causation.
#[test]
fn each_attention_record_is_caused_by_the_signal_record_it_answers() {
    let platform = platform();
    let mut loop_ = ResearchLoop::new(
        ResearchConfig::default(),
        BTreeMap::from([("Equity".to_string(), set(&["obj-A"]))]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(round.raised, 1, "premise: one gap raised");
    let mut ambient = journal();
    let written = ambient
        .record(1, &round, start())
        .unwrap_or_else(|e| panic!("{e}"));
    // One signal, its activation, the plan the started item compiled to
    // (R12-REASON-3061), and the assumption-health record the coverage gap's
    // challenge put on the table (R12-AMBIENT-1143).
    assert_eq!(
        written, 4,
        "one signal, its activation, the sweep's plan and the assumption health"
    );
    let events: Vec<_> = ambient.log().events().collect();
    assert!(events.iter().all(|e| e.lineage.producer == AMBIENT_ORIGIN));
    let signal = &events[0];
    let attention = &events[1];
    assert!(signal.lineage.causation_id.is_none());
    assert_eq!(
        attention.lineage.causation_id.as_ref().map(|c| c.as_str()),
        Some(signal.event_id.as_str()),
        "{attention:?}"
    );
}

/// R12-AMBIENT-1026/1140/1144: a NOW surprise is journaled as a scored
/// surprise signal per specialist, and each activation names the specialist
/// it woke. Mutation: drop the `round.woken.insert` in `sweep_over` — every
/// `woke` is `None`.
#[test]
fn a_now_surprise_is_journaled_with_its_score_and_each_wake_names_its_specialist() {
    let mut platform = platform();
    let calm: Vec<SensedRecord> = (0..30_i64)
        .map(|i| {
            let close = if i % 2 == 0 { 100.1 } else { 99.9 };
            bar(
                "obj-N",
                start().saturating_sub(Duration::from_days(30 - i)),
                close,
            )
        })
        .collect();
    platform.observe(calm);
    platform.run_cycle(start());
    let mut loop_ = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap_or_else(|e| panic!("{e}"));
    let quiet = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        quiet.now_surprises, 0,
        "premise: a calm tape surprises nothing"
    );

    platform.observe(vec![bar("obj-N", start(), 130.0)]);
    let after = start().saturating_add(Duration::from_days(1));
    platform.run_cycle(after);
    let round = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(round.now_surprises >= 1, "premise: a surprise");

    let mut ambient = journal();
    ambient
        .record(2, &round, after)
        .unwrap_or_else(|e| panic!("{e}"));
    let journaled = read(ambient.log());
    let surprise_scores: Vec<u32> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { signal, .. }
                if signal
                    .id()
                    .starts_with(qip_deepbrain::now::NOW_SIGNAL_PREFIX) =>
            {
                Some(signal.severity_bp())
            }
            _ => None,
        })
        .collect();
    assert!(
        !surprise_scores.is_empty() && surprise_scores.iter().all(|s| *s > 0),
        "each surprise signal carries its score: {journaled:?}"
    );
    let woke: BTreeSet<String> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Attention {
                event,
                woke: Some(specialist),
                ..
            } if event.disposition == Disposition::Activated => Some(specialist.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        woke,
        set(&["adversarial", "forecast", "risk", "sensing"]),
        "{journaled:?}"
    );
    // The risk specialist's signal is a risk signal on `obj-N`, which the
    // platform holds a tape for, so the sweep generated a conditional
    // scenario from it, unprompted, and the journal holds it caused by
    // that signal (R12-AMBIENT-1005/3064): the 30% move that raised it,
    // stated as a one-day shock. Mutation: drop the
    // `self.conditional_scenarios(..)` call in `sweep_over` — no scenario
    // record is journaled and the assertion fails.
    let scenarios: Vec<(&String, &qip_simulation_engine::scenario::Scenario)> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Scenario {
                signal_id,
                scenario,
                ..
            } => Some((signal_id, scenario)),
            _ => None,
        })
        .collect();
    let risk = scenarios
        .iter()
        .find(|(id, _)| id.ends_with("/risk"))
        .unwrap_or_else(|| panic!("the risk surprise yields a scenario: {journaled:?}"));
    let shock = risk
        .1
        .shocks
        .iter()
        .find(|s| s.factor == "price:obj-N")
        .unwrap_or_else(|| panic!("the scenario shocks the surprised subject: {risk:?}"));
    assert!((shock.magnitude - 0.3).abs() < 0.01, "{shock:?}");
    assert!(!risk.1.historical);
}

/// Assumption health is maintained state (R12-AMBIENT-1143): a coverage gap
/// challenges the assumption that some brain can answer the class, and a
/// later sweep that raises no gap — the class now has a tape — still
/// journals the assumption, holding for one sweep, rather than nothing.
/// Mutation: skip the `AssumptionHealth` append in `AmbientJournal::record`
/// — the second sweep journals no health record and the test fails.
#[test]
fn a_sweep_with_no_new_gap_still_journals_the_held_assumption_health() {
    let mut platform = platform();
    let coverage = BTreeMap::from([("Equity".to_string(), set(&["obj-A"]))]);
    let mut loop_ =
        ResearchLoop::new(ResearchConfig::default(), coverage).unwrap_or_else(|e| panic!("{e}"));
    let first = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        first.raised, 1,
        "premise: the coverage gap is raised: {first:?}"
    );
    assert_eq!(first.assumptions.len(), 1, "{first:?}");
    assert_eq!(first.assumptions[0].state(), "challenged");

    let calm: Vec<SensedRecord> = (0..5_i64)
        .map(|i| {
            bar(
                "obj-A",
                start().saturating_add(Duration::from_mins(i)),
                100.0,
            )
        })
        .collect();
    platform.observe(calm);
    let later = start().saturating_add(Duration::from_hours(1));
    let second = loop_
        .sweep(&platform, later, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(second.raised, 0, "premise: no new gap: {second:?}");

    let mut ambient = journal();
    ambient
        .record(2, &second, later)
        .unwrap_or_else(|e| panic!("{e}"));
    let held: Vec<_> = read(ambient.log())
        .into_iter()
        .filter_map(|r| match r {
            AmbientRecord::AssumptionHealth { assumptions, .. } => Some(assumptions),
            _ => None,
        })
        .collect();
    assert_eq!(held.len(), 1, "one health record for the quiet sweep");
    assert_eq!(held[0].len(), 1);
    assert_eq!(held[0][0].state(), "holding");
    assert_eq!(held[0][0].sweeps_since, 1);
    assert_eq!(held[0][0].challenges, 1);
}

/// R12-AMBIENT-1141/3065: an anomaly scan over price jumps is journaled as
/// signals, each with the router's decision. Mutation: drop the
/// `round.attention_events.extend` in `sweep_over` — no attention record.
#[test]
fn an_anomaly_scan_is_journaled_as_signals_each_with_the_routers_decision() {
    let mut platform = platform();
    let mut records_in = Vec::new();
    for symbol in ["obj-A", "obj-B", "obj-C"] {
        for day in 0..61i64 {
            let close = if day == 60 {
                125.0
            } else if day % 2 == 0 {
                100.1
            } else {
                99.9
            };
            records_in.push(bar(
                symbol,
                start().saturating_sub(Duration::from_days(61 - day)),
                close,
            ));
        }
    }
    platform.observe(records_in);
    let mut loop_ = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap_or_else(|e| panic!("{e}"));
    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(round.anomalies >= 3, "premise: three anomalies");
    let mut ambient = journal();
    ambient
        .record(1, &round, start())
        .unwrap_or_else(|e| panic!("{e}"));
    let journaled = read(ambient.log());
    let signalled: BTreeSet<String> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { signal, .. } => Some(signal.id().to_string()),
            _ => None,
        })
        .collect();
    let decided: BTreeSet<String> = journaled
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Attention { event, .. } => Some(event.signal_id.clone()),
            _ => None,
        })
        .collect();
    assert!(!signalled.is_empty(), "{journaled:?}");
    assert!(
        journaled.iter().any(|r| matches!(
            r,
            AmbientRecord::Signal { signal, .. }
                if signal.evidence_ids().iter().any(|e| e == "detector:anomaly_cluster")
        )),
        "the scan's cluster is on the record with its detector: {journaled:?}"
    );
    assert!(!decided.is_empty(), "{journaled:?}");
    assert!(decided.is_subset(&signalled), "{journaled:?}");
}

/// R12-AMBIENT-1141: the opportunity half of the ambient scans. A price
/// discrepancy that persists for `PERSIST_SWEEPS` consecutive sweeps is a
/// missed opportunity, and it reaches the journal as an Opportunity-class
/// signal, not as one more anomaly. Without this the opportunity scan could
/// be mapped to any class and the journal would still look busy.
///
/// Mutation: map `GapTrigger::MissedOpportunity` to `SignalClass::Assumption`
/// in `research::signal_class` and no Opportunity-class record is journaled.
#[test]
fn a_discrepancy_persisting_for_the_sweep_count_is_journaled_as_an_opportunity_class_signal() {
    let mut platform = platform();
    let mut records_in = Vec::new();
    for day in 0..61i64 {
        let close = if day == 60 {
            125.0
        } else if day % 2 == 0 {
            100.1
        } else {
            99.9
        };
        records_in.push(bar(
            "obj-P",
            start().saturating_sub(Duration::from_days(61 - day)),
            close,
        ));
    }
    platform.observe(records_in);
    let mut loop_ = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ambient = journal();
    let opportunity = |records: &[AmbientRecord]| -> Vec<String> {
        records
            .iter()
            .filter_map(|r| match r {
                AmbientRecord::Signal { signal, .. }
                    if signal.class() == SignalClass::Opportunity =>
                {
                    Some(signal.evidence_ids().join(","))
                }
                _ => None,
            })
            .collect()
    };
    for sweep in 1..=u64::from(PERSIST_SWEEPS) {
        let round = loop_
            .sweep(&platform, start(), &set(&[]), &set(&["US"]))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            round.anomalies >= 1,
            "premise: the jump is flagged on sweep {sweep}"
        );
        ambient
            .record(sweep, &round, start())
            .unwrap_or_else(|e| panic!("{e}"));
        if sweep < u64::from(PERSIST_SWEEPS) {
            assert!(
                opportunity(&read(ambient.log())).is_empty(),
                "premise: no opportunity before the discrepancy has persisted"
            );
        }
    }
    let journaled = read(ambient.log());
    let found = opportunity(&journaled);
    assert!(
        found
            .iter()
            .any(|evidence| evidence.split(',').any(|e| e == "persistent:obj-P")),
        "the persistent discrepancy is journaled as an Opportunity-class signal: {journaled:?}"
    );
}

/// `main`'s resolution: a blank variable is refused, an unset one journals
/// beside a file event log and in memory beside an in-memory one, and a
/// file journal survives a restart with its chain intact. Mutation: in
/// `JournalDestination::resolve`, map a blank value to `InMemory` — the
/// refusal is gone.
#[test]
fn the_journal_destination_refuses_a_blank_value_and_follows_the_event_logs_durability() {
    let file_log = EventLogDestination::file("/var/qip/events.jsonl");
    assert!(JournalDestination::resolve(Some("  ".into()), &file_log).is_err());
    assert_eq!(
        JournalDestination::resolve(None, &file_log).unwrap_or_else(|e| panic!("{e}")),
        JournalDestination::File(PathBuf::from("/var/qip/events.jsonl.ambient.jsonl"))
    );
    assert_eq!(
        JournalDestination::resolve(None, &EventLogDestination::InMemory)
            .unwrap_or_else(|e| panic!("{e}")),
        JournalDestination::InMemory
    );

    let dir = std::env::temp_dir().join(format!("qip-ambient-journal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let path = dir.join("ambient.jsonl");
    let destination = JournalDestination::resolve(
        Some(path.display().to_string()),
        &EventLogDestination::InMemory,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let platform = platform();
    let mut loop_ = ResearchLoop::new(
        ResearchConfig::default(),
        BTreeMap::from([("Equity".to_string(), set(&["obj-A"]))]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    {
        let mut first = AmbientJournal::open(&destination).unwrap_or_else(|e| panic!("{e}"));
        first
            .record(1, &round, start())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let mut second = AmbientJournal::open(&destination).unwrap_or_else(|e| panic!("{e}"));
    second
        .record(2, &round, start())
        .unwrap_or_else(|e| panic!("{e}"));
    let cycles: Vec<u64> = read(second.log())
        .iter()
        .filter_map(|r| match r {
            AmbientRecord::Signal { cycle, .. } | AmbientRecord::Attention { cycle, .. } => {
                Some(*cycle)
            }
            _ => None,
        })
        .collect();
    assert_eq!(cycles, [1, 1, 2, 2]);
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}

/// R12-AMBIENT-3065/7026: a storm — three hundred asset classes with no
/// history in one sweep — never wakes more than the deep brain's attention
/// budget of 256 an hour; every signal past it is deferred or shed on the
/// record, not dropped and not woken. Mutation: build the loop's router with
/// a budget of 1024 — all three hundred are activated.
#[test]
fn a_storm_of_signals_never_wakes_past_the_attention_budget_and_the_excess_is_on_the_record() {
    let platform = platform();
    let coverage: BTreeMap<String, BTreeSet<String>> = (0..300)
        .map(|i| (format!("class-{i:03}"), set(&["obj-A"])))
        .collect();
    let mut loop_ =
        ResearchLoop::new(ResearchConfig::default(), coverage).unwrap_or_else(|e| panic!("{e}"));
    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(round.raised, 300, "premise: a storm of 300 gaps");
    let mut ambient = journal();
    ambient
        .record(1, &round, start())
        .unwrap_or_else(|e| panic!("{e}"));
    let journaled = read(ambient.log());
    let mut by_disposition: BTreeMap<String, usize> = BTreeMap::new();
    for record in &journaled {
        if let AmbientRecord::Attention { event, .. } = record {
            *by_disposition
                .entry(format!("{:?}", event.disposition))
                .or_default() += 1;
        }
    }
    assert_eq!(
        by_disposition.get("Activated"),
        Some(&256),
        "{by_disposition:?}"
    );
    let held = by_disposition.get("Deferred").copied().unwrap_or(0)
        + by_disposition.get("Shed").copied().unwrap_or(0);
    assert_eq!(held, 44, "{by_disposition:?}");
}

/// A tick campaign the sweep opened is concluded once a later sweep's fills
/// re-measure its venue, and the delta is journaled (R12-EXPAND-6042): ten
/// fills forty basis points off the twin open a fill-prediction campaign at
/// the venue; eight more after it opened conclude it into a microstructure
/// delta whose id is the campaign's, measured over those eight; a third
/// sweep with nothing new concludes nothing again. Mutation: drop the
/// `self.conclude_campaigns(..)` call in `sweep_over` — no delta and the
/// test fails.
#[test]
fn a_campaign_whose_venue_is_remeasured_is_concluded_into_a_journaled_delta() {
    use qip_deepbrain::research::FillObservation;
    let platform = platform();
    let fill = |minutes: i64, bps: f64| FillObservation {
        venue: "XSIM".to_string(),
        object_id: "BTC-USD".to_string(),
        filled_at: start().saturating_add(Duration::from_mins(minutes)),
        trade_error_bps: Some(bps),
    };
    let mut fills: Vec<FillObservation> = (0..10)
        .map(|i| fill(i - 60, if i % 2 == 0 { 35.0 } else { 45.0 }))
        .collect();
    let mut loop_ = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap_or_else(|e| panic!("{e}"));
    let first = loop_
        .sweep_over(&platform, &fills, &[], start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        first.campaigns_opened >= 1,
        "premise: a campaign opened: {first:?}"
    );
    assert!(first.deltas.is_empty(), "nothing re-measured yet");

    fills.extend((0..8).map(|i| fill(i + 1, if i % 2 == 0 { 38.0 } else { 42.0 })));
    let later = start().saturating_add(Duration::from_hours(1));
    let second = loop_
        .sweep_over(&platform, &fills, &[], later, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !second.deltas.is_empty(),
        "the re-measured campaign concluded: {second:?}"
    );
    for delta in &second.deltas {
        assert_eq!(delta.venue.as_str(), "XSIM");
        let [residual] = delta.residuals.as_slice() else {
            panic!("one instrument studied: {delta:?}");
        };
        assert_eq!(
            residual.samples, 8,
            "measured over the fills since it opened"
        );
        assert_eq!(residual.observed, Decimal::from_int(40));
    }
    let mut ambient = journal();
    ambient
        .record(2, &second, later)
        .unwrap_or_else(|e| panic!("{e}"));
    let journaled: Vec<String> = read(ambient.log())
        .into_iter()
        .filter_map(|r| match r {
            AmbientRecord::MicrostructureDelta { delta, .. } => Some(delta.delta_id.clone()),
            _ => None,
        })
        .collect();
    let concluded: Vec<String> = second.deltas.iter().map(|d| d.delta_id.clone()).collect();
    assert_eq!(journaled, concluded, "every delta is journaled");

    let third = loop_
        .sweep_over(
            &platform,
            &fills,
            &[],
            later.saturating_add(Duration::from_hours(1)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        third.deltas.is_empty(),
        "a concluded campaign is not concluded twice"
    );
}
