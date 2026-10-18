//! The research and operations loops reached from the deep brain's run loop
//! (W1.10, W1.12, W1.15, W3.5): gaps detected over platform state become
//! ambient signals, the attention router decides what wakes, what wakes is
//! queued as research with a priced information request, and the operations
//! loop opens incidents and remediates only through its runbook.
//!
//! Each test asserts its premise first — that the platform holds the state
//! the detector reads — so an empty platform cannot pass for a quiet one.

use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::evolution::{EvolutionConfig, EvolutionEngine};
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::operations::{CapabilitySwitch, IterationFacts, Operations, quantum_outcome};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop, limit_approach_signals};
use qip_deepbrain::status::NodeStatus;
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::platform::SolverRoutingJournal;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
use qip_observability::Telemetry;
use qip_observability::aiops::{Effector, EntityKey, SignalClass};
use qip_observability::metrics::{labels, names};
use qip_optimization_engine::QuantumDispatch;
use qip_risk::limits::{LimitSet, RiskState};
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::collections::{BTreeMap, BTreeSet};
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

fn coverage(pairs: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
    pairs
        .iter()
        .map(|(class, instruments)| ((*class).to_string(), set(instruments)))
        .collect()
}

fn research(coverage: BTreeMap<String, BTreeSet<String>>) -> ResearchLoop {
    ResearchLoop::new(ResearchConfig::default(), coverage)
        .unwrap_or_else(|e| panic!("the research loop assembles: {e}"))
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

/// Sixty quiet days around 100 and then a 25% jump, for each symbol.
fn jumps(symbols: &[&str]) -> Vec<SensedRecord> {
    let mut records = Vec::new();
    for symbol in symbols {
        for day in 0..61i64 {
            let close = if day == 60 {
                125.0
            } else if day % 2 == 0 {
                100.1
            } else {
                99.9
            };
            let at = start().saturating_sub(Duration::from_days(61 - day));
            records.push(bar(symbol, at, close));
        }
    }
    records
}

/// R12-CONTRACT-7086: the research loop `main.rs` builds holds a validated
/// Brain Registry entry for every specialist a NOW surprise can wake, each
/// naming only tools this node registered, and each survives the contract's
/// own decode unchanged.
///
/// The failure this prevents: a wake attributable to a label and nothing
/// else — no tools, no permissions, no abstention rule, no authority — which
/// is what "the risk specialist" was until 2026-10-11.
#[test]
fn every_wakeable_specialist_is_a_registered_brain_spec_naming_only_tools_this_node_has() {
    let loop_ = research(coverage(&[]));
    let brains = loop_.brains();
    let specialists = qip_kernel::now_stage::Specialist::ALL;
    // Premise: there are specialists to cover.
    assert!(!specialists.is_empty());
    assert_eq!(brains.len(), specialists.len(), "{brains:?}");
    let tools = qip_deepbrain::research::registered_tools().unwrap_or_else(|e| panic!("{e}"));
    for specialist in specialists {
        let spec = brains
            .get(specialist.as_str())
            .unwrap_or_else(|| panic!("{} has no BrainSpec", specialist.as_str()));
        spec.validate()
            .unwrap_or_else(|e| panic!("{}: {e}", spec.id));
        for tool in &spec.tools {
            assert!(
                tools.get(tool).is_some(),
                "{} names unregistered {tool}",
                spec.id
            );
        }
        assert!(
            spec.authority_envelope.starts_with("advisory_only"),
            "{}: {}",
            spec.id,
            spec.authority_envelope
        );
        let bytes = serde_json::to_vec(spec).unwrap_or_else(|e| panic!("{e}"));
        let decoded = qip_contracts::BrainSpec::decode(&bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(&decoded, spec, "a round trip preserves every field");
    }
}

/// R12-CONTRACT-7078, 7079 and 7085 on the path `qip-deepbrain`'s run loop
/// takes (`main.rs` builds `ResearchLoop` from `ResearchConfig::from_lookup`):
/// the loop *produces* an `AmbientSignal` from a raised gap, the attention
/// router turns it into an `AttentionEvent`, the loop *consumes* that event's
/// feedback link as the advisory's origin, and the desk queues a
/// `CurriculumItem` carrying every §27 field. One test, because the three
/// contracts are only worth anything joined: a curriculum item nobody can
/// trace to the signal that raised it is research with no reason on record.
#[test]
fn a_raised_gap_flows_from_ambient_signal_through_attention_event_to_a_complete_curriculum_item() {
    let platform = platform();
    assert!(platform.price_history().is_empty(), "premise: no history");
    let mut loop_ = research(coverage(&[("Equity", &["obj-A", "obj-B"])]));

    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(round.raised, 1, "premise: one gap raised: {round:?}");
    assert_eq!(loop_.desk().queue().len(), 1, "premise: one item queued");

    // The signal the loop emitted is `ambient-1`; the advisory is filed under
    // the attention event's feedback link, which names it.
    let advisories: Vec<_> = loop_.advisories().collect();
    assert_eq!(advisories.len(), 1, "{advisories:?}");
    assert_eq!(advisories[0].origin_signal(), "ambient-1");

    let ranked = loop_.desk().queue().ranked();
    let item = ranked[0].item();
    assert!(
        advisories[0].view().ends_with(&item.research_question),
        "the advisory and the curriculum item name one question: {:?} / {:?}",
        advisories[0].view(),
        item.research_question
    );
    assert!(!item.research_question.trim().is_empty());
    assert_eq!(item.owner, "qip-deepbrain/research");
    assert!(!item.tools_and_data.is_empty(), "{item:?}");
    assert!(item.budget > Decimal::ZERO, "an uncosted item: {item:?}");
    assert!(!item.evaluation_suite.trim().is_empty(), "{item:?}");
    assert_eq!(item.stop_conditions.len(), 2, "{item:?}");
    assert!(
        ranked[0].origin().is_some(),
        "the queued item keeps the gap that raised it"
    );
}

#[test]
fn an_asset_class_with_no_history_becomes_a_queued_research_item_through_the_attention_router() {
    let platform = platform();
    assert!(platform.price_history().is_empty(), "premise: no history");
    let metrics = platform.telemetry().metrics.clone();
    let mut loop_ = research(coverage(&[("Equity", &["obj-A", "obj-B"])]));

    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(round.raised, 1, "{round:?}");
    assert_eq!(round.admissions.get("queued"), Some(&1), "{round:?}");
    assert_eq!(
        round.attention.get(&("reflex_model_update", "activated")),
        Some(&1),
        "an unsupported-request gap is an assumption signal: {round:?}"
    );
    assert_eq!(loop_.desk().queue().len(), 1);
    assert_eq!(
        loop_.advisories().count(),
        1,
        "the mesh's output is an advisory"
    );
    // Full uncertainty and no economic value: the coverage detector measures
    // no money and states zero. This asserted one launch until lane H made
    // the Curiosity Engine's rank §9.10's product (R12-AMBIENT-3106,
    // "zeroing any multiplicative factor zeroes the score"); under the
    // additive rank it replaced, 4000 - 2000 cleared the floor, so the loop
    // would pay 2000 bp to resolve an uncertainty no decision's value rides
    // on. The gap is still research — queued above — and asks for nothing.
    // `research::tests::a_gap_with_economic_value_launches_on_the_product_rank`
    // proves a gap that does carry economic value launches, so this zero is
    // the rank and not a dead path.
    assert_eq!(round.info_launched, 0, "{round:?}");
    assert_eq!(round.info_halted, 0, "{round:?}");
    assert_eq!(
        loop_.info_requests().count(),
        0,
        "a request was recorded for a gap of no economic significance"
    );
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::RESEARCH_GAPS,
            &labels([("class", "data"), ("outcome", "queued")])
        ),
        1
    );
    assert_eq!(
        snapshot.gauge(names::RESEARCH_QUEUE_ITEMS, &labels([])),
        Some(1.0)
    );
}

#[test]
fn with_no_eligible_jurisdiction_every_admission_is_refused_and_nothing_is_queued() {
    let platform = platform();
    let mut loop_ = research(coverage(&[("Equity", &["obj-A"])]));
    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&[]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(round.raised, 1, "premise: a gap was raised");
    assert_eq!(round.admissions.get("refused"), Some(&1), "{round:?}");
    assert!(loop_.desk().queue().is_empty());
    assert_eq!(round.info_launched, 0, "refused research asks for nothing");
}

#[test]
fn a_gap_worth_less_than_its_cheapest_observation_launches_no_information_request() {
    let platform = platform();
    // One of three instruments covered: a weaker coverage gap than none.
    let mut weak = platform;
    weak.observe(vec![bar("obj-A", start(), 100.0)]);
    assert!(
        weak.price_history().contains_key("obj-A"),
        "premise: the bar reached the price history"
    );
    let mut loop_ = research(coverage(&[("Equity", &["obj-A", "obj-B", "obj-C"])]));
    let round = loop_
        .sweep(&weak, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    // Severity 2/3 and, as for every coverage gap, no economic value: the
    // §9.10 product ranks it zero, below the 1000 bp floor. (Under the
    // additive rank this read "6667 bp of uncertainty is worth 2667 against a
    // 2000 bp acquisition"; the arithmetic changed with lane H, the outcome
    // did not.)
    assert_eq!(round.admissions.get("queued"), Some(&1), "{round:?}");
    assert_eq!(round.info_launched, 0, "{round:?}");
}

#[test]
fn price_jumps_on_several_subjects_reach_the_queue_through_the_anomaly_mapper_as_one_cluster() {
    let mut platform = platform();
    platform.observe(jumps(&["obj-A", "obj-B", "obj-C"]));
    assert_eq!(platform.price_history().len(), 3, "premise: three series");
    let mut loop_ = research(BTreeMap::new());

    let round = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(
        round.anomalies >= 3,
        "premise: the jumps are anomalies: {round:?}"
    );
    let clustered: Vec<String> = loop_
        .desk()
        .queue()
        .ranked()
        .iter()
        .map(|e| e.item().research_question.clone())
        .filter(|q| q.contains("subjects show the same"))
        .collect();
    assert_eq!(
        clustered.len(),
        1,
        "one investigation nobody asked for: {round:?}"
    );
    let lineage = loop_
        .desk()
        .lineage(&clustered[0])
        .unwrap_or_else(|| panic!("the cluster keeps its lineage"));
    assert_eq!(lineage.derivation.rule, "detector:anomaly_cluster");
}

#[test]
fn the_run_loop_sweeps_research_and_observes_operations_every_cycle() {
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
    let mut research = research(coverage(&[("Equity", &["obj-A"])]));
    let mut operations = Operations::new(start()).unwrap_or_else(|e| panic!("{e}"));
    let mut researched = 0;
    let mut observed = 0;
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
            operations: Some(&mut operations),
            tick: None,
            permissions: None,
            ambient: None,
            agency: None,
        },
        |outcome| {
            researched += usize::from(outcome.research.is_some());
            observed += usize::from(outcome.operations.is_some());
            assert!(
                outcome.side_failures.is_empty(),
                "{:?}",
                outcome.side_failures
            );
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((researched, observed), (2, 2));
    let deployment = EntityKey::new("deployment", "qip-deepbrain");
    assert!(
        operations
            .graph()
            .classes_for(&deployment)
            .contains(&SignalClass::Infrastructure)
    );
}

// --- operations -------------------------------------------------------------

fn facts(at: Timestamp, ms: f64, drifted: &[String]) -> IterationFacts<'_> {
    IterationFacts {
        at,
        cycle: 1,
        cycle_started_at: at,
        iteration_ms: ms,
        problems: 0,
        anomalies: Some(2),
        drifted,
    }
}

#[test]
fn a_level_shift_in_iteration_time_opens_one_incident_and_the_runbook_sheds_research() {
    let platform = platform();
    let mut ops = Operations::new(start()).unwrap_or_else(|e| panic!("{e}"));
    let mut incidents = Vec::new();
    for i in 0..24i64 {
        let ms = if i < 12 {
            10.0 + (i % 2) as f64
        } else {
            1_000.0
        };
        let at = start().saturating_add(Duration::from_secs(i));
        let round = ops
            .observe(&platform, &facts(at, ms, &[]))
            .unwrap_or_else(|e| panic!("{e}"));
        incidents.extend(round.incident);
        if i == 11 {
            // Premise: a steady series opened nothing and shed nothing.
            assert!(incidents.is_empty() && !ops.is_disabled("research"));
        }
    }
    assert_eq!(
        incidents.len(),
        1,
        "one shift is one incident: {incidents:?}"
    );
    let incident = &incidents[0];
    assert!(incident.remediation.is_ok(), "{incident:?}");
    assert!(ops.is_disabled("research"));
    let audit = ops.remediator().audit();
    assert_eq!(audit.records().len(), 1);
    assert!(audit.verify());
    let sources: BTreeSet<_> = incident.timeline.iter().map(|e| e.source).collect();
    assert!(sources.contains(&qip_observability::aiops::TimelineSource::Telemetry));
    assert!(sources.contains(&qip_observability::aiops::TimelineSource::DataAnomaly));
}

#[test]
fn signals_of_all_five_classes_attach_to_the_shared_deployment_entity() {
    let platform = platform();
    let mut ops = Operations::new(start()).unwrap_or_else(|e| panic!("{e}"));
    let drifted = vec!["model-1".to_string()];
    ops.observe(&platform, &facts(start(), 10.0, &drifted))
        .unwrap_or_else(|e| panic!("{e}"));
    ops.observe_fills(&[("XNYS".to_string(), 40.0, start())], start())
        .unwrap_or_else(|e| panic!("{e}"));
    let deployment = EntityKey::new("deployment", "qip-deepbrain");
    let classes = ops.graph().classes_for(&deployment);
    assert_eq!(classes.len(), 5, "{classes:?}");
    assert!(
        ops.graph()
            .classes_for(&EntityKey::new("venue", "XNYS"))
            .contains(&SignalClass::TradingBehaviour)
    );
}

#[test]
fn the_effector_carries_out_nothing_but_disabling_this_nodes_own_research() {
    let mut switch = CapabilitySwitch::default();
    assert!(switch.apply("restart", "research").is_err());
    assert!(
        switch
            .apply("disable_capability", "execution-engine")
            .is_err()
    );
    assert!(
        !switch.is_disabled("research"),
        "premise: nothing applied yet"
    );
    assert!(switch.apply("disable_capability", "research").is_ok());
    assert!(switch.is_disabled("research"));
}

fn routing(dispatch: QuantumDispatch, ran: &[&str]) -> SolverRoutingJournal {
    SolverRoutingJournal {
        chosen: "classical".into(),
        ran: ran.iter().map(|s| (*s).to_string()).collect(),
        objective: 1.0,
        classical_objective: 1.0,
        improvement_over_classical: 0.0,
        measured_quantum_advantage: None,
        quantum_note: "attempting QAOA on ibm, a simulator".into(),
        quantum_dispatch: dispatch,
        compute: None,
    }
}

/// Since the quantum arm went submit-and-poll, a cycle that submits a job, or
/// finds one still out, has not learned how it went; counting either as a
/// failure would page operations for every job in flight. Mutation: make
/// `quantum_outcome` return `Some("failed")` for a submitted or pending job
/// (the note-reading rule it replaced), and the first two assertions fail.
#[test]
fn only_a_job_that_resolved_this_cycle_is_an_outcome_and_one_that_did_not_run_failed() {
    let submitted = QuantumDispatch {
        submitted: Some(7),
        ..QuantumDispatch::default()
    };
    assert_eq!(
        quantum_outcome(&routing(submitted, &["exact_enumeration"])),
        None
    );
    let pending = QuantumDispatch {
        pending: Some(7),
        ..QuantumDispatch::default()
    };
    assert_eq!(
        quantum_outcome(&routing(pending, &["exact_enumeration"])),
        None
    );

    let harvested = QuantumDispatch {
        harvested: Some(7),
        ..QuantumDispatch::default()
    };
    assert_eq!(
        quantum_outcome(&routing(
            harvested.clone(),
            &["exact_enumeration", "quantum"]
        )),
        Some("succeeded")
    );
    assert_eq!(
        quantum_outcome(&routing(harvested, &["exact_enumeration"])),
        Some("failed"),
        "a harvested answer dropped as stale or refused by the judge is a failed trial"
    );
    for resolved_without_answer in [
        QuantumDispatch {
            abandoned: Some(7),
            ..QuantumDispatch::default()
        },
        QuantumDispatch {
            lost: Some(7),
            ..QuantumDispatch::default()
        },
    ] {
        assert_eq!(
            quantum_outcome(&routing(resolved_without_answer, &["exact_enumeration"])),
            Some("failed")
        );
    }
}
/// AMBIENT-3079 reached from the deep brain: a surprise the kernel's NOW
/// Brain raises during a cycle reaches this loop's attention router as one
/// signal per specialist, and forecast, adversarial, sensing and risk
/// specialists are each woken. Mutation: drop the `surprises_since` read
/// from `ResearchLoop::sweep`, and nothing wakes.
#[test]
fn a_now_surprise_raised_in_the_cycle_wakes_forecast_adversarial_sensing_and_risk_specialists() {
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
    let mut loop_ = research(BTreeMap::new());
    let quiet = loop_
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        quiet.now_surprises, 0,
        "premise: a calm tape surprises nothing"
    );
    assert!(quiet.now_wakes.is_empty());

    platform.observe(vec![bar("obj-N", start(), 130.0)]);
    let after = start().saturating_add(Duration::from_days(1));
    platform.run_cycle(after);
    let round = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(round.now_surprises >= 1, "premise: a surprise");
    let woken: BTreeSet<&str> = round.now_wakes.keys().copied().collect();
    assert_eq!(
        woken,
        BTreeSet::from(["adversarial", "forecast", "risk", "sensing"]),
        "the surprise wakes exactly these four"
    );
    // A wake is work (R12-AMBIENT-3079): each of the four specialists was
    // handed the surprise as a research question the desk now holds under
    // its registered brain, not only ticked on a counter. Mutation: restore
    // the bare `continue` after the counter in `ResearchLoop::sweep_over`
    // — `wake_items` stays empty and the first assertion fails.
    let given: BTreeSet<&str> = round.wake_items.keys().copied().collect();
    assert_eq!(given, woken, "every woken specialist was given an item");
    for (specialist, question) in &round.wake_items {
        let entry = loop_
            .desk()
            .queue()
            .entry(question)
            .unwrap_or_else(|| panic!("the {specialist} specialist's question is queued"));
        let brain = &loop_.brains()[specialist];
        assert!(
            entry
                .origin()
                .is_some_and(|gap| gap.signal.evidence.contains(&format!("brain:{}", brain.id))),
            "the item names the brain that owns it: {entry:?}"
        );
    }
    // Read once: a second sweep with no new cycle wakes nothing again.
    let again = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.now_surprises, 0);
}

/// The pre-limit sentinel (R12-AMBIENT-3070): a book at 90% of its leverage
/// limit — inside it, past the conservative set's 85% warning threshold —
/// raises one risk-review signal naming the limit, and the same book at 10%
/// raises nothing. Through the sweep, a fresh platform's empty book raises
/// none while its limits are evaluated, so the quiet case is a measured
/// quiet and not a sentinel that never ran. Mutation: read `check.breaches`
/// filtered to `Severity::Breach` instead of `check.warnings()` — the 90%
/// book raises nothing and the first assertion fails.
#[test]
fn a_book_at_ninety_percent_of_a_limit_raises_a_risk_review_and_at_ten_percent_raises_nothing() {
    let limits = LimitSet::conservative_default();
    let equity = Decimal::from_int(1_000_000);
    let book = |gross: f64| RiskState {
        equity,
        cash: equity,
        gross_exposure: Decimal::from_f64(gross).unwrap_or_else(|| panic!("a decimal")),
        net_exposure: Decimal::ZERO,
        ..RiskState::default()
    };
    // Leverage 1.35 against a limit of 1.5 is 90% in use.
    let near = limits.check(&book(1_350_000.0));
    assert!(near.evaluated > 0, "premise: limits were evaluated");
    assert!(
        !near.is_blocked(),
        "premise: 90% of a limit is not a breach"
    );
    let mut issued = 0;
    let raised = limit_approach_signals(&near, start(), Duration::from_hours(1), &mut issued)
        .unwrap_or_else(|e| panic!("{e}"));
    let leverage: Vec<_> = raised
        .iter()
        .filter(|(signal, _)| signal.subject() == "limit:leverage")
        .collect();
    assert_eq!(
        leverage.len(),
        1,
        "one review for the approached limit: {raised:?}"
    );
    let (signal, utilisation) = leverage[0];
    assert!((utilisation - 0.9).abs() < 1e-9, "{utilisation}");
    assert_eq!(signal.class(), qip_contracts::ambient::SignalClass::Risk);
    assert_eq!(
        signal.wake_targets(),
        &[qip_contracts::ambient::Pathway::RiskReview]
    );
    assert_eq!(signal.severity_bp(), 9_000);

    let far = limits.check(&book(150_000.0));
    let mut issued = 0;
    let quiet = limit_approach_signals(&far, start(), Duration::from_hours(1), &mut issued)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        quiet.iter().all(|(s, _)| s.subject() != "limit:leverage"),
        "10% of a limit is not approaching it: {quiet:?}"
    );

    let platform = platform();
    assert!(
        platform
            .limits()
            .check(&platform.risk_state_from(platform.risk_figures()))
            .evaluated
            > 0
    );
    let round = research(BTreeMap::new())
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        round.limit_warnings.is_empty(),
        "{:?}",
        round.limit_warnings
    );
}

/// The evolution round that ran in a cycle's gap rides on that cycle's
/// sweep record (R12-EXPAND-1117): where the engine turned, the research
/// round names the subject it searched; where it did not, the record says
/// so with `None` rather than a stale trace. Mutation: drop the
/// `round.evolution = Some(..)` wiring in `node::run_with` — the trace is
/// never set and the `traced` count stays at zero.
#[test]
fn a_sweep_record_carries_the_evolution_round_that_preceded_it() {
    let manual = Arc::new(ManualClock::new(start()));
    let clock: Arc<dyn Clock> = manual.clone();
    let config = PlatformConfig::default();
    let seed = config.seed;
    let context = Context::new(clock.clone(), seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // The synthetic feed is read up to the clock, so the clock is advanced
    // a minute per cycle below: one bar per minute, and the search has the
    // two bars it needs within the first cycles. The wall-clock interval is
    // a millisecond so the loop does not wait between them.
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(12),
        ..DeepBrainConfig::default()
    };
    let environment = SyntheticEnvironment::demo(
        start(),
        EnvironmentConfig {
            seed,
            step: Duration::from_mins(1),
            ..EnvironmentConfig::default()
        },
    );
    let mut engine = EvolutionEngine::over_synthetic(
        EvolutionConfig {
            every_cycles: 1,
            minimum_bars: 2,
            candidates: 2,
            challengers: 1,
            ..EvolutionConfig::default()
        },
        environment,
        seed,
        start(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let roster = qip_deepbrain::roster::clear(start()).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut research = research(BTreeMap::new());
    let mut turned = 0;
    let mut traced = 0;
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        Some(&mut engine),
        Loops {
            desk: None,
            research: Some(&mut research),
            operations: None,
            tick: None,
            permissions: None,
            ambient: None,
            agency: None,
        },
        |outcome| {
            manual.advance(Duration::from_mins(1));
            let round = outcome
                .research
                .as_ref()
                .unwrap_or_else(|| panic!("research sweeps every cycle"));
            match &outcome.evolution {
                Some(summary) => {
                    turned += 1;
                    let trace = round
                        .evolution
                        .as_ref()
                        .unwrap_or_else(|| panic!("a turn is on the sweep record"));
                    assert_eq!(trace.subject, summary.subject);
                    assert_eq!(trace.proposed, summary.proposed);
                    assert_eq!(trace.registered, summary.registered);
                    traced += 1;
                }
                None => assert!(round.evolution.is_none(), "no turn, no trace"),
            }
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        turned >= 1,
        "premise: the engine turned at least once in twelve cycles"
    );
    assert_eq!(traced, turned);
}

/// Launched information requests reach the Scout Fabric (R12-AMBIENT-9033):
/// scout jobs handed to the discovery desk wait in hand-off order and the
/// desk's next pass reads each job's entities as discovery targets after
/// the platform's own, each once, and takes the queue. Mutation: drop the
/// scout loop in `DiscoveryDesk::maybe_run` — the entities are not targeted
/// and the assertion fails.
#[test]
fn a_scout_job_handed_to_the_discovery_desk_becomes_a_target_of_its_next_pass() {
    let mut platform = platform();
    let mut desk = qip_deepbrain::discovery::DiscoveryDesk::new(
        qip_deepbrain::discovery::DiscoveryConfig {
            every_cycles: 1,
            ..qip_deepbrain::discovery::DiscoveryConfig::default()
        },
        vec![],
    );
    let job = |id: &str, rank_bp: u32, entities: &[&str]| qip_deepbrain::research::ScoutRequest {
        request_id: id.to_string(),
        question: format!("question {id}"),
        rank_bp,
        entities: entities.iter().map(|e| (*e).to_string()).collect(),
    };
    let jobs = vec![
        job("info-b", 6_000, &["venue:XSIM"]),
        job("info-a", 3_000, &["venue:XSIM", "class:Equity"]),
    ];
    desk.hand_off(&jobs);
    let queued: Vec<_> = desk.scout_queue().cloned().collect();
    assert_eq!(queued, jobs, "the desk holds the jobs in hand-off order");

    let before: BTreeSet<String> = platform
        .discovery_targets(start())
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|t| t.entity)
        .collect();
    assert!(
        !before.contains("venue:XSIM") && !before.contains("class:Equity"),
        "premise: the platform does not target them itself: {before:?}"
    );
    let pass = desk
        .maybe_run(&mut platform, 1, start())
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("the pass runs on its cadence"));
    let targeted: Vec<&str> = pass
        .targets
        .iter()
        .map(|t| t.entity.as_str())
        .filter(|e| e.starts_with("venue:") || e.starts_with("class:"))
        .collect();
    assert_eq!(
        targeted,
        ["venue:XSIM", "class:Equity"],
        "each once, in order"
    );
    assert_eq!(desk.scout_queue().count(), 0, "the pass took the queue");
}
