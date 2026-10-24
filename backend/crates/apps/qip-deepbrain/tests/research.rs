//! The research and operations loops reached from the deep brain's run loop
//! (W1.10, W1.12, W1.15, W3.5): gaps detected over platform state become
//! ambient signals, the attention router decides what wakes, what wakes is
//! queued as research with a priced information request, and the operations
//! loop opens incidents and remediates only through its runbook.
//!
//! Each test asserts its premise first — that the platform holds the state
//! the detector reads — so an empty platform cannot pass for a quiet one.

use qip_capital_fabric::{CapitalLocation, DemandKind, Region};
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::evolution::{EvolutionConfig, EvolutionEngine};
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::operations::{
    CapabilitySwitch, IterationFacts, Operations, RemediationJournal, quantum_outcome, runbooks,
};
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
use qip_observability::aiops::{
    AuditRecord, AuditSink, Effector, EntityKey, Remediator, SignalClass,
};
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
            warehouse: None,
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
        package_promoted: false,
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

/// A sink that cannot take a record, as a full disk or a lost file would be.
#[derive(Debug)]
struct UnwritableJournal;

impl AuditSink for UnwritableJournal {
    fn journal(&mut self, _record: &AuditRecord) -> qip_core::error::Result<()> {
        Err(qip_core::error::Error::io("the journal's disk is full"))
    }
}

/// R12-CICD-9081. An effect whose audit record stayed in the process left no
/// account of itself once the restart it was waiting for came.
#[test]
fn an_incident_journals_the_authorising_record_before_the_effect_and_the_outcome_after() {
    let platform = platform();
    let journal = RemediationJournal::open(None, Arc::new(ManualClock::new(start())))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut ops =
        Operations::journalled(start(), journal.clone()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        journal.records().unwrap_or_default().is_empty(),
        "premise: nothing is journalled before an incident"
    );
    for i in 0..24i64 {
        let ms = if i < 12 {
            10.0 + (i % 2) as f64
        } else {
            1_000.0
        };
        let at = start().saturating_add(Duration::from_secs(i));
        ops.observe(&platform, &facts(at, ms, &[]))
            .unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(ops.is_disabled("research"), "premise: the runbook ran");
    let records = journal.records().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(records.len(), 2, "authorisation then outcome: {records:?}");
    assert!(
        records[0]
            .result
            .starts_with("authorised by shed-research-on-iteration-shift"),
        "{:?}",
        records[0]
    );
    assert_eq!(
        records[1].result,
        "research disabled until the process restarts"
    );
    assert_eq!(
        records[1].prev_hash, records[0].hash,
        "the two are one chain"
    );
    assert_eq!(&records[..], ops.remediator().audit().records());
}

/// R12-CICD-9081. Mutation: call `apply` before `journal_last` and this
/// fails, because the effector has already switched research off.
#[test]
fn an_effect_whose_audit_record_cannot_be_journalled_is_refused_and_not_applied() {
    let mut remediator = Remediator::journalled(
        runbooks(),
        CapabilitySwitch::default(),
        Box::new(UnwritableJournal),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !remediator.effector().is_disabled("research"),
        "premise: research is on"
    );
    let refusal = remediator
        .remediate("INC-1", "disable_capability", "research")
        .err()
        .unwrap_or_else(|| panic!("an unjournalled action must be refused"));
    assert!(
        refusal.message().contains("could not be journalled"),
        "{}",
        refusal.message()
    );
    assert!(
        !remediator.effector().is_disabled("research"),
        "the effector must not have run"
    );
    assert!(remediator.audit().verify());
}

/// A sink that takes the authorising record and then goes: the disk fills
/// between the record that precedes the effect and the one that follows it.
#[derive(Debug, Default)]
struct FailsAfterTheAuthorisation {
    taken: usize,
}

impl AuditSink for FailsAfterTheAuthorisation {
    fn journal(&mut self, _record: &AuditRecord) -> qip_core::error::Result<()> {
        self.taken += 1;
        if self.taken > 1 {
            return Err(qip_core::error::Error::io("the journal's disk is full"));
        }
        Ok(())
    }
}

/// An action that ran and whose outcome record then failed to journal is an
/// applied action. `open_incident` counted it `refused` and marked the
/// incident's remediation failed while research was in fact switched off, so
/// the series and the incident said the opposite of the node.
///
/// Mutation: in `Operations::open_incident`, label the unjournalled case
/// `applied` instead of `applied_unjournalled` — the count reads 0 and the
/// first assertion fires. Mutation: make `Remediator::remediate_outcome`
/// return `Err` for the unjournalled case — the incident takes the refused
/// arm and the same assertion fires.
#[test]
fn an_action_that_ran_but_could_not_journal_its_outcome_is_counted_applied_not_refused() {
    let platform = platform();
    let metrics = platform.telemetry().metrics.clone();
    let mut ops =
        Operations::journalled_to(start(), Box::new(FailsAfterTheAuthorisation::default()))
            .unwrap_or_else(|e| panic!("{e}"));
    let mut incident = None;
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
        incident = incident.or(round.incident);
    }
    assert!(ops.is_disabled("research"), "premise: the runbook ran");
    let counted = |outcome: &str| {
        metrics.snapshot().counter(
            names::AIOPS_REMEDIATIONS,
            &labels([("action", "disable_capability"), ("outcome", outcome)]),
        )
    };
    assert_eq!(counted("applied_unjournalled"), 1);
    assert_eq!(
        counted("refused"),
        0,
        "research is off, so the action was not refused"
    );
    let incident = incident.unwrap_or_else(|| panic!("premise: a shift opened an incident"));
    let remediation = incident
        .remediation
        .unwrap_or_else(|why| panic!("the incident marks an applied action failed: {why}"));
    assert!(
        remediation.contains("could not be journalled"),
        "the incident says the outcome was not journalled: {remediation}"
    );
}

/// R12-OBS-9164: model package freshness and the research queue's depth are
/// objectives backed by recorded series. A round of the operations loop and
/// a research sweep move both from unmeasured to measured.
///
/// Mutation: delete the `names::MODEL_PACKAGE_FRESHNESS_CHECKS` count in
/// `Operations::observe` or the `names::RESEARCH_QUEUE_CHECKS` count in
/// `sweep_over`; the matching objective stays at zero observations.
#[test]
fn a_round_moves_the_model_freshness_and_research_queue_objectives_from_unmeasured_to_measured() {
    let platform = platform();
    let metrics = platform.telemetry().metrics.clone();
    let statuses = || {
        qip_observability::critical_paths::evaluate_where(&metrics.snapshot(), |path| {
            path.slo.service == "deepbrain"
        })
    };
    let observations = |name: &str| {
        statuses()
            .iter()
            .find(|s| s.slo.name == name)
            .unwrap_or_else(|| panic!("{name} is not a deepbrain objective"))
            .observations
    };
    assert_eq!(
        observations("model-package-freshness"),
        0,
        "premise: nothing measured before the round"
    );
    assert_eq!(observations("research-queue-depth"), 0, "premise");

    let mut ops = Operations::new(start()).unwrap_or_else(|e| panic!("{e}"));
    ops.observe(
        &platform,
        &IterationFacts {
            package_promoted: true,
            ..facts(start(), 10.0, &[])
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut loop_ = research(coverage(&[("Equity", &["obj-A"])]));
    loop_
        .sweep(&platform, start(), &set(&["vendor"]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(observations("model-package-freshness"), 1);
    assert_eq!(observations("research-queue-depth"), 1);
    assert!(
        statuses()
            .iter()
            .filter(|s| ["model-package-freshness", "research-queue-depth"]
                .contains(&s.slo.name.as_str()))
            .all(|s| s.is_met),
        "a fresh package and a shallow queue meet both"
    );
}

/// A platform with four days of cash demand at its declared venue, so the
/// planner has a lane to forecast and, over a five-day horizon, a movement
/// that pays for itself.
fn platform_with_a_funding_lane() -> Platform {
    let mut platform = platform();
    let (venue, region) = platform
        .settlement_book()
        .venues()
        .map(|(venue, region)| (venue.clone(), region.as_str().to_string()))
        .next()
        .unwrap_or_else(|| panic!("premise: the platform declares a settling venue"));
    let lane = CapitalLocation::new(Region::new(&region), qip_core::Currency::USD, venue);
    for day in 0..4 {
        platform.record_capital_demand(
            lane.clone(),
            DemandKind::Cash,
            start().saturating_add(Duration::from_days(day)),
            qip_core::dec!("250000"),
        );
    }
    platform
}

fn capital_research(sentinel: bool) -> ResearchLoop {
    let config = ResearchConfig {
        capital_sentinel: sentinel,
        capital_horizon: Duration::from_days(5),
        ..ResearchConfig::default()
    };
    ResearchLoop::new(config, BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"))
}

fn capital_signals(round: &qip_deepbrain::research::ResearchRound) -> usize {
    round
        .signals
        .iter()
        .filter(|s| s.class() == qip_contracts::ambient::SignalClass::Capital)
        .count()
}

/// R12-AMBIENT-3072/3065: the DECIDE stage's pre-positioning plan reaches the
/// attention router as a capital signal that takes the capital-preparation
/// pathway and is dispatched as advice, not offered to the research desk as a
/// gap; with the sentinel off the sweep raises none.
///
/// Mutation: route `SignalClass::Capital` to `Pathway::ResearchTask` in
/// `RoutingPolicy::standard`; the event's pathway is not capital preparation
/// and the advice is never dispatched.
#[test]
fn a_recommended_movement_is_a_capital_preparation_signal_dispatched_as_advice_and_absent_when_the_sentinel_is_off()
 {
    let platform = platform_with_a_funding_lane();
    let at = start().saturating_add(Duration::from_days(4));
    let plan = platform
        .pre_position(at, Duration::from_days(5))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        plan.moves.len(),
        1,
        "premise: the plan recommends one movement: {}",
        plan.describe()
    );

    let mut off = capital_research(false);
    let quiet = off
        .sweep(&platform, at, &set(&["vendor"]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(capital_signals(&quiet), 0, "the sentinel is off");
    assert!(quiet.capital_advice.is_empty());

    let mut on = capital_research(true);
    let round = on
        .sweep(&platform, at, &set(&["vendor"]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(capital_signals(&round), 1, "{round:?}");
    let event = round
        .attention_events
        .iter()
        .find(|e| e.pathway == qip_contracts::ambient::Pathway::CapitalPreparation)
        .unwrap_or_else(|| panic!("no event took the capital pathway: {round:?}"));
    assert_eq!(
        event.disposition,
        qip_contracts::ambient::Disposition::Activated
    );
    assert_eq!(round.capital_advice.len(), 1, "{round:?}");
    assert_eq!(
        round.capital_advice[0].amount_base,
        plan.moves[0].amount_base
    );
    assert!(
        on.advisories()
            .any(|a| a.view().contains("CapitalPreparation")),
        "the advice is filed as an advisory"
    );
    assert!(
        !on.desk()
            .queue()
            .ranked()
            .iter()
            .any(|e| e.item().research_question.contains("capital:")),
        "a capital signal is not a research gap"
    );
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
            warehouse: None,
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

/// R12-OBS-9151: the central plane carries model freshness. The age of the
/// last promoted model package is absent until one is promoted, grows with
/// every iteration after, and a newer promotion resets it. A gauge written
/// only at a promotion would read zero for ever after it, the stalest model
/// looking the freshest.
///
/// Mutation: drop the `self.package_promoted_at = Some(at)` assignment in
/// `Operations::observe`; the gauge never appears.
#[test]
fn a_promoted_model_package_resets_the_freshness_gauge_and_it_ages_between_promotions() {
    let platform = platform();
    let metrics = platform.telemetry().metrics.clone();
    let age = || {
        metrics
            .snapshot()
            .gauge(names::MODEL_PACKAGE_AGE, &labels([]))
    };
    let mut ops = Operations::new(start()).unwrap_or_else(|e| panic!("{e}"));
    let at = |hours: i64| start().saturating_add(Duration::from_hours(hours));
    let observe = |ops: &mut Operations, at: Timestamp, promoted: bool| {
        let facts = IterationFacts {
            package_promoted: promoted,
            ..facts(at, 10.0, &[])
        };
        ops.observe(&platform, &facts)
            .unwrap_or_else(|e| panic!("{e}"));
    };

    observe(&mut ops, at(0), false);
    assert_eq!(age(), None, "no package promoted, no freshness figure");

    observe(&mut ops, at(1), true);
    assert_eq!(age(), Some(0.0));
    observe(&mut ops, at(3), false);
    assert_eq!(age(), Some(7_200.0), "the package ages between promotions");

    observe(&mut ops, at(4), true);
    assert_eq!(age(), Some(0.0), "a newer promotion resets the age");
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

/// R12-EXPAND-6059: the tools the node registers are the tools its settings
/// say it runs, by the names the acquisition planner resolves them by.
///
/// The failure this prevents: the registry held three desks and the hosted
/// endpoint, so a gap that needed a classical solver or a quantum backend —
/// both of which the process runs — was queued as a tool to build and the
/// sweep spent compute rebuilding what the router already had; and the
/// opposite, a registry that listed a quantum backend on a node whose
/// compute settings attached none, admitting a gap unblocked on a tool that
/// would never answer. `tick-replay` stays unregistered because the node does
/// not hold it (the deployed-sweep test in `expansion_rows.rs` depends on
/// that).
#[test]
fn the_node_registers_every_tool_kind_it_actually_runs() {
    use qip_agents::tools::ToolKind;
    use qip_deepbrain::research::{node_tools, registered_tools, registered_tools_holding};
    use qip_expansion::plan::AcquisitionAction;
    use qip_kernel::compute_routing::{ComputeSettings, QuantumProviderChoice};

    let off = ComputeSettings::default();
    let simulated = ComputeSettings {
        provider: QuantumProviderChoice::Simulated,
        ..off
    };
    let names = |tools: &qip_agents::tools::ToolRegistry, kind| -> Vec<String> {
        tools
            .by_kind(kind)
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    };

    // Premise: the bare registry holds neither a solver nor a quantum
    // backend, so what follows is added by `node_tools` and nothing else.
    let bare = registered_tools().unwrap_or_else(|e| panic!("{e}"));
    assert!(names(&bare, ToolKind::Solver).is_empty());
    assert!(names(&bare, ToolKind::QuantumBackend).is_empty());

    // Off: the classical router's solver, no quantum backend, no connector.
    let held = registered_tools_holding(&node_tools(&off, &[])).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(names(&held, ToolKind::Solver), ["classical-solver"]);
    assert!(names(&held, ToolKind::QuantumBackend).is_empty());

    // Simulated and two polled sources: the backend and a connector each.
    let sources = vec!["src-a".to_string(), "src-b".to_string()];
    let held = registered_tools_holding(&node_tools(&simulated, &sources))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(names(&held, ToolKind::QuantumBackend), ["quantum-backend"]);
    assert_eq!(
        names(&held, ToolKind::Connector),
        ["connector/src-a", "connector/src-b", "source-discovery"]
    );
    // The names are the planner's: a gap formulating a solver finds both
    // tools it needs registered rather than queued to build.
    for (name, kind) in AcquisitionAction::FormulateSolver.tools() {
        let spec = held
            .get(name)
            .unwrap_or_else(|| panic!("{name} unregistered"));
        assert_eq!(spec.kind(), *kind);
    }
    assert!(held.get("tick-replay").is_none());

    // The loop the root builds counts what it holds.
    let loop_ = ResearchLoop::new_for_node(
        ResearchConfig::default(),
        coverage(&[]),
        &simulated,
        &sources,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let round = {
        let mut loop_ = loop_;
        loop_
            .sweep(&platform(), start(), &set(&["src-a"]), &set(&["US"]))
            .unwrap_or_else(|e| panic!("{e}"))
    };
    assert_eq!(
        round.registries.tools,
        qip_deepbrain::research::tool_count(&held),
        "{:?}",
        round.registries
    );
}

// --- regret reaches the sweep (R12-AMBIENT-3071) ---------------------------

/// A platform that knows `AAA`, so a refused order and the twin's pricing of
/// it have an instrument to be about.
fn platform_knowing_aaa() -> Platform {
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::costs::LiquidityProfile;
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                ObjectId::from_string("obj-AAA".to_string()),
                "AAA",
                InstrumentType::CommonStock,
                LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(Decimal::from_int(100))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .unwrap_or_else(|e| panic!("a valid object: {e}")),
        )
        .unwrap_or_else(|e| panic!("insertable: {e}"));
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("the platform assembles: {e}"))
}

/// A flat tape: `count` quiet daily bars ending at `start`, then a flat
/// `level` for `days` after it. The twin marks a declined path at the open of
/// a bar. `Platform::capture_submission` books a buy against the ask, so the
/// twin's gross for a declined buy is `(entry - exit) * quantity` (see
/// `qip-kernel/tests/learning.rs`, where a *rise* scores the decline
/// unfavourable): a level far below the ~100 entry is what makes each
/// declined buy score as a regret. The premise below asserts the score
/// rather than trusting this reading.
fn tape_stepping_to(level: f64, count: i64, days: i64) -> Vec<SensedRecord> {
    let mut records = Vec::new();
    for i in 0..count {
        let wiggle = if i % 2 == 0 { 0.3 } else { -0.3 };
        records.push(bar(
            "obj-AAA",
            start().saturating_sub(Duration::from_days(count - i)),
            100.0 + wiggle,
        ));
    }
    for day in 1..=days {
        records.push(bar(
            "obj-AAA",
            start().saturating_add(Duration::from_days(day)),
            level,
        ));
    }
    records
}

/// R12-AMBIENT-3071: counterfactual regret reaches the deep brain's sweep.
/// Three declines the twin prices as regrets at one gate (`MIN_REGRETS`) are
/// read by `ResearchLoop::sweep` straight off `Platform::declined_scores`,
/// priced, and routed as an Opportunity-class signal to the research-task
/// pathway naming that gate.
///
/// The failure this prevents: LEARN prices every veto and nothing reads the
/// result, so a gate that keeps refusing what would have earned is a number
/// on a counter and never a learning objective. The pure observation is
/// covered by `a_repeated_veto_gap_carries_the_earnings_its_gate_forwent`;
/// this proves the platform-to-sweep read and the routing, which that test
/// does not touch.
///
/// Mutation: delete `observations.extend(veto_observations(&declines)?)` in
/// `ResearchLoop::sweep_over` — no Opportunity-class signal is raised and the
/// last block fails.
#[test]
fn three_regretted_declines_at_one_gate_reach_the_sweep_as_an_opportunity_class_research_signal() {
    use qip_contracts::ambient::{Pathway, SignalClass};
    use qip_deepbrain::research::MIN_REGRETS;
    use qip_execution_engine::order::Side;

    let mut platform = platform_knowing_aaa();
    platform.observe(tape_stepping_to(30.0, 90, 5));
    for n in 0..MIN_REGRETS {
        let order = platform.order_from(
            ObjectId::from_string("obj-AAA".to_string()),
            Side::Buy,
            Decimal::from_int(1000),
            Decimal::from_int(100),
            &format!("prop-refused-{n}"),
            Vec::new(),
            start(),
        );
        assert!(
            platform.submit_order(order, start()).is_err(),
            "premise: an untraceable order is refused"
        );
    }
    assert_eq!(platform.declined_awaiting_score(), MIN_REGRETS);
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));

    // Premise: LEARN priced all three and every one is a regret at one gate,
    // so the sweep has exactly the evidence the detector asks for.
    let scores = platform.declined_scores();
    assert_eq!(scores.len(), MIN_REGRETS, "{scores:?}");
    assert!(scores.iter().all(|s| s.regret), "{scores:?}");
    let gate = scores[0].gate.clone();
    assert!(scores.iter().all(|s| s.gate == gate), "{scores:?}");

    let mut loop_ = research(coverage(&[]));
    let round = loop_
        .sweep(
            &platform,
            start().saturating_add(Duration::from_days(3)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap_or_else(|e| panic!("{e}"));

    let evidence = format!("declined_scores:gate:{gate}");
    let signal = round
        .signals
        .iter()
        .find(|s| s.class() == SignalClass::Opportunity && s.evidence_ids().contains(&evidence))
        .unwrap_or_else(|| {
            panic!(
                "no Opportunity-class signal names `{evidence}`: {:?}",
                round.signals
            )
        });
    let event = round
        .attention_events
        .iter()
        .find(|e| e.signal_id == signal.id())
        .unwrap_or_else(|| panic!("the signal was never routed: {:?}", round.attention_events));
    assert_eq!(
        event.pathway,
        Pathway::ResearchTask,
        "the regret gap is routed to the research-task pathway: {event:?}"
    );
}

// --- residual gaps are priced, and ranked by their deadline (R12-EXPAND-6048) ---

/// Fill one buy of `quantity` at 100 on a platform that knows `AAA`, and
/// hand the platform back with its venue's name and the notional it traded.
fn platform_with_a_fill_of(quantity: i64) -> (Platform, String, Decimal) {
    use qip_execution_engine::order::Side;
    let mut platform = platform_knowing_aaa();
    platform.observe(tape_stepping_to(100.0, 90, 5));
    let order = platform.order_from(
        ObjectId::from_string("obj-AAA".to_string()),
        Side::Buy,
        Decimal::from_int(quantity),
        Decimal::from_int(100),
        "prop-filled",
        vec!["hyp-prop-filled".to_string()],
        start(),
    );
    platform
        .submit_order(order, start())
        .unwrap_or_else(|e| panic!("the order is accepted: {e}"));
    let notionals = qip_deepbrain::research::fill_notionals(&platform)
        .unwrap_or_else(|e| panic!("notionals: {e}"));
    let (venue, notional) = notionals
        .iter()
        .next()
        .map(|(v, n)| (v.clone(), *n))
        .unwrap_or_else(|| panic!("premise: the fill is on the outcome record"));
    (platform, venue, notional)
}

/// The sweep's residual gap for `venue`, from fills that each missed the twin
/// by `bps`, and its economic value on the router's basis-point scale.
fn residual_gap_value_bp(platform: &Platform, venue: &str, bps: f64) -> u32 {
    use qip_contracts::ambient::SignalClass;
    use qip_deepbrain::research::FillObservation;
    let fills: Vec<FillObservation> = (0..4)
        .map(|i| FillObservation {
            venue: venue.to_string(),
            object_id: "obj-AAA".to_string(),
            filled_at: start().saturating_add(Duration::from_mins(i)),
            trade_error_bps: Some(bps),
        })
        .collect();
    let mut loop_ = research(coverage(&[]));
    let round = loop_
        .sweep_over(
            platform,
            &fills,
            &[],
            start().saturating_add(Duration::from_days(1)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let evidence = format!("fill_scores:{venue}");
    round
        .signals
        .iter()
        .find(|s| s.class() == SignalClass::Risk && s.evidence_ids().contains(&evidence))
        .unwrap_or_else(|| panic!("no residual signal for `{venue}`: {:?}", round.signals))
        .economic_value_bp()
}

/// R12-EXPAND-6048: the notional the platform filled at a venue prices that
/// venue's residual gap. Two platforms miss the twin by the same 40 bps; the
/// one that filled three times the size states three times the economic value
/// to the router, and the figure is the mean miss times the notional traded.
///
/// The failure prevented: residual gaps carried no size, so slippage cost was
/// unmeasurable from the fill record and a venue missing on a rounding error's
/// worth ranked level with one missing on the book.
///
/// Mutation: in `sweep_over`, call `unexplained_residuals` (the unpriced
/// detector) instead of `unexplained_residuals_priced`; both gaps state zero
/// and the first assertion on the smaller fill's figure fails.
#[test]
fn a_residual_gap_is_priced_from_the_notional_the_platform_filled() {
    let (small, venue_small, notional_small) = platform_with_a_fill_of(50);
    let (large, venue_large, notional_large) = platform_with_a_fill_of(150);
    assert_eq!(venue_small, venue_large, "premise: one simulated venue");
    assert!(
        notional_large > notional_small && notional_small.is_positive(),
        "premise: {notional_large} against {notional_small}"
    );

    // 40 bps of the notional, in units, then in the scale's hundredths.
    let expected =
        |notional: Decimal| (40.0 * notional.to_f64() / 10_000.0 / 100.0 * 10_000.0).round() as u32;
    let small_bp = residual_gap_value_bp(&small, &venue_small, 40.0);
    let large_bp = residual_gap_value_bp(&large, &venue_large, 40.0);
    assert!(small_bp > 0, "the smaller fills' gap states no value");
    assert_eq!(small_bp, expected(notional_small));
    assert_eq!(large_bp, expected(notional_large));
    assert!(large_bp > small_bp);
}

/// R12-EXPAND-6048: the deadline the research loop holds at admission reaches
/// the desk's ranking. A gap admitted with its whole window ahead is queued at
/// the urgency floor, not at the zero the desk used to state for every gap.
///
/// Mutation: pass `0.0` for the urgency to `offer_by_deadline` in
/// `ResearchLoop::admit`; the queued item's urgency is zero and this fails.
#[test]
fn a_gap_admitted_by_the_research_loop_is_queued_at_the_urgency_its_deadline_gives_it() {
    use qip_deepbrain::research::URGENCY_FLOOR_BP;
    let (platform, venue, _) = platform_with_a_fill_of(50);
    let fills: Vec<qip_deepbrain::research::FillObservation> = (0..4)
        .map(|i| qip_deepbrain::research::FillObservation {
            venue: venue.clone(),
            object_id: "obj-AAA".to_string(),
            filled_at: start().saturating_add(Duration::from_mins(i)),
            trade_error_bps: Some(40.0),
        })
        .collect();
    let mut loop_ = research(coverage(&[]));
    let round = loop_
        .sweep_over(
            &platform,
            &fills,
            &[],
            start().saturating_add(Duration::from_days(1)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        round.admissions.get("queued").copied().unwrap_or(0),
        1,
        "premise: the residual gap was queued: {:?}",
        round.admissions
    );
    let queued = loop_.desk().queue().ranked();
    let urgency = queued
        .iter()
        .map(|e| e.score().urgency)
        .fold(f64::NEG_INFINITY, f64::max);
    let floor = f64::from(URGENCY_FLOOR_BP) / 10_000.0;
    assert!(
        (urgency - floor).abs() < 1e-9,
        "queued at urgency {urgency}, not the floor {floor}"
    );
}

// --- the federation's questions reach the sweep (R12-AMBIENT-10093) ----------

/// A platform whose desk holds AAA, over bars the world models disagree on
/// materially, so its federation pass issues a request.
fn platform_holding_a_disputed_instrument() -> Platform {
    use qip_execution_engine::order::Side;
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_reasoning_engine::redteam::ReviewPolicy;
    use qip_risk::limits::{Limit, LimitKind};

    let object = ObjectId::from_string("obj-AAA");
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object.clone(),
                "AAA",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(Decimal::from_int(100))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .unwrap_or_else(|e| panic!("a valid object: {e}")),
        )
        .unwrap_or_else(|e| panic!("insertable: {e}"));
    let limits = LimitSet::new("research-test")
        .with(Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        ))
        .with(Limit::new(
            "max-leverage",
            LimitKind::MaxLeverage { limit: 2.0 },
        ));
    let config = PlatformConfig {
        review: ReviewPolicy {
            minimum_surviving_confidence: 0.10,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    };
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe, limits)
        .unwrap_or_else(|e| panic!("the platform assembles: {e}"));
    // The same series the kernel's federation tests feed: noise with one jump.
    let mut price = 100.0_f64;
    let records: Vec<SensedRecord> = (0..120usize)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == 80 { 0.09 } else { 0.0 };
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((120 - i) as i64));
            bar("obj-AAA", at, price)
        })
        .collect();
    platform.observe(records);
    let order = platform.order_from(
        object,
        Side::Buy,
        Decimal::from_int(100),
        Decimal::from_int(100),
        "prop-book",
        vec!["hyp-book".to_string()],
        start(),
    );
    platform
        .submit_order(order, start())
        .unwrap_or_else(|e| panic!("the paper venue takes the order: {e}"));
    platform
}

/// The failure this guards: the federation priced a disagreement over capital
/// at stake and the request went nowhere, because nothing in the deep brain
/// read it — so the question sat on the review and no sweep ever ranked it.
/// Mutation: drop the `admit_federation_requests` call in `sweep_over` — no
/// request launches and the `info_launched` assertion fails.
#[test]
fn a_federation_request_on_a_held_instrument_is_launched_by_the_sweep_and_one_with_no_licensed_source_is_not()
 {
    let mut platform = platform_holding_a_disputed_instrument();
    assert!(
        platform.positions_at_cost().contains_key("obj-AAA"),
        "premise: the desk holds AAA"
    );
    let _ = platform.run_cycle(start());
    let requests = platform.federation_information_requests().to_vec();
    assert_eq!(requests.len(), 1, "premise: the pass issued a request");
    let sources = &requests[0].0.allowed_sources;
    assert!(!sources.is_empty(), "premise: it names a source");

    let mut unlicensed = research(coverage(&[]));
    let round = unlicensed
        .sweep(&platform, start(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        unlicensed
            .info_requests()
            .all(|r| !r.id().starts_with("fed-")),
        "no source is licensed, so nothing is asked: {round:?}"
    );

    let licensed: BTreeSet<String> = sources.iter().cloned().collect();
    let mut licensed_loop = research(coverage(&[]));
    let round = licensed_loop
        .sweep(&platform, start(), &licensed, &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    let federation: Vec<_> = licensed_loop
        .info_requests()
        .filter(|r| r.id().starts_with("fed-"))
        .collect();
    assert_eq!(federation.len(), 1, "{round:?}");
    assert!(round.info_launched >= 1, "{round:?}");
}
