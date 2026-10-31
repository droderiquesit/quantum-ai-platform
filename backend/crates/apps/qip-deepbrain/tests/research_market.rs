//! The deep brain's research loop as the binary runs it, after lane R2-DEEP:
//! poisoning findings over the world fabric become research, the compute
//! market decides what runs and retires what stops yielding, a fill-bias
//! campaign is carried out and concluded, and the Cognitive Compiler
//! distils the promoted result into a reflex package a cell can load
//! (R12-EVID-2039, R12-AMBIENT-7202/3110..3114, R12-EXPAND-6025/6049..6051,
//! R12-REASON-7203/3115).
//!
//! Every test drives `ResearchLoop::sweep` or `sweep_over` — the same entry
//! `node::run_with` calls between cycles — and asserts its premise before
//! its property.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::reflex_package::{
    PackageSigner, PackageVerifier, ReflexInput, SignedReflexPackage,
};
use qip_core::error::{Error, Result};
use qip_core::{Context, Decimal, Duration, Timestamp};
use qip_deepbrain::research::{
    FillObservation, META_MARGINAL_VALUE, PromotionLane, ResearchConfig, ResearchLoop,
};
use qip_expansion::desk::CloseReason;
use qip_expansion::meta::{ReputationKey, Standing};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::Telemetry;
use qip_observability::metrics::labels;
use qip_risk::limits::LimitSet;
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_world_model::{EvidenceFabric, EvidenceRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const VENUE: &str = "XSIM";

fn t0() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t0(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn checked(i: usize) -> CheckedEvidence {
    let hash: String = std::iter::repeat_n(char::from(b'a' + i as u8), 64).collect();
    let record = EvidenceRecord::new(
        format!("k{i}"),
        format!("https://site{i}.example"),
        t0().saturating_sub(Duration::from_mins(5)),
        t0().saturating_sub(Duration::from_mins(4)),
        "connector:test/1.0".to_string(),
        "commercial".to_string(),
        "Acme CEO arrested".to_string(),
        0.9,
    )
    .unwrap()
    .with_claimed_at(t0().saturating_sub(Duration::from_mins(10)))
    .with_metadata(META_SUBJECT.into(), "ACME".into())
    .with_metadata(META_ATTRIBUTE.into(), "ceo_status".into())
    .with_metadata(META_VALUE.into(), "arrested".into());
    let (verdict, _) = EvidenceFabric::default()
        .admit(record.clone(), Some(&hash), t0())
        .unwrap();
    CheckedEvidence {
        record,
        document_hash: hash,
        verdict,
    }
}

/// A coordinated campaign forwarded into the world fabric is found by the
/// sweep and raised as a data gap naming its sources, so a poisoned feed
/// becomes research into replacing it. The premise is the audit's finding:
/// before the forward the same sweep finds nothing. Mutation: delete the
/// poisoning loop in `sweep_over` — `round.poisoning` is empty and the
/// queue holds no such question.
#[test]
fn a_poisoned_feed_in_the_world_fabric_becomes_a_data_gap_naming_its_sources() {
    let mut platform = platform();
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
    let quiet = research
        .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert!(quiet.poisoning.is_empty(), "premise: {quiet:?}");

    platform.absorb_evidence((0..3).map(checked).collect(), t0());
    let round = research
        .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
        .unwrap();

    assert_eq!(round.evidence_held, 3, "{round:?}");
    assert_eq!(
        round.poisoning.get("coordinated_manipulation"),
        Some(&1),
        "{round:?}"
    );
    let queued: Vec<String> = research
        .desk()
        .queue()
        .ranked()
        .iter()
        .map(|e| e.item().research_question.clone())
        .filter(|q| q.contains("coordinated_manipulation from"))
        .collect();
    assert_eq!(queued.len(), 1, "{round:?}");
    assert!(queued[0].contains("https://site0.example"), "{queued:?}");
}

/// The ADR 0110 signer's stand-in: the plumbing, not the cryptography.
struct TestKey;

impl PackageSigner for TestKey {
    fn key_id(&self) -> &str {
        "test-key"
    }
    fn sign(&self, digest: &str) -> Result<String> {
        Ok(format!("signed:{digest}"))
    }
}

impl PackageVerifier for TestKey {
    fn verify(&self, key_id: &str, digest: &str, signature: &str) -> Result<()> {
        if key_id == "test-key" && signature == format!("signed:{digest}") {
            Ok(())
        } else {
            Err(Error::denied("the signature does not verify"))
        }
    }
}

/// Ten scored fills before `t0`, then `post` after it, all ~40 bp off.
fn fills(post: usize) -> Vec<FillObservation> {
    fills_off(post, 40.0)
}

/// [`fills`] at any bias: the same spread around `bias` basis points.
fn fills_off(post: usize, bias: f64) -> Vec<FillObservation> {
    let at = |offset_mins: i64| t0().saturating_add(Duration::from_mins(offset_mins));
    let mut out: Vec<FillObservation> = (0..10)
        .map(|i| FillObservation {
            venue: VENUE.to_string(),
            object_id: "BTC-USD".to_string(),
            filled_at: at(i - 60),
            trade_error_bps: Some(if i % 2 == 0 { bias - 5.0 } else { bias + 5.0 }),
        })
        .collect();
    out.extend((0..post as i64).map(|i| FillObservation {
        venue: VENUE.to_string(),
        object_id: "BTC-USD".to_string(),
        filled_at: at(i + 1),
        trade_error_bps: Some(if i % 2 == 0 { bias - 2.0 } else { bias + 2.0 }),
    }));
    out
}

/// The whole road in the binary's loop: the twin's fill residuals raise a
/// gap, the market grants it, the desk starts it, the next sweep's fills
/// complete a four-environment campaign that passes nine criteria, the desk
/// promotes it, and the compiler publishes a signed reflex package a cell
/// loads and applies at that venue only. Mutation: delete the
/// `self.distil(..)` call in `carry_out` — the capability is promoted but no
/// package is published, and the store assertion fails.
#[test]
fn a_fill_bias_found_by_research_is_promoted_and_published_as_a_loadable_reflex_package() {
    let platform = platform();
    let store = Arc::new(MemoryBlobStore::new());
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap()
        .with_package_signer(Box::new(TestKey))
        .with_package_store(store.clone());

    let first = research
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert!(
        first.desk.started.iter().any(|q| q.contains(VENUE)),
        "premise: the residual gap was granted and started: {first:?}"
    );
    assert_eq!(first.campaigns.get("waiting"), Some(&1), "{first:?}");

    let later = t0().saturating_add(Duration::from_hours(1));
    let second = research
        .sweep_over(&platform, &fills(8), &[], later, &set(&[]), &set(&["US"]))
        .unwrap();

    assert_eq!(second.campaigns.get("promoted"), Some(&1), "{second:?}");
    assert!(
        research
            .desk()
            .closed()
            .any(|c| c.reason == CloseReason::Promoted && c.question.contains(VENUE))
    );
    assert_eq!(second.packages.get("published"), Some(&1), "{second:?}");
    let keys = store.list("").unwrap();
    assert_eq!(keys.len(), 1, "{keys:?}");
    let bytes = store.get(&keys[0]).unwrap().unwrap();
    let loaded = SignedReflexPackage::load(&bytes, &TestKey, later).unwrap();
    let at = |venue: &str| {
        BTreeMap::from([("venue".to_string(), ReflexInput::Label(venue.to_string()))])
    };
    assert_eq!(
        loaded.evaluate(&at(VENUE)).get("twin_entry_bps"),
        Some(&-40)
    );
    assert!(loaded.evaluate(&at("ELSEWHERE")).is_empty());
}

/// Without a signer the compiled package is held unsigned and nothing is
/// written: there is no unsigned path to a cell. Mutation: write the
/// unsigned package's encoding in the `(None, _)` arm — the store holds a
/// key and the assertion fails.
#[test]
fn without_a_signer_a_compiled_package_is_held_and_nothing_is_published() {
    let platform = platform();
    let store = Arc::new(MemoryBlobStore::new());
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())
        .unwrap()
        .with_package_store(store.clone());
    research
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    let round = research
        .sweep_over(
            &platform,
            &fills(8),
            &[],
            t0().saturating_add(Duration::from_hours(1)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap();
    assert_eq!(
        round.packages.get("held_unsigned"),
        Some(&1),
        "premise: a package was compiled: {round:?}"
    );
    assert!(store.list("").unwrap().is_empty());
    assert!(research.packages().all(|p| p.signed.is_none()));
}

/// ADR 0149 d6 (R12-EXPAND-6052): a promoted correction inside the reflex
/// bounds is recorded reflex and held unsigned, and one past
/// `MAX_ADJUSTMENT_BPS` is recorded research and yields no package. The
/// lane is read from the compiled form: the same campaign, at a bias the
/// compiler cannot express in one bounded adjustment, goes to research.
/// Without the record a refused compile was a count, and nothing said which
/// promoted question had been held back from the reflex lane.
///
/// Mutation: record `Reflex` on the compile-refused arm of `distil`.
#[test]
fn a_promoted_correction_inside_the_reflex_bounds_is_recorded_reflex_and_one_past_them_research() {
    let lane_after = |bias: f64| {
        let platform = platform();
        let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
        research
            .sweep_over(
                &platform,
                &fills_off(0, bias),
                &[],
                t0(),
                &set(&[]),
                &set(&["US"]),
            )
            .unwrap();
        let round = research
            .sweep_over(
                &platform,
                &fills_off(8, bias),
                &[],
                t0().saturating_add(Duration::from_hours(1)),
                &set(&[]),
                &set(&["US"]),
            )
            .unwrap();
        assert_eq!(
            round.campaigns.get("promoted"),
            Some(&1),
            "premise: the correction at {bias} bp was promoted: {round:?}"
        );
        assert_eq!(round.lanes.len(), 1, "{:?}", round.lanes);
        let (question, lane) = round.lanes.iter().next().unwrap();
        assert!(question.contains(VENUE), "{question}");
        (
            lane.clone(),
            round,
            research.packages().cloned().collect::<Vec<_>>(),
        )
    };

    let (inside, round, packages) = lane_after(40.0);
    let PromotionLane::Reflex { package_id } = &inside else {
        panic!("a 40 bp correction was not recorded reflex: {inside:?}");
    };
    assert_eq!(round.packages.get("held_unsigned"), Some(&1), "{round:?}");
    assert_eq!(packages.len(), 1);
    assert_eq!(&packages[0].compiled.package.package_id, package_id);
    assert!(packages[0].signed.is_none(), "an unsigned lane signed");

    // 600 bp is past MAX_ADJUSTMENT_BPS (500): one rule cannot carry it.
    let (past, round, packages) = lane_after(600.0);
    let PromotionLane::Research { refusal } = &past else {
        panic!("a 600 bp correction was recorded reflex: {past:?}");
    };
    assert!(refusal.contains("500"), "{refusal}");
    assert_eq!(round.packages.get("refused"), Some(&1), "{round:?}");
    assert!(
        packages.is_empty(),
        "a research-lane result yielded a package"
    );
}

/// Research whose acquisition has no executor realises nothing, and the
/// market retires it after its window rather than letting it hold a
/// started slot for twenty-four sweeps; the same question raised again is
/// declined as retired. Mutation: drop the `desk.retire` call in
/// `measure_last_round` — no sweep reports a retirement and the first
/// assertion fails.
#[test]
fn research_that_realises_nothing_is_retired_by_the_market_and_stays_retired() {
    let platform = platform();
    let coverage = BTreeMap::from([("Equity".to_string(), set(&["obj-A"]))]);
    let mut research = ResearchLoop::new(ResearchConfig::default(), coverage).unwrap();
    let first = research
        .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert_eq!(first.desk.started.len(), 1, "premise: {first:?}");

    let mut retired_at = None;
    let mut declined_after = 0;
    for sweep in 1..8 {
        let now = t0().saturating_add(Duration::from_mins(sweep));
        let round = research
            .sweep(&platform, now, &set(&[]), &set(&["US"]))
            .unwrap();
        if round.retired > 0 && retired_at.is_none() {
            retired_at = Some(sweep);
        } else if retired_at.is_some() {
            declined_after += round.declined.get("retired").copied().unwrap_or(0);
        }
    }

    let retired_at = retired_at.expect("the unproductive item was never retired");
    assert!(
        retired_at <= i64::from(research.meta().policy().retire_after) + 1,
        "retired at sweep {retired_at}"
    );
    assert!(
        research
            .desk()
            .closed()
            .any(|c| c.reason == CloseReason::Unproductive)
    );
    assert!(
        declined_after >= 1,
        "the retired question was granted again"
    );
    assert!(
        research
            .acquisitions()
            .all(|a| a.outcome.as_str() == "not_executable"),
        "premise: nothing could be carried out for a coverage gap"
    );
}

/// Reputation is kept by domain, horizon and regime (R12-AMBIENT-3111): a
/// window measured under the regime the platform classified informs that
/// cell, and leaves every other regime's cell — and the pooled
/// `research/all` cell this replaced — uninformed. Mutation: restore the
/// `"research", "all"` literals in `run_market` and `measure_last_round`
/// — the platform-regime cell is `Uninformed` and the first assertion
/// fails.
#[test]
fn a_measured_window_in_one_regime_leaves_another_regimes_cell_uninformed() {
    let platform = platform();
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
    let first = research
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    let question = first
        .desk
        .started
        .iter()
        .find(|q| q.contains(VENUE))
        .cloned()
        .unwrap_or_else(|| panic!("premise: the residual gap started: {first:?}"));
    let domain = research
        .desk()
        .queue()
        .entry(&question)
        .and_then(|e| e.origin())
        .and_then(|gap| gap.signal.affected_domains.first().cloned())
        .unwrap_or_else(|| panic!("the item names a domain"));
    let later = t0().saturating_add(Duration::from_mins(10));
    let second = research
        .sweep_over(&platform, &fills(2), &[], later, &set(&[]), &set(&["US"]))
        .unwrap();
    assert_eq!(
        second.measured, 1,
        "premise: one window measured: {second:?}"
    );

    let regime = platform.regime_context(&domain);
    assert!(
        !regime.is_empty(),
        "premise: the platform classifies a regime"
    );
    let book = research.meta().reputation();
    let measured = ReputationKey::new(&domain, "hour", &regime).unwrap();
    assert!(
        matches!(book.standing(&measured), Standing::Informed(_)),
        "the cell the window was measured in is informed: {measured:?}"
    );
    let other_regime = if regime == "crisis/extreme" {
        "quiet/normal"
    } else {
        "crisis/extreme"
    };
    let other = ReputationKey::new(&domain, "hour", other_regime).unwrap();
    assert_eq!(book.standing(&other), Standing::Uninformed);
    let pooled = ReputationKey::new(&domain, "research", "all").unwrap();
    assert_eq!(book.standing(&pooled), Standing::Uninformed);
}

/// The marginal value on the gauge is what the latest window added
/// (R12-AMBIENT-3112, 10189): a window whose two new residuals narrowed
/// the bias estimate realises something, and the next window, which gathered
/// nothing and narrowed nothing, records zero marginal value on
/// `qip_meta_marginal_value_bp` — not the average of the two. Mutation:
/// delete the `metrics.gauge(META_MARGINAL_VALUE, ..)` write in
/// `measure_last_round` — the gauge is absent and the first read fails.
#[test]
fn a_second_unit_that_realises_nothing_records_zero_marginal_value() {
    let platform = platform();
    let metrics = platform.telemetry().metrics.clone();
    // A fill-bias item is a `Model` gap, so its work kind on the gauge is `model`.
    let gauge = || {
        metrics
            .snapshot()
            .gauge(META_MARGINAL_VALUE, &labels([("kind", "model")]))
    };
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
    let first = research
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert!(
        first.desk.started.iter().any(|q| q.contains(VENUE)),
        "premise: the residual gap started: {first:?}"
    );
    assert_eq!(gauge(), None, "premise: nothing measured yet");

    let second = research
        .sweep_over(
            &platform,
            &fills(2),
            &[],
            t0().saturating_add(Duration::from_mins(10)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap();
    assert_eq!(second.measured, 1, "{second:?}");
    let narrowed = gauge().unwrap_or_else(|| panic!("the first window is on the gauge"));
    assert!(
        narrowed > 0.0,
        "premise: two new residuals narrowed the estimate: {narrowed}"
    );

    let third = research
        .sweep_over(
            &platform,
            &fills(2),
            &[],
            t0().saturating_add(Duration::from_mins(20)),
            &set(&[]),
            &set(&["US"]),
        )
        .unwrap();
    assert_eq!(third.measured, 1, "{third:?}");
    assert_eq!(
        gauge(),
        Some(0.0),
        "a window that narrowed nothing added nothing"
    );
}

/// Each research agent is bounded by its stated compute budget on the
/// deployed market (R12-AMBIENT-9034): the fill-bias item a sweep grants
/// with no agent budget is declined `over_agent_budget` once its agent —
/// the reputation cell the bid is weighted by — is stated a budget of zero,
/// and nothing starts. The budget is read from the variable `main.rs`
/// passes, so the parse is on the path too. Mutation: call `allocate` in
/// `run_market` — the zero-budget agent is granted and the test fails.
#[test]
fn a_research_agent_stated_a_zero_budget_is_declined_and_starts_nothing() {
    let platform = platform();
    let mut probe = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
    let granted = probe
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    let question = granted
        .desk
        .started
        .iter()
        .find(|q| q.contains(VENUE))
        .cloned()
        .unwrap_or_else(|| panic!("premise: unbounded, the item is granted: {granted:?}"));
    let domain = probe
        .desk()
        .queue()
        .entry(&question)
        .and_then(|e| e.origin())
        .and_then(|gap| gap.signal.affected_domains.first().cloned())
        .unwrap();
    let regime = platform.regime_context(&domain);
    let raw = format!("{domain}/hour/{regime}=0");
    let config = ResearchConfig::from_lookup(&|name: &str| {
        (name == qip_deepbrain::research::AGENT_BUDGETS_VARIABLE).then(|| raw.clone())
    })
    .unwrap();
    assert_eq!(config.agent_budgets.len(), 1, "premise: the budget parsed");
    let mut bounded = ResearchLoop::new(config, BTreeMap::new()).unwrap();
    let round = bounded
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert_eq!(
        round.declined.get("over_agent_budget"),
        Some(&1),
        "{round:?}"
    );
    assert!(round.desk.started.is_empty(), "{round:?}");
    assert!(
        qip_deepbrain::research::parse_agent_budgets("market/hour=1").is_err(),
        "a key without a regime is refused"
    );
}

/// R12-REASON-3061: a sweep's started items are compiled into a plan the
/// agency proves inside the goal's budget, deterministic across runs and
/// citing the evidence that raised each item, and the ambient journal writes
/// it beside the signals. Before this a sweep left counters: which items
/// started was visible, why this set inside this budget was not.
///
/// Mutation: delete the `round.plan = plan` assignment in
/// `ResearchLoop::plan_sweep` (or the `SweepPlan` append in the journal);
/// the round carries no plan and the journal no record.
#[test]
fn a_sweep_compiles_its_started_items_into_a_deterministic_cited_journaled_plan() {
    let run = || {
        let mut platform = platform();
        platform.absorb_evidence((0..3).map(checked).collect(), t0());
        let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
        let round = research
            .sweep(&platform, t0(), &set(&[]), &set(&["US"]))
            .unwrap();
        (round, research)
    };
    let (round, _) = run();
    assert!(
        !round.desk.started.is_empty(),
        "premise: the sweep started an item: {round:?}"
    );
    assert!(round.plan_refused.is_none(), "{round:?}");
    let plan = round
        .plan
        .clone()
        .unwrap_or_else(|| panic!("a sweep that started work compiled no plan: {round:?}"));
    assert_eq!(plan.steps.len(), round.desk.started.len(), "{plan:?}");
    assert!(plan.total_cost <= plan.budget, "{plan:?}");
    assert!(
        plan.cites.iter().any(|c| c.starts_with("poisoning:")),
        "the plan cites the evidence that raised its item: {plan:?}"
    );

    // Deterministic: the same inputs, a fresh platform and loop, the same plan.
    let (again, _) = run();
    assert_eq!(again.plan, round.plan);

    // Journaled: one SweepPlan record, decodable, equal to the round's.
    let mut journal = qip_deepbrain::ambient_journal::AmbientJournal::open(
        &qip_deepbrain::ambient_journal::JournalDestination::InMemory,
    )
    .unwrap();
    journal.record(1, &round, t0()).unwrap();
    let plans: Vec<_> = qip_deepbrain::ambient_journal::records(journal.log())
        .unwrap()
        .into_iter()
        .filter_map(|r| match r {
            qip_deepbrain::ambient_journal::AmbientRecord::SweepPlan { plan, .. } => Some(plan),
            _ => None,
        })
        .collect();
    assert_eq!(plans, vec![plan]);
}

/// R12-AMBIENT-9034: the compute market's bound and weights reach the event
/// log. Until this the sweep line carried `units_granted` and a count of
/// declines, and the agent budgets lived only in the process's
/// configuration, so neither the bound an agent ran under nor the weight its
/// grant was priced on could be read back from the record. The journaled
/// `Market` record carries the budgets the round allocated within, every
/// grant with its reputation weight, every decline with the market's reason,
/// and what was spent — and it is the same allocation the grants were carried
/// out from.
///
/// Mutation: delete the `AmbientRecord::Market` append in
/// `AmbientJournal::record` — no market record is journaled and the first
/// assertion fails. Verified and restored.
#[test]
fn a_research_round_journals_the_agent_budgets_and_model_weights_it_allocated_on() {
    use qip_deepbrain::ambient_journal::{
        AmbientJournal, AmbientRecord, JournalDestination, records,
    };

    let platform = platform();
    // Unbounded: the item is granted, and the grant carries its weight.
    let mut unbounded = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new()).unwrap();
    let round = unbounded
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    assert!(
        round.units_granted >= 1,
        "premise: the market granted: {round:?}"
    );
    let mut journal = AmbientJournal::open(&JournalDestination::InMemory).unwrap();
    journal.record(1, &round, t0()).unwrap();
    let markets: Vec<_> = records(journal.log())
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            AmbientRecord::Market { cycle, market } => Some((cycle, market)),
            _ => None,
        })
        .collect();
    assert_eq!(markets.len(), 1, "one market record per sweep that bid");
    let (cycle, market) = &markets[0];
    assert_eq!(*cycle, 1);
    assert!(
        market.agent_budgets.is_empty(),
        "no budget was stated: {market:?}"
    );
    assert_eq!(
        market.grants.iter().map(|g| g.units).sum::<u32>(),
        round.units_granted,
        "the journaled grants are the grants the round carried out"
    );
    let grant = market
        .grants
        .iter()
        .find(|g| g.work_id.contains(VENUE))
        .unwrap_or_else(|| panic!("the fill-bias item's grant is on the record: {market:?}"));
    assert!(
        grant.weight_bp > 0,
        "the grant names the weight it was priced on"
    );
    assert!(!grant.domain.is_empty() && !grant.regime.is_empty());
    assert!(grant.expected_bp > 0.0 && !grant.charged.is_empty());
    assert!(
        market.spent.values().any(|units| units.is_positive()),
        "something was spent: {market:?}"
    );

    // Bounded to zero for the item's own cell: the record carries the
    // budget line and the decline the bound caused.
    let domain = grant.domain.clone();
    let regime = grant.regime.clone();
    let raw = format!("{domain}/{}/{regime}=0", grant.horizon);
    let config = ResearchConfig::from_lookup(&|name: &str| {
        (name == qip_deepbrain::research::AGENT_BUDGETS_VARIABLE).then(|| raw.clone())
    })
    .unwrap();
    let mut bounded = ResearchLoop::new(config, BTreeMap::new()).unwrap();
    let round = bounded
        .sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))
        .unwrap();
    let mut journal = AmbientJournal::open(&JournalDestination::InMemory).unwrap();
    journal.record(1, &round, t0()).unwrap();
    let market = records(journal.log())
        .unwrap()
        .into_iter()
        .find_map(|record| match record {
            AmbientRecord::Market { market, .. } => Some(market),
            _ => None,
        })
        .expect("the bounded sweep journals its market");
    assert_eq!(market.agent_budgets.len(), 1, "{market:?}");
    assert_eq!(market.agent_budgets[0].domain, domain);
    assert_eq!(market.agent_budgets[0].units, Decimal::ZERO);
    assert!(
        market
            .declined
            .iter()
            .any(|(work, reason)| work.contains(VENUE) && reason == "over_agent_budget"),
        "the bound's decline is on the record with its reason: {market:?}"
    );
}
