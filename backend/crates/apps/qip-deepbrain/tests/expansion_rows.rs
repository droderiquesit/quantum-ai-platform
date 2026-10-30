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

use qip_agents::tools::{ToolKind, ToolName, ToolPermission};
use qip_contracts::expansion::OntologyTypeFamily;
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
        registered_tools()
            .unwrap()
            .get(&ToolName::parse("tick-replay").unwrap())
            .is_none(),
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
    // Replaying a venue's ticks needs the replay simulator and the adapter
    // that reads the venue (R12-EXPAND-6037); the node holds neither, so the
    // deployed sweep proposes both, as one pack under the one gap.
    let proposals = research.desk().queue().proposals();
    let proposed: Vec<(&str, ToolKind)> = proposals.iter().map(|p| (p.name(), p.kind())).collect();
    assert_eq!(
        proposed,
        [
            ("tick-replay", ToolKind::Simulator),
            ("venue-adapter", ToolKind::VenueAdapter)
        ],
        "the tool specs proposed: {round:?}"
    );
    for proposal in proposals {
        assert_eq!(
            proposal.scope(),
            &BTreeSet::from([ToolPermission::Read]),
            "a proposed tool starts read-only"
        );
    }
    assert_eq!(round.desk.proposals, 2);

    let ranked = research.desk().queue().ranked();
    let blocked = ranked
        .iter()
        .find(|e| e.kind() == TaskKind::Campaign)
        .expect("the execution research is queued");
    assert_eq!(
        blocked.state(),
        &State::Blocked {
            tools: vec!["tick-replay".to_string(), "venue-adapter".to_string()]
        }
    );
    let [pack] = research.desk().queue().packs() else {
        panic!(
            "one capability pack per blocked gap: {:?}",
            research.desk().queue().packs()
        );
    };
    assert_eq!(pack.lineage(), blocked.item().research_question);
    assert_eq!(pack.tools(), proposals);
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
        "premise: the residual gap was granted and started"
    );
    assert_eq!(
        opening.campaigns.get("waiting"),
        Some(&1),
        "premise: with no post-start fills the campaign has no paper or shadow stage and \
         nothing is promoted"
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
        "premise: a correction that beats the champion is promoted"
    );
    assert_eq!(better.capabilities().count(), 1);

    let mut worse = deployed(BTreeMap::new());
    let refused = two_sweeps(&mut worse, &fills(40.0, 0, 0.0), &fills(40.0, 8, -40.0));

    assert_eq!(
        refused.campaigns.get("null_recorded"),
        Some(&1),
        "a correction worse than the incumbent was not refused"
    );
    assert_eq!(
        refused.campaigns.get("promoted"),
        None,
        "nothing is promoted"
    );
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
    assert!(again.raised >= 1, "premise: the same gap is raised again");
    assert_eq!(
        again.admissions.get("refused"),
        Some(&1),
        "a question already found null was admitted again"
    );
    // The null question itself, compared whole. This asserted that no queued
    // question mentioned the venue at all, which stopped being the property
    // once the sweep's tick campaigns reached the curriculum (RM-P9-08): the
    // same fills now also queue tick replays at `XSIM`, and a build of the
    // `tick-replay` tool those are blocked on — different questions that name
    // the venue legitimately. A substring of the venue matched them and failed
    // on research the loop is meant to queue, which is the delimited-token
    // trap `.claude/rules/architecture/01-testing-strategy.md` names.
    let null_questions: Vec<String> = worse
        .desk()
        .closed()
        .filter(|c| c.reason == CloseReason::NullRecorded && c.question.contains(VENUE))
        .map(|c| c.question.clone())
        .collect();
    assert!(
        !null_questions.is_empty(),
        "premise: a null question at the venue is on record"
    );
    let queued: Vec<String> = worse
        .desk()
        .queue()
        .ranked()
        .iter()
        .map(|e| e.item().research_question.clone())
        .collect();
    assert!(
        queued.iter().all(|q| !null_questions.contains(q)),
        "the null question is back in the queue: null {null_questions:?}, queued {queued:?}"
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
        assert_eq!(round.campaigns.get("promoted"), Some(&1), "one promotion");
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
    // ADR 0142 d3: shadow before paper.
    let order = ["Historical", "Simulated", "Shadow", "Paper"];
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

/// R12-EXPAND-6062 on the shipped path (ADR 0158 decision 15): a promoted
/// fill-bias correction is recorded in the loop's Capability Registry under
/// the verb `promotion::verb_for` gives its class, with the eligibility the
/// promotion declares, and the recent-history deque and the register agree.
/// Until this change the entry went into the deque and no register, so a
/// consumer asking "what can this platform simulate" had nothing to read.
/// Mutation: delete the `capability_registry.promote(..)` call in the
/// conclude arm — `capability_count()` stays 0 and the first assertion fails.
#[test]
fn a_promoted_fill_bias_correction_is_recorded_in_the_capability_registry_with_paper_and_shadow_eligibility()
 {
    let mut research = deployed(BTreeMap::new());
    assert_eq!(
        research.capability_registry().capability_count(),
        0,
        "premise: nothing promoted yet"
    );
    let round = two_sweeps(&mut research, &fills(40.0, 0, 0.0), &fills(40.0, 8, 40.0));
    assert_eq!(
        round.campaigns.get("promoted"),
        Some(&1),
        "premise: one promotion"
    );
    assert!(round.capabilities_refused.is_empty(), "{round:?}");

    let registry = research.capability_registry();
    assert_eq!(registry.capability_count(), 1, "one verb recorded");
    let promoted: Vec<_> = research.capabilities().collect();
    assert_eq!(promoted.len(), 1, "premise: the deque holds the promotion");
    let recorded = registry
        .get_capability(promoted[0].verb)
        .unwrap_or_else(|| panic!("the promoted verb {:?} is not registered", promoted[0].verb));
    assert_eq!(recorded.implementations, promoted[0].implementations);
    assert!(
        recorded.eligibility.contains("paper") && recorded.eligibility.contains("shadow"),
        "{recorded:?}"
    );
    assert!(
        !recorded.eligibility.contains("live"),
        "no capability is ever eligible live: {recorded:?}"
    );
    assert_eq!(registry.all_verbs(), vec![promoted[0].verb]);
}

/// A platform that observed thirteen reference-data updates to obj-A's
/// `shares_outstanding`, twelve near a thousand and the last at five
/// thousand: a fundamental surprise no registered type explains.
fn reference_surprised() -> Platform {
    use qip_financial::intelligence::ReferenceDataUpdate;
    let mut platform = platform_over(Universe::new());
    let updates: Vec<SensedRecord> = (0..13)
        .map(|i| {
            let at = t0().saturating_sub(Duration::from_hours(13 - i));
            let value = if i == 12 {
                5_000.0
            } else {
                1_000.0 + if i % 2 == 0 { 5.0 } else { -5.0 }
            };
            SensedRecord::ReferenceData(Box::new(ReferenceDataUpdate {
                object_id: "obj-A".to_string(),
                field: "shares_outstanding".to_string(),
                previous_value: None,
                new_value: format!("{value}"),
                effective_from: at,
                provenance: Provenance::new("reference-test", at, at),
                evidence_unretrievable: false,
            }))
        })
        .collect();
    assert_eq!(
        platform.observe(updates),
        13,
        "premise: every update absorbed"
    );
    platform
}

/// R12-EXPAND-6029 and R12-EXPAND-1021 on the shipped path. A reference
/// field the world model holds is read by the sweep as a named observation;
/// a surprise in it the schema cannot explain is a type-wanting observation,
/// so an ontology gap; and the gap leaves one validated `OntologyPatch`
/// citing the signal that raised it and the subject it was about, registered
/// in the loop's ontology as a candidate type whose description cites the
/// gap. A second sweep over the same surprise proposes the next version
/// rather than colliding with the first. Until this change the detector
/// context carried prices alone, so no observation ever wanted a type and
/// the ontology registry only ever held what it started with.
///
/// Mutations: delete the `round.ontology_patches.push(patch)` arm — the
/// first patch assertion fails; delete `self.ontology.register(..)` in
/// `propose_type` — `get_latest` is `None`; delete the `with_observations`
/// feed in `sweep_over` — no anomaly, no gap, no patch.
#[test]
fn an_observation_wanting_a_type_leaves_a_validated_ontology_patch_citing_it_and_registers_the_candidate()
 {
    let platform = reference_surprised();
    let mut research = deployed(BTreeMap::new());
    let (licensed, eligible) = deployed_bounds();
    let name = "candidate-obj-A-shares-outstanding";
    assert!(
        research
            .ontology()
            .get_latest(OntologyTypeFamily::Event, name)
            .is_none(),
        "premise: no candidate registered before the sweep"
    );

    let round = research
        .sweep(&platform, t0(), &licensed, &eligible)
        .unwrap();
    assert!(
        round.anomalies >= 1,
        "premise: the surprise was detected: {round:?}"
    );
    assert!(round.raised >= 1, "premise: a gap was raised: {round:?}");
    assert!(
        round.ontology_patches_refused.is_empty(),
        "{:?}",
        round.ontology_patches_refused
    );
    assert_eq!(
        round.ontology_patches.len(),
        1,
        "{:?}",
        round.ontology_patches
    );
    let patch = &round.ontology_patches[0];
    patch.validate().expect("the patch validates");
    assert_eq!(patch.family, OntologyTypeFamily::Event);
    assert_eq!(patch.name, name);
    assert_eq!(patch.version, 1);
    let signal_id = patch
        .cited_observations
        .iter()
        .find(|c| c.starts_with("ambient-"))
        .unwrap_or_else(|| panic!("the patch cites the signal that raised it: {patch:?}"));
    assert!(
        round.signals.iter().any(|s| s.id() == signal_id),
        "the cited signal is one this sweep emitted"
    );
    assert!(
        patch
            .cited_observations
            .iter()
            .any(|c| c == "subject:obj-A/shares_outstanding"),
        "{patch:?}"
    );
    let registered = research
        .ontology()
        .get_latest(OntologyTypeFamily::Event, name)
        .expect("the candidate is registered");
    assert_eq!(registered.version, 1);
    assert!(
        registered.description.contains(signal_id),
        "the registered type cites the gap: {registered:?}"
    );

    let again = research
        .sweep(
            &platform,
            t0().saturating_add(Duration::from_hours(1)),
            &licensed,
            &eligible,
        )
        .unwrap();
    assert_eq!(
        again.ontology_patches.len(),
        1,
        "{:?}",
        again.ontology_patches
    );
    assert_eq!(
        again.ontology_patches[0].version, 2,
        "the next version, not a collision"
    );
    assert_eq!(
        research
            .ontology()
            .get_latest(OntologyTypeFamily::Event, name)
            .map(|t| t.version),
        Some(2)
    );
}

/// RM-P9-08 at the loop: a tick campaign the sweep opens reaches the
/// curriculum as research, and is not only counted.
///
/// `qip-expansion`'s `campaigns::observations` states a sweep's campaigns as
/// the observations the curriculum raises a tick replay from, and its own
/// tests hold that statement. What they cannot see is whether the deep
/// brain's loop ever hands its campaigns over. Until this change it did not:
/// `ResearchLoop::sweep_over` counted each opened campaign on
/// `TICK_CAMPAIGNS_OPENED` and dropped it, so the counter rose, the
/// curriculum never heard, and the residual that opened the campaign stayed
/// the loss nobody could explain. Mutation: delete the
/// `observations.extend(campaigns::observations(..))` line in `sweep_over`.
/// The campaigns are still opened and counted, and no signal names the
/// detector.
#[test]
fn a_tick_campaign_the_sweep_opens_reaches_the_curriculum_as_a_signal_naming_its_detector() {
    let mut research = deployed(BTreeMap::new());
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    // Ten fills at a venue the detector has never seen, each missing the
    // twin by 35 to 45 basis points against a 25 basis point threshold, so
    // both the new-venue and the fill-prediction triggers fire.
    let round = research
        .sweep_over(
            &platform,
            &fills(40.0, 0, 0.0),
            &[],
            t0(),
            &licensed,
            &eligible,
        )
        .unwrap();

    // Premise: the sweep opened campaigns. With none there is nothing for
    // the curriculum to hear, and the property below would pass on silence.
    assert!(
        round.campaigns_opened > 0,
        "premise: the sweep opened no tick campaign"
    );

    let from_campaigns: Vec<_> = round
        .signals
        .iter()
        .filter(|signal| {
            signal.evidence_ids().first().map(String::as_str) == Some("detector:tick_campaign")
        })
        .collect();
    // The messages state the failure and format nothing read off the round:
    // the loop holds the package store, so CodeQL's cleartext-logging taint
    // covers every value a sweep returns (`83b413fc`, ADR 0117).
    assert!(
        !from_campaigns.is_empty(),
        "the sweep opened tick campaigns and no signal names the tick-campaign detector as \
         its origin"
    );
    // And it is the campaign at this venue on this instrument, not a stray:
    // the evidence carries the instrument the fills were on.
    assert!(
        from_campaigns.iter().all(|signal| signal
            .evidence_ids()
            .iter()
            .any(|id| id == "instrument:BTC-USD")),
        "a tick-campaign signal does not carry the fills' instrument"
    );
}

/// A run of the expansion engine consults the source, ontology, tool and
/// brain registries and its record names them (R12-EXPAND-1115): the loop
/// as `main.rs` builds it holds every ontology type its detectors read, and
/// each sweep's record carries what each registry held when it ran — the
/// licensed sources the desk handed it, the types, the tools and the
/// brains. Mutation: drop the `round.registries = ..` assignment in
/// `sweep_over` — every count reads zero and the first assertion fails.
#[test]
fn a_sweep_records_the_source_ontology_tool_and_brain_registries_it_consulted() {
    let platform = platform_over(Universe::new());
    let mut research = deployed(coverage("Equity", &["obj-A"]));
    for (family, name, _) in qip_deepbrain::research::SWEEP_ONTOLOGY {
        assert!(
            research.ontology().get_latest(family, name).is_some(),
            "premise: {family:?}.{name} is registered"
        );
    }
    let licensed = BTreeSet::from(["src-a".to_string(), "src-b".to_string()]);
    let round = research
        .sweep(
            &platform,
            t0(),
            &licensed,
            &BTreeSet::from(["US".to_string()]),
        )
        .unwrap();
    let held = &round.registries;
    // The five types every detector reads, plus what the composition root
    // seeds from the coverage map: the `Equity` Asset type and the
    // coverage-gap Event type.
    assert_eq!(
        held.ontology_types,
        qip_deepbrain::research::SWEEP_ONTOLOGY.len() + 2,
        "{held:?}"
    );
    assert_eq!(held.sources, 2, "{held:?}");
    assert_eq!(
        held.tools,
        qip_deepbrain::research::tool_count(&registered_tools().unwrap()),
        "{held:?}"
    );
    assert_eq!(held.brains, research.brains().len(), "{held:?}");
    assert!(held.tools > 0 && held.brains > 0, "{held:?}");
}

/// R12-EXPAND-6046: the models the learning round found drifted reach the
/// sweep as exactly one `ModelDrift` signal per model, and a sweep with no
/// new drift raises none, so a drift measured once is not re-reported for
/// ever. Before this the learning round's drifted list reached only the
/// operations timeline, and `GapTrigger::ModelDrift` had no producer.
///
/// Mutation: delete the `observations.extend(model_drift(..))` line in
/// `sweep_over`; the drift is noted and no signal names the detector.
#[test]
fn a_drifted_model_raises_exactly_one_model_drift_gap() {
    let mut research = deployed(BTreeMap::new());
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let from_drift = |round: &qip_deepbrain::research::ResearchRound| -> Vec<Vec<String>> {
        round
            .signals
            .iter()
            .filter(|s| {
                s.evidence_ids().first().map(String::as_str) == Some("detector:model_drift")
            })
            .map(|s| s.evidence_ids().to_vec())
            .collect()
    };
    // Premise: an empty platform with no drift noted raises no drift signal.
    let quiet = research
        .sweep_over(&platform, &[], &[], t0(), &licensed, &eligible)
        .unwrap();
    assert!(
        from_drift(&quiet).is_empty(),
        "premise: no drift, no signal"
    );

    // The node hands the sweep the learning round's list; one model listed
    // twice is still one model.
    research.note_drifted_models(["ridge@3".to_string(), "ridge@3".to_string()]);
    let round = research
        .sweep_over(&platform, &[], &[], t0(), &licensed, &eligible)
        .unwrap();
    let raised = from_drift(&round);
    assert_eq!(raised.len(), 1, "exactly one drift signal: {raised:?}");
    assert!(raised[0].iter().any(|e| e == "model:ridge@3"), "{raised:?}");

    let after = research
        .sweep_over(&platform, &[], &[], t0(), &licensed, &eligible)
        .unwrap();
    assert!(
        from_drift(&after).is_empty(),
        "a drift is reported once per learning round"
    );
}

/// R12-TICK-2072: drift the learning round measured is a curriculum item, not
/// only a counter. The node hands `drifted_references` to the sweep, the sweep
/// raises a `ModelDrift` gap, and `ExpansionDesk::offer` queues it as a
/// validated research item naming the model; an undrifted model queues none.
///
/// Mutation: delete the `observations.extend(model_drift(..))` line in
/// `sweep_over`; the drifted model reaches the sweep and no item names it.
#[test]
fn a_drifted_model_opens_a_curriculum_task_and_an_undrifted_one_does_not() {
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let names_model = |research: &ResearchLoop, model: &str| {
        research
            .desk()
            .queue()
            .ranked()
            .iter()
            .any(|e| e.item().research_question.contains(model))
    };

    let mut research = deployed(BTreeMap::new());
    research
        .sweep_over(&platform, &[], &[], t0(), &licensed, &eligible)
        .unwrap();
    assert!(
        !names_model(&research, "ridge@3"),
        "premise: with no drift noted no item names the model"
    );

    research.note_drifted_models(["ridge@3".to_string()]);
    let round = research
        .sweep_over(&platform, &[], &[], t0(), &licensed, &eligible)
        .unwrap();
    assert!(
        names_model(&research, "ridge@3"),
        "the drifted model opened no curriculum item: {round:?}"
    );
    assert!(
        !names_model(&research, "lasso@1"),
        "a model that did not drift must not be named"
    );
}

fn evaluation(question: &str, passing: bool) -> qip_expansion::promotion::EvaluationRecord {
    use qip_expansion::promotion::{Criterion, EvaluationRecord, Finding};
    let criteria = [
        Criterion::Calibration,
        Criterion::Robustness,
        Criterion::Leakage,
        Criterion::RegimeSensitivity,
        Criterion::Cost,
        Criterion::Latency,
        Criterion::Safety,
        Criterion::LegalEligibility,
        Criterion::IncrementalValue,
    ];
    EvaluationRecord::new(
        question,
        criteria
            .into_iter()
            .enumerate()
            .map(|(i, criterion)| {
                (
                    criterion,
                    Finding {
                        // The first criterion is the one a failing record
                        // fails, so the other eight pass either way.
                        passed: passing || i != 0,
                        evidence: format!("run-{i}"),
                    },
                )
            })
            .collect(),
    )
    .unwrap()
}

/// R12-EXPAND-6056: the sweep's ontology is extended only on a passing
/// evaluation that the new version cites. Before this `register` accepted any
/// higher version, so the schema every detector reads could be rewritten with
/// nothing on record to say why.
///
/// Mutation: drop the `evaluation.passed` refusal in
/// `OntologyRegistry::extend`; the failing evaluation versions the type.
#[test]
fn an_ontology_extension_without_an_evaluation_is_refused_and_a_cited_one_versions_the_type() {
    use qip_contracts::expansion::OntologyTypeFamily;
    let mut research = deployed(BTreeMap::new());
    let before = research
        .ontology()
        .get_latest(OntologyTypeFamily::Asset, "asset_class")
        .unwrap();
    assert_eq!(before.version, 1, "premise: the seeded type is version 1");

    let failed = research.extend_ontology(
        OntologyTypeFamily::Asset,
        "asset_class",
        "qip-deepbrain/research:asset_class@2",
        "the asset-class map, with a venue dimension",
        &evaluation("venue dimension", false),
    );
    assert!(failed.is_err(), "a failing evaluation extended the type");
    assert_eq!(
        research
            .ontology()
            .get_latest(OntologyTypeFamily::Asset, "asset_class")
            .unwrap()
            .version,
        1
    );

    let version = research
        .extend_ontology(
            OntologyTypeFamily::Asset,
            "asset_class",
            "qip-deepbrain/research:asset_class@2",
            "the asset-class map, with a venue dimension",
            &evaluation("venue dimension", true),
        )
        .unwrap();
    assert_eq!(version, 2);
    let after = research
        .ontology()
        .get_latest(OntologyTypeFamily::Asset, "asset_class")
        .unwrap();
    assert_eq!(after.version, 2);
    assert!(
        after
            .description
            .contains("[evaluation: evaluation:venue dimension]"),
        "the new version cites its evaluation: {after:?}"
    );
    assert!(
        research
            .ontology()
            .get_type(OntologyTypeFamily::Asset, "asset_class", 1)
            .is_some(),
        "version 1 is kept beside version 2"
    );
    // A type nobody registered is not extended into existence.
    assert!(
        research
            .extend_ontology(
                OntologyTypeFamily::Asset,
                "never_registered",
                "x",
                "y",
                &evaluation("other", true),
            )
            .is_err()
    );
}

/// R12-EXPAND-6056: the registry `main.rs` builds is seeded from the coverage
/// map the detector reads - an Asset type per asset class (bare or
/// `asset_class:`-prefixed keys), the coverage-gap Event type - and not from
/// the other dimensions' values.
///
/// Mutation: skip the Asset registration loop in `sweep_ontology_over` - the
/// classes are absent and the first assertion fires.
#[test]
fn the_ontology_a_node_builds_names_every_asset_class_the_coverage_map_holds() {
    use qip_contracts::expansion::OntologyTypeFamily::{Asset, Event};
    let map = BTreeMap::from([
        ("Equity".to_string(), BTreeSet::from(["a".to_string()])),
        (
            "asset_class:Crypto".to_string(),
            BTreeSet::from(["b".to_string()]),
        ),
        ("venue:XSIM".to_string(), BTreeSet::from(["a".to_string()])),
    ]);
    let ontology = qip_deepbrain::research::sweep_ontology_over(&map).unwrap();
    for class in ["Equity", "Crypto"] {
        assert!(
            ontology.get_latest(Asset, class).is_some(),
            "{class} is not an Asset type"
        );
    }
    assert!(
        ontology.get_latest(Asset, "XSIM").is_none(),
        "a venue was registered as an asset class"
    );
    assert!(
        ontology
            .get_latest(Event, qip_deepbrain::research::COVERAGE_GAP_EVENT)
            .is_some()
    );
}

/// R12-EXPAND-6056: the sweep consults the registry for the asset classes the
/// coverage detector reads. Handed a registry that does not name one, it
/// stops by name instead of raising a coverage gap about a class nobody
/// registered; handed the seeded registry it runs.
///
/// Mutation: delete the `check_coverage_registered` call in `sweep_over` - the
/// unregistered class sweeps and the refusal assertion fires.
#[test]
fn a_sweep_over_an_asset_class_the_ontology_does_not_name_is_refused_by_name() {
    let platform = platform_over(Universe::new());
    let map = coverage("Equity", &["obj-A"]);
    let bare = qip_deepbrain::research::sweep_ontology().unwrap();
    let mut research = deployed(map.clone()).with_ontology(bare).unwrap();
    let (licensed, jurisdictions) = deployed_bounds();
    let refused = research
        .sweep(&platform, t0(), &licensed, &jurisdictions)
        .expect_err("a class the ontology never named was swept");
    assert!(
        refused.message().contains("`Equity`"),
        "the refusal does not name the class: {}",
        refused.message()
    );

    let seeded = qip_deepbrain::research::sweep_ontology_over(&map).unwrap();
    let mut research = deployed(map).with_ontology(seeded).unwrap();
    research
        .sweep(&platform, t0(), &licensed, &jurisdictions)
        .expect("the seeded registry names the class and the sweep runs");
}

/// A stand-in for a hosted model that refuses every second call. No
/// provider is reached from a test: the tally wraps whatever model it is
/// given, and this is the model.
#[derive(Debug, Default)]
struct HalfRefusing {
    calls: std::sync::atomic::AtomicU64,
}

impl qip_ai::language::LanguageModel for HalfRefusing {
    fn name(&self) -> &str {
        "half-refusing"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn complete(
        &self,
        _request: &qip_ai::language::ModelRequest,
        now: Timestamp,
    ) -> qip_core::error::Result<qip_ai::language::Completion> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call.is_multiple_of(2) {
            return Err(qip_core::error::Error::unavailable("the provider refused"));
        }
        Ok(qip_ai::language::Completion {
            text: "an answer".to_string(),
            structured: None,
            model: "half-refusing".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            finish_reason: qip_ai::language::FinishReason::Complete,
            produced_at: now,
        })
    }
}

/// R12-EXPAND-6040: refused and malformed hosted-model replies are counted
/// where the adapter answers, and once enough have accumulated a rate past
/// `MODEL_ERROR_RATE` raises exactly one error-rate surprise naming the
/// model; the next sweep starts a fresh window. Before this the chain fell
/// through every refusal to the templates and nothing anywhere could tell a
/// provider refusing half its calls from one that never failed.
///
/// Mutation: count an `Err` reply as served in `CountedModel::complete`; the
/// rate reads zero and no signal is raised.
#[test]
fn a_model_whose_replies_are_refused_past_the_rate_raises_one_error_rate_gap() {
    use qip_ai::language::{DeterministicModel, FallbackChain, LanguageModel, ModelRequest};
    use qip_deepbrain::language::ReplyTally;
    use qip_deepbrain::research::{MODEL_ERROR_RATE, MODEL_REPLIES_MIN};

    let tally = ReplyTally::new();
    let chain = FallbackChain::new(vec![
        tally.wrap(Arc::new(HalfRefusing::default())),
        Arc::new(DeterministicModel::new()),
    ]);
    let mut research = deployed(BTreeMap::new()).with_reply_tally("half-refusing".into(), tally);
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let request = ModelRequest::new("narrate", "the cycle");
    let call = |n: u64| {
        for _ in 0..n {
            let _ = chain.complete(&request, t0());
        }
    };
    let error_signals = |research: &mut ResearchLoop| -> Vec<Vec<String>> {
        research
            .sweep_over(&platform, &[], &[], t0(), &licensed, &eligible)
            .unwrap()
            .signals
            .iter()
            .filter(|s| {
                s.evidence_ids().first().map(String::as_str) == Some("detector:model_error_rate")
            })
            .map(|s| s.evidence_ids().to_vec())
            .collect()
    };

    // Premise: half refused is past the stated rate, and one call short of
    // the minimum sample nothing is judged.
    const { assert!(0.5 > MODEL_ERROR_RATE) };
    call(MODEL_REPLIES_MIN - 1);
    assert!(
        error_signals(&mut research).is_empty(),
        "premise: below the sample, no judgement"
    );

    call(1);
    let raised = error_signals(&mut research);
    assert_eq!(raised.len(), 1, "exactly one error-rate signal: {raised:?}");
    assert!(
        raised[0].iter().any(|e| e == "model:half-refusing"),
        "{raised:?}"
    );

    // The window was taken: the same failures are not reported twice.
    assert!(error_signals(&mut research).is_empty());
}

/// R12-EXPAND-6010, on the binary's own sweep. A venue whose fills miss the
/// twin past the threshold raises one unexplained-P&L gap however many of
/// its fills did so, and a venue whose fills sit inside the threshold raises
/// none. The count is of queued items the residual detector derived, read
/// from each item's lineage, so a gap from any other detector over the same
/// fills (the tick campaigns, for one) is not mistaken for this one.
/// Mutation: flip `mean > threshold_bps` to `mean < threshold_bps` in
/// `qip_expansion::detectors::unexplained_residuals` — no residual gap is
/// queued for the missing venue and the count assertion fails at zero.
#[test]
fn one_fill_residual_past_its_threshold_raises_exactly_one_unexplained_pnl_gap() {
    let fill = |venue: &str, mins: i64, bps: f64| FillObservation {
        venue: venue.to_string(),
        object_id: "BTC-USD".to_string(),
        filled_at: t0().saturating_sub(Duration::from_mins(mins)),
        trade_error_bps: Some(bps),
    };
    let observed = [
        fill(VENUE, 20, 40.0),
        fill(VENUE, 10, 44.0),
        fill("XQUIET", 20, 3.0),
        fill("XQUIET", 10, -4.0),
    ];
    assert!(
        observed.iter().filter(|f| f.venue == VENUE).all(|f| f
            .trade_error_bps
            .is_some_and(|b| b.abs() > qip_deepbrain::research::RESIDUAL_BPS)),
        "premise: both fills at the venue miss past the threshold"
    );
    let platform = platform_over(Universe::new());
    let (licensed, eligible) = deployed_bounds();
    let mut research = deployed(BTreeMap::new());
    research
        .sweep_over(&platform, &observed, &[], t0(), &licensed, &eligible)
        .unwrap();

    let residual_items: Vec<String> = research
        .desk()
        .queue()
        .ranked()
        .into_iter()
        .map(|e| e.item().research_question.clone())
        .filter(|q| {
            research
                .desk()
                .lineage(q)
                .is_some_and(|l| l.derivation.rule == "detector:unexplained_residual")
        })
        .collect();
    assert_eq!(
        residual_items.len(),
        1,
        "two fills at one venue raised other than one gap: {residual_items:?}"
    );
    assert!(
        residual_items[0].contains(&format!("`{VENUE}`")),
        "the one gap does not name the missing venue: {residual_items:?}"
    );
    assert!(
        !residual_items[0].contains("`XQUIET`"),
        "the quiet venue raised the gap: {residual_items:?}"
    );
}
