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
use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ModelId, ObjectId, Timestamp, dec};
use qip_lifecycle::band::BandMethod;
use qip_lifecycle::demotion::{DemotionMonitor, DemotionTrigger, LiveObservation, PilotBaseline};
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, KillCondition, LeakageAudit, PaperEvidence,
    PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence, StrategyEvidence,
};
use qip_lifecycle::gates::{
    Admission, Gate, HoldoutGate, PaperGate, PilotGate, ScaledGate, ShadowGate,
};
use qip_lifecycle::ledger::{AuthorisedPromotion, LifecycleLedger, attempt_promotion};
use qip_lifecycle::scoring::{annualised_sharpe, periodic_sharpe};
use qip_lifecycle::trials::{JOURNAL_PREFIX, StrategyFamily, TrialBook};
use qip_observability::metrics::{Metrics, labels, names};
use qip_simulation_engine::validation::{PurgedSplit, assess_overfitting, deflated_sharpe};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn strategy() -> StrategyId {
    StrategyId::new("momentum-v3")
}

fn family() -> Result<StrategyFamily> {
    StrategyFamily::new("momentum")
}

/// A trial book that knows the test strategy's family, with nothing charged.
fn opened_book() -> Result<TrialBook> {
    let mut book = TrialBook::in_memory();
    book.open_family(&family()?, start())?;
    book.enrol(&strategy(), &family()?, start())?;
    Ok(book)
}

/// A ledger whose holdout promotions can be charged. `LifecycleLedger::new()`
/// alone refuses them, by design: without a book the lifetime trial count is
/// unknown.
fn ledger() -> Result<LifecycleLedger> {
    Ok(LifecycleLedger::new().with_trial_book(opened_book()?))
}

/// Evidence charged to a fresh family, for handing to a gate directly. The
/// ordinary path charges through `attempt_promotion`; a gate evaluated on its
/// own needs the account the ledger would have attached.
fn charged(evidence: StrategyEvidence) -> Result<StrategyEvidence> {
    let trials = evidence.holdout.as_ref().map_or(1, |h| h.trials);
    let account = opened_book()?.charge(&strategy(), trials, start())?;
    Ok(evidence.with_trial_account(account))
}

/// The smallest store the port admits, so the book's durability can be
/// exercised without depending on an adapter crate.
#[derive(Debug, Default)]
struct MemoryStore(Mutex<BTreeMap<String, serde_json::Value>>);

impl MemoryStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, serde_json::Value>>> {
        self.0
            .lock()
            .map_err(|_| Error::io("the test store's lock is poisoned"))
    }
}

impl KeyValueStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self.lock()?.get(key).cloned())
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.lock()?.insert(key.to_string(), value);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<bool> {
        Ok(self.lock()?.remove(key).is_some())
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .lock()?
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn len(&self) -> Result<usize> {
        Ok(self.lock()?.len())
    }
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
    let mut ledger = ledger()?;
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
                Some(dual_approval(
                    "s",
                    start(),
                    "a stated reason for the record",
                )?),
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
    let admission = Admission {
        outcome,
        band: None,
    };
    let error = ledger
        .record_promotion(&strategy(), promotion, admission, "jumping to shadow")
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

    let outcome = HoldoutGate::default().evaluate(&charged(evidence.clone())?, start());
    assert!(!outcome.passed, "one leaking feature fails the gate");

    let passing = outcome.findings.iter().filter(|(_, ok, _)| *ok).count();
    assert!(
        passing >= 4,
        "the rest of the evidence passed and still did not carry the gate: {:?}",
        outcome.findings
    );
    assert_eq!(outcome.failures().len(), 1, "exactly one check failed");

    let mut ledger = ledger()?;
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
    let evidence = charged(StrategyEvidence::new().with_holdout(holdout))?;

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
    let evidence = charged(StrategyEvidence::new().with_holdout(holdout))?;

    let outcome = HoldoutGate::default().evaluate(&evidence, start());
    let credible = outcome
        .findings
        .iter()
        .find(|(name, _, _)| name == "deflated_sharpe_credible")
        .ok_or_else(|| qip_core::error::Error::not_found("credibility finding"))?;
    assert!(
        !credible.1,
        "a sub-threshold result cannot pass on probability"
    );
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
    let outcome = PilotGate::default().evaluate(&StrategyEvidence::new().with_pilot(bare), start());
    assert!(!outcome.passed);
    let failed: Vec<&str> = outcome
        .failures()
        .iter()
        .map(|(n, _, _)| n.as_str())
        .collect();
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

    let outcome = ScaledGate::default().evaluate(&StrategyEvidence::new().with_scaled(scaled), now);
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

    let outcome = ScaledGate::default().evaluate(&StrategyEvidence::new().with_scaled(scaled), now);
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
    let mut ledger = ledger()?;
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
        let mut ledger = ledger()?;
        walk_to_scaled(&mut ledger)?;
        ledger.demote(
            &strategy(),
            target,
            "risk-monitor",
            "a stated reason",
            start(),
        )?;
        assert_eq!(ledger.stage_of(&strategy()), target);
    }
    Ok(())
}

#[test]
fn retirement_is_terminal_and_a_retired_strategy_must_be_re_proposed_as_a_new_candidate()
-> Result<()> {
    let mut ledger = ledger()?;
    walk_to_scaled(&mut ledger)?;
    ledger.retire(
        &strategy(),
        "portfolio-committee",
        "the edge is gone",
        start(),
    )?;
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Retired);

    // Nothing promotes out of retirement: the ladder has no rung above it…
    assert!(GateStage::Retired.next().is_none());
    let error = AuthorisedPromotion::advance(GateStage::Retired, None, start())
        .expect_err("retired is terminal");
    assert!(error.message().contains("terminal"), "{error:?}");

    // …and the ledger refuses even a correctly-formed promotion for a retired
    // strategy, so a stale `from` cannot resurrect it.
    let evidence = charged(full_evidence(start(), start())?)?;
    let promotion = AuthorisedPromotion::advance(GateStage::Candidate, None, start())?;
    let admission = HoldoutGate::default().admit(&evidence, start());
    let error = ledger
        .record_promotion(&strategy(), promotion, admission, "trying again")
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
    let mut ledger = ledger()?;
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
    let last = history
        .last()
        .ok_or_else(|| qip_core::error::Error::not_found("entry"))?;
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
    let mut ledger = ledger()?;
    let pilot_start = start();
    let evidence = full_evidence(pilot_start, pilot_start)?;
    for target in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
    ] {
        let approval = if target.requires_human_approval() {
            Some(dual_approval(
                "momentum-v3",
                pilot_start,
                "the gate checks all passed",
            )?)
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

/// Every rung a strategy climbs is counted by the rungs left and entered, and
/// by nothing else. The strategy's own id is refused as a label: rungs are
/// seven and closed, strategies are however many the foundry proposes, and a
/// series keyed on them grows until it cannot be scraped.
#[test]
fn every_promotion_is_counted_by_the_rungs_it_moves_between() -> Result<()> {
    let metrics = Arc::new(Metrics::new("lifecycle-test"));
    let mut ledger = ledger()?.with_metrics(Arc::clone(&metrics));
    assert_eq!(
        metrics.snapshot().counter_total(names::STRATEGY_PROMOTIONS),
        0,
        "nothing has moved yet"
    );

    walk_to_scaled(&mut ledger)?;
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Scaled);

    let snapshot = metrics.snapshot();
    for (from, to) in [
        (GateStage::Candidate, GateStage::Holdout),
        (GateStage::Holdout, GateStage::Paper),
        (GateStage::Paper, GateStage::Shadow),
        (GateStage::Shadow, GateStage::Pilot),
        (GateStage::Pilot, GateStage::Scaled),
    ] {
        assert_eq!(
            snapshot.counter(
                names::STRATEGY_PROMOTIONS,
                &labels([("from", from.as_str()), ("to", to.as_str())])
            ),
            1,
            "one move from {} to {}",
            from.as_str(),
            to.as_str()
        );
    }
    assert_eq!(snapshot.counter_total(names::STRATEGY_PROMOTIONS), 5);
    assert_eq!(
        snapshot.counter_total(names::STRATEGY_DEMOTIONS),
        0,
        "a walk up is not a demotion"
    );
    for series in snapshot
        .series
        .iter()
        .filter(|s| s.name == names::STRATEGY_PROMOTIONS)
    {
        assert!(
            series.labels.values().all(|v| v != strategy().as_str()),
            "the strategy id is an unbounded label: {:?}",
            series.labels
        );
    }
    Ok(())
}

/// An automatic demotion for decayed performance is a capital-affecting action
/// no human was in the loop for. It reached the ledger and no series, so the
/// one move an operator most needed to see was the one nothing charted.
#[test]
fn an_automatic_demotion_for_decayed_performance_is_counted() -> Result<()> {
    let metrics = Arc::new(Metrics::new("lifecycle-test"));
    let (mut ledger, baseline) = pilot_fixture()?;
    ledger.attach_metrics(Arc::clone(&metrics));
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Pilot);
    assert_eq!(
        metrics.snapshot().counter_total(names::STRATEGY_DEMOTIONS),
        0,
        "nothing has been demoted yet"
    );

    let now = start().saturating_add(Duration::from_days(60));
    let mut observation = healthy_observation(now);
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
    assert_eq!(demotion.from, GateStage::Pilot);
    assert_eq!(demotion.to, GateStage::Shadow);

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::STRATEGY_DEMOTIONS,
            &labels([("from", "pilot"), ("to", "shadow")])
        ),
        1,
        "one demotion out of capital; series: {:?}",
        snapshot.series.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    assert_eq!(snapshot.counter_total(names::STRATEGY_DEMOTIONS), 1);
    Ok(())
}

#[test]
fn regime_drift_demotes_a_strategy_out_of_capital() -> Result<()> {
    let (mut ledger, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(60));
    let mut observation = healthy_observation(now);
    // Same edge per unit of risk, four times the realised volatility: the
    // market the strategy was sized in is gone.
    observation.returns = good_returns(8, 60, 0.0015)
        .iter()
        .map(|r| r * 4.0)
        .collect();

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
        let triggers = monitor.triggers(&baseline, &observation, Some(&healthy_registry(now)), now);
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
        DemotionTrigger::OutsideHoldoutBand {
            live_sharpe: 0.1,
            lower: 2.0,
            upper: 9.0,
            observations: 30,
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
    let evidence = charged(full_evidence(
        start(),
        start().saturating_add(Duration::from_days(120)),
    )?)?;
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
        assert!(
            outcome.passed,
            "{:?}: {:?}",
            gate.stage(),
            outcome.failures()
        );
    }
    Ok(())
}

/// A parameter sweep split across runs must not correct each run against its
/// own count. Blueprint rule 25: deflated Sharpe is corrected against the
/// family's cumulative lifetime trials, never per batch. Before the trial
/// book, `HoldoutGate` passed `holdout.trials` — this run's number — straight
/// into `deflated_sharpe`, so two runs of twelve corrected for twelve each.
#[test]
fn a_second_run_is_corrected_against_the_first_runs_trials_as_well() -> Result<()> {
    let mut ledger = ledger()?;
    let second = StrategyId::new("momentum-v3-run2");
    ledger
        .trial_book_mut()
        .ok_or_else(|| Error::not_found("trial book"))?
        .enrol(&second, &family()?, start())?;

    // A series strong enough to pass at twelve trials and at twenty-four,
    // but weak enough that the two corrections give different numbers.
    let mut holdout = strong_holdout()?;
    holdout.holdout_returns = good_returns(3, 400, 0.0010);
    assert_eq!(holdout.trials, 12);
    let evidence = StrategyEvidence::new().with_holdout(holdout.clone());

    // Premise: a single-run correction and a two-run correction differ, in
    // the direction of more trials meaning less confidence.
    let single = deflated_sharpe(&holdout.holdout_returns, 12, holdout.periods_per_year)?;
    let double = deflated_sharpe(&holdout.holdout_returns, 24, holdout.periods_per_year)?;
    assert!(
        single.is_credible() && double.is_credible(),
        "{single:?} {double:?}"
    );
    assert!(double.expected_maximum > single.expected_maximum);
    assert!(
        double.probability < single.probability,
        "twenty-four trials must deflate harder than twelve: {} vs {}",
        double.probability,
        single.probability
    );
    assert_ne!(double.summarise(), single.summarise());

    // Run one: the first candidate from the sweep.
    attempt_promotion(
        &mut ledger,
        &strategy(),
        &evidence,
        None,
        "run one",
        start(),
    )?;
    let book = ledger
        .trial_book()
        .ok_or_else(|| Error::not_found("trial book"))?;
    assert_eq!(book.lifetime_trials(&family()?), Some(12));

    // Run two: another candidate from the same sweep, an hour later. Its own
    // evidence still says twelve, and the book says twenty-four.
    let later = start().saturating_add(Duration::from_hours(1));
    attempt_promotion(&mut ledger, &second, &evidence, None, "run two", later)?;
    let book = ledger
        .trial_book()
        .ok_or_else(|| Error::not_found("trial book"))?;
    assert_eq!(book.lifetime_trials(&family()?), Some(24));

    // What the gate recorded for run two is the two-run statistic, exactly.
    let admission = ledger
        .admission_evidence(&second, GateStage::Holdout)
        .ok_or_else(|| Error::not_found("run two's admission"))?;
    let detail = |name: &str| -> Result<String> {
        admission
            .findings
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, _, d)| d.clone())
            .ok_or_else(|| Error::not_found(name))
    };
    assert_eq!(detail("deflated_sharpe_credible")?, double.summarise());
    assert_ne!(detail("deflated_sharpe_credible")?, single.summarise());
    assert!(
        detail("lifetime_trial_count_known")?.contains("12 trial(s) this run on top of 12"),
        "{}",
        detail("lifetime_trial_count_known")?
    );
    Ok(())
}

/// An unknown lifetime count is not zero and is not this run's number. Every
/// path that could stand in for the book with a smaller count is refused,
/// and each refusal names the act that would make the count known.
#[test]
fn a_promotion_whose_lifetime_trial_count_is_unknown_is_refused_naming_what_to_do() -> Result<()> {
    let evidence = StrategyEvidence::new().with_holdout(strong_holdout()?);

    // Premise: with a known count the same evidence passes the same gate.
    attempt_promotion(
        &mut ledger()?,
        &strategy(),
        &evidence,
        None,
        "known",
        start(),
    )?;

    // A ledger with no book at all.
    let mut bare = LifecycleLedger::new();
    let error = attempt_promotion(&mut bare, &strategy(), &evidence, None, "no book", start())
        .expect_err("no book means no lifetime count");
    assert_eq!(error.code(), "denied", "{error:?}");
    assert!(error.message().contains("unknown"), "{error:?}");
    assert!(error.message().contains("with_trial_book"), "{error:?}");
    assert!(error.message().contains("not zero"), "{error:?}");
    assert_eq!(bare.stage_of(&strategy()), GateStage::Candidate);
    assert!(bare.history(&strategy()).is_empty());

    // A book that has never heard of the strategy.
    let mut unenrolled = LifecycleLedger::new().with_trial_book(TrialBook::in_memory());
    let error = attempt_promotion(
        &mut unenrolled,
        &strategy(),
        &evidence,
        None,
        "not enrolled",
        start(),
    )
    .expect_err("a strategy in no family charges nowhere");
    assert_eq!(error.code(), "denied", "{error:?}");
    assert!(error.message().contains("TrialBook::enrol"), "{error:?}");
    assert_eq!(unenrolled.stage_of(&strategy()), GateStage::Candidate);

    // The gate itself, handed evidence with no account, fails the named check
    // rather than reading the run's own twelve.
    let outcome = HoldoutGate::default().evaluate(&evidence, start());
    assert!(!outcome.passed);
    let known = outcome
        .findings
        .iter()
        .find(|(name, _, _)| name == "lifetime_trial_count_known")
        .ok_or_else(|| Error::not_found("lifetime_trial_count_known"))?;
    assert!(!known.1);
    assert!(known.2.contains("unknown"), "{}", known.2);
    assert!(
        !outcome
            .findings
            .iter()
            .any(|(_, _, d)| d.contains("12 trial(s) alone")),
        "the run's own count must not have been deflated against: {:?}",
        outcome.findings
    );
    let error = HoldoutGate::default()
        .deflated(&evidence)
        .expect_err("no account, no statistic");
    assert_eq!(error.code(), "denied");
    Ok(())
}

/// A count that lives only in a process is a per-run count with extra steps.
/// The book writes each record to its store before admitting it, replays the
/// journal on reopening, and refuses a journal whose count was lowered or
/// whose chain has a gap.
#[test]
fn a_trial_book_replays_its_journal_from_the_store_and_refuses_a_tampered_one() -> Result<()> {
    let store = Arc::new(MemoryStore::default());
    let as_port = |s: &Arc<MemoryStore>| -> Arc<dyn KeyValueStore> { Arc::clone(s) as _ };
    {
        let mut book = TrialBook::open(as_port(&store))?;
        assert!(book.is_durable());
        assert_eq!(book.lifetime_trials(&family()?), None, "unknown, not zero");
        book.open_family(&family()?, start())?;
        assert_eq!(book.lifetime_trials(&family()?), Some(0), "opened is known");
        book.enrol(&strategy(), &family()?, start())?;
        let first = book.charge(&strategy(), 12, start())?;
        let second = book.charge(
            &strategy(),
            30,
            start().saturating_add(Duration::from_hours(1)),
        )?;
        assert_eq!(first.lifetime(), 12);
        assert_eq!(second.prior(), 12);
        assert_eq!(second.lifetime(), 42);
    }
    assert_eq!(store.len()?, 4, "opened, enrolled, two charges");

    // The process restarts: the count is what it was.
    let reopened = TrialBook::open(as_port(&store))?;
    assert_eq!(reopened.lifetime_trials(&family()?), Some(42));
    assert_eq!(reopened.family_of(&strategy()), Some(&family()?));
    assert_eq!(reopened.journal(&family()?).len(), 4);
    reopened.verify()?;
    assert!(
        reopened
            .journal(&family()?)
            .windows(2)
            .all(|w| w[1].previous == w[0].hash),
        "each record chains to the one before"
    );

    // Somebody lowers the last charge's total in the store.
    let key = |sequence: u64| -> Result<String> {
        Ok(format!("{JOURNAL_PREFIX}{}/{sequence:020}", family()?))
    };
    let last = key(3)?;
    let mut record = store
        .get(&last)?
        .ok_or_else(|| Error::not_found(last.clone()))?;
    record["lifetime_after"] = serde_json::Value::from(12);
    store.put(&last, record)?;
    let error = TrialBook::open(as_port(&store)).expect_err("a lowered count must not replay");
    assert!(error.message().contains("does not hash"), "{error:?}");

    // Somebody removes a record from the middle instead: the survivor no
    // longer chains to what precedes it, and the book still refuses.
    let middle = key(2)?;
    let first_charge = store
        .get(&middle)?
        .ok_or_else(|| Error::not_found(middle.clone()))?;
    assert!(store.delete(&middle)?);
    let error = TrialBook::open(as_port(&store)).expect_err("a gap in the chain must not replay");
    assert!(
        error.message().contains("sequence") || error.message().contains("chain"),
        "{error:?}"
    );
    // Put it back unaltered and the tampered total is once again the only fault.
    store.put(&middle, first_charge)?;
    let error = TrialBook::open(as_port(&store)).expect_err("still tampered");
    assert!(error.message().contains("does not hash"), "{error:?}");
    Ok(())
}

/// The two ways to launder a count without lying about any single run:
/// reopen the family at zero, or move the strategy to a family with a smaller
/// count. Both are refused, and so is a run that claims to have tried nothing.
#[test]
fn a_family_opens_once_and_a_member_cannot_take_its_trials_elsewhere() -> Result<()> {
    let mut book = opened_book()?;
    book.charge(&strategy(), 100, start())?;
    assert_eq!(book.lifetime_trials(&family()?), Some(100));

    let error = book
        .open_family(&family()?, start())
        .expect_err("a family opens once");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("already open with 100"),
        "{error:?}"
    );
    assert_eq!(
        book.lifetime_trials(&family()?),
        Some(100),
        "nothing was reset"
    );

    let fresh = StrategyFamily::new("momentum-fresh")?;
    book.open_family(&fresh, start())?;
    let error = book
        .enrol(&strategy(), &fresh, start())
        .expect_err("a strategy cannot change family");
    assert_eq!(error.code(), "denied");
    assert!(error.message().contains("cannot move"), "{error:?}");
    assert_eq!(book.family_of(&strategy()), Some(&family()?));

    // Re-enrolling in the same family is a no-op, not a second membership.
    book.enrol(&strategy(), &family()?, start())?;
    assert_eq!(
        book.journal(&family()?).len(),
        3,
        "opened, enrolled, charged"
    );

    let error = book
        .charge(&strategy(), 0, start())
        .expect_err("a candidate was tried, so the count is at least one");
    assert_eq!(error.code(), "invalid");

    // A stranger to the book has no count, and asking does not create one.
    let stranger = StrategyId::new("nobody-enrolled-this");
    assert_eq!(book.family_of(&stranger), None);
    let error = book
        .charge(&stranger, 1, start())
        .expect_err("no family, no count");
    assert!(error.message().contains("unknown"), "{error:?}");
    assert!(
        StrategyFamily::new("bad/name").is_err() && StrategyFamily::new("  ").is_err(),
        "a family name is a key segment"
    );
    Ok(())
}

/// Blueprint rule 27: backtest and live share the production crates, so they
/// cannot diverge. This crate's own scoring helpers delegate to
/// `qip_simulation_engine::validation` rather than restating `mean / stddev`,
/// and the scaled gate — the one place that had written it out by hand —
/// now reports the engine's number. Held to the last bit, on series with
/// drift, without, and with no variance at all.
#[test]
fn every_sharpe_this_crate_reports_is_the_simulation_engines_sharpe() -> Result<()> {
    let series: Vec<Vec<f64>> = vec![
        good_returns(1, 400, 0.0018),
        good_returns(11, 60, -0.0002),
        good_returns(5, 30, 0.0),
        vec![0.001; 25],
        vec![0.004, -0.003, 0.002, 0.001, -0.002, 0.003, 0.0, 0.001],
    ];
    assert!(
        series
            .iter()
            .any(|r| qip_numerics::stats::stddev(r) < 1e-12),
        "premise: one flat series"
    );
    assert!(
        series
            .iter()
            .any(|r| periodic_sharpe(r).is_ok_and(|s| s < 0.0)),
        "premise: one loser"
    );
    for returns in &series {
        let engine =
            assess_overfitting(std::slice::from_ref(returns), std::slice::from_ref(returns))?;
        let periodic = periodic_sharpe(returns)?;
        assert_eq!(
            periodic.to_bits(),
            engine.out_of_sample_sharpe.to_bits(),
            "{returns:?}"
        );
        assert_eq!(periodic.to_bits(), engine.in_sample_sharpe.to_bits());

        // The annualised figure is the one the deflated Sharpe calls
        // `observed`. The engine refuses to deflate a flat series, which is
        // the one place the helper and the engine legitimately part.
        for periods in [252.0, 52.0, 12.0, 0.5] {
            let annualised = annualised_sharpe(returns, periods)?;
            match deflated_sharpe(returns, 1, periods) {
                Ok(deflated) => assert!(
                    (annualised - deflated.observed).abs() <= 1e-12,
                    "{annualised} vs {} at {periods}",
                    deflated.observed
                ),
                Err(error) => assert!(
                    qip_numerics::stats::stddev(returns) < 1e-12 || returns.len() < 20,
                    "{error:?}"
                ),
            }
        }
    }

    // The scaled gate quotes the same figure, to the same two decimals.
    let pilot_start = start();
    let now = pilot_start.saturating_add(Duration::from_days(120));
    let scaled = strong_scaled(pilot_start, now)?;
    let expected = periodic_sharpe(&scaled.pilot_returns)?;
    assert!(expected >= 0.5, "premise: the fixture clears the bar");
    let outcome = ScaledGate::default().evaluate(&StrategyEvidence::new().with_scaled(scaled), now);
    let detail = outcome
        .findings
        .iter()
        .find(|(name, _, _)| name == "pilot_performance_sustained")
        .map(|(_, _, d)| d.clone())
        .ok_or_else(|| Error::not_found("pilot_performance_sustained"))?;
    assert!(
        detail.starts_with(&format!("realised pilot Sharpe {expected:.2} ")),
        "{detail}"
    );
    Ok(())
}

/// ADR 0023 step 9 requires a strategy to run "inside its holdout band" and
/// records that no band was defined anywhere in the tree. The holdout gate
/// now defines one from the statistic it admits on, and the ledger carries it
/// on that admission — with the method, the instant, the observations and
/// the lifetime trial count it was computed under, so it can be reproduced.
#[test]
fn a_holdout_admission_carries_the_band_its_validation_produced() -> Result<()> {
    let mut ledger = ledger()?;
    let evidence = StrategyEvidence::new().with_holdout(strong_holdout()?);
    assert!(
        ledger.holdout_band(&strategy()).is_none(),
        "premise: nothing yet"
    );

    attempt_promotion(
        &mut ledger,
        &strategy(),
        &evidence,
        None,
        "validated",
        start(),
    )?;
    let band = *ledger
        .holdout_band(&strategy())
        .ok_or_else(|| Error::not_found("holdout band"))?;
    assert!(
        band.lower < band.centre && band.centre < band.upper,
        "{band:?}"
    );
    assert!(band.standard_error > 0.0);
    assert_eq!(band.method, BandMethod::NINETY_FIVE);
    assert_eq!(band.as_of, start());
    assert_eq!(band.observations, 400);
    assert_eq!(band.periods_per_year.to_bits(), 252.0_f64.to_bits());
    assert_eq!(
        band.trials, 12,
        "the lifetime count, which after one run is the run's"
    );

    // It is the band the gate itself produced from this evidence, and the
    // admission says so in a finding a reviewer can read.
    let admission = HoldoutGate::default().admit(&charged(evidence.clone())?, start());
    assert_eq!(admission.band, Some(band));
    let entry = ledger
        .history(&strategy())
        .first()
        .ok_or_else(|| Error::not_found("entry"))?;
    assert_eq!(entry.band, Some(band));
    assert!(
        admission
            .outcome
            .findings
            .iter()
            .any(|(name, ok, detail)| name == "holdout_band_defined"
                && *ok
                && detail.contains("12 lifetime trial(s)")),
        "{:?}",
        admission.outcome.findings
    );

    // A second holdout admission from a family with more trials records the
    // count it was corrected under.
    let second = StrategyId::new("momentum-v3-run2");
    ledger
        .trial_book_mut()
        .ok_or_else(|| Error::not_found("trial book"))?
        .enrol(&second, &family()?, start())?;
    attempt_promotion(&mut ledger, &second, &evidence, None, "validated", start())?;
    assert_eq!(
        ledger
            .holdout_band(&second)
            .map(|b| b.trials)
            .ok_or_else(|| Error::not_found("second band"))?,
        24
    );
    Ok(())
}

/// Live performance consistent with the holdout, at the live sample's own
/// precision, is not a demotion — a band that demoted for noise would empty
/// the book of every real strategy inside a month.
#[test]
fn live_performance_inside_the_holdout_band_is_not_demoted() -> Result<()> {
    let (mut ledger, baseline) = pilot_fixture()?;
    let now = start().saturating_add(Duration::from_days(60));
    let observation = healthy_observation(now);
    let verdict = ledger.band_verdict(&strategy(), &observation.returns)?;
    assert!(verdict.inside, "premise: {}", verdict.describe());
    assert!(verdict.lower < verdict.live && verdict.live < verdict.upper);

    let (triggers, demotion) = DemotionMonitor::default().enforce(
        &mut ledger,
        &baseline,
        &observation,
        Some(&healthy_registry(now)),
        now,
    )?;
    assert!(
        !triggers
            .iter()
            .any(|t| matches!(t, DemotionTrigger::OutsideHoldoutBand { .. })),
        "{triggers:?}"
    );
    assert!(demotion.is_none(), "{demotion:?}");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Pilot);
    Ok(())
}

/// Outside the band is a demotion candidate, decided where demotions are
/// already decided and counted on the series operators already watch. The
/// live figure here is far *above* the holdout — nothing else trips, so the
/// band is the only trigger, and a result too good to be the validated
/// strategy is treated as what it is.
#[test]
fn live_performance_outside_the_holdout_band_is_demoted_and_counted() -> Result<()> {
    let metrics = Arc::new(Metrics::new("lifecycle-test"));
    let (mut ledger, baseline) = pilot_fixture()?;
    ledger.attach_metrics(Arc::clone(&metrics));
    let now = start().saturating_add(Duration::from_days(60));
    let mut observation = healthy_observation(now);
    observation.returns = good_returns(9, 60, 0.0080);
    let verdict = ledger.band_verdict(&strategy(), &observation.returns)?;
    assert!(!verdict.inside, "premise: {}", verdict.describe());
    assert!(verdict.live > verdict.upper, "premise: above, not below");

    let (triggers, demotion) = DemotionMonitor::default().enforce(
        &mut ledger,
        &baseline,
        &observation,
        Some(&healthy_registry(now)),
        now,
    )?;
    assert_eq!(triggers.len(), 1, "only the band tripped: {triggers:?}");
    let DemotionTrigger::OutsideHoldoutBand {
        live_sharpe,
        lower,
        upper,
        observations,
    } = triggers[0]
    else {
        return Err(Error::invalid(format!("{triggers:?}")));
    };
    assert_eq!(
        (live_sharpe, lower, upper),
        (verdict.live, verdict.lower, verdict.upper)
    );
    assert_eq!(observations, 60);
    let demotion = demotion.ok_or_else(|| Error::not_found("demotion"))?;
    assert_eq!(
        (demotion.from, demotion.to),
        (GateStage::Pilot, GateStage::Shadow)
    );
    assert!(
        demotion.rationale.contains("outside the holdout band"),
        "{}",
        demotion.rationale
    );
    assert!(demotion.approver.is_none(), "no human in the loop");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Shadow);
    assert_eq!(
        metrics.snapshot().counter(
            names::STRATEGY_DEMOTIONS,
            &labels([("from", "pilot"), ("to", "shadow")])
        ),
        1
    );
    Ok(())
}

/// No band, no judgement. A strategy never admitted to holdout has nothing to
/// be inside of, and a holdout admission that arrives without its band is
/// refused rather than recorded with a gap the Phase 3 gate would later fall
/// through.
#[test]
fn judging_or_admitting_without_a_holdout_band_is_refused() -> Result<()> {
    let ledger = LifecycleLedger::new();
    let error = ledger
        .band_verdict(&strategy(), &good_returns(8, 60, 0.0015))
        .expect_err("nothing to be inside of");
    assert_eq!(error.code(), "denied");
    assert!(error.message().contains("no holdout band"), "{error:?}");
    assert!(error.message().contains("holdout gate"), "{error:?}");

    // The same evidence admits with its band and is refused without it.
    let evidence = charged(StrategyEvidence::new().with_holdout(strong_holdout()?))?;
    let admission = HoldoutGate::default().admit(&evidence, start());
    assert!(
        admission.outcome.passed && admission.band.is_some(),
        "premise"
    );
    let mut ledger = LifecycleLedger::new();
    let promotion = AuthorisedPromotion::advance(GateStage::Candidate, None, start())?;
    let error = ledger
        .record_promotion(
            &strategy(),
            promotion.clone(),
            Admission {
                outcome: admission.outcome.clone(),
                band: None,
            },
            "band mislaid",
        )
        .expect_err("a holdout admission without its band is refused");
    assert_eq!(error.code(), "denied");
    assert!(error.message().contains("carries no band"), "{error:?}");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Candidate);
    ledger.record_promotion(&strategy(), promotion, admission, "band attached")?;
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Holdout);
    assert!(ledger.holdout_band(&strategy()).is_some());

    // And a band cannot be smuggled onto another rung's admission.
    let band = *ledger
        .holdout_band(&strategy())
        .ok_or_else(|| Error::not_found("band"))?;
    let paper =
        PaperGate::default().evaluate(&StrategyEvidence::new().with_paper(strong_paper()), start());
    let promotion = AuthorisedPromotion::advance(GateStage::Holdout, None, start())?;
    let error = ledger
        .record_promotion(
            &strategy(),
            promotion,
            Admission {
                outcome: paper,
                band: Some(band),
            },
            "band on the wrong rung",
        )
        .expect_err("a band belongs to the holdout admission");
    assert_eq!(error.code(), "invalid");
    assert_eq!(ledger.stage_of(&strategy()), GateStage::Holdout);
    Ok(())
}
