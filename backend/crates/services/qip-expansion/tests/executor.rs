//! Research carried out: a started item's acquisition, research only; the
//! fill-bias campaign across historical, simulated, paper and shadow; its
//! nine-criterion evaluation; and the desk's conclusion — promoted, or its
//! null recorded (R12-EXPAND-6025, 6049, 6050, 6051).
//!
//! The failure each test prevents is the one the audit found: `conclude`
//! had no caller, so no research ever became a capability. Every test
//! starts an item through the desk exactly as the deep brain does, so a
//! campaign that cannot be concluded on a real started item cannot pass.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_agents::tools::{ToolKind, ToolName, ToolRegistry};
use qip_contracts::expansion::CapabilityVerb;
use qip_core::{Decimal, Duration, Timestamp};
use qip_expansion::curriculum::{Budget, State};
use qip_expansion::desk::{CloseReason, DeskConfig, ExpansionDesk};
use qip_expansion::detectors::{Residual, unexplained_residuals};
use qip_expansion::executor::{
    AcquisitionEffect, AcquisitionOutcome, CampaignPolicy, FillResidual, acquire,
    run_fill_bias_campaign,
};
use qip_expansion::gap::raise;
use qip_expansion::promotion::Criterion;
use std::collections::BTreeSet;

const VENUE: &str = "XSIM";

fn t0() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn desk() -> ExpansionDesk {
    let mut tools = ToolRegistry::new();
    tools
        .register(
            ToolName::parse("model-fit").unwrap(),
            ToolKind::ModelEndpoint,
        )
        .unwrap();
    ExpansionDesk::new(
        DeskConfig {
            quiet_sweeps: 3,
            max_started_sweeps: 24,
            jurisdiction: "US".into(),
            owner: "test".into(),
        },
        tools,
    )
    .unwrap()
}

/// A desk with one started fill-residual item at `VENUE`, and its question.
fn started_desk() -> (ExpansionDesk, String) {
    let mut desk = desk();
    let observation = unexplained_residuals(
        &[Residual {
            subject: VENUE.to_string(),
            error_bps: 40.0,
        }],
        25.0,
    )
    .unwrap()
    .pop()
    .expect("premise: a 40 bp miss against a 25 bp bar is a gap");
    let raised = raise(observation).unwrap();
    desk.offer(
        raised,
        &BTreeSet::new(),
        &BTreeSet::from(["US".to_string()]),
    );
    let report = desk
        .finish_sweep(Budget {
            compute: Decimal::from_int(10),
            capital: Decimal::from_int(10),
        })
        .unwrap();
    let question = report
        .started
        .first()
        .cloned()
        .expect("premise: it started");
    (desk, question)
}

/// Ten fills before `t0` at 35..45 bp, then `post` fills after it at
/// `post_error` ± 2 bp.
fn residuals(post: usize, post_error: f64) -> Vec<FillResidual> {
    let mut out: Vec<FillResidual> = (0..10)
        .map(|i| FillResidual {
            venue: VENUE.to_string(),
            at: t0().saturating_sub(Duration::from_mins(60 - i)),
            error_bps: if i % 2 == 0 { 35.0 } else { 45.0 },
        })
        .collect();
    out.extend((0..post).map(|i| FillResidual {
        venue: VENUE.to_string(),
        at: t0().saturating_add(Duration::from_mins(i as i64 + 1)),
        error_bps: post_error + if i % 2 == 0 { -2.0 } else { 2.0 },
    }));
    // A fill at another venue, which the campaign must not read.
    out.push(FillResidual {
        venue: "OTHER".to_string(),
        at: t0(),
        error_bps: -500.0,
    });
    out
}

/// A real, stable bias passes all nine criteria across the four
/// environments, and the desk converts it into a capability that is not a
/// money-moving verb. Mutation: delete the shadow stage's
/// `campaign.record(Environment::Shadow, ..)` — `is_complete` is false, the
/// first assertion fails, and `conclude` would refuse with three of four
/// environments.
#[test]
fn a_real_fill_bias_runs_four_environments_passes_nine_criteria_and_becomes_a_capability() {
    let (mut desk, question) = started_desk();
    let lineage = desk.lineage(&question).unwrap().clone();
    let policy = CampaignPolicy::default();

    let result = run_fill_bias_campaign(
        &question,
        &lineage,
        &residuals(8, 40.0),
        t0(),
        &policy,
        true,
        Decimal::from_int(4),
        Decimal::from_int(48),
    )
    .unwrap()
    .expect("premise: enough fills on both sides of the start");

    assert!(result.campaign.is_complete(), "{:?}", result.campaign);
    assert!(
        result.evaluation.passed(),
        "{:?}",
        result.evaluation.failed()
    );
    assert!((result.correction.bias_bps - 40.0).abs() < 1e-9);
    assert!(result.shadow_improvement_bps > 30.0, "{result:?}");

    let capability = desk
        .conclude(&result.campaign, &result.evaluation)
        .expect("a passing campaign on a started item is promoted");
    assert!(!matches!(
        capability.verb,
        CapabilityVerb::Trade
            | CapabilityVerb::Settle
            | CapabilityVerb::Hedge
            | CapabilityVerb::Transfer
            | CapabilityVerb::Purchase
    ));
    assert_eq!(capability.eligibility, "paper and shadow only");
    assert!(desk.queue().entry(&question).is_none());
    assert!(
        desk.closed()
            .any(|c| c.question == question && c.reason == CloseReason::Promoted)
    );
}

/// A bias that does not hold out of sample fails incremental value by name,
/// and the desk records the null and closes the item rather than paying for
/// the same question twice. Mutation: make the `IncrementalValue` finding
/// compare the paper improvement's magnitude (`.abs()`) — it passes on a
/// correction that made things worse and the first assertion fails.
#[test]
fn a_bias_that_does_not_hold_out_of_sample_records_its_null() {
    let (mut desk, question) = started_desk();
    let lineage = desk.lineage(&question).unwrap().clone();

    let result = run_fill_bias_campaign(
        &question,
        &lineage,
        &residuals(8, 0.0),
        t0(),
        &CampaignPolicy::default(),
        true,
        Decimal::from_int(4),
        Decimal::from_int(48),
    )
    .unwrap()
    .expect("premise: the campaign ran");

    let failed = result.evaluation.failed();
    assert!(failed.contains(&Criterion::IncrementalValue), "{failed:?}");
    assert!(desk.conclude(&result.campaign, &result.evaluation).is_err());
    assert!(
        desk.closed()
            .any(|c| c.question == question && c.reason == CloseReason::NullRecorded)
    );
}

/// R12-EXPAND-6061. A concluded null that names no experiment is free text
/// that stops a later repeat; the desk records the campaign's own stage
/// references, `<campaign id>/<stage>`, so the verdict can be audited.
/// Mutation: record `&[]` in `conclude`'s null arm; the registry refuses an
/// uncited null, nothing is recorded, and `held.len() == 1` fails.
#[test]
fn a_concluded_null_cites_the_campaigns_own_experiment_references() {
    let (mut desk, question) = started_desk();
    let lineage = desk.lineage(&question).unwrap().clone();
    let result = run_fill_bias_campaign(
        &question,
        &lineage,
        &residuals(8, 0.0),
        t0(),
        &CampaignPolicy::default(),
        true,
        Decimal::from_int(4),
        Decimal::from_int(48),
    )
    .unwrap()
    .expect("premise: the campaign ran");
    let refs = result.campaign.experiment_refs().unwrap();
    assert_eq!(refs.len(), 4, "premise: all four stages ran: {refs:?}");
    assert!(desk.conclude(&result.campaign, &result.evaluation).is_err());

    let held = desk.research().history(&question);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].experiments.len(), 4);
    let id = result.campaign.id().unwrap();
    assert!(
        held[0]
            .experiments
            .iter()
            .all(|r| r.starts_with(&format!("{id}/"))),
        "{:?}",
        held[0].experiments
    );
    assert!(held[0].experiments.contains(&format!("{id}/shadow")));
}

/// Research that cost more than it was authorised fails the cost criterion
/// by name. Mutation: compare against `budget * 2` — it passes and the
/// assertion fails.
#[test]
fn research_charged_beyond_its_authorised_budget_fails_the_cost_criterion() {
    let (desk, question) = started_desk();
    let lineage = desk.lineage(&question).unwrap().clone();
    let result = run_fill_bias_campaign(
        &question,
        &lineage,
        &residuals(8, 40.0),
        t0(),
        &CampaignPolicy::default(),
        true,
        Decimal::from_int(60),
        Decimal::from_int(48),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.evaluation.failed(), vec![Criterion::Cost]);
}

/// Too few fills since the start is "not yet", not a failure: the item keeps
/// running. Mutation: return a result with zero paper residuals — the
/// `is_none` assertion fails.
#[test]
fn too_few_fills_since_the_start_is_waiting_not_a_failed_campaign() {
    let (desk, question) = started_desk();
    let lineage = desk.lineage(&question).unwrap().clone();
    let waiting = run_fill_bias_campaign(
        &question,
        &lineage,
        &residuals(3, 40.0),
        t0(),
        &CampaignPolicy::default(),
        true,
        Decimal::ZERO,
        Decimal::from_int(48),
    )
    .unwrap();
    assert!(waiting.is_none());
    assert_eq!(
        *desk.queue().entry(&question).unwrap().state(),
        State::Started
    );
}

/// A fill-residual item's acquisition gathers the venue's recorded fills
/// into a digest — and only that venue's — and is research only; an item
/// whose acquisition has no executor says so rather than pretending
/// (R12-EXPAND-6049). Mutation: drop the venue filter in `for_venue` — the
/// count includes the other venue's fill and the assertion fails.
#[test]
fn an_acquisition_gathers_only_its_venue_and_one_with_no_executor_says_so() {
    let (desk, question) = started_desk();
    let lineage = desk.lineage(&question).unwrap().clone();
    let record = acquire(&question, &lineage, &residuals(8, 40.0), t0()).unwrap();
    assert_eq!(record.effect, AcquisitionEffect::ResearchOnly);
    match &record.outcome {
        AcquisitionOutcome::Gathered { records, .. } => assert_eq!(*records, 18),
        other => panic!("the fill acquisition did not gather: {other:?}"),
    }

    let mut other = lineage;
    other.derivation.evidence = vec!["detector:coverage".to_string()];
    let record = acquire(&question, &other, &residuals(8, 40.0), t0()).unwrap();
    assert!(
        matches!(record.outcome, AcquisitionOutcome::NotExecutable { .. }),
        "{record:?}"
    );
    assert_eq!(record.effect, AcquisitionEffect::ResearchOnly);
}

/// The desk starts only what the compute market granted: an eligible item
/// it did not grant is deferred with that reason, budget or no budget.
/// Mutation: ignore `allowed` in `schedule_within` — the item starts and the
/// assertion fails.
#[test]
fn an_item_the_market_did_not_grant_is_deferred_and_not_started() {
    let mut desk = desk();
    let observation = unexplained_residuals(
        &[Residual {
            subject: VENUE.to_string(),
            error_bps: 40.0,
        }],
        25.0,
    )
    .unwrap()
    .pop()
    .unwrap();
    desk.offer(
        raise(observation).unwrap(),
        &BTreeSet::new(),
        &BTreeSet::from(["US".to_string()]),
    );
    let report = desk
        .finish_sweep_within(
            Budget {
                compute: Decimal::from_int(100),
                capital: Decimal::from_int(100),
            },
            Some(&BTreeSet::new()),
        )
        .unwrap();
    assert!(report.started.is_empty(), "{report:?}");
    assert_eq!(report.deferred.len(), 1, "premise: the item was eligible");
}

/// Retiring a started item closes it as unproductive; the question leaves
/// the queue. Mutation: close it as `OutOfTime` — the reason assertion fails.
#[test]
fn a_retired_started_item_closes_as_unproductive() {
    let (mut desk, question) = started_desk();
    assert!(desk.retire(&question).unwrap(), "premise: it had started");
    assert!(desk.queue().entry(&question).is_none());
    assert!(
        desk.closed()
            .any(|c| c.question == question && c.reason == CloseReason::Unproductive)
    );
    assert!(
        desk.retire(&question).is_err(),
        "a second retirement is visible"
    );
}
