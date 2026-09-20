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
//! One test says how nearly additive the module is: the same cycle, stage for
//! stage, on a platform whose central plane has been used and one whose has
//! not — with a small enumerated set of exceptions, each written out in that
//! test as a substitution so the assertion stays an equality rather than being
//! excused by comparing less.
//!
//! **No count is given here, because this sentence said "one exception" while
//! the test carried two.** ADR 0064's family review was the first; ADR 0066's
//! cadence added a second on 2026-09-14 and this line was not amended; §49.1's
//! objective reader added a third on 2026-09-19. The exceptions are the
//! substitutions in
//! `attaching_the_central_plane_changes_no_stage_but_the_family_review_the_learn_stage_now_runs`
//! and reading them is the only way to know how many there are.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::allocation::{Allocation, AllocationPlan, StrategyProposal};
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
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance as DataProvenance};
use qip_financial::universe::Universe;
use qip_kernel::central::darkness::{RegionSpokeAgain, RegionWentDark};
use qip_kernel::central::{
    ArbitragePolicy, BreakOrigin, CellOutcome, CellReport, CentralConfig, CentralPlane,
    DispositionInstruction, DispositionOutcome, DispositionRefused, IssuedCapital, LearningVerdict,
    ReconciliationBreak, RetirementDisposition, StrategyCandidate, StrategyDna, WhitelistIssue,
    WhitelistOutcome, WhitelistedMarket, WhitelistedVenue, capital_subject,
};
use qip_kernel::central::{HorizonArming, HorizonClaim, HorizonPolicy};
use qip_kernel::central::{ManifestDecision, RegionMembership, RegionShare, RegionTransition};
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_lifecycle::evidence::{
    CrossValidationRun, DatasetManifest, FeatureTiming, HoldoutEvidence, KillCondition,
    LeakageAudit, PaperEvidence, PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence,
    StrategyEvidence,
};
use qip_lifecycle::trials::StrategyFamily;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_optimization_engine::horizons::Horizon;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use std::collections::BTreeMap;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

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

/// A manifest wide enough for every hand-built holdout in this suite: the
/// gate checks the series and its folds fit inside the bars it names.
fn fixture_manifest() -> Result<DatasetManifest> {
    DatasetManifest::new(
        "obj-AAA",
        VENUE,
        2_000,
        start().saturating_sub(Duration::from_days(2_000)),
        start(),
        qip_core::sha256_hex(b"fixture bars"),
    )
}

fn full_evidence(id: &StrategyId, cell: &str) -> Result<StrategyEvidence> {
    Ok(StrategyEvidence::new()
        .with_holdout(strong_holdout()?)
        .with_simulation(fixture_manifest()?)
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

/// Until ADR 0064 this was `attaching_the_central_plane_leaves_a_cycle_exactly_as_it_was`
/// and asserted whole-report equality. That claim is now false by design, for
/// exactly one clause of exactly one stage: the LEARN stage reviews the
/// families the factory has registered, so a plane holding a candidate reports
/// a standing and an empty plane reports nothing.
///
/// The assertion is narrowed rather than dropped, and narrowed by enumerating
/// the permitted difference rather than by comparing less. Seven stages must
/// still match outcome for outcome; the LEARN stage must match in everything
/// but a suffix, and that suffix must be the family review and nothing else —
/// so a change that perturbed the counterfactual pass, the rule review or the
/// attribution would still fail here. The name says what it asserts, because
/// a test called "exactly as it was" that no longer checks that is a false
/// statement in the one place a reader looks first.
#[test]
fn attaching_the_central_plane_changes_no_stage_but_the_family_review_the_learn_stage_now_runs()
-> Result<()> {
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

    assert_eq!(actual.cycle, expected.cycle);
    assert_eq!(actual.correlation_id, expected.correlation_id);
    assert_eq!(actual.halted, expected.halted);
    for (actual_stage, expected_stage) in actual.stages.iter().zip(expected.stages.iter()) {
        assert_eq!(actual_stage.stage, expected_stage.stage);
        if actual_stage.stage != Stage::Learn {
            assert_eq!(
                actual_stage, expected_stage,
                "the central plane changed the {:?} stage, which it is not part of",
                actual_stage.stage
            );
        }
    }
    let actual_learn = actual
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    let expected_learn = expected
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert_eq!(actual_learn.produced, expected_learn.produced);
    assert_eq!(actual_learn.problems, expected_learn.problems);
    // The premise the whole comparison rests on: the empty plane says nothing
    // about families, so the suffix below is the difference and not part of a
    // line both platforms write.
    assert!(
        !expected_learn
            .detail
            .contains("reviewed against funding standing"),
        "the untouched platform reviewed a family, so there is no difference to attribute: {}",
        expected_learn.detail
    );
    // The second permitted difference, added 2026-09-14 with ADR 0066's
    // cadence. A platform with a central plane has a registered strategy
    // family and one without does not, so the cadence holds the evaluation-tier
    // census for a *different stated reason* — "this is cycle 1" against "no
    // strategy is registered under a family". Both are saving cycles and
    // neither runs any work; what differs is the sentence naming why, which is
    // the cadence correctly reporting a real difference in its subject rather
    // than the central plane leaking into a second decision.
    //
    // It is substituted here rather than matched loosely, so this assertion
    // stays an equality: a third difference, or a change in either clause,
    // still fails. Relaxing it to `contains` would have retired the property
    // the test exists for.
    let expected_with_a_registered_family = expected_learn.detail.replace(
        "no strategy is registered under a family, so there is nothing to tier",
        "the evaluation-tier census runs on one cycle in 12 and this is cycle 1",
    );
    assert_ne!(
        expected_with_a_registered_family, expected_learn.detail,
        "the premise failed: the cadence clause this substitution accounts for is not in the \
         detail, so the equality below would be comparing something else"
    );
    // The third permitted difference, added 2026-09-19 with §49.1's reader.
    // The LEARN stage now closes with a line saying where the blueprint's own
    // targets stand, and it differs between the two platforms for a reason
    // that is the reader working rather than the plane leaking: `worked` has
    // absorbed a cell report, so §49.1's reconciliation objective rests on an
    // observation, and `untouched` has absorbed nothing, so it rests on none.
    // One met and fourteen unobserved against nothing met and fifteen
    // unobserved is exactly the distinction the reader exists to make, and a
    // reader that printed the same sentence on both would be the one this
    // repository refuses.
    //
    // Substituted rather than appended, because the §49.1 clause is last on
    // both details and the family review sits in front of it. Writing it as
    // one replacement keeps the assertion below an equality: a fourth
    // difference, or any change to either clause, still fails.
    let permitted = expected_with_a_registered_family.replace(
        "§49.1: 0 of 15 objective(s) met, 0 missed, 15 unobserved (2 fed by this process)",
        "1 strategy family(ies) reviewed against funding standing, 1 of them funded \
         [central-tests (1 member(s), 1 funded)]; the review allocates nothing; §49.1: 1 of 15 \
         objective(s) met, 0 missed, 14 unobserved (2 fed by this process)",
    );
    assert_ne!(
        permitted, expected_with_a_registered_family,
        "the premise failed: the §49.1 clause this substitution accounts for is not in the \
         detail, so the equality below would be comparing something else"
    );
    assert_eq!(
        actual_learn.detail, permitted,
        "the central plane changed the LEARN stage by more than the family review it now runs, \
         the cadence's own reason for saving, and the §49.1 objective the absorbed cell report \
         put an observation under"
    );
    // One extra record on the log, and one only: the family review's.
    assert_eq!(
        actual.events_logged,
        expected.events_logged + 1,
        "the central plane wrote a record beyond the one family review"
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
            fixture_liquidity(),
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

/// A platform whose centre names `venues` in its arbitrage policy — the
/// configured half of what the centre knows a cell may trade at.
fn platform_with_arbitrage(venues: &[&str]) -> Result<Platform> {
    let config = PlatformConfig::default().with_central(CentralConfig {
        arbitrage: Some(arbitrage_policy(venues)),
        ..CentralConfig::default()
    });
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// A cell report carrying one refusal the lot gate made at `venue`, as the
/// delta sink would build it.
fn report_with_lot_refusal(venue: &str) -> CellReport {
    report_with_lot_refusal_from(CELL, venue)
}

/// `count` refusals under the withdrawn-venue gate at `venue`, all on one
/// report — what one pass of a desk installed before a withdrawal puts on
/// the wire once `qip_edge::feasibility::assess` reads slot 11, one refusal
/// per intent it still offers through the withdrawn venue.
///
/// `count` is a parameter because the two things a caller wants to say are
/// different facts: `1` is a cell still routing there, and `8` is a pass's
/// whole intent fan-out, which is what must not buy eight window seats.
fn report_with_withdrawn_venue_refusals(cell: &str, venue: &str, count: usize) -> CellReport {
    CellReport::new(cell, start()).with_refusals(
        (0..count)
            .map(|_| qip_mesh::delta::DeltaRefusal {
                gate: qip_contracts::feasibility::GATE_WITHDRAWN_VENUE.to_string(),
                reason: format!("{venue} is withdrawn on feasibility evidence"),
                venue: Some(venue.to_string()),
            })
            .collect(),
    )
}

/// The same report, from a named cell rather than the fixed [`CELL`] —
/// what a second, genuinely distinct cell corroborating the same venue looks
/// like on the wire.
fn report_with_lot_refusal_from(cell: &str, venue: &str) -> CellReport {
    CellReport::new(cell, start()).with_refusals(vec![qip_mesh::delta::DeltaRefusal {
        gate: "feasibility_lot".to_string(),
        reason: "10.5 is not a whole number of lots".to_string(),
        venue: Some(venue.to_string()),
    }])
}

fn feasibility_refusals_under(platform: &Platform, venue: &str, constraint: &str) -> u64 {
    platform.telemetry().metrics.snapshot().counter(
        names::FEASIBILITY_REFUSALS,
        &labels([("venue", venue), ("constraint", constraint)]),
    )
}

/// A traceable off-lot buy on `AAA`, which the desk's lot gate refuses
/// before any other control — one feasibility refusal at the desk's venue.
fn refuse_off_lot(platform: &mut Platform, n: usize) -> Result<()> {
    let order = platform.order_from(
        ObjectId::from_string("obj-AAA"),
        qip_execution_engine::order::Side::Buy,
        dec!("10.5"),
        dec!("100"),
        &format!("prop-off-lot-{n}"),
        vec![format!("hyp-off-lot-{n}")],
        start(),
    );
    let error = platform
        .submit_order(order, start())
        .expect_err("ten and a half shares of a one-lot listing reached the venue");
    assert!(
        error.message().contains("infeasible (feasibility_lot):"),
        "the premise failed: refused for another reason than the lot grid: {}",
        error.message()
    );
    Ok(())
}

fn withdrawals(platform: &Platform) -> Result<Vec<qip_kernel::venue_review::VenueWithdrawal>> {
    use qip_events::{EventFilter, Topic};
    platform
        .replay_journal(&EventFilter::new().topic(Topic::VenueWithdrawn))?
        .iter()
        .map(|envelope| {
            Ok(envelope
                .decode::<qip_kernel::venue_review::VenueWithdrawal>()?
                .body)
        })
        .collect()
}

#[test]
fn ten_feasibility_refusals_of_which_eight_are_on_one_venue_withdraw_it_and_the_next_order_there_is_refused()
-> Result<()> {
    // Blueprint §12.3's fourth row, end to end on the desk seam. Eight desk
    // refusals at the desk's one broker and two cell refusals at the policy
    // venue share one window; LEARN finds the desk venue at eight of ten,
    // journals the withdrawal *before* withdrawing, and the next on-grid
    // order — one the premise proves is otherwise accepted — is refused
    // under `venue-availability` with the reinstatement path named. The
    // cell venue, at two of ten, is untouched.
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    // Premise: an on-grid order is accepted, so the refusal at the end is
    // the withdrawal and not some control that refuses every order.
    let on_grid = platform.order_from(
        ObjectId::from_string("obj-AAA"),
        qip_execution_engine::order::Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-on-grid-before",
        vec!["hyp-on-grid".to_string()],
        start(),
    );
    platform.submit_order(on_grid, start())?;
    assert!(platform.withdrawn_venues().is_empty());
    assert!(withdrawals(&platform)?.is_empty());

    for n in 0..8 {
        refuse_off_lot(&mut platform, n)?;
    }
    for _ in 0..2 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), start())?;
    }
    let window = platform.feasibility_refusals();
    assert_eq!(window.len(), 10, "the premise is a window of ten");
    assert_eq!(
        window
            .iter()
            .filter(|refusal| refusal.venue == "simulated-venue")
            .count(),
        8
    );

    let cycle = platform.run_cycle(start());
    let learn = cycle.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn
            .detail
            .contains("venue simulated-venue withdrawn on feasibility evidence (8 of 10"),
        "LEARN did not report the withdrawal: {}",
        learn.detail
    );
    let records = withdrawals(&platform)?;
    assert_eq!(records.len(), 1, "the withdrawal is not on the record");
    assert_eq!(records[0].venue, "simulated-venue");
    assert_eq!(records[0].constraint, "feasibility_lot");
    assert_eq!((records[0].sample, records[0].count), (10, 8));
    assert!((records[0].share - 0.8).abs() < f64::EPSILON);
    assert_eq!(
        records[0].seams,
        vec![qip_kernel::venue_review::FeasibilitySeam::Desk]
    );
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"]
    );

    // The consequence at the desk seam.
    let after = platform.order_from(
        ObjectId::from_string("obj-AAA"),
        qip_execution_engine::order::Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-on-grid-after",
        vec!["hyp-on-grid".to_string()],
        start(),
    );
    let error = platform
        .submit_order(after, start())
        .expect_err("an on-grid order reached a withdrawn venue");
    assert!(
        error
            .message()
            .contains("withdrawn on feasibility evidence")
            && error.message().contains("two operator signatures"),
        "the refusal does not name the withdrawal or the way back: {}",
        error.message()
    );
    assert_eq!(
        platform.telemetry().metrics.snapshot().counter(
            names::ORDERS_REFUSED,
            &labels([("control", "venue-availability")])
        ),
        1,
        "the refusal is not charted under the venue-availability control"
    );

    // Reviewed again on the next cycle, the same window withdraws nothing
    // more: the desk venue is already withdrawn and the cell venue is two
    // of ten. One record, not one per cycle.
    platform.run_cycle(start());
    assert_eq!(withdrawals(&platform)?.len(), 1);
    assert_eq!(platform.withdrawn_venues().len(), 1);
    Ok(())
}

#[test]
fn ten_refusals_purely_for_a_venue_withdrawal_do_not_move_the_instruments_sizing_confidence()
-> Result<()> {
    // A code-review finding on ADR 0062 crossing ADR 0055: `capture_
    // submission` pushed every refusal onto the declined queue
    // unconditionally, including `RefusalReason::VenueUnavailable`, and
    // `counterfactual_sizing_multiplier` groups what the twin priced by
    // instrument alone, with no gate filter. Once a venue is withdrawn,
    // every further order routed there is refused this way, every cycle,
    // for as long as it stays withdrawn — an administrative fact about the
    // venue's reachability, not a judgment about whether the order was
    // well sized — and ten such refusals, easily reached within a cycle or
    // two of a withdrawal, would otherwise flood the instrument's
    // declined-score sample and could halve its sizing confidence for a
    // reason no rule found.
    let mut platform = platform_with_arbitrage(&[VENUE])?;

    // Withdraw the desk's one broker on ten off-lot refusals — legitimate,
    // single-source evidence ADR 0062 accepts alone, and untouched by this
    // fix (`Malformed` stays sizing evidence; only the withdrawal refusals
    // that follow are the ones under test).
    for n in 0..10 {
        refuse_off_lot(&mut platform, n)?;
    }
    platform.run_cycle(start());
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"],
        "the premise failed: the desk's venue was not withdrawn"
    );
    let declined_before = platform.declined_awaiting_score();

    // A different instrument from the one the off-lot refusals used, so its
    // declined-score sample starts genuinely empty and cannot be confused
    // with the evidence that triggered the withdrawal above.
    let object_id = ObjectId::from_string("obj-BBB");
    assert_eq!(
        platform.sizing_confidence(object_id.as_str(), start())?,
        Decimal::ONE,
        "the premise failed: sizing was already narrowed before any refusal on this instrument"
    );

    for n in 0..10 {
        let order = platform.order_from(
            object_id.clone(),
            qip_execution_engine::order::Side::Buy,
            dec!("1000"),
            dec!("100"),
            &format!("prop-withdrawn-{n}"),
            vec![format!("hyp-withdrawn-{n}")],
            start(),
        );
        let error = platform
            .submit_order(order, start())
            .expect_err("an order to a withdrawn venue was accepted");
        assert!(
            error
                .message()
                .contains("withdrawn on feasibility evidence"),
            "the refusal is not the venue withdrawal: {}",
            error.message()
        );
    }
    assert_eq!(
        platform.declined_awaiting_score(),
        declined_before,
        "a venue-withdrawal refusal was queued for the twin"
    );
    assert_eq!(
        platform.sizing_confidence(object_id.as_str(), start())?,
        Decimal::ONE,
        "ten refusals for venue withdrawal alone narrowed the instrument's sizing confidence"
    );
    Ok(())
}

#[test]
fn a_window_dominated_by_a_venue_the_policy_does_not_name_changes_no_whitelist() -> Result<()> {
    // Why evidence can never add a venue, shown from the other side: the
    // desk's broker is not a policy venue, so a cluster on it withdraws the
    // desk's broker and leaves the cells' whitelist exactly as the policy
    // and the grant produced it — nothing omitted, nothing added, and the
    // journaled issue names no withdrawal. The withdrawn set is read only
    // to `retain`; a venue it names that the policy does not is a venue the
    // whitelist never carried.
    let now = start();
    let id = strategy();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    issue(platform.central_mut(), &id, CELL, now)?;
    let before = platform.issue_cycle_whitelist(CELL, now)?;
    let WhitelistOutcome::Emitted {
        edges: 2,
        withdrawn,
        ..
    } = &before.outcome
    else {
        panic!("the premise failed: {}", before.describe());
    };
    assert!(withdrawn.is_empty());

    for n in 0..10 {
        refuse_off_lot(&mut platform, n)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec!["simulated-venue"],
        "the premise failed: the desk venue was not withdrawn"
    );

    let after = platform.issue_cycle_whitelist(CELL, now)?;
    assert_eq!(
        after.outcome,
        before.outcome,
        "a withdrawal of a venue the policy does not name changed the whitelist's outcome: {}",
        after.describe()
    );
    assert_eq!(after.whitelist.conversions, before.whitelist.conversions);
    assert!(
        after
            .whitelist
            .conversions
            .iter()
            .all(|conversion| conversion.venue == VENUE),
        "a conversion names a venue the policy does not"
    );
    Ok(())
}

#[test]
fn ten_refusals_from_one_cell_do_not_withdraw_the_only_policy_venue() -> Result<()> {
    // The security fix: before it, this exact scenario — ten reports from
    // one cell registration, naming one venue — withdrew the platform's only
    // policy venue on the evidence of a single, unauthenticated reporter.
    // The cell→centre uplink authenticates nobody (`qip-api/src/mesh.rs`,
    // `qip-edge/src/mesh.rs`), so one compromised or spoofed cell process
    // could deny the venue to every other cell and the desk. Corroboration
    // from a second distinct cell is now required before edge-only evidence
    // clears the bar (`VENUE_WITHDRAWAL_MIN_CELLS`), so the same ten reports
    // from the same one cell withdraw nothing: no record is journaled and
    // the whitelist the centre next issues is exactly what it was before.
    let now = start();
    let id = strategy();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    issue(platform.central_mut(), &id, CELL, now)?;
    let before = platform.issue_cycle_whitelist(CELL, now)?;
    assert!(
        !before.is_empty(),
        "the premise failed: the grant emits no whitelist"
    );

    for _ in 0..10 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
    }
    assert_eq!(
        platform.feasibility_refusals().len(),
        10,
        "the premise failed: the refusals did not reach the window"
    );
    platform.run_cycle(now);
    assert_eq!(
        withdrawals(&platform)?,
        Vec::new(),
        "a single cell's uncorroborated evidence withdrew a venue"
    );
    assert!(
        platform.withdrawn_venues().is_empty(),
        "a single cell's uncorroborated evidence withdrew a venue"
    );

    let after = platform.issue_cycle_whitelist(CELL, now)?;
    assert_eq!(
        after.outcome,
        before.outcome,
        "the whitelist moved on evidence from one cell alone: {}",
        after.describe()
    );
    Ok(())
}

#[test]
fn ten_cell_refusals_from_two_distinct_cells_at_the_only_policy_venue_withdraw_it_and_the_whitelist_says_so()
-> Result<()> {
    // The admitting half of the same fix, so it isn't just a refusal:
    // corroboration from a second, genuinely distinct cell still withdraws
    // the venue exactly as ten refusals from one cell did before the fix.
    // Six from `CELL` and four from a second registration together clear
    // both the sample/share bars and the two-cell corroboration bar, and
    // the next whitelist the centre issues for either cell is empty and
    // says why — `AllWithdrawn`, on the journaled issue — so the installer
    // installs nothing. Reviewed once: the venue is withdrawn on the first
    // cycle and the second finds nothing new.
    const OTHER_CELL: &str = "cell-fra-1";
    let now = start();
    let id = strategy();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    issue(platform.central_mut(), &id, CELL, now)?;
    assert!(
        !platform.issue_cycle_whitelist(CELL, now)?.is_empty(),
        "the premise failed: the grant emits no whitelist"
    );

    for _ in 0..6 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
    }
    for _ in 0..4 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    let records = withdrawals(&platform)?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].venue, VENUE);
    assert_eq!(
        records[0].seams,
        vec![qip_kernel::venue_review::FeasibilitySeam::Edge]
    );

    let issue = platform.issue_cycle_whitelist(CELL, now)?;
    assert_eq!(
        issue.outcome,
        WhitelistOutcome::AllWithdrawn {
            venues: vec![VENUE.to_string()]
        },
        "{}",
        issue.describe()
    );
    assert!(issue.is_empty(), "a conversion survived the withdrawal");
    Ok(())
}

#[test]
fn a_refusal_naming_a_venue_no_grant_permits_is_counted_unknown_and_kept_out_of_the_window()
-> Result<()> {
    // The refusal case first. A cell that ships a refusal at a venue neither
    // the arbitrage policy nor any live grant names is a cell reporting a
    // venue the centre never told it about. Admitting it would let a delta
    // mint a `venue` label and, ten refusals later, withdraw a name nothing
    // permitted in the first place — a withdrawal nobody could explain. It
    // is counted under the `unknown` literal, and the window does not move.
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    assert!(
        platform.feasibility_refusals().is_empty(),
        "the premise failed: the window is not empty before any report"
    );

    let ingestion = platform.ingest_cell_report(report_with_lot_refusal("XZZZ"), start())?;
    assert!(
        ingestion.feasibility_refusals.is_empty(),
        "a refusal at an unpermitted venue was admitted: {:?}",
        ingestion.feasibility_refusals
    );
    assert_eq!(
        ingestion.feasibility_refusals_unattributed,
        vec![("unknown".to_string(), "feasibility_lot".to_string())]
    );
    assert!(
        platform.feasibility_refusals().is_empty(),
        "a refusal at an unpermitted venue reached the window"
    );
    assert_eq!(
        feasibility_refusals_under(&platform, "unknown", "feasibility_lot"),
        1
    );
    assert_eq!(
        feasibility_refusals_under(&platform, "XZZZ", "feasibility_lot"),
        0,
        "the cell's own venue string became a label"
    );

    // And a gate outside the feasibility vocabulary, at a known venue, is
    // counted under `other` and admitted no further: only a feasibility
    // gate says anything about a venue.
    let posture =
        CellReport::new(CELL, start()).with_refusals(vec![qip_mesh::delta::DeltaRefusal {
            gate: "halted".to_string(),
            reason: "the cell is halted".to_string(),
            venue: Some(VENUE.to_string()),
        }]);
    let ingestion = platform.ingest_cell_report(posture, start())?;
    assert!(ingestion.feasibility_refusals.is_empty());
    assert_eq!(
        ingestion.feasibility_refusals_unattributed,
        vec![(VENUE.to_string(), "other".to_string())]
    );
    assert!(platform.feasibility_refusals().is_empty());
    Ok(())
}

#[test]
fn an_edge_feasibility_refusal_travels_on_the_cell_report_and_lands_in_the_window() -> Result<()> {
    // The failure this guards: `CellReport` carried no refusals at all, so
    // the window blueprint §12.3's fourth row is judged over could hold the
    // desk's feasibility refusals and no cell's, and a venue every cell found
    // infeasible looked, from the centre, like a venue nobody had trouble
    // with. A refusal the lot gate made at a venue the policy names lands in
    // the window under the edge seam, at the report's instant, and is counted
    // under the same series the desk's refusals are.
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    assert!(platform.feasibility_refusals().is_empty());

    let ingestion = platform.ingest_cell_report(report_with_lot_refusal(VENUE), start())?;
    assert_eq!(ingestion.feasibility_refusals.len(), 1);
    assert!(ingestion.feasibility_refusals_unattributed.is_empty());

    let window = platform.feasibility_refusals();
    assert_eq!(
        window.len(),
        1,
        "the carried refusal did not land in the window"
    );
    assert_eq!(window[0].venue, VENUE);
    assert_eq!(window[0].constraint, "feasibility_lot");
    assert_eq!(
        window[0].seam,
        qip_kernel::venue_review::FeasibilitySeam::Edge
    );
    assert_eq!(
        window[0].cell,
        Some(CELL.to_string()),
        "the reporting cell's identity did not travel into the window, which is what \
         corroboration is checked against"
    );
    assert_eq!(window[0].at, start());
    assert_eq!(
        feasibility_refusals_under(&platform, VENUE, "feasibility_lot"),
        1
    );
    assert_eq!(
        feasibility_refusals_under(&platform, "unknown", "feasibility_lot"),
        0,
        "a refusal at a configured venue was counted as unknown"
    );
    Ok(())
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
            sized_against: issued.envelope().signature().to_string(),
            withdrawn: Vec::new(),
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

// --- §23.1 LEVEL 1: the corpus retains a grant, and LEARN measures on it -------

/// The retention gap, closed and proven at the seam that writes it: a day the
/// centre held a live grant and settled nothing is retained as a day of the
/// record, and a day after that grant lapsed is not.
///
/// Before this, `absorb` ran only on a settlement, so the first day left no
/// session at all and was indistinguishable afterwards from the second. The
/// difference matters because one is a return of zero — a fact — and the
/// other is not a return, and anything aligning strategies to one calendar
/// has to fill the gap or throw the day away unless the record keeps them
/// apart.
#[test]
fn a_day_under_a_live_grant_that_settled_nothing_is_retained_and_a_day_after_it_lapsed_is_not()
-> Result<()> {
    let mut plane = plane()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    let issued = issue(&mut plane, &id, CELL, start())?;
    let grant = issued.envelope().gross_limit();
    assert!(
        issued.envelope().is_live(start()),
        "premise: the grant is live at the instant it was issued"
    );

    // A report the cell sent while holding the grant, carrying no fill at all.
    let mut kill_switch = qip_risk_engine::autonomy::KillSwitch::new();
    let quiet = start().saturating_add(Duration::from_hours(1));
    plane.ingest(CellReport::new(CELL, quiet), &mut kill_switch, quiet)?;

    // And one three days later, by which time the eight-hour grant has lapsed
    // and nothing has re-issued it.
    let lapsed = start().saturating_add(Duration::from_days(3));
    assert!(
        !issued.envelope().is_live(lapsed),
        "premise: the grant has expired by the second report"
    );
    plane.ingest(CellReport::new(CELL, lapsed), &mut kill_switch, lapsed)?;

    let calendar = plane.realised_calendar(lapsed.saturating_add(Duration::from_days(1)));
    assert_eq!(
        calendar.day_count(),
        1,
        "one of the two days was under a grant the centre held live"
    );
    let day = start().start_of_day();
    let observed = calendar
        .by_strategy()
        .get(&id)
        .and_then(|days| days.get(&day))
        .copied()
        .ok_or_else(|| qip_core::Error::not_found("the granted day is in the calendar"))?;
    assert_eq!(observed.grant, grant, "the day carries the grant behind it");
    assert_eq!(
        observed.pnl,
        Decimal::ZERO,
        "and no P&L, because nothing settled"
    );
    assert_eq!(
        observed.fraction(),
        Some(0.0),
        "which is a return of zero, not an absence"
    );
    Ok(())
}

/// One session's fills for `id`, sized so the day's attributed P&L is `pnl`.
fn session_fills(
    id: &StrategyId,
    tag: &str,
    quantity: Decimal,
    pnl: Decimal,
    at: Timestamp,
) -> Result<(
    Vec<qip_mesh::delta::DeltaOrder>,
    Vec<qip_contracts::wire::FillRecord>,
)> {
    use qip_contracts::message::BookSide;
    let entry = dec!("100");
    let exit = entry
        + pnl
            .checked_div(quantity)
            .ok_or_else(|| qip_core::Error::numeric("a positive quantity divides any P&L"))?;
    let (buy, bought) = strategy_order_and_fill(
        id,
        &format!("ord-{tag}-buy"),
        BookSide::Ask,
        quantity,
        entry,
        at,
    );
    let (sell, sold) = strategy_order_and_fill(
        id,
        &format!("ord-{tag}-sell"),
        BookSide::Bid,
        quantity,
        exit,
        at,
    );
    Ok((vec![buy, sell], vec![bought, sold]))
}

/// Blueprint §23.1 LEVEL 1 through the cycle: the LEARN stage measures family
/// structure on the days the centre's own corpus retained a grant for, and
/// the cycle's journal carries what it found.
///
/// Nothing here calls `measure`, `family_structure` or the clustering stage.
/// The capability was complete, tested and unreachable, because the corpus
/// could not produce one series per strategy on one calendar: a day that
/// settled nothing left no session, so aligning three strategies to one
/// window would have meant inventing observations for the days they were
/// quiet. One of the three strategies here is quiet on every fifth session,
/// and it is clustered anyway — on a zero it actually earned, under a grant
/// the centre actually held.
#[test]
fn the_learn_stage_measures_family_structure_on_the_days_the_corpus_retained_a_grant() -> Result<()>
{
    use qip_kernel::central::CLUSTERING_WINDOW;

    // Three strategies at one cell, so the cell's headroom has to hold three
    // grants rather than the two the default budget leaves room for. Nothing
    // about the measurement depends on the figure; it is the allocator's
    // refusal, correctly applied, that would otherwise leave the third
    // strategy ungranted and the population too small to correlate.
    let mut config = PlatformConfig::default();
    config.central.per_cell = Decimal::from_int(9_000_000);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    let ids = [
        StrategyId::new("family-alpha"),
        StrategyId::new("family-beta"),
        StrategyId::new("family-gamma"),
    ];
    for id in &ids {
        register(platform.central_mut(), id, CELL)?;
        walk_to(platform.central_mut(), id, GateStage::Pilot)?;
    }

    let mut grants = Vec::new();
    for id in &ids {
        let issued = issue(platform.central_mut(), id, CELL, start())?;
        grants.push(issued.envelope().gross_limit());
    }
    let quantity = grants[0]
        .checked_div(dec!("1000"))
        .ok_or_else(|| qip_core::Error::numeric("a thousand divides any grant"))?;

    for session in 0..(CLUSTERING_WINDOW as i64) {
        let at = start().saturating_add(Duration::from_days(session));
        let mut orders = Vec::new();
        let mut fills = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            // The grant is re-issued each session, because an envelope lives
            // eight hours and a day under a lapsed one is not a day under a
            // grant.
            issue(platform.central_mut(), id, CELL, at)?;
            // Gamma trades on four sessions in five. The fifth is a day it
            // held the grant and made nothing.
            if index == 2 && session % 5 == 4 {
                continue;
            }
            // Alpha and beta track one factor, gamma another, so the
            // clustering has a structure to find rather than noise.
            let shape = if index < 2 {
                ((session % 11) as f64 - 5.0) * 0.002
            } else {
                ((session % 7) as f64 - 3.0) * 0.002
            };
            let wobble = ((session % 3) as f64 - 1.0) * 0.0003 * ((index + 1) as f64);
            let pnl = Decimal::from_f64((shape + wobble) * grants[index].to_f64())
                .ok_or_else(|| qip_core::Error::numeric("a finite return"))?;
            let (session_orders, session_fills) =
                session_fills(id, &format!("s{session}-{index}"), quantity, pnl, at)?;
            orders.extend(session_orders);
            fills.extend(session_fills);
        }
        let ingestion = platform.ingest_cell_report(
            CellReport::new(CELL, at)
                .with_orders(orders)
                .with_fills(fills),
            at,
        )?;
        assert!(
            ingestion.settlement.refused.is_empty(),
            "premise: session {session} settled: {:?}",
            ingestion.settlement.refused
        );
    }

    // The corpus, before any cycle reads it: three strategies aligned on a
    // window of closed sessions, including the sessions gamma was quiet on.
    let measuring_at = start().saturating_add(Duration::from_days(CLUSTERING_WINDOW as i64));
    let calendar = platform.central().realised_calendar(measuring_at);
    assert_eq!(
        calendar.day_count(),
        CLUSTERING_WINDOW,
        "premise: every session the desk held a grant on is a day of the calendar"
    );
    let quiet_day = start()
        .saturating_add(Duration::from_days(4))
        .start_of_day();
    assert_eq!(
        calendar
            .by_strategy()
            .get(&ids[2])
            .and_then(|days| days.get(&quiet_day))
            .map(|day| (day.pnl, day.fraction())),
        Some((Decimal::ZERO, Some(0.0))),
        "premise: a session gamma held the grant and settled nothing is a return of zero"
    );

    let report = platform.run_cycle(measuring_at);
    let learn = report
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        learn
            .detail
            .contains("3 strategy(ies) clustered into 2 family(ies)"),
        "the stage says what it measured: {}",
        learn.detail
    );
    assert!(
        learn.detail.contains(&format!(
            "on {CLUSTERING_WINDOW} closed session(s) (12 stress, 108 calm)"
        )),
        "and over how much, at the decile the window is cut at: {}",
        learn.detail
    );

    // And the cycle's own entry carries it, so the measurement is
    // reproducible from the log without the returned report.
    let journaled: Vec<Option<qip_kernel::central::FamilyStructureJournal>> = platform
        .replay_journal(
            &qip_events::EventFilter::new().topic(qip_events::Topic::LearningCompleted),
        )?
        .iter()
        .map(|event| {
            event
                .decode::<qip_kernel::platform::CycleJournalEntry>()
                .map(|envelope| envelope.body.family_structure)
        })
        .collect::<Result<Vec<_>>>()?;
    let measured = journaled
        .last()
        .and_then(|entry| *entry)
        .ok_or_else(|| qip_core::Error::not_found("the cycle journalled its measurement"))?;
    assert_eq!(measured.strategies, 3);
    assert_eq!(measured.sessions, CLUSTERING_WINDOW);
    assert_eq!(measured.stress_sessions, 12);
    assert_eq!(measured.calm_sessions, CLUSTERING_WINDOW - 12);
    assert_eq!(measured.excluded_unaligned, 0);
    assert_eq!(measured.excluded_flat, 0);
    assert_eq!(measured.pairs_total, 3);
    assert!(
        measured.mean_intra_family_correlation > measured.mean_inter_family_correlation,
        "the two strategies on one factor are filed together: intra {} inter {}",
        measured.mean_intra_family_correlation,
        measured.mean_inter_family_correlation
    );
    Ok(())
}

/// The other half of §23.1 LEVEL 1, and the state of every deployment as this
/// is written: no grant, no measurement, however much the cells settle.
///
/// `CentralPlane::issue` has no production caller — every call site is a test —
/// and it is the only writer of the plane's envelope map, so `retain_grants`
/// retains no day and the calendar the family stage reads stays empty. This
/// runs the cycle with everything else the measurement needs already true:
/// three strategies at pilot, `CLUSTERING_WINDOW` closed sessions, every one
/// settled and attributed, and the same corpus read by the demotion monitor in
/// the same LEARN stage. The returns are shaped on two factors exactly as the
/// test above shapes them, so the population is one a clustering would succeed
/// on. The only thing absent is the grant.
///
/// What it prevents is the repair that would look like progress: supplying a
/// denominator the centre never signed, so that the stage has something to
/// report. A day's return is P&L over the grant it was made under, and a
/// family drawn on an invented denominator is a correlation between two
/// numbers nobody granted — capital structure asserted by arithmetic rather
/// than by two humans. `realised.rs` proves that of one series; this proves it
/// through the cycle, where the fabrication could be put anywhere between the
/// fill and the journal. It is also what would fail first if a stage were ever
/// wired to issue capital for itself.
#[test]
fn the_learn_stage_measures_no_family_structure_on_a_corpus_the_centre_never_granted() -> Result<()>
{
    use qip_kernel::central::CLUSTERING_WINDOW;

    let mut config = PlatformConfig::default();
    config.central.per_cell = Decimal::from_int(9_000_000);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    let ids = [
        StrategyId::new("ungranted-alpha"),
        StrategyId::new("ungranted-beta"),
        StrategyId::new("ungranted-gamma"),
    ];
    for id in &ids {
        register(platform.central_mut(), id, CELL)?;
        walk_to(platform.central_mut(), id, GateStage::Pilot)?;
    }
    // No `issue` call anywhere below. That is the whole of the difference from
    // the test above, and it is the difference a deployment has.
    for id in &ids {
        assert!(
            platform.central().envelope(CELL, id).is_none(),
            "premise: {id} stands at pilot and holds no envelope"
        );
    }

    // What the allocator would have sized each of these at, used only to shape
    // the P&L into two correlated factors. Nothing reads it as a denominator,
    // which is the point: if anything downstream invents one, the clustering
    // has a population to succeed on and this test fails.
    let would_have_granted = 2_000_000.0_f64;
    let quantity = dec!("2000");
    let mut fills_settled = 0usize;
    for session in 0..(CLUSTERING_WINDOW as i64) {
        let at = start().saturating_add(Duration::from_days(session));
        let mut orders = Vec::new();
        let mut fills = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            let shape = if index < 2 {
                ((session % 11) as f64 - 5.0) * 0.002
            } else {
                ((session % 7) as f64 - 3.0) * 0.002
            };
            let wobble = ((session % 3) as f64 - 1.0) * 0.0003 * ((index + 1) as f64);
            let pnl = Decimal::from_f64((shape + wobble) * would_have_granted)
                .ok_or_else(|| qip_core::Error::numeric("a finite return"))?;
            let (session_orders, session_fills) =
                session_fills(id, &format!("u{session}-{index}"), quantity, pnl, at)?;
            orders.extend(session_orders);
            fills.extend(session_fills);
        }
        let ingestion = platform.ingest_cell_report(
            CellReport::new(CELL, at)
                .with_orders(orders)
                .with_fills(fills),
            at,
        )?;
        assert!(
            ingestion.settlement.refused.is_empty(),
            "premise: session {session} settled: {:?}",
            ingestion.settlement.refused
        );
        fills_settled += ingestion.settlement.fills_settled;
    }
    // Two fills a session each — an entry and an exit — for every strategy on
    // every session of the window. Asserted because a corpus that recorded
    // nothing would satisfy every absence below for the wrong reason.
    assert_eq!(
        fills_settled,
        CLUSTERING_WINDOW * ids.len() * 2,
        "premise: every session's fills were booked by the centre"
    );

    let measuring_at = start().saturating_add(Duration::from_days(CLUSTERING_WINDOW as i64));
    assert_eq!(
        platform.central().live_outcomes(measuring_at).len(),
        ids.len(),
        "premise: all three strategies have closed, settled sessions in the corpus"
    );
    // And the calendar, which is the family stage's only input, has nothing:
    // a settled day whose grant the centre never held is not an observation of
    // a return, because there is no denominator it could be a fraction of.
    let calendar = platform.central().realised_calendar(measuring_at);
    assert!(
        calendar.is_empty(),
        "the calendar holds no day under a grant"
    );
    assert_eq!(calendar.day_count(), 0);
    assert_eq!(calendar.strategy_count(), 0);

    let report = platform.run_cycle(measuring_at);
    let learn = report
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    // Matched on `clustered into`, which is `FamilyStructureJournal::describe`'s
    // own wording, and not on `family(ies)`. It was the latter until ADR 0064
    // put a second, unrelated family on this line — the *provenance* family,
    // the sweep a candidate was registered under — and the loose token then
    // made this test fail for a reason that had nothing to do with the
    // correlation clustering it is about. The exact trap
    // `.claude/rules/architecture/01-testing-strategy.md` names: a substring
    // that was unique when it was written and stopped being so without
    // anybody touching this file.
    assert!(
        !learn.detail.contains("clustered into"),
        "the stage measured no family structure: {}",
        learn.detail
    );
    // And the distinguishing positive: the *other* family review did run on
    // this same cycle. Without this the assertion above would pass equally
    // well on a LEARN stage that had stopped saying anything about families
    // at all, which is a different platform from the one under test.
    assert!(
        learn
            .detail
            .contains("strategy family(ies) reviewed against funding standing"),
        "the provenance-family review did not run, so the absence above proves nothing: {}",
        learn.detail
    );

    let entry = platform
        .replay_journal(
            &qip_events::EventFilter::new().topic(qip_events::Topic::LearningCompleted),
        )?
        .last()
        .ok_or_else(|| qip_core::Error::not_found("the cycle journalled an entry"))?
        .decode::<qip_kernel::platform::CycleJournalEntry>()?
        .body;
    // The premise the absence rests on, in the entry itself: this cycle's
    // LEARN did read the realised corpus — it reviewed every strategy in it
    // against the pilot baseline — so the missing measurement is the missing
    // grant and not a stage that never ran.
    let review = entry
        .strategy_review
        .ok_or_else(|| qip_core::Error::not_found("LEARN reviewed the realised sessions"))?;
    assert_eq!(
        review.reviewed + review.skipped,
        ids.len(),
        "premise: every strategy's sessions reached the review this cycle"
    );
    assert_eq!(
        entry.family_structure, None,
        "and no family structure was measured on capital nobody granted"
    );
    Ok(())
}

// --- blueprint §23.4: the horizon gate the LEARN stage arms ------------------

/// The desk's §23.4 statement, with the whole risk budget in one bucket and a
/// single unit in the one every fixture below claims against.
///
/// `deployable` is the pool `Horizon::HoursToDays` is allocated against, and
/// every claim here names that horizon, so a test can make a pool tight or
/// roomy by moving one number without disturbing the sum — which
/// `CapitalPools::new` refuses to let drift.
fn horizon_policy(strategies: &[&StrategyId], deployable: Decimal) -> Result<HorizonPolicy> {
    let inventory = Decimal::from_int(10_000_000)
        .checked_sub(deployable)
        .ok_or_else(|| qip_core::Error::numeric("the split fits inside the budget"))?;
    Ok(HorizonPolicy {
        available_inventory: inventory,
        deployable_capital: deployable,
        capital_not_reserved_for_calls: Decimal::ZERO,
        reserved_capital: Decimal::ZERO,
        claims: strategies
            .iter()
            .map(|strategy| HorizonClaim {
                strategy: (*strategy).clone(),
                source: "research-enrolment".to_string(),
                horizon: Horizon::HoursToDays,
            })
            .collect(),
        despite: None,
    })
}

/// A platform whose central plane carries `policy`, with room at the cell for
/// more than the two grants the default per-cell limit leaves.
fn platform_with_horizons(policy: HorizonPolicy) -> Result<Platform> {
    let mut config = PlatformConfig::default();
    config.central.per_cell = Decimal::from_int(9_000_000);
    config.central.horizons = Some(policy);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// The arming the LEARN stage journalled on the last cycle.
fn journalled_arming(platform: &Platform) -> Result<Option<HorizonArming>> {
    Ok(platform
        .replay_journal(
            &qip_events::EventFilter::new().topic(qip_events::Topic::LearningCompleted),
        )?
        .last()
        .ok_or_else(|| qip_core::Error::not_found("the cycle journalled an entry"))?
        .decode::<qip_kernel::platform::CycleJournalEntry>()?
        .body
        .horizon_arming)
}

/// Blueprint §23.4 through the cycle: LEARN arms the pool gate on the
/// allocator's own budgets, and a promotion whose bucket is already over its
/// pool is refused rather than trimmed.
///
/// The arithmetic, the seam and the gate were each complete and tested before
/// this, and reachable by nobody — the shape
/// `.claude/rules/domains/risk-and-execution.md` names as a defect rather than
/// a spare part. What is proven here is the call path and nothing else: a real
/// cycle's LEARN stage, the plane's own allocator sizing the proposal book, and
/// the refusal arriving out of `StrategyFactory::promote`.
#[test]
fn a_promotion_whose_bucket_is_over_its_pool_is_refused_once_the_learn_stage_has_armed_the_gate()
-> Result<()> {
    let incumbent = StrategyId::new("horizon-incumbent");
    let candidate = StrategyId::new("horizon-candidate");
    // One currency unit of deployable capital, so any budget at all breaches.
    let mut platform =
        platform_with_horizons(horizon_policy(&[&incumbent, &candidate], dec!("1"))?)?;

    // The incumbent reaches a capital-holding rung *before* the gate is armed,
    // which is the position a desk is really in: the pool was divided after
    // strategies were already drawing on it.
    register(platform.central_mut(), &incumbent, CELL)?;
    walk_to(platform.central_mut(), &incumbent, GateStage::Pilot)?;
    register(platform.central_mut(), &candidate, CELL)?;
    walk_to(platform.central_mut(), &candidate, GateStage::Shadow)?;

    let report = platform.run_cycle(start());
    let learn = report
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        learn
            .detail
            .contains("the §23.4 horizon gate is armed on 2"),
        "the stage says what it armed the gate with: {}",
        learn.detail
    );

    // The premise, and the half that makes the refusal below mean anything: the
    // allocator really did size both strategies, and the figures the gate holds
    // are its sizes rather than a number typed beside the split.
    let arming = journalled_arming(&platform)?
        .ok_or_else(|| qip_core::Error::not_found("the cycle journalled its arming"))?;
    assert_eq!(
        arming.strategies_budgeted, 2,
        "both proposals were sized by the allocator: {arming:?}"
    );
    assert!(
        arming.budgeted > dec!("1"),
        "the budgets the allocator produced exceed the deployable pool, or this \
         test measures nothing: {}",
        arming.budgeted
    );
    let deployable = arming
        .standings
        .iter()
        .find(|standing| standing.bucket.as_str() == "hours_to_days")
        .ok_or_else(|| qip_core::Error::not_found("the contested bucket is reported"))?;
    assert_eq!(deployable.pool, dec!("1"));
    assert_eq!(
        deployable.committed, arming.budgeted,
        "both budgets are charged to the one pool both strategies claimed"
    );

    // And the refusal, out of the ordinary promotion path.
    let approval = dual_approval(
        candidate.as_str(),
        start(),
        "every gate check passed with the evidence attached",
    )?;
    let error = platform
        .central_mut()
        .factory_mut()
        .promote(&candidate, Some(approval), "the gate passed", start())
        .expect_err("an over-committed pool must refuse the promotion");
    assert_eq!(error.code(), "denied", "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("horizon-candidate"),
        "the refusal names the promotion it is about: {message}"
    );
    assert!(
        message.contains("hours_to_days is over its pool of 1 by"),
        "the refusal names the bucket, its pool and the overdraft: {message}"
    );
    assert!(
        message.contains("will not trim them for you"),
        "and says it declined to trim rather than choosing which strategy goes \
         unfunded: {message}"
    );
    assert_eq!(
        platform.central().factory().stage_of(&candidate),
        GateStage::Shadow,
        "the refusal left the candidate where it was"
    );
    Ok(())
}

/// The other half, without which the test above proves only that something
/// refuses: a gate that refused every promotion would satisfy every assertion
/// in it.
#[test]
fn the_gate_the_learn_stage_arms_admits_a_promotion_the_pools_have_room_for() -> Result<()> {
    let incumbent = StrategyId::new("horizon-incumbent");
    let candidate = StrategyId::new("horizon-candidate");
    // The whole budget deployable: every claim below is at that horizon, and
    // the allocator cannot size more than the budget it was given.
    let mut platform = platform_with_horizons(horizon_policy(
        &[&incumbent, &candidate],
        Decimal::from_int(10_000_000),
    )?)?;
    register(platform.central_mut(), &incumbent, CELL)?;
    walk_to(platform.central_mut(), &incumbent, GateStage::Pilot)?;
    register(platform.central_mut(), &candidate, CELL)?;
    walk_to(platform.central_mut(), &candidate, GateStage::Shadow)?;

    platform.run_cycle(start());
    let arming = journalled_arming(&platform)?
        .ok_or_else(|| qip_core::Error::not_found("the cycle journalled its arming"))?;
    // Premise: the gate really is armed and really is holding figures, so the
    // admission below is a gate passing rather than a gate absent.
    assert_eq!(arming.strategies_budgeted, 2);
    assert!(!arming.is_breached(), "{arming:?}");

    let approval = dual_approval(
        candidate.as_str(),
        start(),
        "every gate check passed with the evidence attached",
    )?;
    platform.central_mut().factory_mut().promote(
        &candidate,
        Some(approval),
        "the gate passed",
        start(),
    )?;
    assert_eq!(
        platform.central().factory().stage_of(&candidate),
        GateStage::Pilot
    );

    // And the verdict is on the record, because a promotion taken over an
    // unresolved horizon disagreement is otherwise indistinguishable afterwards
    // from one taken on agreement.
    let verdict = platform
        .central()
        .factory()
        .ledger()
        .history(&candidate)
        .last()
        .and_then(|entry| entry.horizon.clone())
        .ok_or_else(|| {
            qip_core::Error::not_found("the ledger entry carries the horizon verdict")
        })?;
    assert_eq!(verdict.horizon.as_str(), "hours_to_days");
    assert!(verdict.treatment.contains("deployable capital"));
    assert!(!verdict.decided_over_disagreement());
    Ok(())
}

/// The unfunded commitment liability the platform's own commitment book holds
/// reaches the gate, and the reserved pool has to meet it as well as the
/// positions booked at the years horizon.
///
/// A commitment nobody allocated against is exactly the one that surprises a
/// desk when it is called. The liability is measured at the cycle's instant
/// rather than restated in the policy beside the split: two claims about one
/// number disagree, and the one in configuration would be the one nobody
/// re-derived.
#[test]
fn the_commitment_books_unfunded_total_reaches_the_gate_the_learn_stage_arms() -> Result<()> {
    let candidate = StrategyId::new("horizon-candidate");
    let mut policy = horizon_policy(&[&candidate], Decimal::from_int(9_000_000))?;
    // A million of reserved capital against nine hundred thousand of unfunded
    // commitments, so the years pool has room for the liability and not for
    // much else.
    policy.available_inventory = Decimal::ZERO;
    policy.capital_not_reserved_for_calls = Decimal::ZERO;
    policy.reserved_capital = Decimal::from_int(1_000_000);
    let mut config = PlatformConfig::default();
    config.central.per_cell = Decimal::from_int(9_000_000);
    config.central.horizons = Some(policy);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut universe = universe();
    universe.insert(private_fund(
        "PEF",
        dec!("1000000"),
        dec!("100000"),
        dec!("0"),
        dec!("400000"),
    )?)?;
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe, limits())?;

    register(platform.central_mut(), &candidate, CELL)?;
    walk_to(platform.central_mut(), &candidate, GateStage::Shadow)?;
    platform.run_cycle(start());

    let arming = journalled_arming(&platform)?
        .ok_or_else(|| qip_core::Error::not_found("the cycle journalled its arming"))?;
    assert_eq!(
        arming.liability,
        dec!("900000"),
        "the liability is the commitment book's own unfunded total, not a \
         figure the policy restated: {arming:?}"
    );
    let years = arming
        .standings
        .iter()
        .find(|standing| standing.bucket.as_str() == "years")
        .ok_or_else(|| qip_core::Error::not_found("the years bucket is reported"))?;
    assert_eq!(
        years.committed,
        dec!("900000"),
        "the reserved pool is charged the liability although no strategy \
         budgeted for it"
    );
    // Premise: nothing is budgeted at the years horizon at all, so the charge
    // above is the liability and nothing else.
    assert!(!years.is_breached(), "the reserved pool still covers it");
    assert_eq!(years.headroom()?, dec!("100000"));
    Ok(())
}

/// A private fund, so the platform's commitment book has an unfunded total to
/// charge the reserved pool with.
fn private_fund(
    symbol: &str,
    committed: Decimal,
    called: Decimal,
    distributed: Decimal,
    residual: Decimal,
) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::PrivateEquityFund,
        LiquidityProfile::illiquid(90.0, 250.0),
    )
    .venue("OTC")
    .price(dec!("100"))
    .extension(Extension::PrivateAsset(PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: committed,
        called_capital: called,
        distributed_capital: distributed,
        residual_value: residual,
        stage: "buyout".to_string(),
        lockup_years: 7.0,
        capital_call_notice_days: 10,
    }))
    .provenance(DataProvenance::synthetic("administrator", start()))
    .build(start())
}

/// A desk that has stated no §23.4 split arms no gate, and its promotions are
/// exactly what they were before this existed.
///
/// The `MaxExpectedShortfall` shape, avoided from the other side. A plane that
/// armed an empty reconciler would refuse every promotion to a capital-holding
/// rung for want of a claim — a control that reads as protection, fires always,
/// and measured nothing.
#[test]
fn a_desk_that_has_stated_no_horizon_split_arms_no_gate_and_promotes_as_it_did_before() -> Result<()>
{
    let candidate = StrategyId::new("horizon-candidate");
    let mut platform = platform()?;
    // Premise: no policy is stated, which is every deployment as this is
    // written.
    assert_eq!(platform.central().config().horizons, None);
    register(platform.central_mut(), &candidate, CELL)?;
    walk_to(platform.central_mut(), &candidate, GateStage::Shadow)?;

    let report = platform.run_cycle(start());
    let learn = report
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        !learn.detail.contains("horizon gate"),
        "the stage arms nothing and says nothing about it: {}",
        learn.detail
    );
    assert_eq!(journalled_arming(&platform)?, None);

    let approval = dual_approval(
        candidate.as_str(),
        start(),
        "every gate check passed with the evidence attached",
    )?;
    platform.central_mut().factory_mut().promote(
        &candidate,
        Some(approval),
        "the gate passed",
        start(),
    )?;
    assert_eq!(
        platform.central().factory().stage_of(&candidate),
        GateStage::Pilot
    );
    assert!(
        platform
            .central()
            .factory()
            .ledger()
            .history(&candidate)
            .last()
            .is_some_and(|entry| entry.horizon.is_none()),
        "no assurance was attached, so the entry records no horizon verdict"
    );
    Ok(())
}

/// A split that does not sum to the risk budget stops the plane at start-up.
///
/// Refused here rather than at the first promotion, because a split that does
/// not sum reaches the lifecycle gate as a refusal of every promotion to a
/// capital-holding rung — indistinguishable, months later, from a book that is
/// genuinely over-committed.
#[test]
fn a_horizon_split_that_does_not_sum_to_the_risk_budget_stops_the_plane_rather_than_the_promotion()
-> Result<()> {
    let candidate = StrategyId::new("horizon-candidate");
    let mut config = CentralConfig::default();
    let mut policy = horizon_policy(&[&candidate], dec!("1"))?;
    // One unit short of the budget the desk configured.
    policy.available_inventory = policy
        .available_inventory
        .checked_sub(dec!("1"))
        .ok_or_else(|| qip_core::Error::numeric("the inventory pool carries a unit to remove"))?;
    config.horizons = Some(policy);
    let error = CentralPlane::new(&[7u8; 32], config).expect_err("the plane refuses to start");
    assert_eq!(error.code(), "invalid", "{error:?}");
    assert!(
        error.message().contains("sum exactly"),
        "the refusal says the pools must sum to the total: {error:?}"
    );

    // And a policy naming no strategy is refused for the opposite reason: it
    // would refuse every promotion for want of a claim, which reads as a
    // control working and is a control with no subject.
    let empty = CentralConfig {
        horizons: Some(horizon_policy(&[], dec!("1"))?),
        ..CentralConfig::default()
    };
    let error = CentralPlane::new(&[7u8; 32], empty).expect_err("the plane refuses to start");
    assert!(error.message().contains("names no strategy"), "{error:?}");
    Ok(())
}

/// A cycle whose arming fails closes the gate rather than leaving the last
/// successful arming in force.
///
/// The defect this pins was found by an independent security review of
/// `27da0c5`, while the gate still had no production producer.
/// `CentralPlane::arm_horizons` attached its assurance only on the success
/// path, so a cycle that could no longer measure the pools went on reconciling
/// promotions against bounds computed from an older liability — the gate
/// stayed green by being out of date. Refusing a promotion is recoverable; the
/// next successful arming lets it through. Admitting one against a liability
/// nobody measured this cycle is not.
///
/// Note what is deliberately NOT asserted: that the assurance is detached.
/// `LifecycleLedger` holds it as an `Option` and `None` is the *ungated*
/// state — the legitimate one for a plane with no stated policy — so a detach
/// would admit every promotion instead of refusing it.
#[test]
fn an_arming_that_fails_refuses_promotions_rather_than_reusing_the_last_successful_one()
-> Result<()> {
    let incumbent = StrategyId::new("horizon-incumbent");
    let candidate = StrategyId::new("horizon-candidate");
    let mut platform =
        platform_with_horizons(horizon_policy(&[&incumbent, &candidate], dec!("1000000"))?)?;
    register(platform.central_mut(), &incumbent, CELL)?;
    walk_to(platform.central_mut(), &incumbent, GateStage::Pilot)?;
    register(platform.central_mut(), &candidate, CELL)?;
    walk_to(platform.central_mut(), &candidate, GateStage::Shadow)?;

    // The premise, and the half without which the refusal below proves nothing:
    // one good cycle arms the gate, and a promotion IS admitted through it. If
    // this ever fails, the assertion after it is passing because the gate was
    // never armed rather than because a failed arming closed it.
    let report = platform.run_cycle(start());
    let learn = report
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        learn
            .detail
            .contains("the §23.4 horizon gate is armed on 2"),
        "the premise: a good cycle armed the gate: {}",
        learn.detail
    );

    // Now an arming that fails, at the seam a failing `unfunded_total` reaches:
    // a negative liability, which `CapitalPools::new` refuses by name.
    let failed = platform
        .central_mut()
        .arm_horizons(dec!("-1"), 0.0, start())
        .expect_err("a negative liability must refuse the arming");
    assert!(
        failed.to_string().contains("unfunded_commitments"),
        "the arming failed for the reason this test intends, not another: {failed}"
    );

    // The gate is now closed, not stale. A promotion the FIRST arming would
    // have admitted — the pool is a million against two small budgets — is
    // refused, and the refusal names the arming failure rather than a bucket.
    let approval = dual_approval(
        candidate.as_str(),
        start(),
        "every gate check passed with the evidence attached",
    )?;
    let error = platform
        .central_mut()
        .factory_mut()
        .promote(&candidate, Some(approval), "the gate passed", start())
        .expect_err("a failed arming must close the gate, not leave the old one in force");
    assert_eq!(error.code(), "denied", "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("could not be armed this cycle"),
        "the refusal says the gate is unarmed rather than naming a pool it did not \
         measure: {message}"
    );
    assert!(
        message.contains("unfunded_commitments"),
        "and carries the arming failure through, so an operator reads why: {message}"
    );
    assert!(
        message.contains("clears itself as soon as one cycle arms"),
        "and says the refusal is recoverable rather than a judgement about the \
         strategy: {message}"
    );
    assert_eq!(
        platform.central().factory().stage_of(&candidate),
        GateStage::Shadow,
        "the refusal left the candidate where it was"
    );
    Ok(())
}

/// The drawdown response shrinks what is charged to the §23.4 pools and leaves
/// the pools themselves at the capital the desk holds.
///
/// The asymmetry is the design, not an oversight, and this test exists because
/// an independent review could not tell the two apart from the code: nothing
/// exercised a non-unit drawdown multiplier through `arm_horizons`, so both
/// readings passed the suite. The pools are a statement of capital held and how
/// liquid it is; the multiplier is an appetite schedule (0.25 at a fifteen per
/// cent drawdown in the shipped one), and a book that has lost fifteen per cent
/// does not hold a quarter of its capital. Scaling the denominator too would
/// put a figure nobody measured on the measuring side of the comparison.
///
/// Both halves are asserted because either alone proves nothing: that the
/// charges moved, and that the pools did not.
#[test]
fn a_drawdown_shrinks_the_charges_and_leaves_the_pools_at_the_capital_the_desk_holds() -> Result<()>
{
    let first = StrategyId::new("horizon-first");
    let second = StrategyId::new("horizon-second");
    // Room in the deployable pool for both budgets, so nothing here turns on a
    // breach; what is measured is the two sides of the comparison.
    let mut platform =
        platform_with_horizons(horizon_policy(&[&first, &second], dec!("4000000"))?)?;
    register(platform.central_mut(), &first, CELL)?;
    register(platform.central_mut(), &second, CELL)?;

    // Premise: the schedule really does respond at the drawdown this test uses.
    // Without this the assertions below would pass on a flat schedule, which is
    // the shape of a test that guards nothing.
    let schedule = qip_capital::allocation::DrawdownSchedule::default();
    assert_eq!(
        schedule.multiplier_at(0.0),
        Decimal::ONE,
        "premise: an undrawn book allocates in full"
    );
    let drawn_multiplier = schedule.multiplier_at(0.15);
    assert!(
        drawn_multiplier.is_positive() && drawn_multiplier < Decimal::ONE,
        "premise: a fifteen per cent drawdown allocates less than in full and \
         more than nothing, so both sides of the comparison are non-trivial: \
         {drawn_multiplier}"
    );

    let full = platform
        .central_mut()
        .arm_horizons(Decimal::ZERO, 0.0, start())?
        .ok_or_else(|| qip_core::Error::not_found("the undrawn arming produced a standing"))?;
    let drawn = platform
        .central_mut()
        .arm_horizons(Decimal::ZERO, 0.15, start())?
        .ok_or_else(|| qip_core::Error::not_found("the drawn arming produced a standing"))?;

    // Premise: the same book was sized both times, and it was sized at all.
    assert_eq!(full.strategies_budgeted, 2, "{}", full.describe());
    assert_eq!(drawn.strategies_budgeted, 2, "{}", drawn.describe());
    assert!(
        full.budgeted.is_positive(),
        "premise: the undrawn arming charged something: {}",
        full.describe()
    );

    // The numerator moved.
    assert!(
        drawn.budgeted < full.budgeted,
        "the drawdown shrank what is charged: {} against {}",
        drawn.budgeted,
        full.budgeted
    );

    // And the denominator did not. Compared bucket by bucket rather than on the
    // total, because a scaling that moved capital between pools while holding
    // the sum would read identically on a total.
    let pools_of = |arming: &HorizonArming| -> BTreeMap<String, Decimal> {
        arming
            .standings
            .iter()
            .map(|standing| (standing.bucket.to_string(), standing.pool))
            .collect()
    };
    let undrawn_pools = pools_of(&full);
    assert_eq!(
        undrawn_pools.get("hours_to_days"),
        Some(&dec!("4000000")),
        "premise: the standings name the desk's stated split: {undrawn_pools:?}"
    );
    assert_eq!(
        pools_of(&drawn),
        undrawn_pools,
        "every pool is the capital the desk holds, whatever the drawdown \
         response is doing to the charges against it"
    );

    // The committed side of the same standings moved, which is what makes the
    // equality above a statement about the pools and not about an empty book.
    let committed_of = |arming: &HorizonArming| -> Option<Decimal> {
        arming
            .standings
            .iter()
            .find(|standing| standing.bucket.to_string() == "hours_to_days")
            .map(|standing| standing.committed)
    };
    let undrawn_committed = committed_of(&full)
        .ok_or_else(|| qip_core::Error::not_found("the undrawn standing names the bucket"))?;
    let drawn_committed = committed_of(&drawn)
        .ok_or_else(|| qip_core::Error::not_found("the drawn standing names the bucket"))?;
    assert!(
        drawn_committed < undrawn_committed,
        "the charge against the bucket fell with the book: {drawn_committed} \
         against {undrawn_committed}"
    );
    Ok(())
}

/// A drawdown deep enough to stop all deployment still charges the unfunded
/// commitment liability against the reserved pool, and the years bucket can
/// still breach.
///
/// This is the case that decides which reading of §23.4 is right. At the
/// schedule's deepest step the multiplier is zero, so the allocator budgets
/// nothing and no charge a drawdown could scale exists. If the pools scaled
/// with it they would all be zero, `CapitalPools::new` would refuse the split
/// by name, the arming would fail and `UnarmedHorizons` would refuse every
/// promotion to a capital-holding rung — a control firing on the claim that the
/// desk holds nothing, at the one drawdown where nothing could breach anyway. A
/// capital call does not shrink because the platform chose to deploy less, so
/// the liability is charged in full against reserved capital the desk still
/// has, and the gate keeps the only judgement it can still usefully make.
#[test]
fn a_drawdown_deep_enough_to_stop_all_deployment_still_charges_the_commitment_liability()
-> Result<()> {
    let first = StrategyId::new("horizon-first");
    let second = StrategyId::new("horizon-second");
    // A reserved pool of a million, taken out of the inventory bucket so the
    // four still sum exactly to the risk budget.
    let mut policy = horizon_policy(&[&first, &second], dec!("1000000"))?;
    policy.available_inventory = policy
        .available_inventory
        .checked_sub(dec!("1000000"))
        .ok_or_else(|| qip_core::Error::numeric("the inventory pool carries the reserve"))?;
    policy.reserved_capital = dec!("1000000");
    let mut platform = platform_with_horizons(policy)?;
    register(platform.central_mut(), &first, CELL)?;
    register(platform.central_mut(), &second, CELL)?;

    // Premise: this drawdown really does stop deployment altogether.
    let schedule = qip_capital::allocation::DrawdownSchedule::default();
    assert_eq!(
        schedule.multiplier_at(0.25),
        Decimal::ZERO,
        "premise: a twenty-five per cent drawdown deploys nothing"
    );

    // Twice the reserved pool, so the breach is unambiguous.
    let arming = platform
        .central_mut()
        .arm_horizons(dec!("2000000"), 0.25, start())?
        .ok_or_else(|| {
            qip_core::Error::not_found(
                "the gate arms in a deep drawdown rather than refusing to measure",
            )
        })?;
    assert_eq!(
        arming.strategies_budgeted,
        0,
        "premise: the allocator sized nothing, so no charge here is a scaled \
         budget: {}",
        arming.describe()
    );
    assert_eq!(
        arming.budgeted,
        Decimal::ZERO,
        "premise: and nothing was charged from the plan: {}",
        arming.describe()
    );

    let years = arming
        .standings
        .iter()
        .find(|standing| standing.bucket.to_string() == "years")
        .ok_or_else(|| qip_core::Error::not_found("the standings name the years bucket"))?;
    assert_eq!(
        years.pool,
        dec!("1000000"),
        "the reserved pool is the capital the desk holds, undisturbed by a \
         deployment schedule: {}",
        arming.describe()
    );
    assert_eq!(
        years.committed,
        dec!("2000000"),
        "and the liability is charged in full against it: {}",
        arming.describe()
    );
    assert!(
        arming.is_breached(),
        "so the gate still finds the book short at the years horizon, which is \
         the only judgement left to make once deployment has stopped: {}",
        arming.describe()
    );
    Ok(())
}

// --- execution cost: the measurement that lets CostOverrun fire ---------------

/// One order the cell sent at `sent`, filled by the venue at `filled`.
///
/// The two prices are separate parameters because that difference is the
/// whole subject: [`strategy_order_and_fill`] above passes one price for
/// both, which is a fill at exactly the price the platform asked for and so
/// a cost of zero — the case that cannot distinguish a working measurement
/// from the constant it replaced.
fn order_filled_away(
    id: &StrategyId,
    order_id: &str,
    side: qip_contracts::message::BookSide,
    quantity: Decimal,
    sent: Decimal,
    filled: Decimal,
    at: Timestamp,
) -> (qip_mesh::delta::DeltaOrder, qip_contracts::wire::FillRecord) {
    let (mut order, mut fill) = strategy_order_and_fill(id, order_id, side, quantity, sent, at);
    order.price = sent;
    fill.price = filled;
    (order, fill)
}

#[test]
fn a_fill_away_from_the_price_the_platform_sent_is_measured_as_execution_cost() -> Result<()> {
    let mut platform = platform()?;
    let id = StrategyId::new("cost-measured");
    register(platform.central_mut(), &id, CELL)?;

    // Bought: sent at 50, filled at 50.10. Paying 0.10 on 50 is 20 basis
    // points, and the sign is positive because the platform paid away.
    let (order, fill) = order_filled_away(
        &id,
        "ord-cost-1",
        qip_contracts::message::BookSide::Ask,
        dec!("100"),
        dec!("50"),
        dec!("50.10"),
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
    assert!(
        ingestion.settlement.breaks.is_empty(),
        "premise: the fill matched an order the centre saw sent: {:?}",
        ingestion.settlement.breaks
    );

    let cost = ingestion.settlement.cost_bps_by_strategy();
    let measured = cost
        .get(id.as_str())
        .copied()
        .ok_or_else(|| qip_core::Error::not_found("the strategy's measured cost"))?;
    assert!(
        (measured - 20.0).abs() < 1e-9,
        "a buy of 100 sent at 50 and filled at 50.10 costs 20bp; the centre measured {measured}"
    );
    Ok(())
}

#[test]
fn price_improvement_is_measured_as_a_negative_cost_rather_than_floored_at_zero() -> Result<()> {
    let mut platform = platform()?;
    let id = StrategyId::new("cost-improved");
    register(platform.central_mut(), &id, CELL)?;

    // The same trade filled *better* than it was sent: bought at 49.90 having
    // asked for 50. Floored at zero this would read as break-even, and a
    // series of improvements would then bias the mean upward toward the
    // overrun the kill condition watches for.
    let (order, fill) = order_filled_away(
        &id,
        "ord-cost-2",
        qip_contracts::message::BookSide::Ask,
        dec!("100"),
        dec!("50"),
        dec!("49.90"),
        start(),
    );
    let ingestion = platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        start(),
    )?;
    let measured = ingestion
        .settlement
        .cost_bps_by_strategy()
        .get(id.as_str())
        .copied()
        .ok_or_else(|| qip_core::Error::not_found("the strategy's measured cost"))?;
    assert!(
        (measured + 20.0).abs() < 1e-9,
        "a buy filled 0.10 below the price sent is 20bp of improvement, so -20; measured \
         {measured}"
    );
    Ok(())
}

#[test]
fn a_sale_filled_below_the_price_sent_costs_rather_than_earns() -> Result<()> {
    let mut platform = platform()?;
    let id = StrategyId::new("cost-sold");
    register(platform.central_mut(), &id, CELL)?;

    // The sign convention is the half of this measurement a wrong guess
    // would invert silently: selling *below* what you asked is paying away
    // exactly as buying above it is. Without this case a cost function that
    // ignored the side would pass the buy tests above and read every sale
    // backwards, turning a venue that fills sales badly into a strategy that
    // looks cheap to trade.
    let (order, fill) = order_filled_away(
        &id,
        "ord-cost-3",
        qip_contracts::message::BookSide::Bid,
        dec!("100"),
        dec!("50"),
        dec!("49.90"),
        start(),
    );
    let ingestion = platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        start(),
    )?;
    let measured = ingestion
        .settlement
        .cost_bps_by_strategy()
        .get(id.as_str())
        .copied()
        .ok_or_else(|| qip_core::Error::not_found("the strategy's measured cost"))?;
    assert!(
        (measured - 20.0).abs() < 1e-9,
        "a sale of 100 asked at 50 and filled at 49.90 costs 20bp; measured {measured}"
    );
    Ok(())
}

#[test]
fn the_cost_overrun_kill_condition_fires_on_a_measured_overrun_and_holds_within_tolerance()
-> Result<()> {
    // The test this whole measurement exists for. `KillCondition::CostOverrun`
    // compares the realised cost against a modelled figure plus a tolerance,
    // and the centre used to hand it the literal `0.0` — so the comparison was
    // `0.0 > modelled + tolerance`, false for every non-negative modelled cost
    // anyone would write. The condition shipped inside kill-condition sets,
    // read as protection, and could not fire. What follows drives a real fill
    // through `ingest_cell_report` and reads the cost back off
    // `live_outcomes`, which is the path `stage_learn` reads.
    let mut platform = platform()?;
    let id = StrategyId::new("cost-overrun");
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    issue(platform.central_mut(), &id, CELL, start())?;

    // Bought 100 sent at 50, filled at 50.25: 50 basis points paid away.
    let (order, fill) = order_filled_away(
        &id,
        "ord-overrun-1",
        qip_contracts::message::BookSide::Ask,
        dec!("100"),
        dec!("50"),
        dec!("50.25"),
        start(),
    );
    let ingestion = platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        start(),
    )?;
    assert!(
        ingestion.settlement.refused.is_empty() && ingestion.settlement.breaks.is_empty(),
        "premise: the fill settled cleanly: {:?} {:?}",
        ingestion.settlement.refused,
        ingestion.settlement.breaks
    );

    // The day must be closed before the series will report it: a day still
    // being traded is not a return yet, and the same rule governs the cost.
    let next_day = start().saturating_add(Duration::from_days(1));
    let outcomes = platform.central().live_outcomes(next_day);
    let outcome = outcomes
        .iter()
        .find(|outcome| outcome.strategy == id)
        .ok_or_else(|| qip_core::Error::not_found("the strategy's live outcome"))?;
    assert!(
        (outcome.realised_cost_bps - 50.0).abs() < 1e-9,
        "the measured cost did not reach the outcome the LEARN stage reads: {}",
        outcome.realised_cost_bps
    );

    let observation = qip_lifecycle::demotion::LiveObservation {
        strategy: id.clone(),
        at: next_day,
        returns: Vec::new(),
        realised_loss: Decimal::ZERO,
        peak_to_trough_drawdown: 0.0,
        consecutive_losing_days: 0,
        realised_cost_bps: outcome.realised_cost_bps,
        envelope: None,
    };

    // Veto: modelled 10bp with 5bp of tolerance is breached by 50bp.
    let breached = qip_lifecycle::evidence::KillCondition::CostOverrun {
        modelled_bps: 10.0,
        tolerance_bps: 5.0,
    }
    .breach(&observation)
    .ok_or_else(|| qip_core::Error::not_found("the breach the overrun should have raised"))?;
    assert!(
        breached.contains("50.00") && breached.contains("10.00"),
        "the breach does not name the measured cost and the modelled one: {breached}"
    );

    // Pass: the same measured cost inside a tolerance a desk actually set.
    // Without this half the test would pass against a condition that fired on
    // everything, which is the failure mode a veto-only fixture cannot see.
    assert!(
        qip_lifecycle::evidence::KillCondition::CostOverrun {
            modelled_bps: 45.0,
            tolerance_bps: 10.0,
        }
        .breach(&observation)
        .is_none(),
        "50bp measured against a modelled 45bp with 10bp of tolerance is within the band and \
         must not demote"
    );
    Ok(())
}

#[test]
fn the_cost_of_a_settlement_is_weighted_by_quantity_and_not_a_mean_of_the_fills() -> Result<()> {
    // Two fills of very different size, costing very differently. A plain
    // mean of the two costs is 55bp; weighted by the quantity that actually
    // paid it, it is 14bp. Every other test in this group uses one fill, and
    // against one fill the two arithmetics agree — so without this case a
    // mean-of-fills would pass the lot while overstating the cost of any
    // strategy that does one small bad trade among many good ones, which is
    // the shape that would demote a working strategy.
    let mut platform = platform()?;
    let id = StrategyId::new("cost-weighted");
    register(platform.central_mut(), &id, CELL)?;

    // 1000 bought at 50, filled at 50.02 → 4bp.
    let (big_order, big_fill) = order_filled_away(
        &id,
        "ord-weight-1",
        qip_contracts::message::BookSide::Ask,
        dec!("1000"),
        dec!("50"),
        dec!("50.02"),
        start(),
    );
    // 100 bought at 50, filled at 50.53 → 106bp.
    let (small_order, small_fill) = order_filled_away(
        &id,
        "ord-weight-2",
        qip_contracts::message::BookSide::Ask,
        dec!("100"),
        dec!("50"),
        dec!("50.53"),
        start(),
    );
    let ingestion = platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_orders(vec![big_order, small_order])
            .with_fills(vec![big_fill, small_fill]),
        start(),
    )?;
    assert_eq!(
        ingestion.settlement.fills_settled, 2,
        "premise: both fills settled, so the mean has two terms to disagree about"
    );

    let measured = ingestion
        .settlement
        .cost_bps_by_strategy()
        .get(id.as_str())
        .copied()
        .ok_or_else(|| qip_core::Error::not_found("the strategy's measured cost"))?;
    let weighted = (4.0 * 1000.0 + 106.0 * 100.0) / 1100.0;
    assert!(
        (measured - weighted).abs() < 1e-9,
        "the cost should be the quantity-weighted {weighted:.4}bp, not the mean of the fills \
         (55bp); measured {measured}"
    );
    Ok(())
}

// --- the dual approval that lets a strategy reach a capital-holding rung ------

/// Walk a strategy to `Shadow`, the rung below the first that needs signing.
///
/// Stops there on purpose: everything below `Pilot` is admitted on evidence
/// alone, so this is the state in which the approval route is the only way up
/// and the tests below are about that route rather than about the ladder.
fn walk_to_shadow(platform: &mut Platform, id: &StrategyId) -> Result<()> {
    register(platform.central_mut(), id, CELL)?;
    walk_to(platform.central_mut(), id, GateStage::Shadow)
}

fn operator(name: &str, at: Timestamp) -> qip_risk_engine::autonomy::OperatorIdentity {
    qip_risk_engine::autonomy::OperatorIdentity::verified(name, "hardware-token", at)
}

const WHY: &str = "the pilot evidence was reviewed against the shadow record";

#[test]
fn one_signature_does_not_promote_and_a_second_from_the_same_person_is_refused() -> Result<()> {
    // The property the whole route exists for. `Pilot` is a rung that holds
    // capital, and one person must not be able to put a strategy on it —
    // including by signing twice from two sessions, which is the shape a
    // single-signer bypass would actually take.
    let mut platform = platform()?;
    let id = StrategyId::new("dual-approval");
    walk_to_shadow(&mut platform, &id)?;
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow,
        "premise: the strategy is one rung below the first that needs signing"
    );

    let first = platform.approve_promotion(&id, &operator("ops-dana", start()), WHY, start())?;
    assert_eq!(first.outcome, "awaiting_countersignature");
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow,
        "one signature moved the strategy; a dual approval is not dual if the first one acts"
    );

    let again = platform
        .approve_promotion(&id, &operator("ops-dana", start()), WHY, start())
        .expect_err("the same operator signing twice is not two approvers");
    assert!(
        again
            .message()
            .contains("a second session is not a second person"),
        "the refusal does not say why one person cannot be two: {}",
        again.message()
    );
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow,
        "signing twice promoted the strategy"
    );
    Ok(())
}

#[test]
fn the_two_promotion_signatures_are_held_to_one_rationale_floor() -> Result<()> {
    // The defect this prevents, and it shipped. `Approval::new` refuses a
    // first rationale under ten trimmed characters. `Approval::countersigned_by`
    // checks none — it cannot, it is never handed the second rationale — and
    // the countersigning arm wrote the second signer's string onto
    // `PromotionApprovalEntry.rationale` and passed it down through
    // `factory().promote` to the lifecycle ledger, neither of which validates
    // it either. So the first signer had to argue and the second could write
    // "x", on the act that puts capital behind a strategy, and the second
    // signer's text is the one the record keeps.
    //
    // Found while fixing the same defect on `reinstate_venue` and then again
    // on `approve_recalibration`. Three sites, one check.
    let mut platform = platform()?;
    let id = StrategyId::new("dual-approval-rationale");
    walk_to_shadow(&mut platform, &id)?;
    // Premise: the strategy is at the rung where a signature is what moves it,
    // so a refusal below is about the rationale and not about the ladder.
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow,
        "premise: the strategy is one rung below the first that needs signing"
    );

    // Premise: the floor is real on the *first* signature and it is ten
    // trimmed characters. Asserted against `Approval::new`'s own refusal
    // rather than against the kernel's mirrored constant, so the two signers
    // cannot silently come to be held to different numbers.
    //
    // **The two strings below are the whole of that proof, and until
    // 2026-09-14 they were not.** This premise passed `"  fine  "` — four
    // trimmed characters — and the countersignature `"   x   "`, one. Both
    // sit so far below either floor that the kernel's constant was free to
    // move: a review dropped it from ten to five and all three tests that
    // claim to hold it stayed green, so the second signer could have been
    // held to half the first signer's bar with nothing catching it. Nine
    // trimmed characters must be refused and exactly ten admitted at *both*
    // signatures, which pins each floor from both sides and so pins them
    // equal.
    const BELOW_FLOOR: &str = "  lot fixed  ";
    const AT_FLOOR: &str = "  grid fixed  ";
    assert_eq!(BELOW_FLOOR.trim().len(), 9);
    assert_eq!(AT_FLOOR.trim().len(), 10);

    let short = platform
        .approve_promotion(&id, &operator("ops-dana", start()), BELOW_FLOOR, start())
        .expect_err("a rationale one character below the floor signed the first half");
    assert!(
        short
            .message()
            .contains("state a rationale somebody can review later"),
        "the first signature was refused for some other reason: {}",
        short.message()
    );

    // And admitted at exactly the floor, which pins `Approval::new`'s number
    // from above: a contracts-side floor of eleven fails here rather than
    // silently holding the first signer to a bar the second is not.
    platform.approve_promotion(&id, &operator("ops-dana", start()), AT_FLOOR, start())?;

    // The countersignature, one character below the same floor. Trimmed, so
    // padding it with spaces does not buy a signer past the bar.
    let thin = platform
        .approve_promotion(&id, &operator("ops-ravi", start()), BELOW_FLOOR, start())
        .expect_err("a countersignature below the floor put capital behind a strategy");
    assert!(
        thin.message().contains("must state a rationale"),
        "the countersignature was refused for some other reason: {}",
        thin.message()
    );
    assert!(
        thin.message().contains("promotion"),
        "the refusal does not name which signature is short: {}",
        thin.message()
    );

    // The consequence, which is the half that matters. A test asserting only
    // the refusal would pass against an implementation that promoted first
    // and complained afterwards.
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow,
        "a countersignature with an unreviewable rationale promoted the strategy anyway"
    );
    assert!(
        !platform.central().factory().holds_capital(&id),
        "a refused countersignature put the strategy on a capital-holding rung"
    );
    // The admitting half, which does double duty. Without it this test passes
    // against an `approve_promotion` that refuses every countersignature —
    // a floor nobody can clear rather than a floor. And because it *promotes*
    // rather than reporting `awaiting_countersignature`, it also proves the
    // first signature survived the refusal above: a refusal on the floor is
    // not a refusal of the pair, so the second signer restates their reason
    // rather than sending the first back to sign again.
    let done =
        platform.approve_promotion(&id, &operator("ops-ravi", start()), AT_FLOOR, start())?;
    assert_eq!(done.outcome, "promoted", "detail: {:?}", done.detail);
    assert_eq!(
        done.approver, "ops-dana",
        "the first signature was discarded by the refused countersignature and this is a \
         fresh pair"
    );
    assert_eq!(done.second_approver.as_deref(), Some("ops-ravi"));
    assert_eq!(
        done.rationale.trim().len(),
        AT_FLOOR.trim().len(),
        "the record does not carry the second signer's own text"
    );
    Ok(())
}

#[test]
fn two_operators_carry_the_strategy_onto_the_rung_and_the_gate_still_rules() -> Result<()> {
    // The pass half. Without it every test here would be satisfied by a route
    // that refuses everything, which is the failure a veto-only fixture
    // cannot see.
    let mut platform = platform()?;
    let id = StrategyId::new("dual-approval-pass");
    walk_to_shadow(&mut platform, &id)?;
    assert!(
        !platform.central().factory().holds_capital(&id),
        "premise: the strategy holds no capital at Shadow"
    );

    platform.approve_promotion(&id, &operator("ops-dana", start()), WHY, start())?;
    let done = platform.approve_promotion(&id, &operator("ops-ravi", start()), WHY, start())?;

    assert_eq!(done.outcome, "promoted", "detail: {:?}", done.detail);
    assert_eq!(done.approver, "ops-dana");
    assert_eq!(done.second_approver.as_deref(), Some("ops-ravi"));
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Pilot,
        "two approvers signed and the strategy did not reach the rung"
    );
    assert!(
        platform.central().factory().baseline(&id).is_some(),
        "reaching the pilot rung did not write the baseline the demotion monitor needs; the \
         interlock this route exists to close is still open"
    );
    Ok(())
}

#[test]
fn a_stale_first_signature_is_discarded_rather_than_countersigned() -> Result<()> {
    // Two signatures far apart are two people agreeing about two different
    // states of the world. The first is dropped and named, so the pair start
    // again on today's evidence rather than completing yesterday's agreement.
    let mut platform = platform()?;
    let id = StrategyId::new("dual-approval-stale");
    walk_to_shadow(&mut platform, &id)?;

    platform.approve_promotion(&id, &operator("ops-dana", start()), WHY, start())?;
    let much_later = start().saturating_add(Duration::from_days(2));
    let refused = platform
        .approve_promotion(&id, &operator("ops-ravi", much_later), WHY, much_later)
        .expect_err("a countersignature two days later is not a review of the same decision");
    assert!(
        refused.message().contains("must follow within"),
        "the refusal does not name the window: {}",
        refused.message()
    );
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Shadow
    );

    // And the stale signature is gone rather than lying in wait: a fresh
    // countersignature now starts a new pair instead of completing the old
    // one, which is what "discarded" has to mean to be worth anything.
    let fresh =
        platform.approve_promotion(&id, &operator("ops-ravi", much_later), WHY, much_later)?;
    assert_eq!(
        fresh.outcome, "awaiting_countersignature",
        "the discarded signature was still there and this completed it"
    );
    assert_eq!(fresh.approver, "ops-ravi");
    Ok(())
}

#[test]
fn a_stale_credential_cannot_sign_a_promotion_that_puts_capital_to_work() -> Result<()> {
    let mut platform = platform()?;
    let id = StrategyId::new("dual-approval-credential");
    walk_to_shadow(&mut platform, &id)?;

    // Authenticated this morning, signing this afternoon. A session token is
    // not evidence that the person named on the record is at the keyboard.
    let long_ago = start().saturating_sub(Duration::from_hours(4));
    let refused = platform
        .approve_promotion(&id, &operator("ops-dana", long_ago), WHY, start())
        .expect_err("a four-hour-old credential is not fresh enough to move capital");
    assert!(
        refused.message().contains("re-authenticate"),
        "the refusal does not say what to do: {}",
        refused.message()
    );

    // The premise that makes the above about freshness and not about the
    // operator: the same person with a fresh credential is admitted.
    let accepted = platform.approve_promotion(&id, &operator("ops-dana", start()), WHY, start())?;
    assert_eq!(accepted.outcome, "awaiting_countersignature");
    Ok(())
}

#[test]
fn a_rung_that_needs_no_signature_refuses_one_rather_than_accepting_it() -> Result<()> {
    // Signing for a rung the gate admits on evidence alone is refused, so an
    // operator is never taught that their signature is what moved a strategy
    // up a rung that takes none.
    let mut platform = platform()?;
    let id = StrategyId::new("dual-approval-unsigned-rung");
    register(platform.central_mut(), &id, CELL)?;
    assert_eq!(
        platform.central().factory().stage_of(&id),
        GateStage::Candidate,
        "premise: the strategy is at a rung whose next takes no approval"
    );

    let refused = platform
        .approve_promotion(&id, &operator("ops-dana", start()), WHY, start())
        .expect_err("the holdout rung takes no human approval");
    assert!(
        refused.message().contains("takes no human approval"),
        "the refusal does not say why: {}",
        refused.message()
    );
    Ok(())
}

// --- blueprint §12.3's fifth row, through the cycle (ADR 0064) ---------------

/// A feature catalogue wide enough for the generator not to starve: at least
/// two features of every type, so a type-preserving choice always has
/// somewhere to go. The same shape `tests/foundry.rs` uses, because the
/// population under test has to be one the production search could mint.
fn family_catalogue(on: &ObjectId) -> Result<qip_strategy::catalogue::FeatureCatalogue> {
    use qip_strategy::ir::Type;
    let mut catalogue = qip_strategy::catalogue::FeatureCatalogue::new();
    for (name, value_type) in [
        ("microprice", Type::Exact),
        ("mid", Type::Exact),
        ("spread", Type::Exact),
        ("imbalance", Type::Statistic),
        ("volatility", Type::Statistic),
        ("momentum", Type::Statistic),
        ("trades", Type::Count),
        ("cancels", Type::Count),
        ("halted", Type::Flag),
        ("auction", Type::Flag),
    ] {
        catalogue.declare(FeatureKey::new(name, on.clone()), value_type)?;
    }
    Ok(catalogue)
}

/// Register `count` candidates under `lineage` through the production path:
/// a `StrategyFoundry` searches, and its `register` hands the survivors to the
/// factory with the search's own trial count attached.
///
/// Through the foundry and not `StrategyFactory::register` directly, because
/// the family the review reads is the *provenance* family — the sweep's
/// lineage — and the foundry is what mints it. A test that named the family
/// itself would prove the review can read a name a test wrote.
fn register_family(platform: &mut Platform, lineage: &str, count: usize) -> Result<()> {
    use qip_evolution::grammar::Grammar;
    use qip_evolution::palette::FeaturePalette;
    use qip_kernel::central::foundry::{HoldoutInputs, StrategyFoundry};

    let on = ObjectId::from_string("obj-AAA");
    let catalogue = family_catalogue(&on)?;
    let grammar = Grammar::over(FeaturePalette::from_catalogue(&catalogue, &on)?);
    let mut foundry = StrategyFoundry::new(
        catalogue,
        grammar,
        CELL,
        venue(),
        lineage,
        // One seed per lineage, so two families are two different searches
        // rather than the same strategies under two names.
        u64::from(lineage.len() as u32) * 7 + 3,
    )?;
    foundry.search(count.max(1) * 4)?;
    let pending: Vec<StrategyId> = foundry
        .pending()
        .iter()
        .take(count)
        .map(|candidate| candidate.id().clone())
        .collect();
    assert_eq!(
        pending.len(),
        count,
        "premise: the search produced {count} compilable candidates for {lineage}"
    );
    for strategy in pending {
        foundry.register(
            platform.central_mut().factory_mut(),
            &strategy,
            HoldoutInputs {
                returns: good_returns(9, 300, 0.0018),
                in_sample_folds: vec![vec![0.001; 40]],
                out_of_sample_folds: vec![vec![0.0006; 20]],
                periods_per_year: 252.0,
                cross_validation: honest_cross_validation(300)?,
                leakage: clean_leakage_audit(),
                manifest: fixture_manifest()?,
            },
            start(),
        )?;
    }
    Ok(())
}

/// Every family-allocation review this process journalled, in stream order.
///
/// Read from the journal, which carries the bodies typed. `filter_map` on the
/// decode rather than `?`, because the topic carries two bodies and a
/// `MisallocationFinding` is not a review.
fn family_reviews(
    platform: &Platform,
) -> Result<Vec<qip_kernel::family_review::FamilyAllocationReview>> {
    use qip_events::{EventFilter, Topic};
    Ok(platform
        .replay_journal(&EventFilter::new().topic(Topic::FamilyAllocationReviewed))?
        .iter()
        .filter_map(|envelope| {
            envelope
                .decode::<qip_kernel::family_review::FamilyAllocationReview>()
                .ok()
                .map(|decoded| decoded.body)
        })
        .collect())
}

/// The deduplication keys the **event log** holds on the family-review topic.
///
/// The log and not the journal, because the log is what spans a restart and
/// the journal is this process's own — a test that counted journal records
/// would see a restarted platform as having written nothing, which is the
/// opposite of the property under test. Keys and not decoded bodies, because
/// the log stores the sealed frame and `AnyEvent::decode` reads the frame
/// rather than the body inside it; `dedup_key` is the exact string
/// `journal_once` consults, so this is the set the suppression is made of.
fn family_review_keys(platform: &Platform) -> Result<Vec<String>> {
    use qip_events::Topic;
    let mut out = Vec::new();
    platform.event_log().replay(|event| {
        if event.topic == Topic::FamilyAllocationReviewed {
            out.push(event.dedup_key());
        }
        Ok(())
    })?;
    Ok(out)
}

#[test]
fn the_learn_stage_reviews_family_allocation_on_a_population_the_foundry_actually_registered()
-> Result<()> {
    // Blueprint §12.3's fifth row reaching the cycle. Two sweeps register
    // candidates through `StrategyFoundry::register` — the production path —
    // and the LEARN stage measures where each family stands and puts it on
    // the record.
    //
    // What this does NOT assert is as much the point as what it does: no
    // weight, grant, budget or bound moves, because there is no code path
    // that would (ADR 0064). The review is a measurement and a record.
    let mut platform = platform()?;

    // Premise: before any family is registered, the review has nothing to
    // journal — so a record found afterwards is this population's and not
    // something the cycle writes unconditionally.
    let quiet = platform.run_cycle(start());
    let quiet_learn = quiet
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        !quiet_learn
            .detail
            .contains("reviewed against funding standing"),
        "an empty population journalled a family review: {}",
        quiet_learn.detail
    );
    assert!(
        family_reviews(&platform)?.is_empty(),
        "premise: no family review is on the record yet"
    );

    register_family(&mut platform, "sweep-alpha", 2)?;
    register_family(&mut platform, "sweep-omega", 3)?;
    assert_eq!(
        platform.central().factory().candidates().count(),
        5,
        "premise: the foundry registered five candidates across two families"
    );
    assert_eq!(
        platform.family_standings().len(),
        2,
        "premise: the factory groups them into exactly two families"
    );

    let cycle = platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    let learn = cycle
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        learn.detail.contains(
            "2 strategy family(ies) reviewed against funding standing, 0 of \
             them funded"
        ),
        "the LEARN stage did not report the family review: {}",
        learn.detail
    );
    // Both families by name, and the counts that distinguish them, so a
    // review that found one family twice would fail here.
    assert!(
        learn.detail.contains("sweep-alpha (2 member(s), 0 funded)"),
        "the first sweep is not named on the cycle: {}",
        learn.detail
    );
    assert!(
        learn.detail.contains("sweep-omega (3 member(s), 0 funded)"),
        "the second sweep is not named on the cycle: {}",
        learn.detail
    );

    let records = family_reviews(&platform)?;
    assert_eq!(
        records.len(),
        1,
        "the review is not on the record exactly once"
    );
    assert_eq!(records[0].families, 2);
    assert_eq!(records[0].funded_families, 0);
    // Both families are refused at this point and the record says so: a
    // registered candidate has not been in front of the holdout gate, so its
    // family's lifetime trial count is zero and there is nothing to deflate
    // against. Refused rather than scored zero, and counted rather than
    // dropped, so a family of ten unevaluated members does not read as a
    // family of none.
    for standing in &records[0].standings {
        assert_eq!(
            standing.admitted, 0,
            "{} was scored unevaluated",
            standing.family
        );
        assert_eq!(standing.refused, standing.members);
    }
    assert_eq!(
        records[0]
            .standings
            .iter()
            .map(|standing| standing.family.as_str())
            .collect::<Vec<_>>(),
        vec!["sweep-alpha", "sweep-omega"],
        "the record's families are not in name order, so a replay would reorder them"
    );
    // Nothing was funded, so there is no funded side to compare against and
    // no finding — the state every deployment of this platform is in, stated
    // rather than inferred.
    assert!(
        !learn
            .detail
            .contains("holds no capital and its deflated evidence"),
        "a misallocation finding was raised with nothing funded: {}",
        learn.detail
    );

    // And the admitting half of the deflation, without which every standing in
    // this platform would read `admitted: 0` for ever and the comparison would
    // be a control that cannot fire — the `MaxExpectedShortfall` shape, which
    // is the one thing ADR 0064 exists to refuse. Putting one candidate of one
    // family in front of the holdout gate charges its family's lifetime count,
    // and the next cycle's standing reads that family as admitted.
    let evaluated = platform
        .central()
        .factory()
        .candidates()
        .find(|candidate| candidate.family().as_str() == "sweep-alpha")
        .map(|candidate| candidate.strategy().clone())
        .ok_or_else(|| qip_core::Error::not_found("a candidate of the first sweep"))?;
    // The gate's verdict is not the subject here — the charge happens before
    // the gate reads, which is the whole reason a failed evaluation still
    // counts as a trial — so a refusal is tolerated and the count is asserted.
    let _ = platform.central_mut().factory_mut().promote(
        &evaluated,
        None,
        "put in front of the holdout gate",
        start(),
    );
    let standings = platform.family_standings();
    let alpha = standings
        .get("sweep-alpha")
        .ok_or_else(|| qip_core::Error::not_found("the first sweep stands"))?;
    assert_eq!(
        alpha.admitted, 2,
        "the holdout charge left the family's members unscoreable, so the comparison could          never fire for any population"
    );
    assert_eq!(alpha.refused, 0);
    assert_eq!(
        standings
            .get("sweep-omega")
            .map(|standing| standing.admitted),
        Some(0),
        "the sweep that was never evaluated is still refused, so the charge is what admitted          the other one"
    );
    Ok(())
}

/// The cycle counter starts at zero on every `Platform::new` — it is not
/// resumed from the log (`grep -n 'cycle: 0,' qip-kernel/src/platform.rs`) —
/// so a process restarted over a journal it already wrote runs a *second*
/// cycle 1. Every record the LEARN stage keys on the cycle therefore has a
/// twin waiting on the next restart, and `qip-api` and `qip-deepbrain` both
/// mount a file-backed journal that spans one.
///
/// The failure this refuses: two records on the log, each claiming to be the
/// family-allocation measurement for cycle 1, disagreeing about a population
/// that changed between the two runs. An auditor replaying the log cannot
/// tell which is the measurement and which is the restart, and the row's
/// whole claim is that it is reproducible from the log alone.
#[test]
fn a_platform_restarted_over_its_own_log_journals_no_second_family_review_for_the_same_cycle()
-> Result<()> {
    use qip_risk::limits::LimitSet;

    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-family-review-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let at = start().saturating_add(Duration::from_mins(5));

    let first_keys = {
        let config = PlatformConfig::default().with_event_log_file(&path);
        let (context, _clock) = Context::deterministic(start(), config.seed);
        let mut platform = Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe(),
            LimitSet::conservative_default(),
        )?;
        register_family(&mut platform, "sweep-alpha", 2)?;
        platform.run_cycle(at);
        let keys = family_review_keys(&platform)?;
        assert_eq!(keys.len(), 1, "premise: one cycle wrote one record");
        assert_eq!(
            family_reviews(&platform)?.len(),
            1,
            "premise: that record is a review and decodes as one"
        );
        keys
    };

    // The restart. A fresh platform over the same file, the same population
    // registered again, the same cycle number reached — because the counter
    // starts at zero and is not resumed.
    let config = PlatformConfig::default().with_event_log_file(&path);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut restarted = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        LimitSet::conservative_default(),
    )?;
    assert_eq!(
        family_review_keys(&restarted)?,
        first_keys,
        "premise: the restarted platform reads the record the first run wrote, so a duplicate \
         would be visible here"
    );
    register_family(&mut restarted, "sweep-alpha", 2)?;
    restarted.run_cycle(at);

    let after_restart = family_review_keys(&restarted)?;
    assert_eq!(
        after_restart, first_keys,
        "the restart wrote a second family review under a key the log already held"
    );

    // The admitting half. A cycle the log has *not* seen is journalled
    // normally — without this the assertion above would pass just as well
    // against a review that had stopped writing anything after the first
    // record, which is a different and worse platform.
    restarted.run_cycle(at.saturating_add(Duration::from_mins(5)));
    let after_next = family_review_keys(&restarted)?;
    assert_eq!(
        after_next.len(),
        2,
        "the next cycle's own measurement was suppressed along with the duplicate: {after_next:?}"
    );
    assert_ne!(
        after_next[0], after_next[1],
        "two records share one key, so one of them is a duplicate"
    );
    assert!(
        after_next[0].starts_with("learning.family_allocation_reviewed:family-review:"),
        "the key is not the review's own: {}",
        after_next[0]
    );

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
#[test]
fn the_slot_the_centre_ships_carries_the_withdrawn_set_it_applies_and_states_no_grid() -> Result<()>
{
    // Policy slot 11 had no producer anywhere, which is why a venue withdrawn
    // at the centre reached a cell only by being omitted from the next
    // whitelist — and a cell that had already installed its desk took no new
    // whitelist. What the producer ships is bounded two ways, and both are
    // asserted here because either failing would be a different defect.
    //
    // First, it ships what the centre is *applying*: `withdrawn_venues` is
    // written only after the `venue.withdrawn` record is in the log, and it
    // is the same field `cycle_whitelist_for` retains against, so the set a
    // cell refuses on and the set the whitelist omits on cannot disagree.
    //
    // Second, the three grid maps stay empty. `central::whitelist`'s register
    // refuses them because the centre's grids are keyed by instrument and the
    // slot is keyed by venue, and `qip_edge::feasibility::effective` takes a
    // slot grid in *preference* to the cell's own — so a re-keyed grid would
    // replace the right number rather than sit beside it. A producer that
    // quietly began filling them would be signing a number nobody computed,
    // and this is the assertion that fails when it does.
    const OTHER_CELL: &str = "cell-fra-1";
    let now = start();
    let id = strategy();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    issue(platform.central_mut(), &id, CELL, now)?;
    assert!(
        !platform.issue_cycle_whitelist(CELL, now)?.is_empty(),
        "the premise failed: the grant emits no whitelist, so nothing is being withdrawn from"
    );
    let before = platform.feasibility_constraints(now);
    assert!(
        before.withdrawn_venues.is_empty(),
        "the premise failed: something was already withdrawn: {:?}",
        before.withdrawn_venues
    );

    for _ in 0..6 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
    }
    for _ in 0..4 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        withdrawals(&platform)?.len(),
        1,
        "the premise failed: no withdrawal was journaled, so the slot has nothing to carry"
    );

    let shipped = platform.feasibility_constraints(now);
    assert_eq!(
        shipped.withdrawn_venues,
        [VENUE.to_string()].into_iter().collect(),
        "the slot does not carry the venue the platform withdrew"
    );
    assert_eq!(
        shipped.withdrawn_venues,
        platform.withdrawn_venues().clone(),
        "the slot and the set the whitelist is retained against disagree"
    );
    assert!(
        shipped.minimum_order.is_empty() && shipped.fee_floor.is_empty() && shipped.tick.is_empty(),
        "the producer began stating a per-venue grid the centre does not hold: {shipped:?}"
    );
    Ok(())
}

#[test]
fn a_repeated_echo_of_one_withdrawal_is_counted_in_full_and_seated_once() -> Result<()> {
    // Two failures this sits between, and the shape that satisfies both.
    //
    // Once the cells refuse a withdrawn venue at pass time, a desk installed
    // before the withdrawal reports one such refusal *per intent per pass*,
    // for as long as it keeps offering cycles through that venue. Admitted
    // whole to a 256-entry rate window they evict every genuine refusal in a
    // few passes and then hold the denominator every other venue's share is
    // measured against, so no second venue could ever reach three in four —
    // a withdrawal control that reads as protection and cannot fire twice,
    // which is the `MaxExpectedShortfall` shape this repository names as the
    // template for what not to ship.
    //
    // Kept out of the window entirely — which is what this test asserted
    // until a security review of the merged lane — the withdrawn venue
    // vanishes from the *denominator* instead, and the runner-up becomes a
    // cluster of whatever remains. That is the finding
    // `a_venue_withdrawn_on_edge_evidence_stays_in_the_denominator_the_runner_up_is_judged_against`
    // below drives; the old assertion here ("admitted to nothing") was the
    // defect, not the guarantee, and it is replaced rather than relaxed.
    //
    // So: the first echo of a venue in a report takes a window seat, because
    // a venue the platform is still attempting belongs in the denominator;
    // every repeat in the same report is counted on the series and seated
    // nowhere, because the number of intents a stale desk enumerated is a
    // fact about the desk and not about the venue.
    const OTHER_CELL: &str = "cell-fra-1";
    const FIRST: &str = "XLON";
    let now = start();
    let mut platform = platform_with_arbitrage(&[VENUE, FIRST])?;

    for _ in 0..6 {
        platform.ingest_cell_report(report_with_lot_refusal(FIRST), now)?;
    }
    for _ in 0..4 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, FIRST), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![FIRST],
        "the premise failed: the first venue was not withdrawn, so there is no echo to make"
    );
    let seated_before = platform.feasibility_refusals().len();
    assert_eq!(
        seated_before, 10,
        "the premise failed: the first cluster is not ten entries: {seated_before}"
    );

    // One pass of a stale desk offering eight cycles through the withdrawn
    // venue. Eight refusals on the wire; one seat.
    let ingestion =
        platform.ingest_cell_report(report_with_withdrawn_venue_refusals(CELL, FIRST, 8), now)?;
    assert_eq!(
        ingestion.feasibility_refusals.len(),
        1,
        "a pass's whole intent fan-out took a window seat each: {:?}",
        ingestion.feasibility_refusals
    );
    assert_eq!(
        ingestion.feasibility_refusals[0].venue, FIRST,
        "the seat was charged to another venue than the one refused"
    );
    assert!(
        ingestion.feasibility_refusals_unattributed.is_empty(),
        "the echo was filed as unattributable, which is the label that means a cell used a gate \
         name this build does not know: {:?}",
        ingestion.feasibility_refusals_unattributed
    );
    assert_eq!(
        ingestion.feasibility_refusals_repeated.len(),
        7,
        "the repeats were not carried, so the series under-counts what the cell refused: {:?}",
        ingestion.feasibility_refusals_repeated
    );
    assert!(
        ingestion
            .feasibility_refusals_repeated
            .iter()
            .all(|(venue, gate)| {
                venue == FIRST && gate == qip_contracts::feasibility::GATE_WITHDRAWN_VENUE
            }),
        "a repeat was counted under another venue or another gate: {:?}",
        ingestion.feasibility_refusals_repeated
    );
    assert_eq!(
        platform.feasibility_refusals().len(),
        seated_before + 1,
        "eight refusals in one report moved the window by other than one seat"
    );
    assert_eq!(
        feasibility_refusals_under(
            &platform,
            FIRST,
            qip_contracts::feasibility::GATE_WITHDRAWN_VENUE
        ),
        8,
        "the series counts something other than the eight refusals the cell made, so an \
         operator cannot see how hard the withdrawal is biting at the edge"
    );
    Ok(())
}

#[test]
fn a_venue_withdrawn_on_edge_evidence_stays_in_the_denominator_the_runner_up_is_judged_against()
-> Result<()> {
    // The cascade this refuses, from the seam it was broken at. A security
    // review of ADR 0062's edge closure found it: `venue_review::assess`
    // documents that a withdrawn venue's later refusals keep the window's
    // denominator honest, and at the desk they do — `OrderManager::submit`
    // runs the feasibility gate before the withdrawn-venue check. At a cell
    // there is no such ordering: every later refusal at the withdrawn venue
    // comes back under `feasibility_withdrawn_venue`, and the centre dropped
    // all of them. So on an edge-only fleet — which is the designed state,
    // since no execution node is deployed and the desk contributes nothing —
    // the withdrawn venue left the denominator the moment it was withdrawn,
    // the runner-up became a cluster of what remained, and the platform
    // withdrew its way down to no venue at all. The unit test named
    // `withdrawing_one_venue_does_not_make_the_runner_up_a_cluster_of_the_remainder`
    // kept passing throughout, because it is arithmetic over a synthetic
    // window and what changed was upstream of it — which is why this one
    // drives the window across a withdrawal from the edge seam instead.
    const OTHER_CELL: &str = "cell-fra-1";
    const FIRST: &str = "XLON";
    let now = start();
    let mut platform = platform_with_arbitrage(&[VENUE, FIRST])?;

    // Sixteen refusals at XLON and four at XNYS: four in five, from two
    // cells, so XLON is withdrawn and XNYS is the runner-up at one in five.
    for _ in 0..10 {
        platform.ingest_cell_report(report_with_lot_refusal(FIRST), now)?;
    }
    for _ in 0..6 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, FIRST), now)?;
    }
    for _ in 0..2 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
    }
    for _ in 0..2 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![FIRST],
        "the premise failed: the first venue was not withdrawn, so there is no denominator \
         question to ask"
    );

    // The cells keep routing there — the stale desk ADR 0062's amendment
    // describes — and the runner-up keeps refusing on its own account. With
    // the echoes dropped, forty-eight further XNYS refusals would make it
    // fifty-two of sixty-eight, three in four of "what remains", and XNYS
    // would be withdrawn on evidence that never grew relative to the fleet's
    // whole refusal traffic. The echoes hold XLON in the denominator, so the
    // same forty-eight are fifty-two of eighty-four.
    for _ in 0..16 {
        platform.ingest_cell_report(report_with_withdrawn_venue_refusals(CELL, FIRST, 1), now)?;
    }
    for _ in 0..24 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
    }
    for _ in 0..24 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    let seated = platform.feasibility_refusals().len();
    assert_eq!(
        seated, 84,
        "the premise failed: the window is not the sixteen XLON refusals, the sixteen echoes \
         and the fifty-two XNYS refusals this test reasons about: {seated}"
    );
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![FIRST],
        "the runner-up was withdrawn as a cluster of the remainder, so the platform withdrew \
         its way down to no venue"
    );

    // And the admitting half, without which the guard above would be
    // indistinguishable from a control that refuses every second withdrawal:
    // XNYS genuinely dominating *all* recent refusals still withdraws it.
    // A hundred against XLON's weight of thirty-two is better than three in
    // four, and it is found.
    for _ in 0..24 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
    }
    for _ in 0..24 {
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![FIRST, VENUE],
        "a venue dominating every recent refusal was not withdrawn, so the first withdrawal \
         disabled the control"
    );
    Ok(())
}

#[test]
fn a_cell_citing_a_withdrawal_the_centre_does_not_hold_is_evidence_and_not_an_echo() -> Result<()> {
    // The security finding this closes. Whether a refusal counted as "the
    // platform citing itself" — and was therefore kept out of the evidence
    // window — used to be decided by the gate string on a report arriving
    // over a wire `qip-edge/src/mesh.rs` says authenticates nobody. No
    // attacker is needed to break that: `Cell::feasibility_constraints`
    // reads slot 11 whatever its freshness, and `self.policy` is replaced
    // only when a new policy is applied, so a cell holding a stale slot
    // after a reinstatement — or after the centre simply stopped shipping
    // policy — refuses every intent at a venue *currently in use* under
    // `feasibility_withdrawn_venue`, for ever.
    //
    // Three consequences followed, and the third is the one that matters:
    // the centre charted a withdrawal that no longer existed, an operator
    // read it as one, and none of those refusals reached the window — so the
    // venue could not be withdrawn a second time on edge evidence for as
    // long as the slot stayed stale. A control that reads as protection and
    // cannot fire. The centre's own withdrawn set decides now, so a cell
    // citing a withdrawal the centre does not hold is an ordinary refusal at
    // a venue in use.
    const OTHER_CELL: &str = "cell-fra-1";
    let now = start();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    assert!(
        platform.withdrawn_venues().is_empty(),
        "the premise failed: the centre already holds a withdrawal, so the cell's claim would \
         be true: {:?}",
        platform.withdrawn_venues()
    );

    for cell in [CELL, OTHER_CELL] {
        for _ in 0..5 {
            let ingestion = platform
                .ingest_cell_report(report_with_withdrawn_venue_refusals(cell, VENUE, 1), now)?;
            assert_eq!(
                ingestion.feasibility_refusals.len(),
                1,
                "a cell's claim about a venue the centre has not withdrawn kept its own \
                 refusal out of the window: {ingestion:?}"
            );
            assert!(
                ingestion.feasibility_refusals_repeated.is_empty(),
                "the centre believed the cell's gate string over its own withdrawn set: {:?}",
                ingestion.feasibility_refusals_repeated
            );
        }
    }
    assert_eq!(
        platform.feasibility_refusals().len(),
        10,
        "the premise failed: the ten refusals are not all in the window"
    );

    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![VENUE],
        "ten corroborated refusals at a venue the centre has not withdrawn withdrew nothing, \
         so a stale slot at one cell can hold the control shut"
    );
    Ok(())
}
/// `count` lot-gate refusals at `venue`, all on one report from `cell` —
/// one pass's whole intent fan-out arriving as a single message.
fn report_with_many_lot_refusals(cell: &str, venue: &str, count: usize) -> CellReport {
    CellReport::new(cell, start()).with_refusals(
        (0..count)
            .map(|_| qip_mesh::delta::DeltaRefusal {
                gate: "feasibility_lot".to_string(),
                reason: "10.5 is not a whole number of lots".to_string(),
                venue: Some(venue.to_string()),
            })
            .collect(),
    )
}

/// Two operators putting `venue` back, both credentials fresh at `now`.
fn reinstate(platform: &mut Platform, venue: &str, now: Timestamp) -> Result<()> {
    let first =
        qip_risk_engine::autonomy::OperatorIdentity::verified("alice", "hardware-token", now);
    let second =
        qip_risk_engine::autonomy::OperatorIdentity::verified("bram", "hardware-token", now);
    platform.reinstate_venue(
        venue,
        &first,
        "the venue's grid was corrected in the catalogue",
        now,
    )?;
    platform.reinstate_venue(
        venue,
        &second,
        "confirmed against the venue's own specification",
        now,
    )?;
    Ok(())
}

#[test]
fn one_report_cannot_be_the_whole_window_however_many_refusals_it_carries() -> Result<()> {
    // **A probe withdrew a venue for the entire platform in two messages.**
    // `venue_review::VENUE_WITHDRAWAL_MIN_CELLS` requires two distinct cells
    // before edge-only evidence can withdraw anything — but it counts
    // distinct cell *names*, not evidence per cell, so thirty refusals in
    // one report plus a single refusal from a second name cleared it, on a
    // wire `qip-edge/src/mesh.rs` says authenticates nobody. The
    // corroboration control can only mean something if one report cannot
    // fill the window, which is why `attribute_refusals` now seats the first
    // refusal per venue per gate in a report and counts the rest.
    //
    // The cap used to apply to `feasibility_withdrawn_venue` alone, and the
    // eight gates it did not cover are the ones this test uses.
    const OTHER_CELL: &str = "cell-fra-1";
    let now = start();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    let ingestion =
        platform.ingest_cell_report(report_with_many_lot_refusals(CELL, VENUE, 30), now)?;
    assert_eq!(
        ingestion.feasibility_refusals.len(),
        1,
        "a report bought more than one window seat for one venue under one gate"
    );
    assert_eq!(
        ingestion.feasibility_refusals_repeated.len(),
        29,
        "the repeats were not carried for counting: {:?}",
        ingestion.feasibility_refusals_repeated
    );
    // The series still counts every refusal the cell made — the cap is on
    // the evidence window, not on what an operator can see.
    assert_eq!(
        feasibility_refusals_under(&platform, VENUE, "feasibility_lot"),
        30,
        "the repeats were dropped from the series as well as from the window"
    );
    platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    assert_eq!(
        platform.feasibility_refusals().len(),
        2,
        "the premise: two reports are two seats"
    );
    platform.run_cycle(now);
    assert!(
        platform.withdrawn_venues().is_empty(),
        "two messages withdrew a venue for the whole platform: {:?}",
        platform.withdrawn_venues()
    );

    // The admitting half: the same thirty refusals, sent as a cell would
    // observe them — one report per pass — still withdraw the venue. The cap
    // bounds what one message may assert, not what a fleet may establish.
    for _ in 0..15 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![VENUE],
        "thirty corroborated refusals across thirty reports withdrew nothing, so the cap \
         disabled the control rather than bounding it"
    );
    Ok(())
}

#[test]
fn a_cell_that_has_not_heard_a_reinstatement_cannot_undo_it() -> Result<()> {
    // **The loop this refuses, measured before it was closed.** A
    // reinstatement makes every cell's policy slot 11 stale about that venue
    // *by construction* — a cell learns on its next policy frame, not on the
    // signature — so until the frame arrives, every intent there comes back
    // under `feasibility_withdrawn_venue`. Those are not echoes: the centre
    // no longer holds the withdrawal. Admitted as evidence about the venue,
    // they withdrew it again on the next LEARN pass, and a probe on this
    // platform did exactly that with a window of 256 entries of which not
    // one was a genuine refusal. The signatures made the cells stale, the
    // staleness withdrew the venue, and no reinstatement could ever have
    // survived a cycle while any two cells were behind.
    const OTHER_CELL: &str = "cell-fra-1";
    let now = start();
    let mut platform = platform_with_arbitrage(&[VENUE])?;
    for _ in 0..6 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![VENUE],
        "the premise failed: twelve corroborated refusals did not withdraw the venue"
    );
    reinstate(&mut platform, VENUE, now)?;
    assert!(
        platform.withdrawn_venues().is_empty(),
        "the premise failed: the venue is still out"
    );

    // Forty passes from each of two cells whose slot 11 never heard the
    // signatures. Interleaved, so both are still in the window at the end:
    // one cell alone would be stopped by the corroboration bar, and this
    // test is about the classification and not about that bar.
    for _ in 0..40 {
        for cell in [CELL, OTHER_CELL] {
            platform
                .ingest_cell_report(report_with_withdrawn_venue_refusals(cell, VENUE, 8), now)?;
        }
    }
    let window = platform.feasibility_refusals();
    assert!(
        window
            .iter()
            .filter(|refusal| refusal.constraint
                == qip_contracts::feasibility::GATE_WITHDRAWN_VENUE)
            .count()
            >= 60,
        "the premise failed: the stale refusals did not reach the window at all, so a pass \
         here would prove nothing about how they are weighed: {}",
        window.len()
    );
    platform.run_cycle(now);
    assert!(
        platform.withdrawn_venues().is_empty(),
        "a venue was withdrawn again because the cells had not yet heard it was back: {:?}",
        platform.withdrawn_venues()
    );

    // The admitting half, twice over. A venue the centre has *never*
    // withdrawn is not excused: a cell citing a withdrawal nobody made is
    // making an ordinary refusal at a venue in use, which is the security
    // finding this must not undo.
    const SECOND: &str = "XLON";
    let mut fresh = platform_with_arbitrage(&[SECOND])?;
    for _ in 0..5 {
        for cell in [CELL, OTHER_CELL] {
            fresh.ingest_cell_report(report_with_withdrawn_venue_refusals(cell, SECOND, 1), now)?;
        }
    }
    fresh.run_cycle(now);
    assert_eq!(
        fresh.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![SECOND],
        "a cell asserting a withdrawal the centre never made was excused as a stale cell, so \
         one stale slot can hold the control shut for ever"
    );
    // And the reinstated venue can still be withdrawn on evidence about the
    // venue rather than about the policy path.
    for _ in 0..12 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![VENUE],
        "a reinstated venue could never be withdrawn again, whatever its grid did"
    );
    Ok(())
}

#[test]
fn a_refusal_that_cannot_withdraw_a_venue_never_evicts_one_that_can() -> Result<()> {
    // A full window is a rate sample of *evidence*. An echo and a pardoned
    // refusal are denominator entries: neither can be a numerator,
    // corroborate a cluster, or name a constraint. If they could evict, a
    // cell reporting at a venue nobody is judging would push the evidence
    // every other venue's withdrawal rests on out of the window — until
    // fewer than `VENUE_WITHDRAWAL_MIN_SAMPLE` genuine entries survived and
    // no venue could be withdrawn at all, which is a control that cannot
    // fire arrived at by arithmetic rather than by anyone's decision.
    const OTHER_CELL: &str = "cell-fra-1";
    const SECOND: &str = "XLON";
    let now = start();
    let mut platform = platform_with_arbitrage(&[VENUE, SECOND])?;
    for _ in 0..6 {
        platform.ingest_cell_report(report_with_lot_refusal(VENUE), now)?;
        platform.ingest_cell_report(report_with_lot_refusal_from(OTHER_CELL, VENUE), now)?;
    }
    platform.run_cycle(now);
    assert_eq!(
        platform.withdrawn_venues().iter().collect::<Vec<_>>(),
        vec![VENUE],
        "the premise failed: the venue the echoes will name is not withdrawn"
    );
    // Fill the rest of the window with genuine refusals at a second venue,
    // below its own share bar, and then flood it with echoes of the first
    // venue's withdrawal — far more than the window could ever hold.
    while platform.feasibility_refusals().len() < 256 {
        platform.ingest_cell_report(report_with_lot_refusal(SECOND), now)?;
    }
    let genuine_before = platform
        .feasibility_refusals()
        .iter()
        .filter(|refusal| refusal.venue == SECOND)
        .count();
    assert!(
        genuine_before >= 200,
        "the premise failed: the window is not mostly the second venue's genuine refusals: \
         {genuine_before}"
    );
    for _ in 0..500 {
        platform.ingest_cell_report(report_with_withdrawn_venue_refusals(CELL, VENUE, 4), now)?;
    }
    assert_eq!(
        platform
            .feasibility_refusals()
            .iter()
            .filter(|refusal| refusal.venue == SECOND)
            .count(),
        genuine_before,
        "five hundred echoes evicted the evidence a second venue would be judged on"
    );
    // And the series still saw every one of them, so nothing is hidden —
    // only kept out of the window.
    assert!(
        feasibility_refusals_under(
            &platform,
            VENUE,
            qip_contracts::feasibility::GATE_WITHDRAWN_VENUE
        ) >= 2000,
        "the echoes were dropped from the series as well as from the window"
    );
    Ok(())
}

// --- blueprint rule 29: simulated against recorded data ----------------------

/// Daily bars as the foundry sees them, unwrapped from the sensed records.
fn recorded_bars(symbol: &str, count: usize) -> Vec<Bar> {
    bars(symbol, count)
        .into_iter()
        .filter_map(|record| match record {
            SensedRecord::Bar(bar) => Some(*bar),
            _ => None,
        })
        .collect()
}

#[test]
fn a_dataset_manifest_names_the_bars_by_content_so_an_edited_price_is_a_different_dataset()
-> Result<()> {
    use qip_kernel::central::foundry::recorded_manifest;

    let on = ObjectId::from_string("obj-AAA");
    let history = recorded_bars("AAA", 600);
    assert_eq!(history.len(), 600, "premise: the fixture produced the bars");

    let manifest = recorded_manifest(&on, &history)?;
    assert_eq!(manifest.bars, 600);
    assert_eq!(manifest.venue, VENUE);
    assert_eq!(manifest.first_open, history[0].open_time);
    assert_eq!(manifest.last_open, history[599].open_time);
    assert_eq!(manifest.content_hash.len(), 64);

    // The same history, manifested again, is the same dataset.
    assert_eq!(recorded_manifest(&on, &history)?, manifest);

    // One close edited by a tick is not.
    let mut edited = history.clone();
    edited[300].close += dec!("0.01");
    let other = recorded_manifest(&on, &edited)?;
    assert_ne!(other.content_hash, manifest.content_hash);
    assert_eq!(other.bars, manifest.bars, "only the content differs");

    // No history is no dataset, and a history at two venues is two.
    let error = recorded_manifest(&on, &[]).expect_err("nothing to manifest");
    assert!(
        error.message().contains("no recorded bars"),
        "{}",
        error.message()
    );
    let mut mixed = history.clone();
    mixed[10].venue = "XLON".to_string();
    let error = recorded_manifest(&on, &mixed).expect_err("two venues");
    assert!(
        error.message().contains("XNYS") && error.message().contains("XLON"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_ladder_refuses_holdout_evidence_that_was_never_simulated_and_admits_the_foundrys()
-> Result<()> {
    use qip_evolution::grammar::Grammar;
    use qip_evolution::palette::FeaturePalette;
    use qip_kernel::central::foundry::{HoldoutInputs, StrategyFoundry, recorded_manifest};

    // The failure this guards: the holdout gate deflated whatever series it
    // was handed, so a candidate whose "holdout" was typed into a fixture
    // climbed the first rung exactly as one that a SimulationClock had
    // walked over a year of bars. Both halves are proven on one platform:
    // the foundry's evidence — the production path, which manifests the
    // bars the candidate was simulated over — is admitted, and the same
    // evidence with the manifest removed is refused.
    let mut platform = platform()?;
    let on = ObjectId::from_string("obj-AAA");
    let catalogue = family_catalogue(&on)?;
    let grammar = Grammar::over(FeaturePalette::from_catalogue(&catalogue, &on)?);
    let mut foundry = StrategyFoundry::new(catalogue, grammar, CELL, venue(), "rule-29", 29)?;
    foundry.search(8)?;
    let pending: Vec<StrategyId> = foundry
        .pending()
        .iter()
        .take(2)
        .map(|candidate| candidate.id().clone())
        .collect();
    assert_eq!(
        pending.len(),
        2,
        "premise: the search produced two candidates"
    );

    let history = recorded_bars("AAA", 1_000);
    for strategy in &pending {
        foundry.register(
            platform.central_mut().factory_mut(),
            strategy,
            // The same series `strong_holdout` admits on, so the one check
            // under test is the only one that can decide the outcome.
            HoldoutInputs {
                returns: good_returns(1, 400, 0.0018),
                in_sample_folds: (0..5).map(|f| good_returns(10 + f, 80, 0.0020)).collect(),
                out_of_sample_folds: (0..5).map(|f| good_returns(20 + f, 80, 0.0018)).collect(),
                periods_per_year: 252.0,
                cross_validation: honest_cross_validation(400)?,
                leakage: clean_leakage_audit(),
                manifest: recorded_manifest(&on, &history)?,
            },
            start(),
        )?;
    }

    // The production path is admitted: the foundry attached the manifest of
    // the bars, and the gate read it. The promotion is asserted before the
    // manifest is inspected so that a foundry which stopped attaching it
    // fails this test on the admission — the property — and not on the
    // inspection.
    let factory = platform.central_mut().factory_mut();
    let simulated = &pending[0];
    let promotion = factory.promote(simulated, None, "simulated over recorded bars", start())?;
    assert_eq!(promotion.to, GateStage::Holdout);
    let carried = factory
        .candidate(simulated)
        .and_then(|candidate| candidate.evidence().simulation.clone())
        .ok_or_else(|| qip_core::Error::not_found("the registered candidate's manifest"))?;
    assert_eq!(carried.bars, 1_000);

    // The same evidence with nothing establishing a simulation.
    let unsimulated = &pending[1];
    let mut evidence = factory
        .candidate(unsimulated)
        .map(|candidate| candidate.evidence().clone())
        .ok_or_else(|| qip_core::Error::not_found("the second candidate"))?;
    assert!(
        evidence.simulation.is_some(),
        "premise: the foundry filled it"
    );
    evidence.simulation = None;
    factory.submit_evidence(unsimulated, evidence)?;
    let error = factory
        .promote(unsimulated, None, "a series from nowhere", start())
        .expect_err("holdout evidence without a dataset manifest is refused");
    assert_eq!(error.code(), "guard", "{error:?}");
    assert!(
        error
            .message()
            .contains("holdout_simulated_against_recorded_data"),
        "the refusal names the check: {}",
        error.message()
    );
    assert_eq!(
        factory.ledger().stage_of(unsimulated),
        GateStage::Candidate,
        "a refused promotion moves nothing"
    );
    Ok(())
}

// --- ADR 0079: a dark region is the centre's word for silence -----------------
//
// Every test in this section pins one half of the ADR's invariant: *no
// quantity the centre derives from a dark reading may be smaller than the
// same quantity derived from the region's last report, and nothing may be
// created by a region going dark.* The failure they prevent is the
// `MaxExpectedShortfall` shape with the sign reversed — a region going
// silent reading at the centre as the platform having become safer.

const DARK_WINDOW: Duration = Duration::from_mins(5);
const LON_REGION: &str = "europe-west2";
const NYC_REGION: &str = "us-east1";
const SIN_REGION: &str = "asia-southeast1";
const NYC_CELL: &str = "cell-nyc-1";
const SIN_CELL: &str = "cell-sin-1";

fn plane_with_dark_window() -> Result<CentralPlane> {
    CentralPlane::new(
        &[7u8; 32],
        CentralConfig {
            region_dark_after: Some(DARK_WINDOW),
            ..CentralConfig::default()
        },
    )
}

/// A report that says only "this cell, in this region, spoke".
fn heard(cell: &str, region: &str, at: Timestamp) -> CellReport {
    CellReport::new(cell, at).with_region(region)
}

/// One second past the window: the first instant a region silent since
/// `start()` reads dark.
fn past_window() -> Timestamp {
    start()
        .saturating_add(DARK_WINDOW)
        .saturating_add(Duration::from_secs(1))
}

fn dark_set(regions: &[&str]) -> std::collections::BTreeSet<String> {
    regions.iter().map(|region| (*region).to_string()).collect()
}

fn share_plan(allocations: &[(&str, &str)]) -> AllocationPlan {
    let allocations: Vec<Allocation> = allocations
        .iter()
        .map(|(cell, notional)| Allocation {
            strategy: StrategyId::new(format!("momentum-{cell}")),
            cell: (*cell).to_string(),
            venue: venue(),
            notional: Decimal::parse(notional).expect("a decimal literal"),
            indicated: Decimal::parse(notional).expect("a decimal literal"),
            risk_adjusted_edge: 0.01,
            binding_constraints: Vec::new(),
        })
        .collect();
    let budget = allocations
        .iter()
        .fold(Decimal::ZERO, |sum, allocation| sum + allocation.notional);
    AllocationPlan {
        at: start(),
        total_budget: budget,
        drawdown: 0.0,
        drawdown_multiplier: Decimal::ONE,
        budget,
        allocations,
        refusals: Vec::new(),
    }
}

fn share_amount(shares: &qip_kernel::central::RegionShares, cell: &str) -> Option<Decimal> {
    shares.for_cell(cell).map(RegionShare::amount)
}

fn region_records<B: qip_events::EventBody>(platform: &Platform) -> Result<Vec<B>> {
    use qip_events::EventFilter;
    platform
        .replay_journal(&EventFilter::new().topic(B::TOPIC))?
        .iter()
        .map(|envelope| Ok(envelope.decode::<B>()?.body))
        .collect()
}

#[test]
fn a_region_dark_window_is_refused_at_zero_and_past_the_envelope_ceiling_and_admitted_between()
-> Result<()> {
    // Zero would derive every region dark between any two reports; a window
    // past the envelope ceiling would notice a region's silence only after
    // every grant in it had expired on its own — a control that fires after
    // the fact it exists to catch. Both are refused at construction rather
    // than clamped, and the refusal names the field so an operator knows
    // which number to change. Between the two the window is admitted, and
    // the default is *off* and says so: the ADR refuses a default on
    // purpose, because there is no measurement to pick one from.
    let with = |window| {
        CentralPlane::new(
            &[7u8; 32],
            CentralConfig {
                region_dark_after: Some(window),
                ..CentralConfig::default()
            },
        )
    };
    let zero = with(Duration::ZERO)
        .err()
        .ok_or_else(|| qip_core::Error::invalid("a zero window was admitted"))?;
    assert!(
        zero.message().contains("region_dark_after"),
        "the refusal does not name the field: {}",
        zero.message()
    );
    let past = with(Duration::from_millis(
        MAXIMUM_ENVELOPE_VALIDITY.as_millis() + 1,
    ))
    .err()
    .ok_or_else(|| qip_core::Error::invalid("a window past the ceiling was admitted"))?;
    assert!(
        past.message().contains("region_dark_after") && past.message().contains("ceiling"),
        "the refusal does not name the field and the ceiling: {}",
        past.message()
    );
    assert_eq!(
        with(MAXIMUM_ENVELOPE_VALIDITY)?.region_dark_after(),
        Some(MAXIMUM_ENVELOPE_VALIDITY),
        "a window exactly at the ceiling is the longest permitted, and was refused"
    );
    assert_eq!(
        plane_with_dark_window()?.region_dark_after(),
        Some(DARK_WINDOW)
    );
    assert_eq!(
        plane()?.region_dark_after(),
        None,
        "the default configuration must leave the derivation off, not pick a window"
    );
    Ok(())
}

#[test]
fn a_region_silent_past_the_window_is_dark_on_the_next_read_and_not_before() -> Result<()> {
    let mut plane = plane_with_dark_window()?;
    let mut switch = qip_risk_engine::autonomy::AutonomyController::new();
    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        start(),
    )?;
    // A cell that names no region derives nothing, however long it is silent:
    // it is unknown, not dark, and unknown already receives nothing.
    plane.ingest(
        heard("cell-nowhere", "", start()),
        switch.kill_switch_mut(),
        start(),
    )?;

    assert!(plane.dark_regions(start()).is_empty());
    let at_window = start().saturating_add(DARK_WINDOW);
    assert!(
        plane.dark_regions(at_window).is_empty(),
        "silent *for* the window read as silent *past* it"
    );
    assert_eq!(
        plane.dark_regions(past_window()),
        dark_set(&[LON_REGION]),
        "a region silent past the window did not read dark on the next read"
    );
    let reading = plane
        .darkness_of(CELL, past_window())
        .ok_or_else(|| qip_core::Error::not_found("the cell's region is dark"))?;
    assert_eq!(reading.region, LON_REGION);
    assert_eq!(reading.last_heard_from, CELL);
    assert_eq!(reading.last_heard_at, start());
    assert_eq!(reading.window, DARK_WINDOW);
    assert_eq!(reading.dark_since(), at_window);
    assert!(
        plane.darkness_of("cell-nowhere", past_window()).is_none(),
        "a cell in no region was read as being in a dark one"
    );

    // Silence is measured on the centre's clock: a report whose own `at` is
    // old but arrives now is heard now, because a silent cell's clock is
    // exactly what the centre cannot read.
    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        past_window(),
    )?;
    assert!(plane.dark_regions(past_window()).is_empty());

    // With no window nothing is ever dark, and the plane says why rather
    // than reporting a healthy fleet.
    let mut off = CentralPlane::new(&[7u8; 32], CentralConfig::default())?;
    off.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        start(),
    )?;
    assert!(
        off.dark_regions(start().saturating_add(Duration::from_days(30)))
            .is_empty()
    );
    assert_eq!(off.region_dark_after(), None);
    Ok(())
}

#[test]
fn a_report_from_a_dark_region_clears_it_and_each_transition_is_offered_until_announced()
-> Result<()> {
    // The transition is what is journaled, and it is offered by the plane
    // until the platform says the record is in the log — so a journal that
    // fails leaves the change pending rather than lost. The announced set
    // decides nothing: the derivation stays dark whether or not the record
    // was written.
    let mut plane = plane_with_dark_window()?;
    let mut switch = qip_risk_engine::autonomy::AutonomyController::new();
    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        start(),
    )?;
    assert!(plane.region_transitions(start()).is_empty());

    let offered = plane.region_transitions(past_window());
    let [RegionTransition::WentDark(reading)] = offered.as_slice() else {
        panic!("one region went dark and the plane offered {offered:?}");
    };
    assert_eq!(reading.region, LON_REGION);
    assert_eq!(
        plane.region_transitions(past_window()),
        offered,
        "a transition nobody announced was not offered again"
    );
    plane.announce(&offered[0]);
    assert!(plane.region_transitions(past_window()).is_empty());
    assert!(plane.announced_dark().contains(LON_REGION));
    assert!(
        plane.dark_regions(past_window()).contains(LON_REGION),
        "announcing a transition changed the derivation, which must read only `last_heard`"
    );

    // Resumption is the first report from any cell of the region, through
    // the same door.
    let later = past_window().saturating_add(Duration::from_secs(1));
    plane.ingest(
        heard("cell-lon-2", LON_REGION, start()),
        switch.kill_switch_mut(),
        later,
    )?;
    assert!(plane.dark_regions(later).is_empty());
    let cleared = plane.region_transitions(later);
    assert_eq!(
        cleared,
        vec![RegionTransition::SpokeAgain {
            region: LON_REGION.to_string(),
            cell: "cell-lon-2".to_string(),
            heard_at: later,
        }]
    );
    plane.announce(&cleared[0]);
    assert!(plane.region_transitions(later).is_empty());
    assert!(!plane.announced_dark().contains(LON_REGION));
    Ok(())
}

#[test]
fn the_platform_journals_region_dark_from_the_act_stage_and_region_lit_at_the_report_that_cleared_it()
-> Result<()> {
    // The non-test call path: a region that went dark by the passage of
    // time is put on the record by the cycle's ACT stage, and the report
    // that clears it writes `region.lit` at the report rather than a cycle
    // later, so the two records bracket exactly the window in which grants
    // were refused. A second cycle does not write the same darkness twice.
    let config = PlatformConfig::default().with_central(CentralConfig {
        region_dark_after: Some(DARK_WINDOW),
        ..CentralConfig::default()
    });
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    platform.ingest_cell_report(heard(CELL, LON_REGION, start()), start())?;
    assert!(region_records::<RegionWentDark>(&platform)?.is_empty());

    platform.run_cycle(past_window());
    let dark = region_records::<RegionWentDark>(&platform)?;
    assert_eq!(
        dark.len(),
        1,
        "the ACT stage did not journal the region going dark: {dark:?}"
    );
    assert_eq!(dark[0].region, LON_REGION);
    assert_eq!(dark[0].last_heard_from, CELL);
    assert_eq!(dark[0].last_heard_at, start());
    assert_eq!(dark[0].window, DARK_WINDOW);
    assert_eq!(dark[0].dark_since, start().saturating_add(DARK_WINDOW));
    assert!(platform.central().announced_dark().contains(LON_REGION));

    platform.run_cycle(past_window().saturating_add(Duration::from_secs(1)));
    assert_eq!(
        region_records::<RegionWentDark>(&platform)?.len(),
        1,
        "a second cycle journaled the same darkness twice"
    );
    assert!(region_records::<RegionSpokeAgain>(&platform)?.is_empty());

    let heard_at = past_window().saturating_add(Duration::from_secs(2));
    platform.ingest_cell_report(heard(CELL, LON_REGION, start()), heard_at)?;
    let lit = region_records::<RegionSpokeAgain>(&platform)?;
    assert_eq!(
        lit.len(),
        1,
        "the clearing report did not journal `region.lit`: {lit:?}"
    );
    assert_eq!(lit[0].region, LON_REGION);
    assert_eq!(lit[0].cell, CELL);
    assert_eq!(lit[0].heard_at, heard_at);
    assert!(!platform.central().announced_dark().contains(LON_REGION));
    Ok(())
}

#[test]
fn a_grant_into_a_dark_region_is_refused_naming_the_reading_and_admitted_once_the_region_speaks()
-> Result<()> {
    let mut plane = plane_with_dark_window()?;
    let id = strategy();
    register(&mut plane, &id, CELL)?;
    walk_to(&mut plane, &id, GateStage::Pilot)?;
    let mut switch = qip_risk_engine::autonomy::AutonomyController::new();
    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        start(),
    )?;

    let error = match issue(&mut plane, &id, CELL, past_window()) {
        Ok(issued) => panic!(
            "a grant was issued into a dark region: {}",
            issued.envelope().signing_payload()
        ),
        Err(error) => error,
    };
    for expected in [LON_REGION, CELL, "is dark", "window"] {
        assert!(
            error.message().contains(expected),
            "the refusal does not carry `{expected}`: {}",
            error.message()
        );
    }
    assert!(
        plane.envelope(CELL, &id).is_none(),
        "a refused grant left an envelope behind"
    );

    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        past_window(),
    )?;
    let issued = issue(&mut plane, &id, CELL, past_window())?;
    assert_eq!(issued.envelope().strategy(), &id);
    assert!(plane.envelope(CELL, &id).is_some());
    Ok(())
}

#[test]
fn a_dark_regions_share_bound_is_frozen_at_its_last_lit_value_while_the_plan_moves_and_no_other_bound_moves()
-> Result<()> {
    // ADR 0079 decision four. The allocator keeps sizing while a region is
    // silent, and the plan it produces will move; a dark region's cells are
    // partitioned at the bound they last had while lit, a lit region's at
    // the plan's current figure, and a dark cell that was never partitioned
    // while lit is withheld with the reason — nothing new exists because a
    // region went quiet.
    let mut plane = plane_with_dark_window()?;
    let mut switch = qip_risk_engine::autonomy::AutonomyController::new();
    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        start(),
    )?;
    plane.ingest(
        heard(NYC_CELL, NYC_REGION, start()),
        switch.kill_switch_mut(),
        start(),
    )?;
    let membership = RegionMembership::new(
        BTreeMap::from([
            (LON_REGION.to_string(), dec!("1000")),
            (NYC_REGION.to_string(), dec!("1000")),
        ]),
        BTreeMap::from([
            (CELL.to_string(), LON_REGION.to_string()),
            ("cell-lon-2".to_string(), LON_REGION.to_string()),
            (NYC_CELL.to_string(), NYC_REGION.to_string()),
        ]),
    )?;

    let lit = plane.region_shares(
        &share_plan(&[(CELL, "600"), (NYC_CELL, "500")]),
        &membership,
        start(),
    )?;
    assert_eq!(share_amount(&lit, CELL), Some(dec!("600")));
    assert_eq!(share_amount(&lit, NYC_CELL), Some(dec!("500")));
    assert_eq!(share_amount(&lit, "cell-lon-2"), Some(Decimal::ZERO));

    // New York speaks again; London does not.
    plane.ingest(
        heard(NYC_CELL, NYC_REGION, start()),
        switch.kill_switch_mut(),
        past_window(),
    )?;
    assert_eq!(plane.dark_regions(past_window()), dark_set(&[LON_REGION]));
    let moved = share_plan(&[(CELL, "300"), (NYC_CELL, "700"), ("cell-lon-2", "100")]);
    let dark = plane.region_shares(&moved, &membership, past_window())?;
    assert_eq!(
        share_amount(&dark, CELL),
        Some(dec!("600")),
        "the dark region's bound moved with the plan"
    );
    assert_eq!(
        share_amount(&dark, "cell-lon-2"),
        Some(Decimal::ZERO),
        "a dark cell was given a share the plan produced after the region went quiet"
    );
    assert_eq!(
        share_amount(&dark, NYC_CELL),
        Some(dec!("700")),
        "a lit region's bound was frozen too"
    );

    // A dark cell never partitioned while lit gets nothing, and the reason
    // travels with the withheld slot.
    let with_newcomer = RegionMembership::new(
        membership.grants().clone(),
        membership
            .cells()
            .iter()
            .map(|(cell, region)| (cell.clone(), region.clone()))
            .chain(std::iter::once((
                "cell-lon-3".to_string(),
                LON_REGION.to_string(),
            )))
            .collect(),
    )?;
    let withheld = plane.region_shares(&moved, &with_newcomer, past_window())?;
    assert!(withheld.for_cell("cell-lon-3").is_none());
    assert!(
        withheld
            .withheld()
            .get("cell-lon-3")
            .is_some_and(|reason| reason.contains("dark")),
        "the newcomer was not withheld with the reason: {:?}",
        withheld.withheld()
    );

    // Once London speaks the plan's current figure applies again.
    let later = past_window().saturating_add(Duration::from_secs(1));
    plane.ingest(
        heard(CELL, LON_REGION, start()),
        switch.kill_switch_mut(),
        later,
    )?;
    let lit_again = plane.region_shares(&moved, &membership, later)?;
    assert_eq!(share_amount(&lit_again, CELL), Some(dec!("300")));
    assert_eq!(share_amount(&lit_again, "cell-lon-2"), Some(dec!("100")));
    Ok(())
}

#[test]
fn after_a_region_goes_dark_no_derived_quantity_is_smaller_than_the_last_reports_and_nothing_new_exists()
-> Result<()> {
    // The invariant ADR 0079 decision seven writes out, in its own shape: a
    // book crowded across `minimum_cells_for_crowding` cells; silence one
    // past the window; every derived quantity is held and nothing new
    // exists; then the region speaks and it clears with none of the above
    // having moved. The mutation this catches is the one §36.3 row three
    // literally asks for — `positions.remove` for a silent cell — which
    // lowers the cell count `crowded` needs and withdraws a recall.
    let mut plane = plane_with_dark_window()?;
    let cells = [
        (CELL, LON_REGION),
        (NYC_CELL, NYC_REGION),
        (SIN_CELL, SIN_REGION),
    ];
    let ids: Vec<StrategyId> = cells
        .iter()
        .map(|(cell, _)| StrategyId::new(format!("momentum-{cell}")))
        .collect();
    for ((cell, _), id) in cells.iter().zip(&ids) {
        register(&mut plane, id, cell)?;
        walk_to(&mut plane, id, GateStage::Pilot)?;
        issue(&mut plane, id, cell, start())?;
    }
    let signatures_before: Vec<Option<String>> = cells
        .iter()
        .zip(&ids)
        .map(|((cell, _), id)| {
            plane
                .envelope(cell, id)
                .map(|envelope| envelope.signature().to_string())
        })
        .collect();
    assert!(signatures_before.iter().all(Option::is_some));
    let membership = RegionMembership::new(
        cells
            .iter()
            .map(|(_, region)| ((*region).to_string(), plane.config().per_cell))
            .collect(),
        cells
            .iter()
            .map(|(cell, region)| ((*cell).to_string(), (*region).to_string()))
            .collect(),
    )?;
    let report_of = |cell: &str, region: &str, id: &StrategyId, at: Timestamp| {
        heard(cell, region, at).with_positions(vec![position(cell, id, INSTRUMENT, dec!("1000"))])
    };
    let shares_at = |plane: &mut CentralPlane, at: Timestamp| -> BTreeMap<String, Decimal> {
        plane
            .grant_manifests(cells.iter().map(|(cell, _)| *cell), &membership, 0.0, at)
            .decisions()
            .iter()
            .filter_map(|(cell, decision)| match decision {
                ManifestDecision::Ship(share) => Some((cell.clone(), share.amount())),
                ManifestDecision::Withhold(_) => None,
            })
            .collect()
    };
    let findings = |findings: &[qip_capital::exposure::ConcentrationFinding]| {
        findings
            .iter()
            .map(|finding| (finding.axis, finding.bucket.clone(), finding.gross))
            .collect::<Vec<_>>()
    };

    let mut switch = qip_risk_engine::autonomy::AutonomyController::new();
    let mut last = None;
    for ((cell, region), id) in cells.iter().zip(&ids) {
        last = Some(plane.ingest(
            report_of(cell, region, id, start()),
            switch.kill_switch_mut(),
            start(),
        )?);
    }
    let lit = last.ok_or_else(|| qip_core::Error::not_found("three reports were ingested"))?;
    // The premise: the book is crowded across all three cells, breaches a
    // concentration, and recalls every one of them.
    assert_eq!(lit.crowded.len(), 1, "{:?}", lit.crowded);
    assert_eq!(lit.crowded[0].cells.len(), 3);
    assert!(!lit.concentrations.is_empty());
    assert_eq!(lit.recalls.len(), 3);
    let gross_lit = plane.gross_notional_by_cell();
    assert_eq!(gross_lit.len(), 3);
    let shares_lit = shares_at(&mut plane, start());
    assert_eq!(
        shares_lit.len(),
        3,
        "the premise: every cell was shipped a share"
    );

    // London falls silent past the window; the other two keep reporting.
    let now = past_window();
    let mut last = None;
    for ((cell, region), id) in cells.iter().zip(&ids).skip(1) {
        last = Some(plane.ingest(
            report_of(cell, region, id, now),
            switch.kill_switch_mut(),
            now,
        )?);
    }
    let dark = last.ok_or_else(|| qip_core::Error::not_found("two reports were ingested"))?;
    assert_eq!(plane.dark_regions(now), dark_set(&[LON_REGION]));

    // Held, not dropped: the same instrument, the same cells, the same
    // findings at no smaller a gross, and the silent cell still recalled.
    assert_eq!(
        dark.crowded.len(),
        1,
        "the crowding vanished: {:?}",
        dark.crowded
    );
    assert_eq!(dark.crowded[0].instrument, INSTRUMENT);
    assert_eq!(dark.crowded[0].cells, lit.crowded[0].cells);
    assert!(dark.crowded[0].cells.contains(&CELL.to_string()));
    assert_eq!(
        findings(&dark.concentrations),
        findings(&lit.concentrations)
    );
    assert!(
        dark.recalls.iter().any(|order| order.cell == CELL),
        "the silent cell was dropped from the recall set: {:?}",
        dark.recalls
    );
    assert_eq!(dark.recalls.len(), lit.recalls.len());
    assert_eq!(plane.gross_notional_by_cell(), gross_lit);
    assert!(
        plane.darkness_of(CELL, now).is_some(),
        "the held book is not marked as a dark region's"
    );

    // Nothing new: no grant, the slot names the region, the withdrawn set
    // and every region's share bound are where they were.
    assert!(issue(&mut plane, &ids[0], CELL, now).is_err());
    let constraints = plane.feasibility_constraints(now);
    assert_eq!(constraints.dark_regions, dark_set(&[LON_REGION]));
    assert!(constraints.withdrawn_venues.is_empty());
    assert_eq!(shares_at(&mut plane, now), shares_lit);
    let signatures_dark: Vec<Option<String>> = cells
        .iter()
        .zip(&ids)
        .map(|((cell, _), id)| {
            plane
                .envelope(cell, id)
                .map(|envelope| envelope.signature().to_string())
        })
        .collect();
    assert_eq!(signatures_dark, signatures_before);

    // The region speaks, through the same door, and clears — with none of
    // the above having moved in the meantime.
    let later = now.saturating_add(Duration::from_secs(1));
    let cleared = plane.ingest(
        report_of(CELL, LON_REGION, &ids[0], later),
        switch.kill_switch_mut(),
        later,
    )?;
    assert!(plane.dark_regions(later).is_empty());
    assert_eq!(cleared.crowded[0].cells, lit.crowded[0].cells);
    assert_eq!(
        findings(&cleared.concentrations),
        findings(&lit.concentrations)
    );
    assert_eq!(plane.gross_notional_by_cell(), gross_lit);
    assert_eq!(shares_at(&mut plane, later), shares_lit);
    assert!(plane.feasibility_constraints(later).dark_regions.is_empty());
    Ok(())
}

/// ADR 0080: the view the producer ships a cell is the schedule filtered to
/// that cell and keyed by instrument alone — so one cell never receives
/// another cell's lot, and the instrument key is the one the cell's own book
/// is keyed by rather than a `cell/instrument` string it would have to split.
#[test]
fn the_per_cell_view_of_scheduled_unwinds_names_only_that_cells_lots_keyed_by_instrument()
-> Result<()> {
    use qip_contracts::message::BookSide;

    let mut platform = platform()?;
    let id = strategy();
    register(platform.central_mut(), &id, CELL)?;
    walk_to(platform.central_mut(), &id, GateStage::Pilot)?;
    let (order, fill) = strategy_order_and_fill(
        &id,
        "ord-view-1",
        BookSide::Ask,
        dec!("100"),
        dec!("50"),
        start(),
    );
    platform.ingest_cell_report(
        CellReport::new(CELL, start())
            .with_orders(vec![order])
            .with_fills(vec![fill]),
        start(),
    )?;
    let LearningReportAt { retired_at, .. } = retire_by_decay(&mut platform, &id)?;
    // Premise: the whole schedule lists the lot, so an empty per-cell view
    // below would be the filter's doing and not an empty schedule's.
    assert_eq!(
        platform
            .central()
            .scheduled_unwinds()
            .get(&id)
            .and_then(|lots| lots.get(&format!("{CELL}/{INSTRUMENT}"))),
        Some(&dec!("-100")),
        "premise: the retirement did not schedule the lot"
    );

    let mine = platform.central().scheduled_unwinds_for(CELL);
    assert_eq!(
        mine.get(&id).and_then(|lots| lots.get(INSTRUMENT)),
        Some(&dec!("-100")),
        "the cell's own view does not name its lot by instrument: {mine:?}"
    );
    assert_eq!(
        mine.get(&id).map(BTreeMap::len),
        Some(1),
        "the view carries a key other than the instrument: {mine:?}"
    );
    assert!(
        platform
            .central()
            .scheduled_unwinds_for("a-cell-holding-nothing")
            .is_empty(),
        "a cell holding none of the strategy's lots was handed one"
    );

    // The view empties by the same arithmetic the schedule does once the
    // cell reports the flatten — no completion record, the books alone.
    let later = retired_at.saturating_add(Duration::from_hours(1));
    let (order, fill) = strategy_order_and_fill(
        &id,
        "ord-view-2",
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
        platform.central().scheduled_unwinds_for(CELL).is_empty(),
        "the lot was flattened and the cell's view still lists it: {:?}",
        platform.central().scheduled_unwinds_for(CELL)
    );
    Ok(())
}

// --- the operator's capital grant (ADR 0075; blueprint §36.3) ----------------

/// The producer every capital-grant signature record carries, as a literal so
/// this suite reads the log the way a replay would rather than through the
/// kernel's constant.
const CAPITAL_GRANT_PRODUCER: &str = "kernel/capital-grant";

fn grant_records(platform: &Platform) -> Result<Vec<qip_kernel::CapitalGrantEntry>> {
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| record.event.lineage.producer == CAPITAL_GRANT_PRODUCER)
        .map(|record| {
            qip_streaming::envelope::StreamEnvelope::from_frame(&record.event)
                .and_then(|envelope| envelope.decode::<qip_kernel::CapitalGrantEntry>())
                .map(|envelope| envelope.body)
        })
        .collect()
}

/// A platform with one strategy standing at pilot, sized by the allocator,
/// and no envelope — the state the operator route finds.
fn platform_at_pilot(id: &StrategyId) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    register(platform.central_mut(), id, CELL)?;
    walk_to(platform.central_mut(), id, GateStage::Pilot)?;
    assert!(
        platform.central().envelope(CELL, id).is_none(),
        "premise: {id} stands at pilot and holds no envelope"
    );
    assert!(
        grant_records(&platform)?.is_empty(),
        "premise: no grant record before anyone signs"
    );
    Ok(platform)
}

#[test]
fn two_operators_present_together_issue_a_grant_through_the_platforms_intent_and_the_decision_is_logged_before_the_envelope()
-> Result<()> {
    // `CentralPlane::issue` had no production caller, and its doc forbids a
    // cycle stage being one. This is the route's intent: two people, the
    // approval naming the cell the allocator sizes at, the pair's decision
    // in the log before the plane is asked, and the envelope after.
    let id = strategy();
    let mut platform = platform_at_pilot(&id)?;
    let first_at = start();
    let second_at = start().saturating_add(Duration::from_mins(2));

    let first = platform.issue_capital(
        &id,
        &operator("alice.chen", first_at),
        first_at,
        "the pilot gate passed and the allocator sized it inside the budget",
        first_at,
    )?;
    assert_eq!(first.outcome, "awaiting_countersignature");
    assert_eq!(first.cell, CELL, "the approval names the cell the allocator sized at");
    assert!(
        platform.central().envelope(CELL, &id).is_none(),
        "one signature issued an envelope"
    );
    assert_eq!(platform.pending_capital_grant(&id), Some(first_at));

    let second = platform.issue_capital(
        &id,
        &operator("bram.oduya", second_at),
        second_at,
        "reviewed the allocation and the pilot evidence independently",
        second_at,
    )?;
    assert_eq!(second.outcome, "issued", "{second:?}");
    assert_eq!(second.second_approver.as_deref(), Some("bram.oduya"));
    let envelope = platform
        .central()
        .envelope(CELL, &id)
        .ok_or_else(|| qip_core::Error::not_found("the envelope the pair issued"))?;
    assert_eq!(second.gross_limit, Some(envelope.gross_limit()));
    assert!(platform.pending_capital_grant(&id).is_none());

    // The approval the chain recorded names the request's own subject —
    // `CapitalRequest::subject`'s form, spelled by the compliance type and
    // not by this test — and the requester is the allocator, never a signer.
    let grant = platform
        .central()
        .compliance()
        .approvals()
        .grants()
        .last()
        .cloned()
        .ok_or_else(|| qip_core::Error::not_found("the chain's grant record"))?;
    let request = CapitalRequest {
        strategy: id.clone(),
        cell: CELL.to_string(),
        gross_limit: envelope.gross_limit(),
        order_limit: envelope.order_limit(),
        loss_limit: envelope.loss_limit(),
        venues: vec![venue()],
        expires_at: envelope.expires_at(),
        requested_by: grant.requested_by.clone(),
    };
    assert_eq!(grant.subject, request.subject());
    assert_eq!(grant.approvers, vec!["alice.chen".to_string(), "bram.oduya".to_string()]);
    assert!(
        !grant.approvers.contains(&grant.requested_by),
        "a signer is recorded as the requester: {}",
        grant.requested_by
    );

    // Three records, in the order the acts happened: the first signature,
    // the pair's decision, then the plane's answer. The middle one is what
    // "journalled before the plane mutates" means, and a process that died
    // between the second and third records would leave a decision with no
    // outcome — a crash to investigate, never a grant to assume.
    let records = grant_records(&platform)?;
    let outcomes: Vec<&str> = records.iter().map(|entry| entry.outcome.as_str()).collect();
    assert_eq!(
        outcomes,
        vec!["awaiting_countersignature", "countersigned", "issued"]
    );
    assert_eq!(records[2].gross_limit, Some(envelope.gross_limit()));
    Ok(())
}

#[test]
fn one_operator_signing_a_capital_grant_twice_is_refused_and_nothing_is_issued() -> Result<()> {
    let id = strategy();
    let mut platform = platform_at_pilot(&id)?;
    platform.issue_capital(
        &id,
        &operator("alice.chen", start()),
        start(),
        "the pilot gate passed and the allocator sized it inside the budget",
        start(),
    )?;
    let again = start().saturating_add(Duration::from_mins(1));
    let error = platform
        .issue_capital(
            &id,
            &operator("alice.chen", again),
            again,
            "signing again from a second session to complete my own approval",
            again,
        )
        .expect_err("one person completed a dual approval");
    assert!(
        error.message().contains("a second session is not a second person"),
        "{}",
        error.message()
    );
    assert!(platform.central().envelope(CELL, &id).is_none());
    assert_eq!(
        grant_records(&platform)?.len(),
        1,
        "a refused countersignature reached the log as a decision"
    );
    // The first signature still stands for a genuine second person.
    assert_eq!(platform.pending_capital_grant(&id), Some(start()));
    Ok(())
}

#[test]
fn a_first_signature_older_than_the_credential_window_is_discarded_rather_than_completed()
-> Result<()> {
    // The chain needs both signers present at issue, so a first signature
    // held past the credential window could only ever be refused by the
    // chain. Discarded here by name instead, and both must sign again.
    let id = strategy();
    let mut platform = platform_at_pilot(&id)?;
    platform.issue_capital(
        &id,
        &operator("alice.chen", start()),
        start(),
        "the pilot gate passed and the allocator sized it inside the budget",
        start(),
    )?;
    let later = start().saturating_add(Duration::from_mins(16));
    let error = platform
        .issue_capital(
            &id,
            &operator("bram.oduya", later),
            later,
            "reviewed the allocation and the pilot evidence independently",
            later,
        )
        .expect_err("a stale first signature was completed");
    assert!(error.message().contains("discarded"), "{}", error.message());
    assert!(platform.central().envelope(CELL, &id).is_none());
    assert!(
        platform.pending_capital_grant(&id).is_none(),
        "the stale signature was left standing"
    );
    Ok(())
}

/// Blueprint §23.1 LEVEL 1 on a corpus the *operator route* granted.
///
/// The test above this file's `the_learn_stage_measures_no_family_structure_
/// on_a_corpus_the_centre_never_granted` pins the measurement empty when the
/// plane's `issue` is never reached, and the granting test beside it reaches
/// `issue` directly, as no binary could. This one grants every session
/// through `Platform::issue_capital` — the intent the route raises — so the
/// LEARN stage's clustering is shown to be fed by the path a deployment has,
/// and not only by a test calling the plane.
#[test]
fn the_learn_stage_measures_family_structure_on_a_corpus_granted_through_the_operator_intent()
-> Result<()> {
    use qip_kernel::central::CLUSTERING_WINDOW;

    let mut config = PlatformConfig::default();
    config.central.per_cell = Decimal::from_int(9_000_000);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits())?;
    let ids = [
        StrategyId::new("granted-alpha"),
        StrategyId::new("granted-beta"),
        StrategyId::new("granted-gamma"),
    ];
    for id in &ids {
        register(platform.central_mut(), id, CELL)?;
        walk_to(platform.central_mut(), id, GateStage::Pilot)?;
    }
    // Two people, each present at the instant of issue, on every grant —
    // at the session's own instant, because the envelope is live from the
    // instant the countersignature issues it and a report at the session's
    // instant reads a grant issued a minute later as not yet held.
    let grant_through_intent = |platform: &mut Platform, id: &StrategyId, at: Timestamp| {
        platform.issue_capital(
            id,
            &operator("alice.chen", at),
            at,
            "the pilot gate passed and the allocator sized it inside the budget",
            at,
        )?;
        platform.issue_capital(
            id,
            &operator("bram.oduya", at),
            at,
            "reviewed the allocation and the pilot evidence independently",
            at,
        )
    };

    let mut grants = Vec::new();
    for id in &ids {
        let issued = grant_through_intent(&mut platform, id, start())?;
        assert_eq!(issued.outcome, "issued", "the intent did not issue {id}: {issued:?}");
        grants.push(
            issued
                .gross_limit
                .ok_or_else(|| qip_core::Error::not_found("the issued grant's gross limit"))?,
        );
    }
    let quantity = grants[0]
        .checked_div(dec!("1000"))
        .ok_or_else(|| qip_core::Error::numeric("a thousand divides any grant"))?;

    for session in 0..(CLUSTERING_WINDOW as i64) {
        let at = start().saturating_add(Duration::from_days(session));
        let mut orders = Vec::new();
        let mut fills = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            // Re-granted each session through the same intent: an envelope
            // lives eight hours, and a day under a lapsed one is not a day
            // under a grant.
            grant_through_intent(&mut platform, id, at)?;
            let shape = if index < 2 {
                ((session % 11) as f64 - 5.0) * 0.002
            } else {
                ((session % 7) as f64 - 3.0) * 0.002
            };
            let wobble = ((session % 3) as f64 - 1.0) * 0.0003 * ((index + 1) as f64);
            let pnl = Decimal::from_f64((shape + wobble) * grants[index].to_f64())
                .ok_or_else(|| qip_core::Error::numeric("a finite return"))?;
            let (session_orders, session_fills) =
                session_fills(id, &format!("g{session}-{index}"), quantity, pnl, at)?;
            orders.extend(session_orders);
            fills.extend(session_fills);
        }
        let ingestion = platform.ingest_cell_report(
            CellReport::new(CELL, at)
                .with_orders(orders)
                .with_fills(fills),
            at,
        )?;
        assert!(
            ingestion.settlement.refused.is_empty(),
            "premise: session {session} settled: {:?}",
            ingestion.settlement.refused
        );
    }

    let measuring_at = start().saturating_add(Duration::from_days(CLUSTERING_WINDOW as i64));
    assert_eq!(
        platform.central().realised_calendar(measuring_at).day_count(),
        CLUSTERING_WINDOW,
        "the calendar does not retain every session the intent granted"
    );
    let measured = platform
        .central()
        .family_structure(measuring_at)?
        .ok_or_else(|| {
            qip_core::Error::not_found("a clustering over a corpus the operator route granted")
        })?;
    assert_eq!(measured.strategies, 3);
    assert_eq!(measured.sessions, CLUSTERING_WINDOW);
    assert!(
        measured.mean_intra_family_correlation > measured.mean_inter_family_correlation,
        "the two strategies on one factor are filed together: intra {} inter {}",
        measured.mean_intra_family_correlation,
        measured.mean_inter_family_correlation
    );

    let report = platform.run_cycle(measuring_at);
    let learn = report
        .stage(Stage::Learn)
        .ok_or_else(|| qip_core::Error::not_found("the LEARN stage ran"))?;
    assert!(
        learn
            .detail
            .contains("3 strategy(ies) clustered into 2 family(ies)"),
        "the stage says what it measured: {}",
        learn.detail
    );
    Ok(())
}
