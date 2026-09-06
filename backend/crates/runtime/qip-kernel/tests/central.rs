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
use qip_contracts::venue::{VenueClass, VenueId};
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
    ArbitragePolicy, BreakOrigin, CellOutcome, CellReport, CentralConfig, CentralPlane,
    DispositionInstruction, DispositionOutcome, DispositionRefused, IssuedCapital, LearningVerdict,
    ReconciliationBreak, RetirementDisposition, StrategyCandidate, StrategyDna, WhitelistIssue,
    WhitelistOutcome, WhitelistedMarket, WhitelistedVenue, capital_subject,
};
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, KillCondition, LeakageAudit, PaperEvidence,
    PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence, StrategyEvidence,
};
use qip_lifecycle::trials::StrategyFamily;
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
use std::collections::BTreeMap;

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
    let candidate = StrategyCandidate::new(
        compiled,
        program,
        StrategyFamily::new("central-tests")?,
        cell,
        venue(),
        start(),
    )?
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
            origin: BreakOrigin::Book,
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
///
/// One asymmetric break at a time, and the mirror only after the first has
/// been asserted on its own. An earlier version ingested both directions in
/// one report and asserted `over == 1 && under == 1`, which is also what a
/// swapped sign produces: anyone "correcting" `difference()` to
/// `external - cell` would have inverted every dashboard and left the suite
/// green.
#[test]
fn a_reconciliation_break_is_recorded_by_direction_and_the_halt_by_cause() -> Result<()> {
    let mut platform = platform()?;
    let id = strategy();
    let over = labels([("direction", "cell_over_venue")]);
    let under = labels([("direction", "venue_over_cell")]);
    let detail_only = labels([("direction", "detail_only")]);
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

    // The cell holds more than the venue confirms, and nothing else.
    let cell_over = CellReport::new(CELL, start())
        .with_positions(vec![position(CELL, &id, INSTRUMENT, dec!("10"))])
        .with_break(ReconciliationBreak {
            instrument: INSTRUMENT.to_string(),
            cell_quantity: dec!("10"),
            external_quantity: dec!("4"),
            detail: "six lots the venue has no record of".to_string(),
            origin: BreakOrigin::Book,
        });
    let ingestion = platform.ingest_cell_report(cell_over, start())?;
    assert_eq!(ingestion.halted, Some(HaltScope::Cell(CELL.to_string())));

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &over),
        1,
        "one break where the cell holds more than the venue confirms"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &under),
        0,
        "a cell-over-venue break must not be charted as its mirror"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &detail_only),
        0
    );
    assert_eq!(snapshot.counter(names::CENTRAL_CELL_HALTS, &halted), 1);

    // The mirror, from the other cell: the venue confirms more than the cell
    // holds. The first series must not move again.
    let venue_over = CellReport::new("cell-nyc-1", start())
        .with_positions(vec![position("cell-nyc-1", &id, INSTRUMENT, dec!("1"))])
        .with_break(ReconciliationBreak {
            instrument: "BBB".to_string(),
            cell_quantity: dec!("1"),
            external_quantity: dec!("3"),
            detail: "two lots the cell never booked".to_string(),
            origin: BreakOrigin::Book,
        });
    let ingestion = platform.ingest_cell_report(venue_over, start())?;
    assert_eq!(
        ingestion.halted,
        Some(HaltScope::Cell("cell-nyc-1".to_string()))
    );

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &under),
        1,
        "one break where the venue confirms more than the cell holds"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &over),
        1,
        "the mirror must not be charted as the first"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &detail_only),
        0
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_CELL_HALTS, &halted),
        2,
        "one scoped halt per halted cell, whatever the number of breaks behind it"
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

/// The third arm. A break whose quantities agree is still a break — the
/// discrepancy lives in the detail, a wrong venue or a wrong settlement date —
/// and it still halts the cell. Nothing exercised the arm before this, so
/// replacing it with either neighbour left the suite green.
#[test]
fn a_break_with_equal_quantities_is_recorded_as_detail_only_and_still_halts() -> Result<()> {
    let mut platform = platform()?;
    let id = strategy();
    let detail_only = labels([("direction", "detail_only")]);
    let over = labels([("direction", "cell_over_venue")]);
    let under = labels([("direction", "venue_over_cell")]);
    let halted = labels([("cause", "reconciliation")]);

    let reconciliation_break = ReconciliationBreak {
        instrument: INSTRUMENT.to_string(),
        cell_quantity: dec!("10"),
        external_quantity: dec!("10"),
        detail: "the venue books the lot for T+1 and the cell for T+2".to_string(),
        origin: BreakOrigin::Book,
    };
    // Premise: the quantities agree, so this is the arm neither sign selects.
    assert_eq!(reconciliation_break.difference(), Decimal::ZERO);

    let report = CellReport::new(CELL, start())
        .with_positions(vec![position(CELL, &id, INSTRUMENT, dec!("10"))])
        .with_break(reconciliation_break);
    let ingestion = platform.ingest_cell_report(report, start())?;
    assert_eq!(ingestion.halted, Some(HaltScope::Cell(CELL.to_string())));
    assert!(platform.autonomy().kill_switch().is_halted(CELL));

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &detail_only),
        1,
        "a break with agreeing quantities is charted as detail-only"
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &over),
        0
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_RECONCILIATION_BREAKS, &under),
        0
    );
    assert_eq!(
        snapshot.counter(names::CENTRAL_CELL_HALTS, &halted),
        1,
        "agreeing quantities do not make a break less of a halt"
    );
    Ok(())
}

/// `recall_acknowledgement` was a public field with a default and no check.
/// At zero, the recall register refused the first concentration recall — but
/// the refusal surfaced inside `ingest`, after a reconciliation break had
/// already halted the cell and raised the incident, and it propagated out of
/// the one call meant to record the halt. Refused at construction instead,
/// naming the field, on both the plane and the platform that carries it.
#[test]
fn a_zero_recall_acknowledgement_window_is_refused_at_construction() -> Result<()> {
    let config = CentralConfig {
        recall_acknowledgement: Duration::ZERO,
        ..CentralConfig::default()
    };
    // Premise: the default itself is accepted, so the refusal is the window's.
    CentralPlane::new(&[7u8; 32], CentralConfig::default())?;

    let Err(error) = CentralPlane::new(&[7u8; 32], config.clone()) else {
        panic!("a plane with no recall window should not assemble");
    };
    let message = error.to_string();
    assert!(
        message.contains("recall_acknowledgement"),
        "the refusal should name the field: {message}"
    );

    let platform_config = PlatformConfig::default().with_central(config);
    let (context, _clock) = Context::deterministic(start(), platform_config.seed);
    let Err(error) = Platform::new(
        platform_config,
        context,
        Telemetry::silent(),
        universe(),
        limits(),
    ) else {
        panic!("a platform carrying a zero recall window should not start");
    };
    assert!(
        error.to_string().contains("recall_acknowledgement"),
        "the platform's refusal should be the plane's: {error}"
    );
    Ok(())
}

/// The plane a deployment builds arrives through `set_central`, after the
/// platform already owns the registry. If the swap did not attach it, every
/// deployed ledger would count its rungs into nothing while the reproducible
/// plane the tests use counted fine — a silence that begins exactly when the
/// real key arrives. This test walks the swapped-in plane, not the default.
#[test]
fn a_swapped_in_central_plane_counts_its_rungs_into_the_platform_registry() -> Result<()> {
    let mut platform = platform()?;
    platform.set_central(plane()?);
    let id = strategy();
    register(platform.central_mut(), &id, CELL)?;
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(names::STRATEGY_PROMOTIONS),
        0,
        "registration is not a rung"
    );

    walk_to(platform.central_mut(), &id, GateStage::Shadow)?;
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow
    );
    platform.central_mut().factory_mut().demote(
        &id,
        GateStage::Paper,
        "test",
        "looked wrong",
        start(),
    )?;

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::STRATEGY_PROMOTIONS,
            &labels([("from", "paper"), ("to", "shadow")])
        ),
        1
    );
    assert_eq!(snapshot.counter_total(names::STRATEGY_PROMOTIONS), 3);
    assert_eq!(
        snapshot.counter(
            names::STRATEGY_DEMOTIONS,
            &labels([("from", "shadow"), ("to", "paper")])
        ),
        1
    );
    for series in snapshot
        .series
        .iter()
        .filter(|s| s.name == names::STRATEGY_PROMOTIONS || s.name == names::STRATEGY_DEMOTIONS)
    {
        assert!(
            !series.help.is_empty(),
            "{} exports undescribed",
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
    // Live beats the baseline by more than the stated error bar and stays
    // inside the band the holdout validation defined. The drift used to be
    // twice this, which put the live Sharpe at twenty against a band topping
    // out near ten — and a strategy performing far *above* its validation is
    // not the strategy that was validated, which is why the band trips in
    // either direction. The property under test is the learn edge widening
    // the error bar on a strategy that did what it said, not the band.
    let outcome = CellOutcome::new(id.clone(), CELL, later, good_returns(77, 60, 0.0030));
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

// --- the cycle whitelist: slot 8 of the shipping payload -----------------------

/// A policy trading `AAA` against `USD` at each of `venues`, funded in `USD`
/// by the strategy the ladder tests issue a grant to.
fn arbitrage_policy(venues: &[&str]) -> ArbitragePolicy {
    ArbitragePolicy {
        strategy: strategy(),
        funding_instrument: "USD".to_string(),
        venues: venues
            .iter()
            .map(|venue| {
                (
                    venue.to_string(),
                    WhitelistedVenue {
                        class: VenueClass::Exchange,
                        taker_cost: dec!("0.0005"),
                    },
                )
            })
            .collect(),
        markets: venues
            .iter()
            .map(|venue| WhitelistedMarket {
                venue: venue.to_string(),
                market: format!("AAA-USD@{venue}"),
                base: "AAA".to_string(),
                quote: "USD".to_string(),
            })
            .collect(),
        start_sizes: BTreeMap::from([("AAA".to_string(), dec!("100"))]),
    }
}

fn plane_with_arbitrage(policy: ArbitragePolicy) -> Result<CentralPlane> {
    CentralPlane::new(
        &[7u8; 32],
        CentralConfig {
            arbitrage: Some(policy),
            ..CentralConfig::default()
        },
    )
}

/// The cell's installer reads an empty whitelist as
/// `Installation::EmptyWhitelist` and installs no desk. That is the state a
/// deployment is in until an operator sets `CentralConfig::arbitrage`, and
/// this test is the statement that the default says so rather than shipping
/// the slot unproduced and leaving the operator to infer it.
#[test]
fn an_unset_arbitrage_policy_emits_an_empty_whitelist_that_says_why() -> Result<()> {
    let plane = plane()?;
    // Premise: the default carries no policy.
    assert!(plane.config().arbitrage.is_none());
    let issue = plane.cycle_whitelist_for(CELL, start())?;
    assert_eq!(issue.outcome, WhitelistOutcome::NoPolicy);
    assert!(issue.is_empty(), "{}", issue.describe());
    assert!(issue.whitelist.start_sizes.is_empty());

    // A policy with no live grant for its strategy at the cell is the other
    // empty case: nothing sizes the funding instrument, and the cell's
    // installer would decline with no envelope regardless.
    let plane = plane_with_arbitrage(arbitrage_policy(&[VENUE]))?;
    let issue = plane.cycle_whitelist_for(CELL, start())?;
    assert_eq!(
        issue.outcome,
        WhitelistOutcome::NoLiveGrant {
            strategy: strategy()
        }
    );
    assert!(issue.is_empty(), "{}", issue.describe());
    Ok(())
}

/// The grant the ladder issues permits one venue. A policy trading at a
/// second is refused where the whitelist is made, naming the venue — not at
/// the cell, whose `graph_from_whitelist` would refuse the whole whitelist
/// and say so only in its delta stream.
#[test]
fn a_policy_venue_the_grant_does_not_permit_is_refused_at_production_not_at_the_cell() -> Result<()>
{
    let now = start();
    let id = strategy();

    // Premise: with only the granted venue, the same grant emits.
    let mut plane = plane_with_arbitrage(arbitrage_policy(&[VENUE]))?;
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    let issued = issue(&mut plane, &id, CELL, now)?;
    assert!(
        issued.envelope().permits_venue(&venue())
            && !issued.envelope().permits_venue(&VenueId::new("XLON")),
        "the ladder grants one venue"
    );
    let accepted = plane.cycle_whitelist_for(CELL, now)?;
    assert_eq!(
        accepted.outcome,
        WhitelistOutcome::Emitted {
            edges: 2,
            sized_against: issued.envelope().signature().to_string()
        },
        "{}",
        accepted.describe()
    );
    assert_eq!(
        accepted.whitelist.start_sizes.get("USD"),
        Some(&issued.envelope().order_limit())
    );

    let mut plane = plane_with_arbitrage(arbitrage_policy(&[VENUE, "XLON"]))?;
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    issue(&mut plane, &id, CELL, now)?;
    let Err(error) = plane.cycle_whitelist_for(CELL, now) else {
        panic!("a venue the grant does not permit should not reach a whitelist");
    };
    let message = error.to_string();
    assert!(
        message.contains("XLON") && message.contains("does not permit"),
        "the refusal should name the venue: {message}"
    );
    Ok(())
}

/// A market naming a venue the policy does not describe has no class and no
/// cost, so no conversion could be made from it. Refused when the plane
/// assembles, naming the market, rather than when the first payload ships.
#[test]
fn a_market_at_an_undescribed_venue_is_refused_when_the_plane_assembles() -> Result<()> {
    let mut policy = arbitrage_policy(&[VENUE]);
    // Premise: the policy is accepted before the market is added.
    plane_with_arbitrage(policy.clone())?;
    policy.markets.push(WhitelistedMarket {
        venue: "XPAR".to_string(),
        market: "AAA-USD@XPAR".to_string(),
        base: "AAA".to_string(),
        quote: "USD".to_string(),
    });
    let Err(error) = plane_with_arbitrage(policy) else {
        panic!("a market at an undescribed venue should not assemble");
    };
    let message = error.to_string();
    assert!(
        message.contains("XPAR") && message.contains("CentralConfig::arbitrage"),
        "the refusal should name the venue and the field: {message}"
    );
    Ok(())
}

/// The slot's digest is over its serialised bytes, and `conversions` and
/// `start_sizes` are skipped when empty so old signatures still verify. The
/// other direction has to hold too: a payload carrying a produced whitelist
/// must survive the wire and verify at the cell with every conversion intact.
#[test]
fn a_signed_payload_carrying_the_whitelist_round_trips_and_verifies() -> Result<()> {
    use qip_contracts::policy::{PolicyPayload, Slot};
    use qip_core::hash::to_hex;
    use qip_core::hmac_sha256;

    let now = start();
    let id = strategy();
    let mut plane = plane_with_arbitrage(arbitrage_policy(&[VENUE]))?;
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    issue(&mut plane, &id, CELL, now)?;
    let emitted = plane.cycle_whitelist_for(CELL, now)?;
    // Premise: there is something to carry.
    assert!(!emitted.is_empty(), "{}", emitted.describe());

    let key = [9u8; 32];
    let mut payload = PolicyPayload::unproduced(1, CELL, now);
    payload.cycle_whitelist = Slot::produced(emitted.whitelist.clone(), now);
    let signed = payload.signed(&key)?;

    let wire = serde_json::to_string(&signed)?;
    assert!(
        wire.contains("\"conversions\"") && wire.contains("\"start_sizes\""),
        "the produced fields must reach the wire"
    );
    let received: PolicyPayload = serde_json::from_str(&wire)?;
    assert_eq!(
        received.cycle_whitelist.value(),
        Some(&emitted.whitelist),
        "every conversion and size survives the wire"
    );
    // Verified the way `qip_edge::policy::VerifiedPolicy::verify` does — the
    // kernel cannot depend on the edge, so the check is recomputed here.
    let expected = to_hex(&hmac_sha256(&key, received.signing_payload()?.as_bytes()));
    assert_eq!(
        received.signature, expected,
        "the signature verifies after the round trip"
    );

    // And a whitelist altered in flight does not: the slot digest is in the
    // signing payload, so one changed cost is a different payload.
    let mut altered = received.clone();
    let mut whitelist = emitted.whitelist.clone();
    whitelist.conversions[0].cost_fraction = dec!("0.5");
    altered.cycle_whitelist = Slot::produced(whitelist, now);
    let recomputed = to_hex(&hmac_sha256(&key, altered.signing_payload()?.as_bytes()));
    assert_ne!(altered.signature, recomputed);
    Ok(())
}

/// A whitelist that reached a cell with no record at the centre would be a
/// permission reproducible from nothing. The platform's entry point journals
/// every issue — including the empty ones, which are the fact an operator
/// asking why the desk never installs needs to find.
#[test]
fn issuing_a_whitelist_through_the_platform_journals_what_was_issued() -> Result<()> {
    use qip_events::{EventFilter, Topic};

    let now = start();
    let id = strategy();
    let config = PlatformConfig::default().with_central(CentralConfig {
        arbitrage: Some(arbitrage_policy(&[VENUE])),
        ..CentralConfig::default()
    });
    let (context, _clock) = Context::deterministic(now, config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    // Premise: nothing has been distributed yet.
    let distributed = EventFilter::new().topic(Topic::PolicyDistributed);
    assert!(platform.replay_journal(&distributed)?.is_empty());

    let empty = platform.issue_cycle_whitelist(CELL, now)?;
    assert_eq!(
        empty.outcome,
        WhitelistOutcome::NoLiveGrant {
            strategy: id.clone()
        }
    );

    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    issue(platform.central_mut(), &id, CELL, now)?;
    let emitted = platform.issue_cycle_whitelist(CELL, now)?;
    assert!(!emitted.is_empty(), "{}", emitted.describe());

    let recorded = platform.replay_journal(&distributed)?;
    assert_eq!(recorded.len(), 2, "both issues were journaled");
    let bodies: Vec<WhitelistIssue> = recorded
        .iter()
        .map(|event| {
            event
                .decode::<WhitelistIssue>()
                .map(|envelope| envelope.body)
        })
        .collect::<Result<_>>()?;
    assert_eq!(bodies, vec![empty, emitted]);
    Ok(())
}

// --- §35.2: a retirement dispositions the strategy's positions -----------------

/// One order the cell sent for the strategy alone, and the venue's fill of
/// it attributed wholly to that strategy — the shortest path to a lot in the
/// centre's books.
fn strategy_order_and_fill(
    id: &StrategyId,
    order_id: &str,
    side: qip_contracts::message::BookSide,
    quantity: Decimal,
    price: Decimal,
    at: Timestamp,
) -> (qip_mesh::delta::DeltaOrder, qip_contracts::wire::FillRecord) {
    let order = qip_mesh::delta::DeltaOrder {
        order_id: order_id.to_string(),
        strategy: id.clone(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: venue(),
        side,
        quantity,
        price,
        simulated: true,
        contributors: vec![qip_contracts::intent::Contributor {
            strategy: id.clone(),
            signed_size: if side == qip_contracts::message::BookSide::Ask {
                quantity
            } else {
                -quantity
            },
            inputs: vec![("book_pressure".to_string(), 1)],
        }],
    };
    let fill = qip_contracts::wire::FillRecord {
        order_id: order_id.to_string(),
        object_id: ObjectId::from_string(INSTRUMENT),
        venue: venue(),
        side,
        quantity,
        price,
        simulated: true,
        at,
        shares: vec![qip_contracts::wire::FillShare {
            strategy: id.clone(),
            quantity,
        }],
    };
    (order, fill)
}

/// Live returns with the drift gone: the same series the lifecycle suite
/// retires on, so the review trips decay and nothing else.
fn decayed_returns() -> Vec<f64> {
    good_returns(11, 60, -0.0002)
}

/// Drive a pilot strategy off capital by decay and then, the default ninety
/// days later and still decaying, to retirement — through `learn_from_cells`
/// alone, exactly as the LEARN edge would. Returns the retiring report.
fn retire_by_decay(platform: &mut Platform, id: &StrategyId) -> Result<LearningReportAt> {
    let demoted_at = start().saturating_add(Duration::from_days(60));
    let demoting = platform.learn_from_cells(
        &[CellOutcome::new(
            id.clone(),
            CELL,
            demoted_at,
            decayed_returns(),
        )],
        demoted_at,
    )?;
    assert_eq!(
        platform.central().factory().stage_of(id),
        GateStage::Shadow,
        "premise: decay demoted the strategy off capital: {:?}",
        demoting.learnings.first().map(|l| &l.review.triggers)
    );
    assert!(
        demoting.dispositions.is_empty(),
        "premise: a demotion that is not a retirement dispositions nothing: {:?}",
        demoting.dispositions
    );

    let retired_at = demoted_at.saturating_add(Duration::from_days(90));
    let retiring = platform.learn_from_cells(
        &[CellOutcome::new(
            id.clone(),
            CELL,
            retired_at,
            decayed_returns(),
        )],
        retired_at,
    )?;
    assert_eq!(
        platform.central().factory().stage_of(id),
        GateStage::Retired,
        "premise: sustained decay at the floor retired the strategy: {:?}",
        retiring.learnings.first().map(|l| &l.review.triggers)
    );
    Ok(LearningReportAt {
        report: retiring,
        retired_at,
    })
}

struct LearningReportAt {
    report: qip_kernel::central::LearningReport,
    retired_at: Timestamp,
}

/// Blueprint §35.2: "on retirement, each position is ... scheduled for
/// unwinding". Before this, `DemotionMonitor::enforce` retired the strategy
/// through the ledger and its lot stayed in the books under a strategy that
/// no longer existed at any rung — the orphan the blueprint calls a
/// reconciliation break — with nothing in the log saying so.
#[test]
fn an_automatic_retirement_schedules_every_lot_the_strategy_holds_for_unwinding_and_journals_it()
-> Result<()> {
    use qip_contracts::message::BookSide;
    use qip_events::{EventFilter, Topic};

    let mut platform = platform()?;
    let id = strategy();
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;

    // The strategy buys a hundred at fifty, and the centre attributes it.
    let (order, fill) = strategy_order_and_fill(
        &id,
        "ord-retire-1",
        BookSide::Ask,
        dec!("100"),
        dec!("50"),
        start(),
    );
    let ingestion = platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        start(),
    )?;
    assert!(
        ingestion.settlement.refused.is_empty(),
        "premise: the fill settled: {:?}",
        ingestion.settlement.refused
    );
    let held = platform
        .central()
        .strategy_lot(CELL, &id, INSTRUMENT)
        .copied()
        .ok_or_else(|| qip_core::Error::not_found("the strategy's lot"))?;
    assert_eq!(
        (held.quantity, held.average_price),
        (dec!("100"), dec!("50")),
        "premise: the attribution holds the lot"
    );
    assert!(
        platform.central().scheduled_unwinds().is_empty(),
        "premise: a lot held by a strategy at a rung is not scheduled for anything"
    );
    let positions = EventFilter::new().topic(Topic::PositionUpdated);
    assert!(
        platform.replay_journal(&positions)?.is_empty(),
        "premise: nothing about positions has been journaled yet"
    );

    let LearningReportAt { report, retired_at } = retire_by_decay(&mut platform, &id)?;

    // One disposition, for the retired strategy, naming the one lot.
    assert_eq!(report.dispositions.len(), 1, "{:?}", report.dispositions);
    let DispositionOutcome::Dispositioned(disposition) = &report.dispositions[0] else {
        panic!(
            "the attribution names the lot, so nothing was there to refuse: {:?}",
            report.dispositions[0]
        );
    };
    assert_eq!(disposition.strategy, id);
    assert_eq!(disposition.retired_at, retired_at);
    assert!(
        disposition.rationale.contains("retirement threshold"),
        "the record carries the ledger's own rationale: {}",
        disposition.rationale
    );
    let keys: Vec<&String> = disposition.positions.keys().collect();
    assert_eq!(keys, vec![&format!("{CELL}/{INSTRUMENT}")]);
    let lot = &disposition.positions[&format!("{CELL}/{INSTRUMENT}")];
    assert_eq!(
        (
            lot.cell.as_str(),
            lot.instrument.as_str(),
            lot.quantity,
            lot.average_price
        ),
        (CELL, INSTRUMENT, dec!("100"), dec!("50"))
    );
    // The instruction flattens: a hundred long is sold a hundred, through
    // the cell's own path, and nothing here is an order.
    assert_eq!(
        lot.instruction,
        DispositionInstruction::Unwind {
            flatten_by: dec!("-100")
        }
    );

    // The same record is in the log, decodable, and equal to what the
    // report said — the disposition is reproducible from the log alone.
    let journaled = platform.replay_journal(&positions)?;
    assert_eq!(journaled.len(), 1, "one disposition, journaled once");
    let replayed = journaled[0].decode::<RetirementDisposition>()?.body;
    assert_eq!(&replayed, disposition);

    // And the lot is now listed as awaiting its unwind, from the ledger and
    // the books rather than from any schedule kept beside them.
    let scheduled = platform.central().scheduled_unwinds();
    assert_eq!(
        scheduled
            .get(&id)
            .and_then(|lots| lots.get(&format!("{CELL}/{INSTRUMENT}"))),
        Some(&dec!("-100"))
    );

    // The cell flattens it — a fill the venue confirmed — and the schedule
    // empties by the same arithmetic that moved the lot.
    let later = retired_at.saturating_add(Duration::from_hours(1));
    let (order, fill) = strategy_order_and_fill(
        &id,
        "ord-retire-2",
        BookSide::Bid,
        dec!("100"),
        dec!("52"),
        later,
    );
    platform.ingest_cell_report(
        CellReport::new(CELL, later)
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        later,
    )?;
    assert!(
        platform.central().scheduled_unwinds().is_empty(),
        "the lot was flattened, so nothing is left to unwind: {:?}",
        platform.central().scheduled_unwinds()
    );
    Ok(())
}

/// The other half of §35.2's answer: a position with no owner is a
/// reconciliation break. When the cell's own book and the attribution
/// disagree about what the retired strategy holds, the centre does not
/// schedule an unwind for either number; it records the disagreement, and
/// that record is what the desk reconciles from.
#[test]
fn a_retirement_whose_lots_the_cells_book_and_the_attribution_disagree_on_is_refused_not_guessed()
-> Result<()> {
    use qip_contracts::message::BookSide;
    use qip_events::{EventFilter, Topic};

    let mut platform = platform()?;
    let id = strategy();
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;

    // The venue filled a hundred; the cell's book says sixty.
    let (order, fill) = strategy_order_and_fill(
        &id,
        "ord-retire-3",
        BookSide::Ask,
        dec!("100"),
        dec!("50"),
        start(),
    );
    platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_positions(vec![position(CELL, &id, INSTRUMENT, dec!("60"))])
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        start(),
    )?;
    assert_eq!(
        platform
            .central()
            .strategy_lot(CELL, &id, INSTRUMENT)
            .map(|lot| lot.quantity),
        Some(dec!("100")),
        "premise: the attribution holds a hundred"
    );
    assert_eq!(
        platform
            .central()
            .reported_positions()
            .map(|p| p.quantity)
            .sum::<Decimal>(),
        dec!("60"),
        "premise: the cell's book claims sixty"
    );

    let LearningReportAt { report, retired_at } = retire_by_decay(&mut platform, &id)?;

    assert_eq!(report.dispositions.len(), 1, "{:?}", report.dispositions);
    let DispositionOutcome::Refused(refusal) = &report.dispositions[0] else {
        panic!(
            "two claims that disagree must be refused, not dispositioned: {:?}",
            report.dispositions[0]
        );
    };
    assert_eq!(refusal.strategy, id);
    assert_eq!(refusal.retired_at, retired_at);
    let discrepancy = refusal
        .discrepancies
        .get(&format!("{CELL}/{INSTRUMENT}"))
        .ok_or_else(|| qip_core::Error::not_found("the disagreeing lot"))?;
    assert_eq!(
        (discrepancy.attributed, discrepancy.reported),
        (dec!("100"), dec!("60"))
    );
    assert!(
        refusal
            .describe()
            .contains("attributed 100, cell reports 60"),
        "{}",
        refusal.describe()
    );

    // The refusal is its own record, and no unwind instruction was written
    // for either number.
    let refusals =
        platform.replay_journal(&EventFilter::new().topic(Topic::ReconciliationCompleted))?;
    assert_eq!(refusals.len(), 1, "the refusal was journaled once");
    assert_eq!(&refusals[0].decode::<DispositionRefused>()?.body, refusal);
    assert!(
        platform
            .replay_journal(&EventFilter::new().topic(Topic::PositionUpdated))?
            .is_empty(),
        "no disposition was guessed"
    );
    Ok(())
}

/// A retired strategy that holds nothing is still recorded as such: the
/// absence of a record would read the same as a retirement nobody
/// dispositioned, and the log is where the desk checks.
#[test]
fn a_retired_strategy_holding_no_lot_is_dispositioned_as_holding_nothing_and_that_is_journaled()
-> Result<()> {
    use qip_events::{EventFilter, Topic};

    let mut platform = platform()?;
    let id = strategy();
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    assert!(
        platform
            .central()
            .strategy_books()
            .keys()
            .all(|(_, owner, _)| owner != &id),
        "premise: the strategy holds nothing anywhere"
    );

    let LearningReportAt { report, retired_at } = retire_by_decay(&mut platform, &id)?;

    assert_eq!(
        report.dispositions.len(),
        1,
        "a retirement with nothing held is still dispositioned: {:?}",
        report.dispositions
    );
    let DispositionOutcome::Dispositioned(disposition) = &report.dispositions[0] else {
        panic!("nothing to disagree about: {:?}", report.dispositions[0]);
    };
    assert_eq!(disposition.strategy, id);
    assert_eq!(disposition.retired_at, retired_at);
    assert!(disposition.positions.is_empty());
    let journaled = platform.replay_journal(&EventFilter::new().topic(Topic::PositionUpdated))?;
    assert_eq!(journaled.len(), 1);
    assert_eq!(
        &journaled[0].decode::<RetirementDisposition>()?.body,
        disposition
    );
    Ok(())
}

// --- §20.3 through the cycle: the LEARN stage reviews what the cells realised --

/// One session's round trip for the strategy: `quantity` bought at par and
/// sold at par plus whatever moves the day's attributed P&L to `pnl`.
fn round_trip(
    id: &StrategyId,
    day: usize,
    quantity: Decimal,
    pnl: Decimal,
    at: Timestamp,
) -> Result<CellReport> {
    use qip_contracts::message::BookSide;
    let entry = dec!("100");
    let exit = entry
        + pnl
            .checked_div(quantity)
            .ok_or_else(|| qip_core::Error::numeric("a positive quantity divides any P&L"))?;
    let (buy, bought) = strategy_order_and_fill(
        id,
        &format!("ord-session-{day}-buy"),
        BookSide::Ask,
        quantity,
        entry,
        at,
    );
    let (sell, sold) = strategy_order_and_fill(
        id,
        &format!("ord-session-{day}-sell"),
        BookSide::Bid,
        quantity,
        exit,
        at,
    );
    Ok(CellReport::new(CELL, at)
        .with_orders(vec![buy, sell])
        .with_fills(vec![bought, sold]))
}

/// The cycle's own record of what its LEARN stage reviewed, decoded from the
/// log rather than read off the returned report.
fn journaled_reviews(
    platform: &Platform,
) -> Result<Vec<Option<qip_kernel::platform::StrategyReviewJournal>>> {
    use qip_events::{EventFilter, Topic};
    platform
        .replay_journal(&EventFilter::new().topic(Topic::LearningCompleted))?
        .iter()
        .map(|event| {
            event
                .decode::<qip_kernel::platform::CycleJournalEntry>()
                .map(|envelope| envelope.body.strategy_review)
        })
        .collect()
}

/// Blueprint §20.3, "retirement is as automated as promotion", proven
/// through the cycle and the ingest path alone — nothing here calls
/// `learn_from_cells`, `learn`, `review` or `retire`. Before this, every one
/// of those was reached only by a test: `stage_learn` never called the
/// strategy review, no composition root did, and a strategy could decay at
/// the floor for a year in a deployed `qip-api` with the trigger written to
/// catch it never once evaluated. The series the review reads is the
/// centre's own attribution of the fills the cell reported, one session per
/// day, and the LEARN stage's record says how many it reviewed, demoted,
/// retired and dispositioned so the outcome is reproducible from the log.
#[test]
fn the_learn_stage_retires_a_strategy_whose_cells_realised_sustained_decay_and_journals_its_disposition()
-> Result<()> {
    use qip_contracts::message::BookSide;
    use qip_events::{EventFilter, Topic};
    use qip_kernel::platform::StrategyReviewJournal;

    let mut platform = platform()?;
    let id = strategy();
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    let issued = issue(platform.central_mut(), &id, CELL, start())?;
    let capital = issued.envelope().gross_limit();
    assert!(
        capital.is_positive(),
        "premise: the grant has a gross limit for a return to be a fraction of"
    );
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Pilot,
        "premise: the strategy holds capital"
    );
    // A tenth of the grant per leg at par, so a day's return of a percent
    // or so moves the exit price by a few units rather than off the scale.
    let quantity = capital
        .checked_div(dec!("1000"))
        .ok_or_else(|| qip_core::Error::numeric("a thousand divides any grant"))?;

    // Sixty closed sessions in decay: each day's attributed P&L is the day's
    // decayed return on the grant, made by a round trip the venue filled.
    let returns = decayed_returns();
    assert!(
        returns.len() >= 20,
        "premise: enough sessions for decay to be judged at all"
    );
    for (day, realised) in returns.iter().enumerate() {
        let at = start().saturating_add(Duration::from_days(day as i64));
        // The test crosses from the f64 return it wants to the Decimal P&L
        // the fills must realise; the platform under test crosses back.
        let pnl = Decimal::from_f64(realised * capital.to_f64())
            .ok_or_else(|| qip_core::Error::numeric("a finite return"))?;
        let ingestion =
            platform.ingest_cell_report(round_trip(&id, day, quantity, pnl, at)?, at)?;
        assert!(
            ingestion.settlement.refused.is_empty(),
            "premise: session {day} settled: {:?}",
            ingestion.settlement.refused
        );
        assert_eq!(
            ingestion.settlement.fills_settled, 2,
            "premise: both legs of session {day} were billed"
        );
    }
    // Then a lot left open, so the retirement has something to disposition.
    let opened_at = start().saturating_add(Duration::from_days(returns.len() as i64));
    let (order, fill) = strategy_order_and_fill(
        &id,
        "ord-session-open",
        BookSide::Ask,
        quantity,
        dec!("100"),
        opened_at,
    );
    platform.ingest_cell_report(
        CellReport::new(CELL, opened_at)
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        opened_at,
    )?;
    assert_eq!(
        platform
            .central()
            .strategy_lot(CELL, &id, INSTRUMENT)
            .map(|lot| lot.quantity),
        Some(quantity),
        "premise: the attribution holds the open lot"
    );
    let positions = EventFilter::new().topic(Topic::PositionUpdated);
    assert!(
        platform.replay_journal(&positions)?.is_empty(),
        "premise: nothing about positions has been journaled by ingest"
    );

    // Cycle one, the day after the last session closed: decay is judged on
    // the closed sessions and the strategy is pushed off capital.
    let demoting_at = opened_at.saturating_add(Duration::from_days(1));
    let demoting = platform.run_cycle(demoting_at);
    let learn = demoting
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow,
        "the LEARN stage demoted the strategy on the sessions its cell realised: {}",
        learn.detail
    );
    assert!(
        learn
            .detail
            .contains("1 strategy(ies) reviewed on realised sessions (1 demoted, 0 retired"),
        "the stage says what its review did: {}",
        learn.detail
    );
    assert!(
        learn.problems.is_empty(),
        "the review ran clean: {:?}",
        learn.problems
    );

    // Cycle two, the retirement threshold later and still decaying: retired
    // without a human, and the open lot scheduled for unwinding.
    let retiring_at = demoting_at.saturating_add(Duration::from_days(90));
    let retiring = platform.run_cycle(retiring_at);
    let learn = retiring
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Retired,
        "the LEARN stage retired the strategy after sustained decay at the floor; \
         the stage said: {}",
        learn.detail
    );
    assert!(
        learn
            .detail
            .contains("1 strategy(ies) reviewed on realised sessions (0 demoted, 1 retired, 1 dispositioned, 0 disposition(s) refused, 0 skipped)"),
        "the stage says what its review did: {}",
        learn.detail
    );

    // The disposition is in the log, from the cycle and nothing else.
    let journaled = platform.replay_journal(&positions)?;
    assert_eq!(journaled.len(), 1, "one retirement, dispositioned once");
    let disposition = journaled[0].decode::<RetirementDisposition>()?.body;
    assert_eq!(disposition.strategy, id);
    assert_eq!(disposition.retired_at, retiring_at);
    assert!(
        disposition.rationale.contains("retirement threshold"),
        "the record carries the ledger's own rationale: {}",
        disposition.rationale
    );
    assert_eq!(
        disposition
            .positions
            .get(&format!("{CELL}/{INSTRUMENT}"))
            .map(|lot| lot.instruction),
        Some(DispositionInstruction::Unwind {
            flatten_by: -quantity
        })
    );

    // And the cycle's own entries carry the counts, so the two reviews are
    // reproducible from the journal without the returned reports.
    assert_eq!(
        journaled_reviews(&platform)?,
        vec![
            Some(StrategyReviewJournal {
                reviewed: 1,
                demoted: 1,
                retired: 0,
                dispositioned: 0,
                dispositions_refused: 0,
                skipped: 0,
            }),
            Some(StrategyReviewJournal {
                reviewed: 1,
                demoted: 0,
                retired: 1,
                dispositioned: 1,
                dispositions_refused: 0,
                skipped: 0,
            }),
        ]
    );

    // A retired strategy is finished with: the next cycle reviews nothing
    // and its entry says so by carrying no review at all.
    let after = platform.run_cycle(retiring_at.saturating_add(Duration::from_days(1)));
    let learn = after
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        !learn.detail.contains("reviewed on realised sessions"),
        "a retired strategy's sessions are not reviewed again: {}",
        learn.detail
    );
    assert_eq!(journaled_reviews(&platform)?.last(), Some(&None));
    Ok(())
}

// --- ADR 0039: the share a cell's grant manifest carries -----------------------

#[test]
fn a_cells_manifest_names_only_grants_whose_gross_fits_its_share() -> Result<()> {
    // The half of ADR 0039 the plan-only suite cannot reach: the manifest a
    // cell is shipped names the grants the centre holds live for it, and is
    // withheld — not trimmed, not shipped anyway — when those grants already
    // sum past the cell's share under the current plan. A manifest that
    // named them regardless would have the cell derive a share the
    // partitioner never produced.
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    let issued = issue(&mut plane, &id, CELL, start())?;
    let envelope = issued.envelope().clone();
    let plan = plane.allocate(0.0, start())?;
    assert_eq!(
        plan.for_cell(CELL),
        envelope.gross_limit(),
        "the premise: the envelope was issued against this plan's gross for the cell"
    );
    let region = "europe-west2".to_string();
    let membership = qip_kernel::central::RegionMembership::new(
        BTreeMap::from([(region.clone(), plan.for_cell(CELL))]),
        BTreeMap::from([(CELL.to_string(), region.clone())]),
    )?;

    let shares = plane.region_shares(&plan, &membership, start())?;
    let share = shares
        .for_cell(CELL)
        .unwrap_or_else(|| panic!("the cell was withheld a share: {:?}", shares.withheld()));
    assert_eq!(share.region(), region);
    assert_eq!(share.amount(), plan.for_cell(CELL));
    assert_eq!(
        share.live_grants(),
        &[envelope.signature().to_string()],
        "the manifest did not name exactly the issued grant"
    );
    assert_eq!(share.named_gross(), envelope.gross_limit());
    assert!(share.named_gross() <= share.amount());
    assert_eq!(share.manifest().live_grants, share.live_grants());

    // A narrower plan — the allocator under a drawdown, say — gives the cell
    // less than its live grant already admits. The cell is withheld, with
    // the reason, rather than shipped a manifest naming a grant its share
    // cannot cover.
    let narrower = qip_capital::allocation::AllocationPlan {
        allocations: plan
            .allocations
            .iter()
            .cloned()
            .map(|mut allocation| {
                allocation.notional -= Decimal::ONE;
                allocation
            })
            .collect(),
        ..plan.clone()
    };
    assert!(
        narrower.for_cell(CELL) < envelope.gross_limit(),
        "the premise: the narrower plan is below the live grant"
    );
    let withheld = plane.region_shares(&narrower, &membership, start())?;
    assert!(
        withheld.for_cell(CELL).is_none(),
        "a manifest was shipped naming grants past the cell's share: {:?}",
        withheld.for_cell(CELL)
    );
    let reason = withheld
        .withheld()
        .get(CELL)
        .unwrap_or_else(|| panic!("the cell was neither shared nor withheld with a reason"));
    assert!(
        reason.contains("past its share") && reason.contains("renewed"),
        "the reason did not say what to do instead: {reason}"
    );
    // And once the grant has expired it no longer counts against the share:
    // the cell is shipped an empty manifest, which its table reads as
    // nothing, rather than being withheld forever on a dead grant.
    let later = envelope.expires_at();
    let after = plane.region_shares(&narrower, &membership, later)?;
    let expired = after.for_cell(CELL).unwrap_or_else(|| {
        panic!(
            "the cell was withheld on an expired grant: {:?}",
            after.withheld()
        )
    });
    assert!(expired.live_grants().is_empty());
    assert_eq!(expired.named_gross(), Decimal::ZERO);
    Ok(())
}

#[test]
fn the_centres_manifests_for_a_regions_cells_never_together_exceed_its_grant_and_each_payload_carries_its_own()
-> Result<()> {
    // The producer's call, end to end at the centre: two cells of one
    // region, each holding a grant this plane issued, and the manifests
    // `grant_manifests` decides for them from the plan it sizes itself. What
    // a cell will derive from its manifest is the gross of the grants it
    // names, so the property is that the two manifests' gross sums to at
    // most the region's grant — and that when it cannot, nothing ships.
    use qip_contracts::policy::{PolicyPayload, Slot};
    let mut plane = plane()?;
    let first = strategy();
    let second = StrategyId::new("central-momentum-2");
    const SECOND_CELL: &str = "cell-lon-2";
    register(&mut plane, &first, CELL)?;
    register(&mut plane, &second, SECOND_CELL)?;
    walk_to(&mut plane, &first, GateStage::Pilot)?;
    walk_to(&mut plane, &second, GateStage::Pilot)?;
    let first_envelope = issue(&mut plane, &first, CELL, start())?.envelope().clone();
    let second_envelope = issue(&mut plane, &second, SECOND_CELL, start())?
        .envelope()
        .clone();
    let plan = plane.allocate(0.0, start())?;
    assert_eq!(
        plan.for_cell(CELL),
        first_envelope.gross_limit(),
        "the premise: the first grant was issued against this plan's gross for its cell"
    );
    assert_eq!(
        plan.for_cell(SECOND_CELL),
        second_envelope.gross_limit(),
        "the premise: the second grant was issued against this plan's gross for its cell"
    );
    let together = plan.for_cell(CELL) + plan.for_cell(SECOND_CELL);
    assert!(
        together.is_positive(),
        "the premise: the plan allocates to both cells"
    );
    let region = "europe-west2";
    let cells = [CELL, SECOND_CELL, "cell-nyc-9"];
    let membership = qip_kernel::central::RegionMembership::parse(&format!(
        "{region}={together}:{CELL},{SECOND_CELL}"
    ))?;
    assert!(
        membership.covering(cells).is_err(),
        "the premise: the third cell is in no region"
    );

    let manifests = plane.grant_manifests(cells, &membership, 0.0, start());
    let mut named_gross = Decimal::ZERO;
    for (cell, envelope) in [(CELL, &first_envelope), (SECOND_CELL, &second_envelope)] {
        let share = match manifests.for_cell(cell) {
            Some(qip_kernel::central::ManifestDecision::Ship(share)) => share,
            other => panic!("{cell} was not shipped a share: {other:?}"),
        };
        assert_eq!(share.region(), region);
        assert_eq!(
            share.live_grants(),
            &[envelope.signature().to_string()],
            "{cell}'s manifest did not name exactly its own grant"
        );
        named_gross += share.named_gross();
        // The slot as the producer places it: a produced manifest naming
        // the grant, on a payload addressed to the cell.
        let mut payload = PolicyPayload::unproduced(1, cell, start());
        let manifest = manifests
            .for_cell(cell)
            .and_then(qip_kernel::central::ManifestDecision::manifest)
            .unwrap_or_else(|| panic!("{cell}'s decision carries no manifest"));
        payload.capital_grants = Slot::produced(manifest, start());
        assert_eq!(
            payload
                .capital_grants
                .value()
                .map(|manifest| manifest.live_grants.clone()),
            Some(vec![envelope.signature().to_string()]),
            "{cell}'s payload does not carry its manifest"
        );
    }
    assert!(
        named_gross <= together,
        "the manifests together name {named_gross} of grants against a grant of {together}"
    );
    match manifests.for_cell("cell-nyc-9") {
        Some(qip_kernel::central::ManifestDecision::Withhold(reason)) => {
            assert!(reason.contains("in no region"), "{reason}");
        }
        other => panic!("a cell in no region was decided as {other:?}"),
    }

    // A grant one unit short of what the plan allocates: the plan is
    // refused whole and neither cell ships a manifest — not the first cell
    // alone, not a scaled pair — with the refusal on each.
    let short = qip_kernel::central::RegionMembership::parse(&format!(
        "{region}={}:{CELL},{SECOND_CELL}",
        together - Decimal::ONE
    ))?;
    let withheld = plane.grant_manifests(cells, &short, 0.0, start());
    for cell in [CELL, SECOND_CELL] {
        match withheld.for_cell(cell) {
            Some(qip_kernel::central::ManifestDecision::Withhold(reason)) => assert!(
                reason.contains("could not be partitioned") && reason.contains("past its grant"),
                "{cell}'s withholding does not carry the refusal: {reason}"
            ),
            other => panic!("{cell} was shipped a share under a grant the plan exceeds: {other:?}"),
        }
        assert_eq!(
            withheld
                .for_cell(cell)
                .and_then(qip_kernel::central::ManifestDecision::manifest),
            None,
            "{cell} was given a manifest under a refused plan"
        );
    }
    Ok(())
}
