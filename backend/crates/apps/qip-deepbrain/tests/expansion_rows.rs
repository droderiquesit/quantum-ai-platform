//! The Intelligence Expansion Engine as the deep brain's binary assembles it
//! (R12-EXPAND-1113, 1114, 1120, 6037, 6041, 6045, 6050).
//!
//! Every test builds the loop the way `main.rs` does — `ResearchConfig` from
//! `ResearchConfig::from_lookup` over the process environment, here an empty
//! one, which is the deployed default; the bounds from a `DiscoveryDesk`
//! built from `DiscoveryConfig::from_lookup` the same way, which is where
//! `node::run_with` reads licensed sources and eligible jurisdictions from —
//! and drives it through `maybe_sweep`, the call `node::run_with` makes
//! between cycles, or through `sweep_over` where a test must hand the loop
//! fills a paper venue would have produced. `sweep` is `sweep_over` over the
//! platform's own fills, so the path is the same one.
//!
//! The failure these exist to stop is the one the assessments recorded: each
//! piece of the engine was built and tested as a library while no shipped
//! binary called it, and a capability with no caller reads as one and is
//! not. Each test asserts its premise before its property, so an empty
//! platform cannot pass for a quiet one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_agents::tools::{ToolKind, ToolPermission};
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_deepbrain::discovery::{DiscoveryConfig, DiscoveryDesk};
use qip_deepbrain::research::{FillObservation, ResearchConfig, ResearchLoop, registered_tools};
use qip_expansion::curriculum::{State, TaskKind};
use qip_expansion::desk::CloseReason;
use qip_expansion::plan::AcquisitionAction;
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::LimitSet;
use qip_storage::MemoryBlobStore;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const VENUE: &str = "XSIM";

fn t0() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn platform_over(universe: Universe) -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t0(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
    .unwrap()
}

/// The environment a deployment that set nothing hands `main.rs`.
fn unset(_: &str) -> Option<String> {
    None
}

/// The research loop exactly as `main.rs` builds it: configuration from the
/// environment, the asset-class coverage map, and the reflex package store.
/// No signer, because `main.rs` installs none.
fn deployed(coverage: BTreeMap<String, BTreeSet<String>>) -> ResearchLoop {
    let config = ResearchConfig::from_lookup(&unset).unwrap();
    ResearchLoop::new(config, coverage)
        .unwrap()
        .with_package_store(Arc::new(MemoryBlobStore::new()))
}

/// The licensed sources and eligible jurisdictions `node::run_with` passes a
/// sweep: the discovery desk's, built from the environment as `main.rs`
/// builds it, before any discovery pass has catalogued a source.
fn deployed_bounds() -> (BTreeSet<String>, BTreeSet<String>) {
    let desk = DiscoveryDesk::new(DiscoveryConfig::from_lookup(&unset).unwrap(), vec![]);
    (
        desk.licensed_sources().clone(),
        desk.eligible_jurisdictions().clone(),
    )
}

fn coverage(class: &str, instruments: &[&str]) -> BTreeMap<String, BTreeSet<String>> {
    BTreeMap::from([(
        class.to_string(),
        instruments.iter().map(|i| (*i).to_string()).collect(),
    )])
}

fn bar(symbol: &str, at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap();
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

/// Sixty quiet days around 100 and then a 25% jump, on three subjects.
fn jumped() -> Platform {
    let mut platform = platform_over(Universe::new());
    let mut records = Vec::new();
    for symbol in ["obj-A", "obj-B", "obj-C"] {
        for day in 0..61i64 {
            let close = match day {
                60 => 125.0,
                d if d % 2 == 0 => 100.1,
                _ => 99.9,
            };
            records.push(bar(
                symbol,
                t0().saturating_sub(Duration::from_days(61 - day)),
                close,
            ));
        }
    }
    platform.observe(records);
    platform
}

/// Eighty quiet days around 100, then forty days compounding 1% a day: a
/// drift in returns the structural-break detector classifies, which the
/// anomaly mapper prices from its z-score.
fn trending() -> Platform {
    let mut platform = platform_over(Universe::new());
    let mut close = 100.0;
    let records = (0..120i64)
        .map(|day| {
            close = if day < 80 {
                if day % 2 == 0 { 100.1 } else { 99.9 }
            } else {
                close * 1.01
            };
            bar(
                "obj-T",
                t0().saturating_sub(Duration::from_days(120 - day)),
                close,
            )
        })
        .collect();
    platform.observe(records);
    platform
}

/// Ten scored fills before `t0` about `historical` bp off, then `post`
/// after it about `later` bp off.
fn fills(historical: f64, post: usize, later: f64) -> Vec<FillObservation> {
    let at = |offset_mins: i64| t0().saturating_add(Duration::from_mins(offset_mins));
    let mut out: Vec<FillObservation> = (0..10)
        .map(|i| FillObservation {
            venue: VENUE.to_string(),
            object_id: "BTC-USD".to_string(),
            filled_at: at(i - 60),
            trade_error_bps: Some(historical + if i % 2 == 0 { -5.0 } else { 5.0 }),
        })
        .collect();
    out.extend((0..post as i64).map(|i| FillObservation {
        venue: VENUE.to_string(),
        object_id: "BTC-USD".to_string(),
        filled_at: at(i + 1),
        trade_error_bps: Some(later + if i % 2 == 0 { -2.0 } else { 2.0 }),
    }));
    out
}

/// The configuration gate in front of everything below. Unset, the loop
/// sweeps every cycle; `0` switches it off; a value that is not a cycle
/// count is refused, and `main.rs` maps the refusal to a configuration error
/// that stops the process rather than running research on a cadence nobody
/// chose. Mutation: delete the `every_cycles == 0` arm of
/// `ResearchLoop::maybe_sweep` — the switched-off loop sweeps, and the
/// `is_none` assertion fails.
#[test]
fn the_research_loop_sweeps_every_cycle_unless_switched_off_and_a_malformed_cadence_is_refused() {
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    assert!(
        eligible.contains("US"),
        "premise: the deployed default researches under the US jurisdiction"
    );

    let config = ResearchConfig::from_lookup(&unset).unwrap();
    assert_eq!(config.every_cycles, 1, "unset, the loop sweeps every cycle");
    let mut on = deployed(BTreeMap::new());
    assert!(
        on.maybe_sweep(&platform, 1, t0(), &licensed, &eligible)
            .unwrap()
            .is_some(),
        "the deployed default ran no sweep"
    );

    let off = |name: &str| (name == "QIP_DEEPBRAIN_RESEARCH_EVERY").then(|| "0".to_string());
    let mut stopped =
        ResearchLoop::new(ResearchConfig::from_lookup(&off).unwrap(), BTreeMap::new()).unwrap();
    assert!(
        stopped
            .maybe_sweep(&platform, 1, t0(), &licensed, &eligible)
            .unwrap()
            .is_none(),
        "QIP_DEEPBRAIN_RESEARCH_EVERY=0 did not switch research off"
    );

    let malformed =
        |name: &str| (name == "QIP_DEEPBRAIN_RESEARCH_EVERY").then(|| "hourly".to_string());
    let refused = ResearchConfig::from_lookup(&malformed).expect_err("a malformed cadence ran");
    assert!(
        refused.message().contains("QIP_DEEPBRAIN_RESEARCH_EVERY"),
        "the refusal does not name the variable to fix: {}",
        refused.message()
    );
}

/// R12-EXPAND-1113. Novelty and gap detection run inside the deployed sweep
/// and leave a record: the same 25% jump on three subjects is surfaced as one
/// unknown-unknown nobody asked about — a cluster no single detector names —
/// recorded as a gap on the series and queued as research that cites the
/// cluster rule, at full novelty because it has never been seen before.
/// Mutation: delete `observations.extend(anomaly_clusters(..))` in
/// `ResearchLoop::sweep_over` — no cluster is queued and the first
/// assertion after the premise fails.
#[test]
fn price_jumps_the_platform_recorded_are_detected_as_one_novel_cluster_and_recorded_as_a_gap() {
    let platform = jumped();
    assert_eq!(platform.price_history().len(), 3, "premise: three series");
    let (licensed, eligible) = deployed_bounds();
    let mut research = deployed(BTreeMap::new());

    let round = research
        .maybe_sweep(&platform, 1, t0(), &licensed, &eligible)
        .unwrap()
        .expect("the deployed default sweeps every cycle");

    assert!(
        round.anomalies >= 3,
        "premise: the jumps are anomalies: {round:?}"
    );
    let clusters: Vec<_> = research
        .desk()
        .queue()
        .ranked()
        .into_iter()
        .filter(|e| {
            e.item()
                .research_question
                .contains("subjects show the same")
        })
        .collect();
    assert_eq!(clusters.len(), 1, "one cluster, queued: {round:?}");
    let cluster = clusters[0];
    assert!(
        (cluster.score().novelty - 1.0).abs() < 1e-12,
        "a first sighting is fully novel"
    );
    let lineage = research
        .desk()
        .lineage(&cluster.item().research_question)
        .expect("the cluster keeps its lineage");
    assert_eq!(lineage.derivation.rule, "detector:anomaly_cluster");
    let snapshot = platform.telemetry().metrics.snapshot();
    assert!(
        snapshot.counter(
            names::RESEARCH_GAPS,
            &labels([("class", "causal"), ("outcome", "queued")])
        ) >= 1,
        "the detection was queued and nothing recorded it"
    );
}

/// R12-EXPAND-1114 and 6041. The curriculum is generated by the platform
/// from its own state, with no prompt: an asset class with no history
/// becomes a `CurriculumItem` in the `ResearchQueue` that passes its own
/// contract, is scored on the curriculum's criteria from what was measured
/// (the detector's severity as uncertainty reduction and information gain,
/// first sighting as full novelty, the declared cost), and carries the
/// derivation that produced it — the rule, the evidence and the violated
/// assumption — so an item that turns out to be noise can be traced to what
/// raised it. The queue ranks what to learn next: in the same sweep the
/// price anomalies, which carry an economic value, are ranked above the
/// coverage gap, whose detector states none and which §9.10's product
/// therefore ranks at zero — queued, because admission is not rank, but
/// learned last. Mutations: (1) delete the `self.lineage.insert(..)` in
/// `ExpansionDesk::offer` — the item is queued with no lineage and the
/// `expect` on it fails; (2) swap `a` and `b` in `ResearchQueue::ranked`'s
/// priority comparison — the zero-priority item is ranked first and the
/// ranking assertion fails.
#[test]
fn an_unsupported_asset_class_becomes_a_validated_curriculum_item_scored_on_measurements_and_carrying_its_derivation()
 {
    let platform = trending();
    assert!(
        !platform.price_history().contains_key("obj-X"),
        "premise: the Equity class's instruments have no history"
    );
    let (licensed, eligible) = deployed_bounds();
    let mut research = deployed(coverage("Equity", &["obj-X", "obj-Y"]));

    let round = research
        .maybe_sweep(&platform, 1, t0(), &licensed, &eligible)
        .unwrap()
        .expect("the deployed default sweeps every cycle");

    let ranked = research.desk().queue().ranked();
    let position = ranked
        .iter()
        .position(|e| {
            e.item()
                .research_question
                .contains("`Equity` is unsupported")
        })
        .unwrap_or_else(|| panic!("the coverage gap was not queued: {round:?}"));
    let valued = ranked
        .iter()
        .filter(|e| e.score().expected_economic_value.is_positive())
        .count();
    assert!(
        valued >= 1,
        "premise: the anomalies queued research with an economic value: {round:?}"
    );
    assert!(
        ranked
            .iter()
            .take(valued)
            .all(|e| e.score().priority() > 0.0),
        "an item with economic value was ranked below one without"
    );
    assert!(
        position >= valued,
        "the zero-value coverage item was ranked above research worth something: {position} \
         of {}",
        ranked.len()
    );
    let entry = ranked[position];
    let item = entry.item();
    item.validate()
        .expect("the generated item passes its contract");
    assert_eq!(item.owner, "qip-deepbrain/research");
    assert_eq!(item.tools_and_data, vec!["source-discovery".to_string()]);
    assert_eq!(item.stop_conditions.len(), 2, "{:?}", item.stop_conditions);
    assert_eq!(entry.kind(), TaskKind::DataAcquisition);
    let score = entry.score();
    assert!(
        (score.uncertainty_reduction - 1.0).abs() < 1e-12,
        "{}",
        score.uncertainty_reduction
    );
    assert!(
        (score.information_gain - 1.0).abs() < 1e-12,
        "{}",
        score.information_gain
    );
    assert!((score.novelty - 1.0).abs() < 1e-12, "{}", score.novelty);
    assert_eq!(score.compute_data_cost, Decimal::from_int(2));
    assert_eq!(score.expected_economic_value, Decimal::ZERO);
    assert!(score.priority().abs() < 1e-12, "{score:?}");

    let lineage = research
        .desk()
        .lineage(&item.research_question)
        .expect("the curriculum item keeps its derivation");
    assert_eq!(lineage.action, AcquisitionAction::DiscoverLawfulData);
    assert_eq!(lineage.derivation.conclusion, item.research_question);
    assert_eq!(lineage.derivation.rule, "detector:coverage");
    assert!(
        lineage
            .derivation
            .evidence
            .contains(&"universe:asset_class:Equity".to_string()),
        "{:?}",
        lineage.derivation.evidence
    );
    assert!(!lineage.derivation.violated_assumptions.is_empty());
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .gauge(names::RESEARCH_QUEUE_ITEMS, &labels([])),
        Some(ranked.len() as f64)
    );
}

/// R12-EXPAND-6045. A gap is emitted as an `AmbientSignal`, the attention
/// router records an `AttentionEvent` activating it, and the activation —
/// not a person — creates the curriculum item. The advisory names the
/// signal it came from and the queued item carries the gap the signal was
/// raised for, so the path from weak signal to research is one chain.
/// Mutation: delete the `self.admit(..)` call in the `Activated` arm of
/// `ResearchLoop::sweep_over` — the signal is still activated and recorded,
/// but nothing is queued and the queue assertion fails.
#[test]
fn an_ambient_signal_the_router_activates_creates_a_curriculum_item_without_a_prompt() {
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let mut research = deployed(coverage("Equity", &["obj-A"]));

    let round = research
        .maybe_sweep(&platform, 1, t0(), &licensed, &eligible)
        .unwrap()
        .expect("the deployed default sweeps every cycle");

    assert_eq!(
        round.attention.get(&("reflex_model_update", "activated")),
        Some(&1),
        "premise: the router activated the signal: {round:?}"
    );
    assert_eq!(
        platform.telemetry().metrics.snapshot().counter(
            names::AMBIENT_ATTENTION,
            &labels([
                ("pathway", "reflex_model_update"),
                ("disposition", "activated")
            ])
        ),
        1,
        "the attention event was not recorded"
    );
    let advisories: Vec<_> = research.advisories().collect();
    assert_eq!(advisories.len(), 1);
    assert_eq!(advisories[0].origin_signal(), "ambient-1");
    let ranked = research.desk().queue().ranked();
    assert_eq!(ranked.len(), 1, "the activation queued nothing: {round:?}");
    let origin = ranked[0]
        .origin()
        .expect("the queued item carries the gap its signal was raised for");
    assert!(
        advisories[0]
            .view()
            .contains(&ranked[0].item().research_question),
        "the advisory {:?} names a different question from the item",
        advisories[0].view()
    );
    assert_eq!(origin.signal.evidence[0], "detector:coverage");
}

/// R12-EXPAND-6037. A gap whose acquisition needs a tool this node does not
/// have — three lot-size refusals at the desk's venue are an execution gap,
/// and replaying ticks needs a `tick-replay` simulator nobody registered —
/// produces a `ToolSpec` proposal and a build item for it, read-only and
/// unregistered, and the build item carries the gap it unblocks. The tool is
/// proposed, not acquired: the registry the node runs with still lacks it.
/// Mutation: delete `self.proposals.push(tool.clone())` in
/// `ResearchQueue::admit` — the build item is queued but no spec is on
/// record, and the proposal assertion fails.
#[test]
fn an_execution_gap_needing_a_tool_the_node_lacks_proposes_a_read_only_tool_spec_with_its_lineage()
{
    let object = ObjectId::from_string("obj-AAA".to_string());
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object.clone(),
                "AAA",
                InstrumentType::CommonStock,
                LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .price(Decimal::from_int(100))
            .provenance(Provenance::synthetic("test", t0()))
            .build(t0())
            .unwrap(),
        )
        .unwrap();
    let mut platform = platform_over(universe);
    platform.observe(
        (0..90i64)
            .map(|day| {
                bar(
                    "obj-AAA",
                    t0().saturating_sub(Duration::from_days(90 - day)),
                    100.0,
                )
            })
            .collect(),
    );
    for _ in 0..3 {
        // Ten and a half shares of a one-lot listing: refused by the
        // simulated venue's lot gate before any order exists there.
        let order = platform.order_from(
            object.clone(),
            serde_json::from_str("\"buy\"").unwrap(),
            Decimal::parse("10.5").unwrap(),
            Decimal::from_int(100),
            "prop-expansion",
            vec!["hyp-expansion".to_string()],
            t0(),
        );
        platform
            .submit_order(order, t0())
            .expect_err("an off-lot order reached the venue");
    }
    assert_eq!(
        platform.feasibility_refusals().len(),
        3,
        "premise: three feasibility refusals are on the platform's record"
    );
    assert!(
        registered_tools().unwrap().get("tick-replay").is_none(),
        "premise: the node has no tick-replay tool"
    );
    let (licensed, eligible) = deployed_bounds();
    let mut research = deployed(BTreeMap::new());

    let round = research
        .maybe_sweep(&platform, 1, t0(), &licensed, &eligible)
        .unwrap()
        .expect("the deployed default sweeps every cycle");

    assert_eq!(
        round.admissions.get("blocked"),
        Some(&1),
        "the execution gap was not queued blocked on its tool: {round:?}"
    );
    let proposals = research.desk().queue().proposals();
    assert_eq!(proposals.len(), 1, "no tool spec was proposed: {round:?}");
    assert_eq!(proposals[0].name(), "tick-replay");
    assert_eq!(proposals[0].kind(), ToolKind::Simulator);
    assert_eq!(
        proposals[0].scope(),
        &BTreeSet::from([ToolPermission::Read]),
        "a proposed tool starts read-only"
    );
    assert_eq!(round.desk.proposals, 1);

    let ranked = research.desk().queue().ranked();
    let blocked = ranked
        .iter()
        .find(|e| e.kind() == TaskKind::Campaign)
        .expect("the execution research is queued");
    assert_eq!(
        blocked.state(),
        &State::Blocked {
            tools: vec!["tick-replay".to_string()]
        }
    );
    let lineage = research
        .desk()
        .lineage(&blocked.item().research_question)
        .expect("the blocked research keeps its lineage");
    assert_eq!(lineage.action, AcquisitionAction::ReplayTicks);
    assert_eq!(lineage.derivation.rule, "detector:refusal_cluster");
    let build = ranked
        .iter()
        .find(|e| e.kind() == TaskKind::ToolBuild)
        .expect("a build item is queued for the missing tool");
    assert!(
        build.item().research_question.contains("`tick-replay`"),
        "{}",
        build.item().research_question
    );
    let origin = build
        .origin()
        .expect("the build item carries the gap it unblocks");
    assert!(
        origin
            .signal
            .evidence
            .iter()
            .any(|e| e.starts_with("feasibility_refusals:simulated-venue:")),
        "{:?}",
        origin.signal.evidence
    );
}

/// Two sweeps of the deployed loop over `first` and then `second`, the
/// second an hour later, returning the second round.
fn two_sweeps(
    research: &mut ResearchLoop,
    first: &[FillObservation],
    second: &[FillObservation],
) -> qip_deepbrain::research::ResearchRound {
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let opening = research
        .sweep_over(&platform, first, &[], t0(), &licensed, &eligible)
        .unwrap();
    assert!(
        opening.desk.started.iter().any(|q| q.contains(VENUE)),
        "premise: the residual gap was granted and started: {opening:?}"
    );
    assert_eq!(
        opening.campaigns.get("waiting"),
        Some(&1),
        "premise: with no post-start fills the campaign has no paper or shadow stage and \
         nothing is promoted: {opening:?}"
    );
    research
        .sweep_over(
            &platform,
            second,
            &[],
            t0().saturating_add(Duration::from_hours(1)),
            &licensed,
            &eligible,
        )
        .unwrap()
}

/// R12-EXPAND-1120, and the null screen R12-EXPAND-6061's check names. A
/// candidate correction is promoted only when its recorded benchmark beats
/// the incumbent: the uncorrected twin is the champion, and a correction
/// fitted on fills ~40 bp rich applied to fills that came in ~40 bp cheap is
/// worse than leaving the twin alone. That candidate is refused, its null is
/// recorded, no capability or reflex package exists, and the same question
/// raised again is refused at admission rather than paid for twice. The
/// first half runs the same road with a correction that does beat the
/// champion, so the refusal is the gate and not a road that promotes
/// nothing. Mutations: (1) delete the `if !failed.is_empty()` refusal in
/// `promotion::promote` — the worse candidate is promoted and the
/// `null_recorded` assertion fails; (2) delete the
/// `self.research.record(.., Verdict::Null ..)` call in
/// `ExpansionDesk::conclude` — the re-raised question is queued again and
/// the `refused` assertion fails.
#[test]
fn a_correction_that_does_not_beat_the_uncorrected_twin_is_refused_promotion_and_its_null_screens_a_repeat()
 {
    let mut better = deployed(BTreeMap::new());
    let promoted = two_sweeps(&mut better, &fills(40.0, 0, 0.0), &fills(40.0, 8, 40.0));
    assert_eq!(
        promoted.campaigns.get("promoted"),
        Some(&1),
        "premise: a correction that beats the champion is promoted: {promoted:?}"
    );
    assert_eq!(better.capabilities().count(), 1);

    let mut worse = deployed(BTreeMap::new());
    let refused = two_sweeps(&mut worse, &fills(40.0, 0, 0.0), &fills(40.0, 8, -40.0));

    assert_eq!(
        refused.campaigns.get("null_recorded"),
        Some(&1),
        "a correction worse than the incumbent was not refused: {refused:?}"
    );
    assert_eq!(refused.campaigns.get("promoted"), None, "{refused:?}");
    assert_eq!(
        worse.capabilities().count(),
        0,
        "a worse candidate became a capability"
    );
    assert_eq!(
        worse.packages().count(),
        0,
        "a worse candidate was compiled"
    );
    assert!(
        worse
            .desk()
            .closed()
            .any(|c| c.reason == CloseReason::NullRecorded && c.question.contains(VENUE)),
        "the null was not recorded on the desk"
    );

    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let again = worse
        .sweep_over(
            &platform,
            &fills(40.0, 8, -40.0),
            &[],
            t0().saturating_add(Duration::from_hours(2)),
            &licensed,
            &eligible,
        )
        .unwrap();
    assert!(
        again.raised >= 1,
        "premise: the same gap is raised again: {again:?}"
    );
    assert_eq!(
        again.admissions.get("refused"),
        Some(&1),
        "a question already found null was admitted again: {again:?}"
    );
    assert!(
        worse
            .desk()
            .queue()
            .ranked()
            .iter()
            .all(|e| !e.item().research_question.contains(VENUE)),
        "the null question is back in the queue"
    );
}

/// R12-EXPAND-6050. The campaign the deployed loop runs crosses historical,
/// simulated, paper and shadow in that order, each stage naming the dataset
/// it ran on, and is reproducible: two independent loops over the same
/// fills record the same four digests. A campaign missing its later stages
/// is not promoted (the opening sweep's `waiting`, asserted in
/// `two_sweeps`). Mutation: delete the `Environment::Shadow`
/// `campaign.record(..)` in `executor::run_fill_bias_campaign` — `promote`
/// refuses the three-stage campaign and the `promoted` assertion fails.
#[test]
fn a_research_campaign_runs_four_environments_in_order_with_provenance_and_replays_to_the_same_digests()
 {
    let run = || {
        let mut research = deployed(BTreeMap::new());
        let round = two_sweeps(&mut research, &fills(40.0, 0, 0.0), &fills(40.0, 8, 40.0));
        assert_eq!(round.campaigns.get("promoted"), Some(&1), "{round:?}");
        let packages: Vec<_> = research.packages().cloned().collect();
        assert_eq!(packages.len(), 1, "the promoted campaign was not compiled");
        packages[0]
            .compiled
            .derivation
            .trace
            .iter()
            .take(4)
            .map(|step| step.statement.clone())
            .collect::<Vec<String>>()
    };

    let first = run();
    let order = ["Historical", "Simulated", "Paper", "Shadow"];
    assert_eq!(first.len(), 4, "{first:?}");
    for (statement, environment) in first.iter().zip(order) {
        let prefix = format!("the {environment} stage ran on dataset ");
        let digest = statement
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("`{statement}` is not the {environment} stage"));
        assert!(
            !digest.trim().is_empty(),
            "the {environment} stage names no dataset"
        );
    }
    assert_eq!(
        first,
        run(),
        "the same fills replayed to different datasets"
    );
}
