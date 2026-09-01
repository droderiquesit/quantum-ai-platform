//! Tests for the central plane.
//!
//! Almost every test here tries to get a cell to hold capital it has not
//! earned: skip the dual approval, ship a bundle for a strategy still in
//! shadow, edit a grant after it was signed, or keep issuing against a
//! strategy an automatic trigger has already pushed down. The rest are about
//! the things a distributed platform can get wrong that a single process
//! cannot — three cells accumulating one name between them, and one cell's
//! books not agreeing with its venue.
//!
//! The last test is the one that says the whole module is additive: the same
//! cycle, stage for stage, on a platform whose central plane has been used and
//! one whose has not.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::allocation::StrategyProposal;
use qip_capital::capacity::CapacityModel;
use qip_capital::envelope::{EnvelopeIssuer, EnvelopeTerms, MAXIMUM_ENVELOPE_VALIDITY};
use qip_capital::exposure::CellPosition;
use qip_compliance::approval::{ApprovalChain, CapitalRequest, OperatorCredential};
use qip_compliance::incident::HaltScope;
use qip_compliance::signing::SigningKey;
use qip_contracts::feature::FeatureKey;
use qip_contracts::gate::GateStage;
use qip_contracts::governance::{Approval, Control};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance as DataProvenance};
use qip_financial::universe::Universe;
use qip_kernel::central::{
    CellOutcome, CellReport, CentralConfig, CentralPlane, IssuedCapital, LearningVerdict,
    ReconciliationBreak, StrategyCandidate, StrategyDna, capital_subject,
};
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, KillCondition, LeakageAudit, PaperEvidence,
    PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence, StrategyEvidence,
};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;

// --- the instants and identities every test shares ---------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// Ninety days of pilot plus a month, so the scaled gate's duration bar is met
/// without every test having to say so.
fn scaled_at() -> Timestamp {
    start().saturating_add(Duration::from_days(120))
}

const CELL: &str = "cell-lon-1";
const VENUE: &str = "XNYS";
const INSTRUMENT: &str = "AAA";

fn strategy() -> StrategyId {
    StrategyId::new("central-momentum")
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

// --- compiled strategies -----------------------------------------------------

/// A one-rule strategy and the arena it points into.
///
/// Compiled rather than hand-built: the thing the centre ships has to be the
/// thing the compiler produced, and a hand-built program would prove nothing
/// about that.
fn compile(id: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = ObjectId::from_string(format!("obj-{id}"));
    let pressure = FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;

    let spec = StrategySpec::new(StrategyId::new(id), subject, Duration::from_millis(250))
        .with_rule(Rule::new(
            "enter",
            SignalKind::Enter,
            Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
            Expr::Exact(Decimal::from_int(100)),
            Expr::Statistic(0.62),
            500,
        ));

    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

// --- evidence that passes every gate ----------------------------------------

/// Returns with a genuine positive drift, drawn from a seeded stream so every
/// run of this suite sees the same numbers.
fn good_returns(seed: u64, n: usize, drift: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n)
        .map(|_| {
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
    let (folds, label_horizon, embargo) = (5, 10, 5);
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

/// The bound the pilot gate reads. Not the grant a cell enforces: that one is
/// issued by the central plane and signed per `qip-capital`.
fn proposed_envelope(id: &StrategyId, cell: &str, now: Timestamp) -> Result<CapitalEnvelope> {
    CapitalEnvelope::new(
        id.clone(),
        cell,
        dec!("250000"),
        dec!("250000"),
        dec!("250000"),
        vec![venue()],
        now,
        now.saturating_add(Duration::from_days(14)),
        "alice.chen",
        "proposed-not-issued",
    )
}

fn strong_pilot(id: &StrategyId, cell: &str, now: Timestamp) -> Result<PilotEvidence> {
    Ok(PilotEvidence {
        approval: Some(dual_approval(
            &format!("{id} pilot"),
            now,
            "shadow agreement held at 100% over 400 decisions",
        )?),
        envelope: Some(proposed_envelope(id, cell, now)?),
        kill_conditions: vec![
            KillCondition::RealisedLoss(dec!("25000")),
            KillCondition::Drawdown(0.08),
            KillCondition::ConsecutiveLosingDays(5),
        ],
    })
}

fn strong_scaled(
    id: &StrategyId,
    pilot_start: Timestamp,
    now: Timestamp,
) -> Result<ScaledEvidence> {
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
            &format!("{id} pilot"),
            pilot_start,
            "shadow agreement held at 100% over 400 decisions",
        )?),
        scaling_approval: Some(dual_approval(
            &format!("{id} scaling"),
            now,
            "ninety days at pilot returned a 0.7 Sharpe inside a quarter of capacity",
        )?),
    })
}

fn full_evidence(id: &StrategyId, cell: &str) -> Result<StrategyEvidence> {
    Ok(StrategyEvidence::new()
        .with_holdout(strong_holdout()?)
        .with_paper(strong_paper())
        .with_shadow(strong_shadow())
        .with_pilot(strong_pilot(id, cell, start())?)
        .with_scaled(strong_scaled(id, start(), scaled_at())?))
}

// --- assembling a plane ------------------------------------------------------

fn plane() -> Result<CentralPlane> {
    CentralPlane::new(&[7u8; 32], CentralConfig::default())
}

fn credentials(at: Timestamp) -> Result<Vec<OperatorCredential>> {
    Ok(vec![
        OperatorCredential::verified("alice.chen", "webauthn", at)?,
        OperatorCredential::verified("bram.oduya", "webauthn", at)?,
    ])
}

fn proposal(id: &StrategyId, cell: &str) -> Result<StrategyProposal> {
    Ok(StrategyProposal {
        strategy: id.clone(),
        cell: cell.to_string(),
        venue: venue(),
        expected_sharpe: 1.8,
        sharpe_standard_error: 0.05,
        capacity: CapacityModel::new(
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 4.0),
            TransactionCostModel::listed(4.0),
            45.0,
            dec!("100"),
            0.5,
        )?,
        capacity_uncertainty: 0.2,
    })
}

/// Register a candidate with evidence that passes every gate.
fn register(plane: &mut CentralPlane, id: &StrategyId, cell: &str) -> Result<()> {
    let (compiled, program) = compile(id.as_str())?;
    let candidate = StrategyCandidate::new(compiled, program, cell, venue(), start())?
        .with_evidence(full_evidence(id, cell)?)
        .with_model("microprice-distilled@3")
        .with_evidence_artifacts(vec![
            format!("sha256:holdout-{id}"),
            format!("sha256:shadow-{id}"),
        ]);
    plane.factory_mut().register(candidate)?;
    plane.set_proposal(proposal(id, cell)?);
    Ok(())
}

/// Walk a registered candidate up to a rung, collecting a dual approval where
/// the rung demands one.
fn walk_to(plane: &mut CentralPlane, id: &StrategyId, target: GateStage) -> Result<()> {
    for (rung, at) in [
        (GateStage::Holdout, start()),
        (GateStage::Paper, start()),
        (GateStage::Shadow, start()),
        (GateStage::Pilot, start()),
        (GateStage::Scaled, scaled_at()),
    ] {
        let approval = if rung.requires_human_approval() {
            Some(dual_approval(
                id.as_str(),
                at,
                "every gate check passed with the evidence attached",
            )?)
        } else {
            None
        };
        plane
            .factory_mut()
            .promote(id, approval, "the gate passed", at)?;
        if rung == target {
            return Ok(());
        }
    }
    Ok(())
}

/// Issue a grant the way a deployment would: an approval naming the request's
/// own subject, two fresh credentials, and a requester who is neither approver.
fn issue(
    plane: &mut CentralPlane,
    id: &StrategyId,
    cell: &str,
    now: Timestamp,
) -> Result<IssuedCapital> {
    let approval = dual_approval(
        &capital_subject(id, cell),
        now,
        "the pilot gate passed and the allocator sized it inside the budget",
    )?;
    plane.issue(id, "research.desk", &approval, &credentials(now)?, 0.0, now)
}

// --- the tests ---------------------------------------------------------------

#[test]
fn a_candidate_with_perfect_evidence_but_no_dual_approval_never_yields_an_envelope() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Shadow)?;
    assert_eq!(plane.factory().stage_of(&id), GateStage::Shadow);

    // No approval at all.
    let unapproved = plane
        .factory_mut()
        .promote(&id, None, "the evidence speaks for itself", start())
        .unwrap_err();
    assert!(
        unapproved.message().contains("recorded human approval"),
        "a promotion to pilot without an approval should say so: {}",
        unapproved.message()
    );

    // One name, which is one short.
    let alone = Approval::new(
        id.as_str(),
        "alice.chen",
        start(),
        "I have reviewed the shadow run myself",
    )?;
    let single = plane
        .factory_mut()
        .promote(&id, Some(alone), "one reviewer is enough", start())
        .unwrap_err();
    assert!(
        single.message().contains("two approvers"),
        "a single approver should be named as the problem: {}",
        single.message()
    );

    // The strategy has not moved, so no envelope can be issued for it however
    // willing the approvers of the *grant* are.
    assert_eq!(plane.factory().stage_of(&id), GateStage::Shadow);
    let refused = issue(&mut plane, &id, CELL, start()).unwrap_err();
    assert!(
        refused.message().contains("holds no capital"),
        "issuance should refuse on the stage, not on the paperwork: {}",
        refused.message()
    );
    assert!(plane.envelope(CELL, &id).is_none());
    Ok(())
}

#[test]
fn a_dna_cannot_be_sealed_for_a_strategy_that_is_still_in_shadow() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Shadow)?;

    // Build a genuinely approved grant outside the plane, so the only thing
    // wrong with the DNA is the rung the strategy stands on.
    let (key, envelope, approved) = approved_grant_outside_the_plane(&id, CELL, start())?;
    let candidate = plane
        .factory()
        .candidate(&id)
        .ok_or_else(|| qip_core::Error::not_found("the candidate was registered"))?;

    let refused = StrategyDna::seal(
        candidate,
        GateStage::Shadow,
        &approved,
        &envelope,
        &key,
        "central-plane",
        start(),
    )
    .unwrap_err();
    assert!(
        refused.message().contains("shadow") && refused.message().contains("holds no capital"),
        "the refusal should name the rung: {}",
        refused.message()
    );

    // The same call at a capital-holding rung succeeds, so the refusal above is
    // about the stage and not about the setup.
    let sealed = StrategyDna::seal(
        candidate,
        GateStage::Pilot,
        &approved,
        &envelope,
        &key,
        "central-plane",
        start(),
    )?;
    assert_eq!(sealed.stage(), GateStage::Pilot);
    sealed.verify(&key, start())?;
    Ok(())
}

#[test]
fn a_tampered_dna_fails_verification_naming_the_section_that_changed() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    let issued = issue(&mut plane, &id, CELL, start())?;
    let dna = plane.ship(&issued, "central-plane", start())?;
    plane.verify_dna(&dna, start())?;

    // Widen the grant after it was signed, exactly as a compromised transport
    // would. The bundle still parses; that is the point.
    let mut wire: serde_json::Value = serde_json::from_str(&serde_json::to_string(&dna)?)?;
    wire["payload"]["envelope"]["gross_limit"] = serde_json::json!("999999999.000000000");
    let tampered: StrategyDna = serde_json::from_value(wire)?;
    assert_ne!(
        tampered.envelope().gross_limit(),
        dna.envelope().gross_limit(),
        "the tamper did not take"
    );

    let refused = plane.verify_dna(&tampered, start()).unwrap_err();
    assert!(
        refused.message().contains("`envelope` section"),
        "verification should name the section that changed: {}",
        refused.message()
    );

    // Editing the section digest to match hides which part changed and no
    // more: the provenance covers the whole payload.
    let mut wire: serde_json::Value = serde_json::from_str(&serde_json::to_string(&tampered)?)?;
    let repaired = qip_core::sha256_hex(&serde_json::to_vec(&tampered.envelope())?);
    wire["payload"]["section_digests"]["envelope"] = serde_json::json!(repaired);
    let doubly_tampered: StrategyDna = serde_json::from_value(wire)?;
    let refused = plane.verify_dna(&doubly_tampered, start()).unwrap_err();
    assert!(
        refused.message().contains("provenance records"),
        "the whole-payload digest should catch a repaired section: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn an_issued_envelope_is_sized_inside_the_allocators_budget_and_expires() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;

    let plan = plane.allocate(0.0, start())?;
    assert!(
        plan.is_within_budget(),
        "the allocator over-committed: {} against {}",
        plan.allocated(),
        plan.budget
    );

    let issued = issue(&mut plane, &id, CELL, start())?;
    let envelope = issued.envelope();
    assert!(envelope.gross_limit().is_positive());
    assert!(
        envelope.gross_limit() <= plan.budget,
        "the grant of {} exceeds the {} budget it was sized against",
        envelope.gross_limit(),
        plan.budget
    );
    assert!(
        envelope.gross_limit() <= plane.config().per_strategy,
        "the per-strategy limit did not bind"
    );

    // Every envelope expires, and the ceiling is hours: that is the only
    // backstop against a cell nobody can reach.
    let life = envelope.expires_at().since(start());
    assert_eq!(life, plane.config().envelope_validity);
    assert!(life <= MAXIMUM_ENVELOPE_VALIDITY);
    assert!(envelope.is_live(start()));
    assert!(
        !envelope.is_live(envelope.expires_at()),
        "an envelope that is still live at its own expiry bounds nothing"
    );

    // The governance record and the cell's bound describe the same grant.
    assert_eq!(
        issued.approved().envelope().signing_payload(),
        envelope.signing_payload()
    );
    assert_eq!(
        issued.approved().approvers(),
        vec!["alice.chen", "bram.oduya"]
    );
    Ok(())
}

#[test]
fn three_cells_holding_one_name_produce_a_concentration_recall() -> Result<()> {
    let mut plane = plane()?;
    let cells = ["cell-lon-1", "cell-nyc-1", "cell-sin-1"];
    let ids: Vec<StrategyId> = cells
        .iter()
        .map(|cell| StrategyId::new(format!("momentum-{cell}")))
        .collect();

    for (id, cell) in ids.iter().zip(cells) {
        register(&mut plane, id, cell)?;
        walk_to(&mut plane, id, GateStage::Pilot)?;
        issue(&mut plane, id, cell, start())?;
    }

    let mut switch = qip_risk_engine::autonomy::AutonomyController::new();
    let mut last = None;
    for (id, cell) in ids.iter().zip(cells) {
        let report = CellReport::new(cell, start()).with_positions(vec![position(
            cell,
            id,
            INSTRUMENT,
            dec!("1000"),
        )]);
        last = Some(plane.ingest(report, switch.kill_switch_mut(), start())?);
    }

    let ingestion =
        last.ok_or_else(|| qip_core::Error::not_found("three reports were ingested"))?;
    let crowded = &ingestion.crowded;
    assert_eq!(
        crowded.len(),
        1,
        "one name is held by all three cells: {crowded:?}"
    );
    assert_eq!(crowded[0].instrument, INSTRUMENT);
    assert_eq!(crowded[0].cells.len(), 3);

    assert!(
        ingestion
            .concentrations
            .iter()
            .any(|finding| finding.axis == "instrument" && finding.bucket == INSTRUMENT),
        "the whole book in one name should breach the instrument limit: {:?}",
        ingestion.concentrations
    );
    assert_eq!(
        ingestion.recalls.len(),
        3,
        "every cell holding the crowded name should be recalled: {:?}",
        ingestion.recalls
    );
    for order in &ingestion.recalls {
        assert!(cells.contains(&order.cell.as_str()));
        // A recall is a request; the grant's own expiry is what actually
        // bounds an unreachable cell.
        assert_eq!(
            order.backstop_expiry,
            order
                .issued_at
                .saturating_add(plane.config().envelope_validity)
        );
        assert!(order.unbounded_window(start()) > Duration::ZERO);
    }
    assert!(plane.recalls().outstanding(start()).len() == 3);
    Ok(())
}

#[test]
fn a_reconciliation_break_halts_that_cell_and_only_that_cell() -> Result<()> {
    let mut platform = platform()?;
    let id = strategy();

    let clean = CellReport::new("cell-nyc-1", start()).with_positions(vec![position(
        "cell-nyc-1",
        &id,
        INSTRUMENT,
        dec!("10"),
    )]);
    let quiet = platform.ingest_cell_report(clean, start())?;
    assert!(quiet.halted.is_none());

    let broken = CellReport::new(CELL, start())
        .with_positions(vec![position(CELL, &id, INSTRUMENT, dec!("10"))])
        .with_break(ReconciliationBreak {
            instrument: INSTRUMENT.to_string(),
            cell_quantity: dec!("10"),
            external_quantity: dec!("4"),
            detail: "six lots the venue has no record of".to_string(),
        });
    let ingestion = platform.ingest_cell_report(broken, start())?;

    assert_eq!(ingestion.halted, Some(HaltScope::Cell(CELL.to_string())));
    let switch = platform.autonomy().kill_switch();
    assert!(
        switch.is_halted(CELL),
        "the reporting cell should be halted"
    );
    assert!(
        !switch.is_halted("cell-nyc-1"),
        "a cell whose book reconciles should keep trading"
    );
    assert!(
        !switch.is_globally_tripped(),
        "one cell's bookkeeping failure is not the platform's outage"
    );
    assert!(!platform.central().may_act(id.as_str(), CELL));
    assert!(platform.central().may_act(id.as_str(), "cell-nyc-1"));
    Ok(())
}

/// A reconciliation break tripped a scoped kill switch and raised an incident
/// and wrote no series, so the highest-consequence thing the central plane does
/// was the one thing no operator could chart. The break is counted by its
/// direction and the halt by its cause; neither the cell nor the instrument is
/// a label, because both are dimensions that grow.
#[test]
fn a_reconciliation_break_is_recorded_by_direction_and_the_halt_by_cause() -> Result<()> {
    let mut platform = platform()?;
    let id = strategy();
    let over = labels([("direction", "cell_over_venue")]);
    let under = labels([("direction", "venue_over_cell")]);
    let halted = labels([("cause", "reconciliation")]);

    // Premise: a report that reconciles moves nothing.
    let clean = CellReport::new("cell-nyc-1", start()).with_positions(vec![position(
        "cell-nyc-1",
        &id,
        INSTRUMENT,
        dec!("10"),
    )]);
    let quiet = platform.ingest_cell_report(clean, start())?;
    assert!(quiet.halted.is_none());
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter_total(names::CENTRAL_RECONCILIATION_BREAKS),
        0,
        "a clean report is not a break"
    );
    assert_eq!(snapshot.counter_total(names::CENTRAL_CELL_HALTS), 0);

    let broken = CellReport::new(CELL, start())
        .with_positions(vec![position(CELL, &id, INSTRUMENT, dec!("10"))])
        .with_break(ReconciliationBreak {
            instrument: INSTRUMENT.to_string(),
            cell_quantity: dec!("10"),
            external_quantity: dec!("4"),
            detail: "six lots the venue has no record of".to_string(),
        })
        .with_break(ReconciliationBreak {
            instrument: "BBB".to_string(),
            cell_quantity: dec!("1"),
            external_quantity: dec!("3"),
            detail: "two lots the cell never booked".to_string(),
        });
    let ingestion = platform.ingest_cell_report(broken, start())?;
    assert_eq!(ingestion.halted, Some(HaltScope::Cell(CELL.to_string())));

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &over),
        1,
        "one break where the cell holds more than the venue confirms"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &under),
        1,
        "one break where the venue confirms more than the cell holds"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_CELL_HALTS, &halted),
        1,
        "one scoped halt, whatever the number of breaks behind it"
    );
    // Bounded by construction: no label names the cell or the instrument.
    for series in snapshot.series.iter().filter(|s| {
        s.name == names::CENTRAL_RECONCILIATION_BREAKS || s.name == names::CENTRAL_CELL_HALTS
    }) {
        assert!(
            !series.labels.contains_key("cell") && !series.labels.contains_key("instrument"),
            "{} is keyed on an unbounded dimension: {:?}",
            series.name,
            series.labels
        );
        assert!(
            !series.help.is_empty(),
            "{} exports without a description",
            series.name
        );
    }
    Ok(())
}

#[test]
fn the_compliance_report_enumerates_all_six_controls_with_its_caveats_intact() -> Result<()> {
    let platform = platform()?;
    let report = platform.compliance_report(start())?;

    assert_eq!(report.statuses().len(), Control::all().len());
    for control in Control::all() {
        let status = report.status(control).ok_or_else(|| {
            qip_core::Error::not_found(format!("{} is reported", control.as_str()))
        })?;
        assert!(
            status.enforced,
            "{} is not enforced: {}",
            control.as_str(),
            status.mechanism
        );
        assert!(
            !status.mechanism.trim().is_empty(),
            "{} names no mechanism",
            control.as_str()
        );
    }
    report.require_fully_enforced()?;

    // The honest gaps are part of the compliance position. A report that lost
    // them would look better and describe less.
    let caveats = report.caveats();
    assert!(
        !caveats.is_empty(),
        "the report enumerated six enforced controls and no caveats, which is not what any of \
         them says about itself"
    );
    assert!(
        caveats
            .iter()
            .any(|(control, _)| *control == Control::SignedArtifactsAndProvenance),
        "the symmetric-signing caveat is the largest one and must survive: {caveats:?}"
    );
    Ok(())
}

#[test]
fn a_demotion_revokes_nothing_retroactively_but_the_next_issuance_refuses() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    let issued = issue(&mut plane, &id, CELL, start())?;
    let granted = issued.envelope().clone();

    // A loss past the condition the pilot gate recorded, an hour later.
    let later = start().saturating_add(Duration::from_hours(1));
    let outcome = CellOutcome::new(id.clone(), CELL, later, good_returns(5, 30, 0.0018))
        .with_realised_loss(dec!("30000"));
    let report = plane.learn(&[outcome], None, later)?;

    let learning = report
        .learnings
        .first()
        .ok_or_else(|| qip_core::Error::not_found("the outcome was reviewed"))?;
    assert!(
        !learning.review.triggers.is_empty(),
        "the realised loss passed a stated kill condition"
    );
    assert_eq!(learning.review.stage_after, GateStage::Shadow);
    assert_eq!(plane.factory().stage_of(&id), GateStage::Shadow);

    // Nothing was clawed back. The grant the cell holds is still live and still
    // bounds it, because a demotion at the centre reaches a cell no faster than
    // a message does — the expiry is what actually stops it.
    assert!(granted.is_live(later));
    assert_eq!(
        plane.envelope(CELL, &id).map(CapitalEnvelope::gross_limit),
        Some(granted.gross_limit())
    );
    // And the record of how it got there is untouched.
    assert!(plane.factory().ledger().reached(&id, GateStage::Pilot));
    assert!(
        plane
            .factory()
            .ledger()
            .admission_evidence(&id, GateStage::Pilot)
            .is_some_and(|outcome| outcome.passed)
    );

    // The next grant is refused, which is the part that is actually enforced.
    let refused = issue(&mut plane, &id, CELL, later).unwrap_err();
    assert!(
        refused.message().contains("holds no capital"),
        "issuance after a demotion should refuse on the stage: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn the_learn_edge_widens_a_strategys_error_bar_and_never_narrows_it() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    issue(&mut plane, &id, CELL, start())?;

    let submitted = plane
        .proposal(&id)
        .ok_or_else(|| qip_core::Error::not_found("the proposal was registered"))?
        .sharpe_standard_error;

    let later = start().saturating_add(Duration::from_hours(1));
    let outcome = CellOutcome::new(id.clone(), CELL, later, good_returns(77, 60, 0.006));
    let report = plane.learn(&[outcome], None, later)?;
    let learning = report
        .learnings
        .first()
        .ok_or_else(|| qip_core::Error::not_found("the outcome was reviewed"))?;

    assert!(
        learning.review.triggers.is_empty(),
        "live beat the baseline, so nothing should have tripped: {:?}",
        learning.review.triggers
    );
    assert!(learning.realised_sharpe > learning.expected_sharpe);
    assert_eq!(learning.verdict, LearningVerdict::Scale);
    assert_eq!(report.scaling_candidates().len(), 1);

    let widened = plane
        .proposal(&id)
        .ok_or_else(|| qip_core::Error::not_found("the proposal survived the update"))?
        .sharpe_standard_error;
    assert!(
        widened > submitted,
        "being wrong by more than the stated error bar should widen it: {submitted} -> {widened}"
    );

    // Scaling is a recommendation. The strategy is still at pilot until the
    // scaled gate has been walked and two more names collected.
    assert_eq!(plane.factory().stage_of(&id), GateStage::Pilot);
    Ok(())
}

#[test]
fn the_whole_walk_from_candidate_to_scaled_is_reconstructable_from_the_ledger() -> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Scaled)?;

    assert_eq!(
        plane.factory().path(&id),
        vec![
            GateStage::Candidate,
            GateStage::Holdout,
            GateStage::Paper,
            GateStage::Shadow,
            GateStage::Pilot,
            GateStage::Scaled,
        ],
        "no rung may be skipped, and shadow least of all"
    );

    let narration = plane.factory().narrate(&id);
    assert_eq!(narration.len(), 5, "one line per move: {narration:?}");
    for line in &narration[..3] {
        assert!(
            line.contains("no approver"),
            "the rungs below capital need none: {line}"
        );
    }
    for line in &narration[3..] {
        assert!(
            line.contains("alice.chen"),
            "every rung that can lose money names somebody: {line}"
        );
    }

    let ledger = plane.factory().ledger();
    for rung in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
        GateStage::Scaled,
    ] {
        let outcome = ledger.admission_evidence(&id, rung).ok_or_else(|| {
            qip_core::Error::not_found(format!("the {} gate's outcome was kept", rung.as_str()))
        })?;
        assert!(outcome.passed);
        assert!(
            !outcome.findings.is_empty(),
            "the {} gate recorded no checks",
            rung.as_str()
        );
    }
    Ok(())
}

#[test]
fn attaching_the_central_plane_leaves_a_cycle_exactly_as_it_was() -> Result<()> {
    let mut untouched = platform()?;
    let mut worked = platform()?;

    // Everything the central plane does, on one of the two platforms.
    let id = strategy();
    register(worked.central_mut(), &id, CELL)?;
    walk_to(worked.central_mut(), &id, GateStage::Pilot)?;
    let issued = issue(worked.central_mut(), &id, CELL, start())?;
    let dna = worked.central().ship(&issued, "central-plane", start())?;
    worked.central().verify_dna(&dna, start())?;
    worked.ingest_cell_report(
        CellReport::new(CELL, start()).with_positions(vec![position(
            CELL,
            &id,
            INSTRUMENT,
            dec!("10"),
        )]),
        start(),
    )?;

    untouched.observe(bars("AAA", 90));
    worked.observe(bars("AAA", 90));
    let expected = untouched.run_cycle(start());
    let actual = worked.run_cycle(start());

    assert_eq!(
        actual, expected,
        "the central plane changed a cycle it is not part of"
    );
    assert!(actual.traversed_every_stage());
    assert_eq!(
        actual
            .stages
            .iter()
            .map(|outcome| outcome.stage)
            .collect::<Vec<_>>(),
        Stage::all()
    );
    assert!(!actual.halted);
    Ok(())
}

// --- shared fixtures for the platform-level tests -----------------------------

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        if let Ok(object) = FinancialObject::builder(
            ObjectId::from_string(format!("obj-{symbol}")),
            symbol,
            InstrumentType::CommonStock,
        )
        .venue(VENUE)
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(DataProvenance::synthetic("test", start()))
        .build(start())
        {
            let _ = universe.insert(object);
        }
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("central-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            // Deterministic pseudo-noise plus a jump two thirds of the way in,
            // so the detectors have something real to find.
            let noise = ((i as f64 * 0.754_877_666_2) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: ObjectId::from_string(format!("obj-{symbol}")),
                venue: VENUE.to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).unwrap_or(Decimal::ONE),
                high: Decimal::from_f64(open.max(price) * 1.002).unwrap_or(Decimal::ONE),
                low: Decimal::from_f64(open.min(price) * 0.998).unwrap_or(Decimal::ONE),
                close: Decimal::from_f64(price).unwrap_or(Decimal::ONE),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

fn position(
    cell: &str,
    strategy: &StrategyId,
    instrument: &str,
    quantity: Decimal,
) -> CellPosition {
    CellPosition {
        cell: cell.to_string(),
        strategy: strategy.clone(),
        instrument: instrument.to_string(),
        sector: Sector::InformationTechnology,
        venue: venue(),
        currency: Currency::USD,
        quantity,
        price: dec!("100"),
    }
}

/// A signed grant and its approval, built without the central plane.
///
/// Used where a test needs a genuine [`qip_compliance::ApprovedCapital`] for a
/// strategy the plane would refuse to issue one for, so the only thing wrong
/// with the resulting bundle is the thing under test.
fn approved_grant_outside_the_plane(
    id: &StrategyId,
    cell: &str,
    now: Timestamp,
) -> Result<(
    SigningKey,
    CapitalEnvelope,
    qip_compliance::approval::ApprovedCapital,
)> {
    let secret = [3u8; 32];
    let key = SigningKey::from_secret("test-key", &secret)?;
    let issuer = EnvelopeIssuer::new(secret.to_vec(), "test-key")?;
    let approval = dual_approval(
        &capital_subject(id, cell),
        now,
        "granted directly for the purposes of this test",
    )?;

    let allocation = qip_capital::allocation::Allocation {
        strategy: id.clone(),
        cell: cell.to_string(),
        venue: venue(),
        notional: dec!("500000"),
        indicated: dec!("500000"),
        risk_adjusted_edge: 1.5,
        binding_constraints: Vec::new(),
    };
    let terms = EnvelopeTerms::from_allocation(&allocation, Duration::from_hours(8));
    let envelope = issuer.issue(&terms, &approval, now)?;

    let request = CapitalRequest {
        strategy: id.clone(),
        cell: cell.to_string(),
        gross_limit: envelope.gross_limit(),
        order_limit: envelope.order_limit(),
        loss_limit: envelope.loss_limit(),
        venues: terms.venues.clone(),
        expires_at: envelope.expires_at(),
        requested_by: "research.desk".to_string(),
    };
    let mut chain = ApprovalChain::new(Decimal::ZERO, key.clone())?;
    let approved = chain.grant(&request, &approval, &credentials(now)?, now)?;
    Ok((key, envelope, approved))
}

#[test]
fn a_platform_signing_with_a_reproducible_secret_says_so_in_its_own_report() -> Result<()> {
    // The platform has no ambient entropy and must not grow one, so a default
    // assembly derives its signing secret from the configured seed. That is
    // the right trade for replay and the wrong key for production: anyone who
    // knows the seed can mint an envelope.
    //
    // The failure this guards against is not the derivation. It is a
    // deployment that never supplied real key material and looks identical to
    // one that did — six controls enforced, nothing amiss. A report that
    // enumerated them while the secret sat in a config file would be accurate
    // and misleading, which is worse than being wrong.
    let platform = platform()?;
    assert!(
        platform.central().signing_key_is_reproducible(),
        "a default assembly should be honest that its key is derived"
    );

    let report = platform.compliance_report(start())?;
    // Still fully enforced: the signing control works, its key is simply not
    // one a production deployment should keep.
    report.require_fully_enforced()?;

    let signing_caveats: Vec<&str> = report
        .caveats()
        .into_iter()
        .filter(|(control, _)| *control == Control::SignedArtifactsAndProvenance)
        .map(|(_, caveat)| caveat)
        .collect();
    assert!(
        signing_caveats
            .iter()
            .any(|caveat| caveat.contains("reproducible") && caveat.contains("set_central")),
        "the report does not disclose the reproducible key, or does not say how to replace it: \
         {signing_caveats:?}"
    );

    // And the crate's own caveat survived alongside it — the honest gap the
    // compliance plane already recorded is not displaced by this one.
    assert!(
        signing_caveats.len() >= 2,
        "adding the key caveat dropped the ones the compliance plane recorded"
    );
    Ok(())
}
