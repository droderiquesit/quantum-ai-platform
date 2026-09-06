//! ADR 0039 across the wire: a payload the kernel produced, applied by
//! `qip-edge` cells.
//!
//! The ADR is proven in two halves on their own sides of the seam. The
//! kernel's suite proves `CentralPlane::grant_manifests` partitions a
//! region's grant into per-cell manifests whose gross never sums past it;
//! the edge's suite proves a cell re-bases its region table from a payload
//! it verified. Neither can see the other: the kernel's test builds the
//! payload and stops, the edge's test signs a manifest by hand. What was
//! never shown is that the manifest the centre actually ships — built by
//! `qip_api::mesh::pending_policy` from a real plane, signed the way
//! `dispatch_policy` signs it — is one a real cell derives the right bound
//! from. A field renamed on one side, a signature the other does not
//! recompute, a share the centre computes from one number and the cell from
//! another, would pass both halves and fail the deployment.
//!
//! So each test here assembles both: a `Platform` whose central plane issued
//! two grants through its own door to two cells filed under one region, the
//! payloads `pending_policy` builds for them, and two `Cell`s opened
//! unfunded that verify and apply those payloads under the same trust root.

#![allow(clippy::panic_in_result_fn)]

use qip_api::mesh::pending_policy;
use qip_capital::allocation::StrategyProposal;
use qip_capital::capacity::CapacityModel;
use qip_compliance::approval::OperatorCredential;
use qip_contracts::feature::FeatureKey;
use qip_contracts::gate::GateStage;
use qip_contracts::governance::Approval;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::PolicyPayload;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_contracts::{CapitalEnvelope, Utilisation};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::VerifiedEnvelope;
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::central::{
    CentralConfig, CentralPlane, ManifestDecision, RegionMembership, StrategyCandidate,
    capital_subject,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, KillCondition, LeakageAudit, PaperEvidence,
    PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence, StrategyEvidence,
};
use qip_lifecycle::trials::StrategyFamily;
use qip_observability::Telemetry;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use std::collections::BTreeMap;

// --- the trust root and the identities every test shares --------------------

/// The one secret both ends hold: the plane signs envelopes and the API
/// signs payloads with it, the cells verify both against it. A test key,
/// as every other suite's is; the property is that both ends use the *same*
/// one, not what it is.
const KEY: &[u8] = b"region-share-acceptance-trust-root";
const REGION: &str = "europe-west2";
const FIRST_CELL: &str = "cell-lon-1";
const SECOND_CELL: &str = "cell-lon-2";
const FIRST_STRATEGY: &str = "region-share-momentum-1";
const SECOND_STRATEGY: &str = "region-share-momentum-2";
const VENUE: &str = "XNYS";
const SYMBOL: &str = "AAA";
/// The operator's ceiling on each cell's table: far above any share the
/// allocator sizes, so the bound the tests read is the centre's share and
/// never the operator's backstop.
fn ceiling() -> Decimal {
    Decimal::from_int(1_000_000_000)
}
/// The gate literal `Cell::hold_region_capital` refuses under, matched by
/// delimited equality: `region_reservation_abandoned` carries it as a prefix.
const RESERVATION_GATE: &str = "region_reservation";
/// The gate literal the envelope's own admission refuses under. Also matched
/// by delimited equality: `capital_reduced` carries it as a prefix.
const CAPITAL_GATE: &str = "capital";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn at(secs: i64) -> Timestamp {
    start().saturating_add(Duration::from_secs(secs))
}

/// Ninety days of pilot plus a month, so the scaled gate's duration bar is
/// met by the evidence without every test having to say so.
fn scaled_at() -> Timestamp {
    start().saturating_add(Duration::from_days(120))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

// --- the centre: a plane that issued two grants through its own door --------

/// A one-rule strategy the factory will register. Compiled rather than
/// hand-built for the same reason the kernel's suite compiles: the thing
/// the centre sizes has to be the thing the compiler produced.
fn registrable(id: &str) -> Result<(CompiledStrategy, Program)> {
    let pressure = FeatureKey::new("book_pressure", object()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_millis(250))
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

fn good_returns(seed: u64, n: usize, drift: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n)
        .map(|_| {
            let u = rng.next_f64() + rng.next_f64() - 1.0;
            drift + u * 0.01
        })
        .collect()
}

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

fn dual_approval(subject: &str, when: Timestamp, rationale: &str) -> Result<Approval> {
    Approval::new(subject, "alice.chen", when, rationale)?.countersigned_by("bram.oduya")
}

/// Evidence that passes every gate up to scaled, so the ladder and not the
/// evidence is what decides whether a grant is issued.
fn full_evidence(id: &StrategyId, cell: &str) -> Result<StrategyEvidence> {
    let observations = 400;
    let holdout = HoldoutEvidence {
        holdout_returns: good_returns(1, observations, 0.0018),
        in_sample_folds: (0..5).map(|f| good_returns(10 + f, 80, 0.0020)).collect(),
        out_of_sample_folds: (0..5).map(|f| good_returns(20 + f, 80, 0.0018)).collect(),
        trials: 12,
        periods_per_year: 252.0,
        cross_validation: honest_cross_validation(observations)?,
        leakage: LeakageAudit {
            timings: (0..8)
                .map(|i| FeatureTiming {
                    feature: format!("feature-{i}"),
                    known_at: start(),
                    used_at: start().saturating_add(Duration::from_hours(1)),
                })
                .collect(),
            restated_without_snapshots: Vec::new(),
        },
    };
    let paper = PaperEvidence {
        against_live_data: true,
        assumed_cost_bps: 8.0,
        realised_cost_bps: (0..400).map(|i| 7.0 + f64::from(i % 5) * 0.2).collect(),
        peak_participation: 0.04,
        modelled_participation_limit: 0.10,
        unfillable_orders: 4,
        filled_orders: 400,
    };
    let shadow = ShadowEvidence {
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
    };
    let pilot = PilotEvidence {
        approval: Some(dual_approval(
            &format!("{id} pilot"),
            start(),
            "shadow agreement held at 100% over 400 decisions",
        )?),
        envelope: Some(CapitalEnvelope::new(
            id.clone(),
            cell,
            dec!("250000"),
            dec!("250000"),
            dec!("250000"),
            vec![venue()],
            start(),
            start().saturating_add(Duration::from_days(14)),
            "alice.chen",
            "proposed-not-issued",
        )?),
        kill_conditions: vec![
            KillCondition::RealisedLoss(dec!("25000")),
            KillCondition::Drawdown(0.08),
            KillCondition::ConsecutiveLosingDays(5),
        ],
    };
    let scaled = ScaledEvidence {
        pilot_returns: good_returns(99, 120, 0.0030),
        pilot_started_at: start(),
        pilot_utilisation: Utilisation {
            gross_committed: dec!("180000"),
            realised_loss: dec!("0"),
            orders_sent: 5_400,
        },
        proposed_notional: dec!("1000000"),
        modelled_capacity: dec!("4000000"),
        pilot_approval: Some(dual_approval(
            &format!("{id} pilot"),
            start(),
            "shadow agreement held at 100% over 400 decisions",
        )?),
        scaling_approval: Some(dual_approval(
            &format!("{id} scaling"),
            scaled_at(),
            "ninety days at pilot returned a 0.7 Sharpe inside a quarter of capacity",
        )?),
    };
    Ok(StrategyEvidence::new()
        .with_holdout(holdout)
        .with_paper(paper)
        .with_shadow(shadow)
        .with_pilot(pilot)
        .with_scaled(scaled))
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

/// Register `id` at `cell` and walk it to pilot, the first rung that holds
/// capital, collecting a dual approval where the rung demands one.
fn register_at_pilot(plane: &mut CentralPlane, id: &StrategyId, cell: &str) -> Result<()> {
    let (compiled, program) = registrable(id.as_str())?;
    let candidate = StrategyCandidate::new(
        compiled,
        program,
        StrategyFamily::new("region-share-tests")?,
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
    for rung in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
    ] {
        let approval = if rung.requires_human_approval() {
            Some(dual_approval(
                id.as_str(),
                start(),
                "every gate check passed with the evidence attached",
            )?)
        } else {
            None
        };
        plane
            .factory_mut()
            .promote(id, approval, "the gate passed", start())?;
    }
    Ok(())
}

/// Issue a grant through the plane's door — `CentralPlane::issue`, the
/// call a deployment makes — so the plane holds the envelope it will later
/// name in a manifest.
fn issue(plane: &mut CentralPlane, id: &StrategyId, cell: &str) -> Result<CapitalEnvelope> {
    let approval = dual_approval(
        &capital_subject(id, cell),
        start(),
        "the pilot gate passed and the allocator sized it inside the budget",
    )?;
    let credentials = vec![
        OperatorCredential::verified("alice.chen", "webauthn", start())?,
        OperatorCredential::verified("bram.oduya", "webauthn", start())?,
    ];
    Ok(plane
        .issue(id, "research.desk", &approval, &credentials, 0.0, start())?
        .envelope()
        .clone())
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    if let Ok(object) = FinancialObject::builder(object(), SYMBOL, InstrumentType::CommonStock)
        .venue(VENUE)
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())
    {
        let _ = universe.insert(object);
    }
    universe
}

/// A platform carrying `plane` as its central plane, as `qip-api` carries
/// the one it hardened with the real key.
fn platform_around(plane: CentralPlane) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let limits = LimitSet::new("region-share-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    );
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits)?;
    platform.set_central(plane);
    Ok(platform)
}

/// The two grants the centre issued, one per cell, and the platform that
/// holds the plane that issued them.
struct Centre {
    platform: Platform,
    first: CapitalEnvelope,
    second: CapitalEnvelope,
}

/// The centre a deployment runs: one plane keyed on the trust root, two
/// strategies at pilot at two cells, two grants issued through the plane's
/// own door.
fn centre_that_issued_both_grants() -> Result<Centre> {
    let mut plane = CentralPlane::new(KEY, CentralConfig::default())?;
    let first_id = StrategyId::new(FIRST_STRATEGY);
    let second_id = StrategyId::new(SECOND_STRATEGY);
    register_at_pilot(&mut plane, &first_id, FIRST_CELL)?;
    register_at_pilot(&mut plane, &second_id, SECOND_CELL)?;
    let first = issue(&mut plane, &first_id, FIRST_CELL)?;
    let second = issue(&mut plane, &second_id, SECOND_CELL)?;
    Ok(Centre {
        platform: platform_around(plane)?,
        first,
        second,
    })
}

/// A centre that came back up: the same trust root, the same strategies at
/// the same rungs, and none of the grants it issued before — the plane's
/// envelope map is in-process state. Its manifests name nothing, which is
/// the fail-closed direction the ADR chose over guessing.
fn centre_that_restarted_without_its_grants() -> Result<Platform> {
    let mut plane = CentralPlane::new(KEY, CentralConfig::default())?;
    register_at_pilot(&mut plane, &StrategyId::new(FIRST_STRATEGY), FIRST_CELL)?;
    register_at_pilot(&mut plane, &StrategyId::new(SECOND_STRATEGY), SECOND_CELL)?;
    platform_around(plane)
}

/// Both cells filed under one region granted exactly `grant`, in the form
/// `QIP_MESH_REGIONS` is written in.
fn both_cells_under(grant: Decimal) -> Result<RegionMembership> {
    RegionMembership::parse(&format!("{REGION}={grant}:{FIRST_CELL},{SECOND_CELL}"))
}

/// One cycle's policy at the centre, as `qip-api` builds and signs it:
/// `pending_policy` over the platform for both cells under `membership`,
/// each payload signed with the trust root exactly as `dispatch_policy`
/// signs it before the courier sends it. Returns the shares' account lines
/// beside the signed payloads, keyed by cell.
fn signed_cycle(
    platform: &mut Platform,
    membership: &RegionMembership,
    now: Timestamp,
) -> Result<(Vec<String>, BTreeMap<String, PolicyPayload>)> {
    let pending = pending_policy(
        platform,
        [FIRST_CELL, SECOND_CELL].into_iter().map(String::from),
        Some(membership),
        now,
    );
    let mut signed = BTreeMap::new();
    for (cell, payload) in pending.payloads {
        signed.insert(cell, payload.signed(KEY)?);
    }
    Ok((pending.shares, signed))
}

/// The share line `pending_policy` wrote for `cell`, which every test reads
/// before trusting the payload: a withheld share ships the slot unproduced,
/// and a test that applied that would be proving an unchanged table.
fn share_line<'a>(lines: &'a [String], cell: &str) -> &'a str {
    lines
        .iter()
        .find(|line| line.starts_with(&format!("region share for {cell}:")))
        .map(String::as_str)
        .unwrap_or_else(|| panic!("no share line for {cell} in {lines:?}"))
}

// --- the edge: two cells, opened unfunded, verifying under the same root ----

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: Decimal::parse(price).expect("a decimal literal"),
            quantity: Decimal::parse(size).expect("a decimal literal"),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100 and enough resting on the ask that
/// depth is never what refuses an order.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in [
        (BookSide::Bid, "99", "10000000"),
        (BookSide::Ask, "101", "10000000"),
    ]
    .iter()
    .enumerate()
    {
        state.apply(&level(index as u64, *side, price, size, at(index as i64)))?;
    }
    Ok(state)
}

/// A strategy under `id` whose one rule always holds, so every pass proposes
/// `size` — the same id the centre registered, because the cell refuses an
/// envelope for one strategy deploying another.
fn firing_strategy(id: &str, size: Decimal) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(size),
            Expr::Statistic(0.5),
            10,
        ),
    );
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A venue that accepts every order and fills nothing, so what the region
/// table commits stays committed and can be read back.
#[derive(Debug, Default)]
struct VenueGateway {
    placed: Vec<(String, Decimal)>,
}

impl Placer for VenueGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((order_id.to_string(), quantity));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        Vec::new()
    }
}

/// A cell of `cell_id` opened unfunded under the operator's ceiling, holding
/// the book and one always-firing strategy deployed under `envelope` — the
/// grant the centre issued, verified here against the same trust root.
///
/// `Cell::new` is the only constructor `qip-edge` has, and it takes no
/// autonomy ceiling: every cell in this suite is assembled through it, so
/// the paper boundary is asserted by construction and checked on the
/// instance below.
fn unfunded_cell(cell_id: &str, envelope: &CapitalEnvelope, size: Decimal) -> Result<Cell> {
    let config = CellConfig::new(cell_id, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_unfunded_region(ceiling())?;
    cell.track(book()?);
    let verified = VerifiedEnvelope::verify(envelope.clone(), KEY, cell_id, at(1))?;
    let (compiled, program) = firing_strategy(envelope.strategy().as_str(), size)?;
    cell.deploy_with_pricing(compiled, program, verified, PricingPolicy::Marketable)?;
    Ok(cell)
}

/// An order size that fits the envelope's per-order limit at the ask with
/// room to spare, so the per-order gate never refuses and what exhausts a
/// share is the sum of passes. Derived from the envelope rather than
/// restated as a literal: the allocator sizes the grant, not this file.
fn order_size_within(envelope: &CapitalEnvelope) -> Decimal {
    envelope
        .order_limit()
        .checked_div(dec!("202"))
        .unwrap_or(Decimal::ZERO)
        .floor_to_step(Decimal::ONE)
}

/// Verify `signed` as the cell's downlink does, and apply it.
fn apply(cell: &mut Cell, signed: &PolicyPayload, now: Timestamp) -> Result<()> {
    let verified =
        VerifiedPolicy::verify(signed.clone(), KEY, cell.config().cell_id.as_str(), now)?;
    cell.apply_policy(verified, now)
}

fn bound_of(cell: &Cell) -> Decimal {
    cell.region_allocation_bound()
        .unwrap_or_else(|| panic!("{} holds no region table", cell.config().cell_id))
}

fn free_of(cell: &Cell) -> Decimal {
    cell.region_allocation_free()
        .unwrap_or_else(|| panic!("{} holds no region table", cell.config().cell_id))
}

/// What the cell has spent of its region share: every pass releases what it
/// did not send, so with nothing mid-pass this is what was committed.
fn charged(cell: &Cell) -> Decimal {
    bound_of(cell) - free_of(cell)
}

fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        // Delimited equality, not `contains`: `capital_reduced` and
        // `region_reservation_abandoned` carry these gates as prefixes.
        .filter(|(recorded, _)| recorded == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

fn share_entries(cell: &Cell) -> usize {
    cell.journal()
        .entries()
        .iter()
        .filter(|entry| matches!(&entry.decision, Decision::RegionShareApplied { .. }))
        .count()
}

// --- the tests ---------------------------------------------------------------

#[test]
fn a_payload_the_centre_built_funds_each_cell_to_exactly_its_share_and_the_two_never_exceed_the_grant()
-> Result<()> {
    let mut centre = centre_that_issued_both_grants()?;
    // The premise on the centre's side: the plan the payload will be
    // partitioned from allocates each cell what its grant was issued
    // against, and the region is granted exactly the two together.
    let plan = centre.platform.central().allocate(0.0, start())?;
    assert_eq!(plan.for_cell(FIRST_CELL), centre.first.gross_limit());
    assert_eq!(plan.for_cell(SECOND_CELL), centre.second.gross_limit());
    let grant = plan.for_cell(FIRST_CELL) + plan.for_cell(SECOND_CELL);
    assert!(grant.is_positive(), "the plan allocates to neither cell");
    let membership = both_cells_under(grant)?;

    let (lines, payloads) = signed_cycle(&mut centre.platform, &membership, at(10))?;
    // The premise on the wire: both cells were shipped a share, not
    // withheld one. The account line is what an operator reads; the
    // decision is what the payload was built from; both are checked.
    for cell in [FIRST_CELL, SECOND_CELL] {
        let line = share_line(&lines, cell);
        assert!(
            line.contains(&format!("of {REGION}'s grant")) && !line.contains("not shipped"),
            "{cell}'s share was not shipped: {line}"
        );
    }
    let decided = centre.platform.central().grant_manifests(
        [FIRST_CELL, SECOND_CELL],
        &membership,
        centre.platform.drawdown(),
        at(10),
    );
    let mut shares_together = Decimal::ZERO;
    for (cell, envelope) in [(FIRST_CELL, &centre.first), (SECOND_CELL, &centre.second)] {
        let share = match decided.for_cell(cell) {
            Some(ManifestDecision::Ship(share)) => share,
            other => panic!("{cell} was not shipped a share: {other:?}"),
        };
        assert_eq!(
            share.live_grants(),
            &[envelope.signature().to_string()],
            "{cell}'s manifest does not name exactly its own grant"
        );
        shares_together += share.amount();
        let carried = payloads
            .get(cell)
            .and_then(|payload| payload.capital_grants.value())
            .unwrap_or_else(|| panic!("{cell}'s payload carries no produced manifest"));
        assert_eq!(
            carried.live_grants,
            share.live_grants(),
            "the payload for {cell} does not carry the manifest the centre decided"
        );
    }
    assert!(
        shares_together <= grant,
        "the centre's shares sum to {shares_together} against a grant of {grant}"
    );

    // The edge: two cells opened unfunded, each verifying the grant the
    // centre issued it and the payload the centre signed for it.
    let mut first = unfunded_cell(FIRST_CELL, &centre.first, dec!("100"))?;
    let mut second = unfunded_cell(SECOND_CELL, &centre.second, dec!("100"))?;
    assert_eq!(
        bound_of(&first),
        Decimal::ZERO,
        "an unfunded cell opened funded"
    );
    assert_eq!(
        bound_of(&second),
        Decimal::ZERO,
        "an unfunded cell opened funded"
    );

    apply(&mut first, &payloads[FIRST_CELL], at(10))?;
    apply(&mut second, &payloads[SECOND_CELL], at(10))?;

    // Each cell's derived bound is its share: the gross of the one grant
    // the manifest names, which is what the centre partitioned.
    assert_eq!(
        bound_of(&first),
        centre.first.gross_limit(),
        "the first cell derived a bound other than its share"
    );
    assert_eq!(
        bound_of(&second),
        centre.second.gross_limit(),
        "the second cell derived a bound other than its share"
    );
    assert_eq!(
        bound_of(&first) + bound_of(&second),
        shares_together,
        "what the cells derived is not what the centre partitioned"
    );
    assert!(
        bound_of(&first) + bound_of(&second) <= grant,
        "the two cells' bounds sum to {} against a grant of {grant}",
        bound_of(&first) + bound_of(&second)
    );
    assert_eq!(
        share_entries(&first),
        1,
        "the first cell did not journal its re-base"
    );
    assert_eq!(
        share_entries(&second),
        1,
        "the second cell did not journal its re-base"
    );

    // The refusal half, at the cells rather than only at the centre: a
    // grant one unit short of the plan ships nothing to either cell, and
    // two cells opened unfunded stay at nothing — not the first cell alone,
    // not a scaled pair. Without this the guard could be deleted and the
    // funded case above would still pass, because there the plan fits.
    let short = grant - Decimal::ONE;
    let (short_lines, short_payloads) =
        signed_cycle(&mut centre.platform, &both_cells_under(short)?, at(20))?;
    let mut short_first = unfunded_cell(FIRST_CELL, &centre.first, dec!("100"))?;
    let mut short_second = unfunded_cell(SECOND_CELL, &centre.second, dec!("100"))?;
    apply(&mut short_first, &short_payloads[FIRST_CELL], at(20))?;
    apply(&mut short_second, &short_payloads[SECOND_CELL], at(20))?;
    // The cross-crate assertion first, so that it — and not the centre's
    // own account of itself — is what a broken partitioner trips.
    assert!(
        bound_of(&short_first) + bound_of(&short_second) <= short,
        "under a grant of {short} the cells were funded to {} and {} together",
        bound_of(&short_first),
        bound_of(&short_second)
    );
    assert_eq!(bound_of(&short_first), Decimal::ZERO);
    assert_eq!(bound_of(&short_second), Decimal::ZERO);
    for cell in [FIRST_CELL, SECOND_CELL] {
        let line = share_line(&short_lines, cell);
        assert!(
            line.contains("not shipped") && line.contains("past its grant"),
            "{cell} was shipped a share under a grant the plan exceeds: {line}"
        );
        assert!(
            short_payloads[cell].capital_grants.value().is_none(),
            "{cell}'s payload carries a manifest under a refused plan"
        );
    }
    Ok(())
}

#[test]
fn each_cell_places_within_its_share_and_a_cell_driven_past_its_share_is_refused_before_the_grant_is()
-> Result<()> {
    let mut centre = centre_that_issued_both_grants()?;
    let grant = centre.first.gross_limit() + centre.second.gross_limit();
    let membership = both_cells_under(grant)?;
    let (lines, payloads) = signed_cycle(&mut centre.platform, &membership, at(10))?;
    for cell in [FIRST_CELL, SECOND_CELL] {
        assert!(
            !share_line(&lines, cell).contains("not shipped"),
            "the premise failed: {cell} was withheld a share"
        );
    }

    let size = order_size_within(&centre.second);
    assert!(
        size.is_positive(),
        "the premise failed: no order fits the grant"
    );
    let mut first = unfunded_cell(FIRST_CELL, &centre.first, size)?;
    let mut second = unfunded_cell(SECOND_CELL, &centre.second, size)?;
    apply(&mut first, &payloads[FIRST_CELL], at(10))?;
    apply(&mut second, &payloads[SECOND_CELL], at(10))?;
    assert_eq!(bound_of(&first), centre.first.gross_limit());
    assert_eq!(bound_of(&second), centre.second.gross_limit());

    // Within: one pass each sends, and what each sent came out of its own
    // share.
    let mut gateway = VenueGateway::default();
    let first_report = first.work(at(60), &mut gateway)?;
    assert_eq!(
        first_report.orders.len(),
        1,
        "the first cell did not send within its share: {:?}",
        first_report.refusals
    );
    let second_report = second.work(at(60), &mut gateway)?;
    assert_eq!(
        second_report.orders.len(),
        1,
        "the second cell did not send within its share: {:?}",
        second_report.refusals
    );
    let per_pass = charged(&second);
    assert!(per_pass.is_positive(), "a pass that sent charged nothing");
    assert_eq!(
        charged(&first),
        per_pass,
        "the two cells were charged differently for one pass"
    );
    assert!(charged(&first) < bound_of(&first));

    // Past: the second cell keeps sending until its share is spent. Every
    // pass before the refusal was inside the share; at the refusal the
    // region table has less than one pass left, so the next order would
    // have crossed the share and — the shares being disjoint and summing
    // to the grant — the grant.
    let mut passes = 1u32;
    let refused = loop {
        passes += 1;
        assert!(
            passes < 10_000,
            "the second cell was never refused inside {passes} passes"
        );
        let report = second.work(at(60 * i64::from(passes)), &mut gateway)?;
        assert!(
            charged(&first) + charged(&second) <= grant,
            "after pass {passes} the two cells together committed more than the grant"
        );
        if report.orders.is_empty() {
            break report;
        }
    };
    assert_eq!(
        refused.signals.len(),
        1,
        "the premise failed: the strategy stopped proposing: {:?}",
        refused.refusals
    );
    assert!(
        free_of(&second) < per_pass,
        "the second cell was refused with {} free, a whole pass's worth",
        free_of(&second)
    );
    // Which gate: the share is the envelope's gross by construction and the
    // envelope is checked first, so at the instant the share is spent the
    // envelope closes too, and it is the envelope that refuses. The region
    // gate is not idle for that — it is the only bound over the *sum* of a
    // cell's strategies, and the one that fires when the centre narrows a
    // share below what the envelopes still permit (the next test). What
    // this asserts is that the cell was refused on capital and nothing
    // else, so the refusal is the grant's and not some other gate's.
    let on_capital = refused_under(&refused, CAPITAL_GATE).len()
        + refused_under(&refused, RESERVATION_GATE).len();
    assert_eq!(
        on_capital, 1,
        "the second cell was not refused exactly once on capital: {:?}",
        refused.refusals
    );
    assert!(
        charged(&first) + charged(&second) <= grant,
        "the two cells together committed more than the grant"
    );
    // The first cell's share is its own: the second cell spending all of
    // its own changed nothing about what the first may still send.
    let again = first.work(at(60 * i64::from(passes) + 30), &mut gateway)?;
    assert_eq!(
        again.orders.len(),
        1,
        "the first cell could not send after the second spent its share: {:?}",
        again.refusals
    );
    assert!(charged(&first) + charged(&second) <= grant);
    Ok(())
}

#[test]
fn a_centre_that_no_longer_names_a_cells_grant_narrows_it_to_nothing_and_the_region_gate_refuses()
-> Result<()> {
    // The one case a faithful partition reaches the region gate before the
    // envelope's: the centre's next payload names none of the grants the
    // cell still holds live. A centre that restarted holds none of the
    // envelopes it issued, and its manifest for each cell is produced and
    // empty — a share of nothing, not a slot left unproduced — so the cell
    // narrows to nothing while its envelope would still admit the order.
    let mut centre = centre_that_issued_both_grants()?;
    let grant = centre.first.gross_limit() + centre.second.gross_limit();
    let membership = both_cells_under(grant)?;
    let (_, funded) = signed_cycle(&mut centre.platform, &membership, at(10))?;
    let mut cell = unfunded_cell(FIRST_CELL, &centre.first, dec!("100"))?;
    apply(&mut cell, &funded[FIRST_CELL], at(10))?;
    assert_eq!(
        bound_of(&cell),
        centre.first.gross_limit(),
        "the premise failed: the first payload did not fund the cell"
    );

    let mut restarted = centre_that_restarted_without_its_grants()?;
    let (lines, empty) = signed_cycle(&mut restarted, &membership, at(20))?;
    let line = share_line(&lines, FIRST_CELL);
    assert!(
        !line.contains("not shipped") && line.contains("0 grant(s) named"),
        "the premise failed: the restarted centre did not ship an empty share: {line}"
    );
    apply(&mut cell, &empty[FIRST_CELL], at(20))?;
    assert_eq!(
        bound_of(&cell),
        Decimal::ZERO,
        "the empty share did not narrow the cell"
    );

    let mut gateway = VenueGateway::default();
    let report = cell.work(at(60), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        1,
        "the premise failed: nothing proposed"
    );
    assert!(
        report.orders.is_empty(),
        "the cell sent under a share of nothing: {:?}",
        report.orders
    );
    assert!(
        refused_under(&report, CAPITAL_GATE).is_empty(),
        "the envelope refused, so the region gate was not what held: {:?}",
        report.refusals
    );
    assert_eq!(
        refused_under(&report, RESERVATION_GATE).len(),
        1,
        "the cell was not refused exactly once under `{RESERVATION_GATE}`: {:?}",
        report.refusals
    );
    Ok(())
}

#[test]
fn a_replayed_lower_sequence_payload_from_the_centre_changes_neither_cells_table() -> Result<()> {
    // The un-widenable property across the wire: the funded payloads the
    // centre shipped first, then a restarted centre's empty shares under a
    // higher sequence, then the funded payloads played again. A captured
    // payload re-widening a cell the centre has since narrowed is the
    // replay ADR 0008 exists to make impossible, and here both the
    // sequence and the signature are the centre's own.
    let mut centre = centre_that_issued_both_grants()?;
    let grant = centre.first.gross_limit() + centre.second.gross_limit();
    let membership = both_cells_under(grant)?;
    let (_, wide) = signed_cycle(&mut centre.platform, &membership, at(10))?;
    let mut restarted = centre_that_restarted_without_its_grants()?;
    let (_, narrow) = signed_cycle(&mut restarted, &membership, at(20))?;
    for cell in [FIRST_CELL, SECOND_CELL] {
        assert!(
            wide[cell].sequence < narrow[cell].sequence,
            "the premise failed: the later cycle did not carry the higher sequence"
        );
    }

    let mut first = unfunded_cell(FIRST_CELL, &centre.first, dec!("100"))?;
    let mut second = unfunded_cell(SECOND_CELL, &centre.second, dec!("100"))?;
    apply(&mut first, &wide[FIRST_CELL], at(10))?;
    apply(&mut second, &wide[SECOND_CELL], at(10))?;
    assert_eq!(
        bound_of(&first),
        centre.first.gross_limit(),
        "the premise failed"
    );
    assert_eq!(
        bound_of(&second),
        centre.second.gross_limit(),
        "the premise failed"
    );
    apply(&mut first, &narrow[FIRST_CELL], at(20))?;
    apply(&mut second, &narrow[SECOND_CELL], at(20))?;
    assert_eq!(
        bound_of(&first),
        Decimal::ZERO,
        "the premise failed: not narrowed"
    );
    assert_eq!(
        bound_of(&second),
        Decimal::ZERO,
        "the premise failed: not narrowed"
    );
    let (first_journal, second_journal) = (first.journal().len(), second.journal().len());
    let (first_sequence, second_sequence) = (
        first.region_share_sequence(),
        second.region_share_sequence(),
    );

    for (cell, payload) in [
        (&mut first, &wide[FIRST_CELL]),
        (&mut second, &wide[SECOND_CELL]),
    ] {
        let replayed = apply(cell, payload, at(30));
        assert!(
            replayed.is_err(),
            "{} applied the replayed lower sequence",
            cell.config().cell_id
        );
    }
    assert_eq!(
        bound_of(&first),
        Decimal::ZERO,
        "the replay re-widened the first cell"
    );
    assert_eq!(
        bound_of(&second),
        Decimal::ZERO,
        "the replay re-widened the second cell"
    );
    assert_eq!(first.region_share_sequence(), first_sequence);
    assert_eq!(second.region_share_sequence(), second_sequence);
    assert_eq!(
        first.journal().len(),
        first_journal,
        "the replay was journaled as a decision"
    );
    assert_eq!(
        second.journal().len(),
        second_journal,
        "the replay was journaled as a decision"
    );
    Ok(())
}

#[test]
fn a_cell_funded_by_the_centres_share_is_still_assembled_paper_only() -> Result<()> {
    // `Cell::new` is the only constructor and takes no autonomy ceiling;
    // `with_unfunded_region` is a builder over the region table and names
    // no ceiling of any other kind. So a cell the centre has funded to its
    // share is at paper trading, with a paper ceiling, sending to a gateway
    // that says it is simulated — and the share changed none of that.
    let mut centre = centre_that_issued_both_grants()?;
    let grant = centre.first.gross_limit() + centre.second.gross_limit();
    let (_, payloads) = signed_cycle(&mut centre.platform, &both_cells_under(grant)?, at(10))?;
    let mut cell = unfunded_cell(FIRST_CELL, &centre.first, dec!("100"))?;
    let before = (cell.autonomy().level(), cell.autonomy().ceiling());
    apply(&mut cell, &payloads[FIRST_CELL], at(10))?;
    assert!(
        bound_of(&cell).is_positive(),
        "the premise failed: the cell was not funded"
    );

    assert_eq!(cell.autonomy().level(), AutonomyLevel::PaperTrading);
    assert_eq!(cell.autonomy().ceiling(), AutonomyLevel::PaperTrading);
    assert!(!cell.autonomy().is_live());
    assert_eq!(
        (cell.autonomy().level(), cell.autonomy().ceiling()),
        before,
        "applying the share moved the cell's autonomy"
    );
    let mut gateway = VenueGateway::default();
    assert!(gateway.is_simulated());
    let report = cell.work(at(60), &mut gateway)?;
    assert_eq!(report.orders.len(), 1, "{:?}", report.refusals);
    assert!(
        report.orders.iter().all(|order| order.simulated),
        "an order left a paper cell marked as other than simulated"
    );
    Ok(())
}
