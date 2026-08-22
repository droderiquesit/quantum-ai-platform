//! Tests for the lifecycle ladder.
//!
//! Nearly every test here tries to get a strategy to hold capital it has not
//! earned: skip a rung, promote itself, buy past a leakage finding with a
//! strong backtest, or come back from retirement. The two that go the other
//! way check that stopping a strategy is easy — a demotion with no approver at
//! all, and each automatic trigger firing with no human anywhere in the call.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry, ModelStage};
use qip_contracts::gate::{GateOutcome, GateStage};
use qip_contracts::governance::Approval;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ModelId, ObjectId, Timestamp, dec};
use qip_lifecycle::demotion::{DemotionMonitor, DemotionTrigger, LiveObservation, PilotBaseline};
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, KillCondition, LeakageAudit, PaperEvidence,
    PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence, StrategyEvidence,
};
use qip_lifecycle::gates::{Gate, HoldoutGate, PaperGate, PilotGate, ScaledGate, ShadowGate};
use qip_lifecycle::ledger::{AuthorisedPromotion, LifecycleLedger, attempt_promotion};
use qip_simulation_engine::validation::PurgedSplit;
use std::collections::BTreeMap;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn strategy() -> StrategyId {
    StrategyId::new("momentum-v3")
}

/// Returns with a genuine positive drift, drawn from a seeded stream so every
/// run of this suite sees the same numbers.
fn good_returns(seed: u64, n: usize, drift: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n)
        .map(|_| {
            // Sum of two uniforms, centred: a crude but deterministic
            // approximation to a bell shape, with no dependence on a
            // distribution implementation that might change.
            let u = rng.next_f64() + rng.next_f64() - 1.0;
            drift + u * 0.01
        })
        .collect()
}

fn clean_leakage_audit() -> LeakageAudit {
    LeakageAudit {
        timings: (0..8)
            .map(|i| FeatureTiming {
                feature: format!("feature-{i}"),
                known_at: start(),
                used_at: start().saturating_add(Duration::from_hours(1)),
            })
            .collect(),
        restated_without_snapshots: Vec::new(),
    }
}

/// A cross-validation run whose reported purge and embargo counts are the ones
/// a real `PurgedSplit` produces, so the holdout gate's reconstruction agrees.
fn honest_cross_validation(observations: usize) -> Result<CrossValidationRun> {
    let folds = 5;
    let label_horizon = 10;
    let embargo = 5;
    let splits = PurgedSplit::new(folds, label_horizon, embargo)?.split(observations)?;
    Ok(CrossValidationRun {
        folds,
        label_horizon,
        embargo,
        observations,
        purged: splits.iter().map(|s| s.purged).sum(),
        embargoed: splits.iter().map(|s| s.embargoed).sum(),
    })
}

fn strong_holdout() -> Result<HoldoutEvidence> {
    let observations = 400;
    Ok(HoldoutEvidence {
        holdout_returns: good_returns(1, observations, 0.0018),
        in_sample_folds: (0..5).map(|f| good_returns(10 + f, 80, 0.0020)).collect(),
        out_of_sample_folds: (0..5).map(|f| good_returns(20 + f, 80, 0.0018)).collect(),
        trials: 12,
        periods_per_year: 252.0,
        cross_validation: honest_cross_validation(observations)?,
        leakage: clean_leakage_audit(),
    })
}

fn strong_paper() -> PaperEvidence {
    PaperEvidence {
        against_live_data: true,
        assumed_cost_bps: 8.0,
        realised_cost_bps: (0..400).map(|i| 7.0 + f64::from(i % 5) * 0.2).collect(),
        peak_participation: 0.04,
        modelled_participation_limit: 0.10,
        unfillable_orders: 4,
        filled_orders: 400,
    }
}

fn strong_shadow() -> ShadowEvidence {
    ShadowEvidence {
        decisions: (0..400)
            .map(|i| ShadowDecision {
                at: start().saturating_add(Duration::from_mins(i)),
                object_id: ObjectId::from_string(format!("obj-{}", i % 20)),
                live: SignalKind::Enter,
                predicted: SignalKind::Enter,
                live_quantity: dec!("100"),
                predicted_quantity: dec!("100"),
            })
            .collect(),
        orders_reached_a_venue: false,
        decision_latency_p99: Duration::from_millis(40),
    }
}

fn dual_approval(subject: &str, at: Timestamp, rationale: &str) -> Result<Approval> {
    Approval::new(subject, "alice.chen", at, rationale)?.countersigned_by("bram.oduya")
}

fn envelope(now: Timestamp, gross: Decimal) -> Result<CapitalEnvelope> {
    CapitalEnvelope::new(
        strategy(),
        "cell-lon-1",
        gross,
        gross,
        gross,
        vec![VenueId::new("XNYS")],
        now,
        now.saturating_add(Duration::from_days(14)),
        "alice.chen",
        "signature-placeholder",
    )
}

fn strong_pilot(now: Timestamp) -> Result<PilotEvidence> {
    Ok(PilotEvidence {
        approval: Some(dual_approval(
            "momentum-v3 pilot",
            now,
            "shadow agreement held at 99.8% over 400 decisions",
        )?),
        envelope: Some(envelope(now, dec!("250000"))?),
        kill_conditions: vec![
            KillCondition::RealisedLoss(dec!("25000")),
            KillCondition::Drawdown(0.08),
            KillCondition::ConsecutiveLosingDays(5),
        ],
    })
}

fn strong_scaled(pilot_start: Timestamp, now: Timestamp) -> Result<ScaledEvidence> {
    Ok(ScaledEvidence {
        pilot_returns: good_returns(99, 120, 0.0030),
        pilot_started_at: pilot_start,
        pilot_utilisation: Utilisation {
            gross_committed: dec!("180000"),
            realised_loss: dec!("0"),
            orders_sent: 5_400,
        },
        proposed_notional: dec!("1000000"),
        modelled_capacity: dec!("4000000"),
        pilot_approval: Some(dual_approval(
            "momentum-v3 pilot",
            pilot_start,
            "shadow agreement held at 99.8% over 400 decisions",
        )?),
        scaling_approval: Some(dual_approval(
            "momentum-v3 scaling",
            now,
            "ninety days at pilot returned a 0.9 Sharpe inside a quarter of capacity",
        )?),
    })
}

fn full_evidence(pilot_start: Timestamp, now: Timestamp) -> Result<StrategyEvidence> {
    Ok(StrategyEvidence::new()
        .with_holdout(strong_holdout()?)
        .with_paper(strong_paper())
        .with_shadow(strong_shadow())
        .with_pilot(strong_pilot(pilot_start)?)
        .with_scaled(strong_scaled(pilot_start, now)?))
}

/// Walk a strategy from candidate to scaled with evidence that passes.
fn walk_to_scaled(ledger: &mut LifecycleLedger) -> Result<Timestamp> {
    let pilot_start = start();
    let scaled_at = pilot_start.saturating_add(Duration::from_days(120));
    let evidence = full_evidence(pilot_start, scaled_at)?;

    for (target, at) in [
        (GateStage::Holdout, pilot_start),
        (GateStage::Paper, pilot_start),
        (GateStage::Shadow, pilot_start),
        (GateStage::Pilot, pilot_start),
        (GateStage::Scaled, scaled_at),
    ] {
        let approval = if target.requires_human_approval() {
            Some(dual_approval(
                "momentum-v3",
                at,
                "every gate check passed with evidence attached",
            )?)
        } else {
            None
        };
        attempt_promotion(
            ledger,
            &strategy(),
            &evidence,
            approval,
            format!("promoting to {}", target.as_str()),
            at,
        )?;
    }
    Ok(scaled_at)
}

#[test]
fn every_path_from_candidate_to_scaled_passes_through_shadow() -> Result<()> {
    let mut ledger = LifecycleLedger::new();
    walk_to_scaled(&mut ledger)?;

    let path = ledger.path(&strategy());
    assert_eq!(
        path,
        vec![
            GateStage::Candidate,
            GateStage::Holdout,
            GateStage::Paper,
            GateStage::Shadow,
            GateStage::Pilot,
            GateStage::Scaled,
        ],
        "the ladder admits exactly one route"
    );
    assert!(ledger.reached(&strategy(), GateStage::Shadow));

    // Exhaustively: from any rung, the only reachable next rung is the one
    // `next()` names, so no sequence of promotions can route around shadow.
    for stage in GateStage::all() {
        if let Some(next) = stage.next() {
            let promotion = AuthorisedPromotion::advance(
                stage,
                Some(dual_approval("s", start(), "a stated reason for the record")?),
                start(),
            )?;
            assert_eq!(promotion.to(), next);
        }
    }
    Ok(())
}

#[test]
fn a_promotion_that_skips_a_rung_cannot_be_constructed() -> Result<()> {
    // There is no constructor that takes a target, so the only way to try is
    // to promote from a rung the strategy is not standing on. The ledger
    // refuses that, and refuses it by name.
    let mut ledger = LifecycleLedger::new();
    let evidence = full_evidence(start(), start())?;
    let promotion = AuthorisedPromotion::advance(GateStage::Paper, None, start())?;
    assert_eq!(promotion.to(), GateStage::Shadow);
    let outcome = ShadowGate::default().evaluate(&evidence, start());
    let error = ledger
        .record_promotion(&strategy(), promotion, outcome, "jumping to shadow")
        .expect_err("a candidate cannot enter shadow");
    assert!(error.message().contains("candidate"), "{error:?}");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Candidate);
    Ok(())
}

#[test]
fn promotion_to_a_capital_holding_stage_without_a_dual_approval_is_refused() -> Result<()> {
    for stage in [GateStage::Shadow, GateStage::Pilot] {
        let target = stage.next().unwrap_or(GateStage::Retired);
        if !target.requires_human_approval() {
            continue;
        }

        // No approval at all.
        let error = AuthorisedPromotion::advance(stage, None, start())
            .expect_err("a capital-holding rung needs an approval");
        assert!(error.message().contains("human approval"), "{error:?}");

        // A single approver is not two approvers.
        let single = Approval::new(
            "momentum-v3",
            "alice.chen",
            start(),
            "I have reviewed the shadow results myself",
        )?;
        let error = AuthorisedPromotion::advance(stage, Some(single), start())
            .expect_err("one approver is not dual control");
        assert!(error.message().contains("two approvers"), "{error:?}");

        // And a countersignature by the same person is refused upstream, in
        // the contract, before it can reach a gate at all.
        let self_countersigned = Approval::new(
            "momentum-v3",
            "alice.chen",
            start(),
            "I have reviewed the shadow results myself",
        )?
        .countersigned_by("alice.chen");
        assert!(self_countersigned.is_err(), "self-approval must be refused");
    }
    Ok(())
}

#[test]
fn a_strategy_failing_one_check_is_not_promoted_however_strong_the_rest() -> Result<()> {
    // The holdout evidence is excellent in every respect except that eight
    // features were audited and one of them was known only after it was used.
    let mut holdout = strong_holdout()?;
    holdout.leakage.timings.push(FeatureTiming {
        feature: "next-day-close".to_string(),
        known_at: start().saturating_add(Duration::from_days(1)),
        used_at: start(),
    });
    let evidence = StrategyEvidence::new().with_holdout(holdout);

    let outcome = HoldoutGate::default().evaluate(&evidence, start());
    assert!(!outcome.passed, "one leaking feature fails the gate");

    let passing = outcome.findings.iter().filter(|(_, ok, _)| *ok).count();
    assert!(
        passing >= 4,
        "the rest of the evidence passed and still did not carry the gate: {:?}",
        outcome.findings
    );
    assert_eq!(outcome.failures().len(), 1, "exactly one check failed");

    let mut ledger = LifecycleLedger::new();
    let error = attempt_promotion(
        &mut ledger,
        &strategy(),
        &evidence,
        None,
        "the Sharpe is excellent",
        start(),
    )
    .expect_err("a failed check blocks the promotion");
    assert!(error.message().contains("no_leakage"), "{error:?}");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Candidate);
    Ok(())
}

#[test]
fn a_leakage_audit_that_examined_nothing_is_not_a_clean_audit() {
    let absent = LeakageAudit::default();
    assert!(absent.findings().is_empty(), "nothing was found");
    assert!(
        !absent.is_clean(),
        "an audit with no findings because it looked at nothing must not read as clean"
    );
}

#[test]
fn a_cross_validation_run_that_did_not_purge_is_caught_by_reconstruction() -> Result<()> {
    let mut holdout = strong_holdout()?;
    // Same folds, same horizon — but the run reports having dropped nothing,
    // which is what plain k-fold on a time series looks like.
    holdout.cross_validation.purged = 0;
    holdout.cross_validation.embargoed = 0;
    let evidence = StrategyEvidence::new().with_holdout(holdout);

    let outcome = HoldoutGate::default().evaluate(&evidence, start());
    assert!(!outcome.passed);
    assert!(
        outcome
            .failures()
            .iter()
            .any(|(name, _, _)| name == "purging_and_embargo_applied"),
        "{:?}",
        outcome.findings
    );
    Ok(())
}

#[test]
fn a_sub_threshold_deflated_sharpe_is_read_as_a_failure_rather_than_a_score() -> Result<()> {
    // Thousands of trials push the expected maximum above what this series
    // shows. Below the selection threshold the deflated probability rises with
    // uncertainty instead of falling, so the gate must not quote it as a
    // confidence.
    let mut holdout = strong_holdout()?;
    holdout.holdout_returns = good_returns(2, 400, 0.0004);
    holdout.trials = 5_000;
    let evidence = StrategyEvidence::new().with_holdout(holdout);

    let outcome = HoldoutGate::default().evaluate(&evidence, start());
    let credible = outcome
        .findings
        .iter()
        .find(|(name, _, _)| name == "deflated_sharpe_credible")
        .ok_or_else(|| qip_core::error::Error::not_found("credibility finding"))?;
    assert!(!credible.1, "a sub-threshold result cannot pass on probability");
    assert!(
        credible.2.contains("not a confidence"),
        "the detail must say why the probability is not read: {}",
        credible.2
    );
    assert!(!outcome.passed);
    Ok(())
}

#[test]
fn a_paper_run_that_pays_more_than_the_backtest_assumed_is_refused() {
    let mut paper = strong_paper();
    // Every fill costs twice what the strategy was priced on.
    paper.realised_cost_bps = paper.realised_cost_bps.iter().map(|c| c * 2.4).collect();
    let evidence = StrategyEvidence::new().with_paper(paper);

    let outcome = PaperGate::default().evaluate(&evidence, start());
    assert!(!outcome.passed);
    assert!(
        outcome
            .failures()
            .iter()
            .any(|(name, _, _)| name == "realised_cost_matches_assumption"),
        "{:?}",
        outcome.findings
    );
}

#[test]
fn a_shadow_run_whose_orders_reached_a_venue_is_not_a_shadow_run() {
    let mut shadow = strong_shadow();
    shadow.orders_reached_a_venue = true;
    let evidence = StrategyEvidence::new().with_shadow(shadow);

    let outcome = ShadowGate::default().evaluate(&evidence, start());
    assert!(!outcome.passed);
    assert!(
        outcome
            .failures()
            .iter()
            .any(|(name, _, _)| name == "orders_were_discarded"),
        "{:?}",
        outcome.findings
    );
}

#[test]
fn shadow_decisions_diverging_from_the_backtest_block_the_rung() {
    let mut shadow = strong_shadow();
    // One decision in twenty computed the opposite side live.
    for (index, decision) in shadow.decisions.iter_mut().enumerate() {
        if index % 20 == 0 {
            decision.live = SignalKind::Exit;
        }
    }
    assert!(shadow.agreement_rate() < 0.98);
    let evidence = StrategyEvidence::new().with_shadow(shadow);

    let outcome = ShadowGate::default().evaluate(&evidence, start());
    assert!(
        outcome
            .failures()
            .iter()
            .any(|(name, _, _)| name == "live_path_agrees_with_backtest"),
        "divergence between the live and research paths invalidates everything upstream: {:?}",
        outcome.findings
    );
}

#[test]
fn a_pilot_without_kill_conditions_or_a_bound_does_not_pass_its_gate() -> Result<()> {
    let bare = PilotEvidence {
        approval: Some(dual_approval(
            "momentum-v3",
            start(),
            "the shadow numbers looked convincing enough",
        )?),
        envelope: None,
        kill_conditions: Vec::new(),
    };
    let outcome = PilotGate::default().evaluate(
        &StrategyEvidence::new().with_pilot(bare),
        start(),
    );
    assert!(!outcome.passed);
    let failed: Vec<&str> = outcome.failures().iter().map(|(n, _, _)| n.as_str()).collect();
    assert!(failed.contains(&"bounded_capital_envelope"), "{failed:?}");
    assert!(failed.contains(&"kill_conditions_stated"), "{failed:?}");
    Ok(())
}

#[test]
fn scaling_on_the_pilots_own_approval_is_refused_because_scaling_is_a_new_decision() -> Result<()> {
    let pilot_start = start();
    let now = pilot_start.saturating_add(Duration::from_days(120));
    let mut scaled = strong_scaled(pilot_start, now)?;
    // Reuse the pilot's approval verbatim: nobody looked at the pilot results.
    scaled.scaling_approval = scaled.pilot_approval.clone();

    let outcome =
        ScaledGate::default().evaluate(&StrategyEvidence::new().with_scaled(scaled), now);
    assert!(!outcome.passed);
    assert!(
        outcome
            .failures()
            .iter()
            .any(|(name, _, _)| name == "scaling_decided_separately"),
        "{:?}",
        outcome.findings
    );
    Ok(())
}

#[test]
fn scaling_beyond_modelled_capacity_is_refused() -> Result<()> {
    let pilot_start = start();
    let now = pilot_start.saturating_add(Duration::from_days(120));
    let mut scaled = strong_scaled(pilot_start, now)?;
    scaled.proposed_notional = scaled.modelled_capacity;

    let outcome =
        ScaledGate::default().evaluate(&StrategyEvidence::new().with_scaled(scaled), now);
    assert!(
        outcome
            .failures()
            .iter()
            .any(|(name, _, _)| name == "capacity_headroom"),
        "{:?}",
        outcome.findings
    );
    Ok(())
}

#[test]
fn a_demotion_succeeds_with_no_approver_at_all() -> Result<()> {
    let mut ledger = LifecycleLedger::new();
    walk_to_scaled(&mut ledger)?;
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Scaled);

    // No approval, no credential, no evidence — not even an attributed caller.
    let demotion = ledger.demote(&strategy(), GateStage::Shadow, "", "looked wrong", start())?;
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Shadow);
    assert!(
        demotion.approver.is_none(),
        "a demotion records no approver, because it needed none"
    );
    assert!(!demotion.is_escalation());
    assert!(demotion.rationale.contains("unattributed"));
    Ok(())
}

#[test]
fn demotion_can_reach_any_lower_rung_from_anywhere() -> Result<()> {
    for target in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
        GateStage::Retired,
    ] {
        let mut ledger = LifecycleLedger::new();
        walk_to_scaled(&mut ledger)?;
        ledger.demote(&strategy(), target, "risk-monitor", "a stated reason", start())?;
        assert_eq!(ledger.stage_of(&strategy()), target);
    }
    Ok(())
}

#[test]
fn retirement_is_terminal_and_a_retired_strategy_must_be_re_proposed_as_a_new_candidate()
-> Result<()> {
    let mut ledger = LifecycleLedger::new();
    walk_to_scaled(&mut ledger)?;
    ledger.retire(&strategy(), "portfolio-committee", "the edge is gone", start())?;
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Retired);

    // Nothing promotes out of retirement: the ladder has no rung above it…
    assert!(GateStage::Retired.next().is_none());
    let error = AuthorisedPromotion::advance(GateStage::Retired, None, start())
        .expect_err("retired is terminal");
    assert!(error.message().contains("terminal"), "{error:?}");

    // …and the ledger refuses even a correctly-formed promotion for a retired
    // strategy, so a stale `from` cannot resurrect it.
    let evidence = full_evidence(start(), start())?;
    let promotion = AuthorisedPromotion::advance(GateStage::Candidate, None, start())?;
    let outcome = HoldoutGate::default().evaluate(&evidence, start());
    let error = ledger
        .record_promotion(&strategy(), promotion, outcome, "trying again")
        .expect_err("a retired strategy cannot be walked back up");
    assert!(error.message().contains("re-proposed"), "{error:?}");

    // A new identity starts from scratch, with none of the old evidence.
    let reborn = StrategyId::new("momentum-v4");
    assert_eq!(ledger.stage_of(&reborn), GateStage::Candidate);
    assert!(ledger.path(&reborn) == vec![GateStage::Candidate]);
    assert!(ledger.history(&reborn).is_empty());
    Ok(())
}

#[test]
fn the_ledger_reconstructs_the_full_path_with_its_evidence_and_approvers() -> Result<()> {
    let mut ledger = LifecycleLedger::new();
    walk_to_scaled(&mut ledger)?;
    ledger.demote(
        &strategy(),
        GateStage::Pilot,
        "decay-monitor",
        "live Sharpe fell below a third of the pilot",
        start().saturating_add(Duration::from_days(200)),
    )?;

    let history = ledger.history(&strategy());
    assert_eq!(history.len(), 6, "five promotions and one demotion");

    // Every escalation carries a gate outcome, and the capital-holding ones
    // carry two names.
    for entry in history.iter().filter(|e| e.is_escalation()) {
        let outcome = entry
            .outcome
            .as_ref()
            .ok_or_else(|| qip_core::error::Error::not_found("gate outcome"))?;
        assert!(outcome.passed);
        assert!(!outcome.findings.is_empty());
        if entry.promotion.to.requires_human_approval() {
            let approval = entry
                .approval
                .as_ref()
                .ok_or_else(|| qip_core::error::Error::not_found("approval"))?;
            assert!(approval.is_dual());
            assert!(entry.promotion.approver.is_some());
        }
    }

    // The demotion carries neither.
    let last = history.last().ok_or_else(|| qip_core::error::Error::not_found("entry"))?;
    assert!(last.outcome.is_none());
    assert!(last.approval.is_none());
    assert!(last.promotion.approver.is_none());

    // And the admission evidence for a rung is retrievable by rung.
    let holdout = ledger
        .admission_evidence(&strategy(), GateStage::Holdout)
        .ok_or_else(|| qip_core::error::Error::not_found("holdout admission"))?;
    assert_eq!(holdout.stage, GateStage::Holdout);
    assert!(
        holdout
            .findings
            .iter()
            .any(|(name, _, _)| name == "deflated_sharpe_above_selection")
    );

    assert_eq!(ledger.narrate(&strategy()).len(), 6);
    Ok(())
}

/// A strategy at pilot, its baseline, and a ledger that agrees.
fn pilot_fixture() -> Result<(LifecycleLedger, PilotBaseline)> {
    let mut ledger = LifecycleLedger::new();
    let pilot_start = start();
    let evidence = full_evidence(pilot_start, pilot_start)?;
    for target in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
    ] {
        let approval = if target.requires_human_approval() {
            Some(dual_approval("momentum-v3", pilot_start, "the gate checks all passed")?)
        } else {
            None
        };
        attempt_promotion(
            &mut ledger,
            &strategy(),
            &evidence,
            approval,
            "walking up",
            pilot_start,
        )?;
    }
    let baseline = PilotBaseline {
        strategy: strategy(),
        established_at: pilot_start,
        returns: good_returns(7, 120, 0.0015),
        kill_conditions: vec![
            KillCondition::RealisedLoss(dec!("25000")),
            KillCondition::Drawdown(0.08),
            KillCondition::ConsecutiveLosingDays(5),
            KillCondition::CostOverrun {
                modelled_bps: 8.0,
                tolerance_bps: 3.0,
            },
        ],
        model_reference: Some("momentum-ranker@2.1".to_string()),
    };
    Ok((ledger, baseline))
}

fn healthy_observation(at: Timestamp) -> LiveObservation {
    LiveObservation {
        strategy: strategy(),
        at,
        returns: good_returns(8, 60, 0.0015),
        realised_loss: dec!("0"),
        peak_to_trough_drawdown: 0.01,
        consecutive_losing_days: 1,
        realised_cost_bps: 8.2,
        envelope: None,
    }
}

fn healthy_registry(now: Timestamp) -> ModelRegistry {
    let mut registry = ModelRegistry::new();
    let mut card = ModelCard::new(
        ModelId::from_string("mdl-momentum-ranker"),
        "momentum-ranker",
        "2.1",
        "quant-research",
        now,
    );
    card.stage = ModelStage::Production;
    card.evaluations.push(EvaluationRecord {
        evaluated_at: now,
        dataset: "holdout-2024".to_string(),
        metrics: BTreeMap::new(),
        passed: true,
    });
    registry.register(card);
    registry
}

#[test]
fn a_healthy_strategy_trips_no_automatic_trigger() -> Result<()> {
    let (ledger, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(30));
    let triggers = DemotionMonitor::default().triggers(
        &baseline,
        &healthy_observation(now),
        Some(&healthy_registry(now)),
        now,
    );
    assert!(triggers.is_empty(), "{triggers:?}");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Pilot);
    Ok(())
}

#[test]
fn performance_decay_against_the_pilot_baseline_demotes_without_a_human() -> Result<()> {
    let (mut ledger, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(60));
    let mut observation = healthy_observation(now);
    // Live returns with the drift gone: same volatility, no edge.
    observation.returns = good_returns(11, 60, -0.0002);

    let (triggers, demotion) = DemotionMonitor::default().enforce(
        &mut ledger,
        &baseline,
        &observation,
        Some(&healthy_registry(now)),
        now,
    )?;
    assert!(
        triggers
            .iter()
            .any(|t| matches!(t, DemotionTrigger::PerformanceDecay { .. })),
        "{triggers:?}"
    );
    let demotion = demotion.ok_or_else(|| qip_core::error::Error::not_found("demotion"))?;
    assert!(demotion.approver.is_none(), "no human was in the loop");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Shadow);
    Ok(())
}

#[test]
fn regime_drift_demotes_a_strategy_out_of_capital() -> Result<()> {
    let (mut ledger, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(60));
    let mut observation = healthy_observation(now);
    // Same edge per unit of risk, four times the realised volatility: the
    // market the strategy was sized in is gone.
    observation.returns = good_returns(8, 60, 0.0015).iter().map(|r| r * 4.0).collect();

    let (triggers, demotion) = DemotionMonitor::default().enforce(
        &mut ledger,
        &baseline,
        &observation,
        Some(&healthy_registry(now)),
        now,
    )?;
    assert!(
        triggers
            .iter()
            .any(|t| matches!(t, DemotionTrigger::RegimeDrift { .. })),
        "{triggers:?}"
    );
    assert!(demotion.is_some());
    assert!(!ledger.stage_of(&strategy()).holds_capital());
    Ok(())
}

#[test]
fn each_stated_kill_condition_fires_on_its_own_breach() -> Result<()> {
    let (_, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(10));
    let monitor = DemotionMonitor::default();

    type Breach = Box<dyn Fn(&mut LiveObservation)>;
    let breaches: Vec<(&str, Breach)> = vec![
        (
            "realised loss",
            Box::new(|o: &mut LiveObservation| o.realised_loss = dec!("30000")),
        ),
        (
            "drawdown",
            Box::new(|o: &mut LiveObservation| o.peak_to_trough_drawdown = 0.11),
        ),
        (
            "consecutive losing sessions",
            Box::new(|o: &mut LiveObservation| o.consecutive_losing_days = 6),
        ),
        (
            "cost overrun",
            Box::new(|o: &mut LiveObservation| o.realised_cost_bps = 14.0),
        ),
    ];

    for (label, breach) in breaches {
        let mut observation = healthy_observation(now);
        breach(&mut observation);
        let triggers =
            monitor.triggers(&baseline, &observation, Some(&healthy_registry(now)), now);
        assert!(
            triggers
                .iter()
                .any(|t| matches!(t, DemotionTrigger::KillConditionBreached { .. })),
            "{label} did not fire: {triggers:?}"
        );
    }
    Ok(())
}

#[test]
fn an_overdue_model_review_demotes_the_strategy_it_drives() -> Result<()> {
    let (mut ledger, baseline) = pilot_fixture()?;
    // A year later the model's evaluation is past its ninety-day validity, so
    // `require_for_decision` refuses it — and a strategy driven by a model
    // that may not drive a decision may not hold capital either.
    let now = start().saturating_add(Duration::from_days(400));
    let registry = healthy_registry(start());

    let (triggers, demotion) = DemotionMonitor::default().enforce(
        &mut ledger,
        &baseline,
        &healthy_observation(now),
        Some(&registry),
        now,
    )?;
    assert!(
        triggers
            .iter()
            .any(|t| matches!(t, DemotionTrigger::ModelReviewOverdue { .. })),
        "{triggers:?}"
    );
    assert!(demotion.is_some());
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Shadow);
    Ok(())
}

#[test]
fn an_expired_capital_envelope_demotes_the_strategy_trading_under_it() -> Result<()> {
    let (mut ledger, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(30));
    let mut observation = healthy_observation(now);
    observation.envelope = Some(envelope(start(), dec!("250000"))?);
    // The envelope was granted for fourteen days; thirty have passed.
    assert!(
        observation
            .envelope
            .as_ref()
            .is_some_and(|e| !e.is_live(now))
    );

    let (triggers, demotion) = DemotionMonitor::default().enforce(
        &mut ledger,
        &baseline,
        &observation,
        Some(&healthy_registry(now)),
        now,
    )?;
    assert!(
        triggers
            .iter()
            .any(|t| matches!(t, DemotionTrigger::CapitalEnvelopeExpired { .. })),
        "{triggers:?}"
    );
    assert!(demotion.is_some());
    assert!(!ledger.stage_of(&strategy()).holds_capital());
    Ok(())
}

#[test]
fn every_automatic_trigger_lands_a_strategy_somewhere_that_holds_no_capital() {
    let triggers = [
        DemotionTrigger::RegimeDrift {
            baseline_volatility: 0.01,
            live_volatility: 0.05,
            shift: 1.6,
        },
        DemotionTrigger::KillConditionBreached {
            condition: "drawdown reaches 8.0%".to_string(),
            detail: "drawdown 11.00% reached the 8.00% kill condition".to_string(),
        },
        DemotionTrigger::ModelReviewOverdue {
            model: "momentum-ranker@2.1".to_string(),
            reason: "the last evaluation is beyond its validity window".to_string(),
        },
        DemotionTrigger::CapitalEnvelopeExpired {
            expired_at: start(),
        },
    ];
    for trigger in triggers {
        for from in [GateStage::Pilot, GateStage::Scaled] {
            assert!(
                !trigger.demote_to(from).holds_capital(),
                "{} left {} holding capital",
                trigger.name(),
                from.as_str()
            );
        }
    }

    // Decay is the one that steps rather than drops, and from pilot the step
    // still lands outside capital.
    let decay = DemotionTrigger::PerformanceDecay {
        baseline_sharpe: 1.2,
        live_sharpe: 0.1,
        retained: 0.08,
    };
    assert_eq!(decay.demote_to(GateStage::Scaled), GateStage::Pilot);
    assert!(!decay.demote_to(GateStage::Pilot).holds_capital());
}

#[test]
fn every_gate_reports_each_check_it_ran_so_a_reviewer_can_see_the_whole_test() -> Result<()> {
    let evidence = full_evidence(start(), start().saturating_add(Duration::from_days(120)))?;
    for gate in [
        Box::new(HoldoutGate::default()) as Box<dyn Gate>,
        Box::new(PaperGate::default()),
        Box::new(ShadowGate::default()),
        Box::new(PilotGate::default()),
    ] {
        let outcome: GateOutcome = gate.evaluate(&evidence, start());
        assert_eq!(outcome.stage, gate.stage());
        assert!(
            outcome.findings.len() >= 3,
            "{:?} reported too little to review: {:?}",
            gate.stage(),
            outcome.findings
        );
        assert!(outcome.passed, "{:?}: {:?}", gate.stage(), outcome.failures());
    }
    Ok(())
}
