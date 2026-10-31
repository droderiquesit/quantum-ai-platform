//! Synthetic future trees over a compact twin state (R12-TICK-7199;
//! §9.9 R12-TICK-3096–3100), and the execution twin recalibrated from fills
//! (R12-TICK-2080).

#![allow(clippy::panic_in_result_fn, clippy::float_cmp)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_simulation_engine::costs::CostModel;
use qip_twin::calibration::FillCalibration;
use qip_twin::futures::{
    Branch, BranchStatus, FutureTree, ObservedMoves, Provenance, TreeConfig, TwinState, WorldDriver,
};
use std::collections::BTreeMap;

fn at(s: i64) -> Timestamp {
    Timestamp::from_secs(1_791_676_800 + s)
}

/// Two instruments, `points` one-second mids each, wiggling deterministically.
fn series(points: usize) -> BTreeMap<String, Vec<(Timestamp, f64)>> {
    let mut out = BTreeMap::new();
    for (name, base) in [("obj-ACME", 100.0), ("obj-BETA", 50.0)] {
        let mut mids = Vec::new();
        for i in 0..points {
            let wiggle = ((i * 7919) % 13) as f64 / 13.0 - 0.5;
            mids.push((at(i as i64), base * (1.0 + 0.001 * wiggle)));
        }
        out.insert(name.to_string(), mids);
    }
    out
}

fn config() -> TreeConfig {
    TreeConfig {
        horizon: Duration::from_secs(60),
        shock_sigmas: vec![1.0, 2.0, 6.0],
        propagation: vec![0.0, 1.0],
        prune_below: 0.01,
        split_beyond_sigmas: 4.0,
        max_branches: 256,
    }
}

fn driver() -> WorldDriver {
    WorldDriver {
        id: "evt-strike".into(),
        family: None,
        sensitivities: BTreeMap::from([("obj-ACME".to_string(), -1.0)]),
        evidence: vec!["wire:strike-announced".into()],
    }
}

/// Mutation: add an `Observed` variant to `Provenance` and stamp it on the
/// baseline branch — the provenance assertion fails, and a document
/// claiming `observed` would deserialise.
#[test]
fn every_branch_is_synthetic_and_a_document_claiming_observed_is_not_a_branch() -> Result<()> {
    let state = TwinState::fit(&series(200), at(199))?;
    let tree = FutureTree::grow(&state, &[driver()], config())?;
    assert!(tree.branches.len() > 1, "premise: a tree was grown");
    assert!(
        tree.branches
            .values()
            .all(|b| b.provenance == Provenance::Synthetic)
    );
    let mut json = serde_json::to_value(tree.branches.values().next().expect("a branch"))?;
    assert_eq!(json["provenance"], "synthetic");
    json["provenance"] = serde_json::Value::String("observed".into());
    assert!(
        serde_json::from_value::<Branch>(json).is_err(),
        "a branch cannot be read back as observed data"
    );
    Ok(())
}

/// Mutation: keep the fitted points on `TwinState` (copy the world into the
/// twin) — the serialised size grows with history.
#[test]
fn the_twin_is_a_compact_learned_state_whose_size_does_not_grow_with_history() -> Result<()> {
    let small = TwinState::fit(&series(50), at(49))?;
    let large = TwinState::fit(&series(5_000), at(4_999))?;
    assert_eq!(
        large.instruments["obj-ACME"].observations, 5_000,
        "premise: all of it was used"
    );
    let small_len = serde_json::to_vec(&small)?.len();
    let large_len = serde_json::to_vec(&large)?.len();
    assert!(
        large_len < small_len + 64,
        "a hundred times the history is not a larger twin: {small_len} vs {large_len} bytes"
    );
    Ok(())
}

/// Mutation: drop the `*at <= as_of` filter in `TwinState::fit` — a mid
/// after the as-of moves the fitted state.
#[test]
fn the_twin_state_reads_no_mid_after_its_as_of() -> Result<()> {
    let mut history = series(100);
    let before = TwinState::fit(&history, at(60))?;
    for mids in history.values_mut() {
        for (t, mid) in mids.iter_mut() {
            if *t > at(60) {
                *mid *= 3.0;
            }
        }
    }
    assert_eq!(TwinState::fit(&history, at(60))?, before);
    Ok(())
}

/// Mutation: generate one sign or one propagation strength only in `grow` —
/// the assumption sets collapse. Mutation: drop `beyond_history` — no branch
/// explores a world outside the fitted history.
#[test]
fn branches_vary_causal_assumptions_and_carry_probability_evidence_propagation_impact_and_hedges()
-> Result<()> {
    let state = TwinState::fit(&series(200), at(199))?;
    let tree = FutureTree::grow(&state, &[driver()], config())?;
    // 3 sizes x 2 signs x 2 strengths, and the baseline.
    assert_eq!(tree.branches.len(), 13);
    let total: f64 = tree.branches.values().map(|b| b.probability).sum();
    assert!(
        (total - 1.0).abs() < 1e-9,
        "the tree is a distribution: {total}"
    );
    let shocks: std::collections::BTreeSet<i64> = tree
        .branches
        .values()
        .map(|b| (b.assumptions["shock_sigma"] * 100.0).round() as i64)
        .collect();
    assert!(shocks.contains(&600) && shocks.contains(&-600) && shocks.contains(&0));
    assert!(
        tree.branches.values().any(|b| b.beyond_history),
        "a world not in history is explored"
    );
    let shocked = tree
        .branches
        .values()
        .find(|b| b.assumptions["shock_sigma"] > 1.5 && b.assumptions["propagation"] > 0.5)
        .expect("a propagated shock");
    assert!(!shocked.evidence.is_empty());
    assert!(
        shocked.propagation.iter().any(|p| p.to == "obj-BETA"),
        "second-order propagation to BETA"
    );
    assert!(shocked.impact_bps["obj-BETA"].abs() > 0.0);
    assert!(
        !shocked.hedges.is_empty(),
        "the worst-hit instrument gets a hedge"
    );
    // The §24.9 contract adoption: a shocked branch crosses as a WorldBranch.
    let contract = shocked.to_world_branch()?;
    assert!(!contract.causal_deltas.is_empty());
    Ok(())
}

/// Mutation: skip the reweighting in `observe` — the branch nearest
/// reality does not gain probability and nothing is pruned.
#[test]
fn reality_reweights_toward_the_branch_it_resembles_and_prunes_the_implausible() -> Result<()> {
    let state = TwinState::fit(&series(200), at(199))?;
    let mut tree = FutureTree::grow(&state, &[driver()], config())?;
    let target = tree
        .branches
        .values()
        .find(|b| b.assumptions["shock_sigma"] == 2.0 && b.assumptions["propagation"] == 1.0)
        .expect("a +2 sigma propagated branch")
        .clone();
    let prior = target.probability;
    let report = tree.observe(&ObservedMoves {
        at: at(260),
        moves_bps: target.impact_bps.clone(),
    })?;
    let posterior = tree.branches[&target.id].probability;
    assert!(
        posterior > prior,
        "the branch reality resembles gains: {prior} -> {posterior}"
    );
    assert!(
        !report.pruned.is_empty(),
        "branches far from reality are pruned"
    );
    for id in &report.pruned {
        assert!(matches!(
            tree.branches[id].status,
            BranchStatus::Pruned { .. }
        ));
    }
    assert!(report.log_score.is_finite());
    Ok(())
}

fn family_driver(id: &str, family: &str, instrument: &str, sensitivity: f64) -> WorldDriver {
    WorldDriver {
        id: id.into(),
        family: Some(family.into()),
        sensitivities: BTreeMap::from([(instrument.to_string(), sensitivity)]),
        evidence: vec![format!("wire:{id}")],
    }
}

/// R12-WORLD-9035. A tree over drivers of two scenario families, reweighted
/// and pruned by reality, still states each family as a distribution whose
/// weights sum to one, the families together as one, and never holds more
/// branches than its bound. Mutations: divide a member's weight by the
/// tree's live mass instead of its family's; give every driver branch the
/// baseline family; read a driver's family from its id; drop the bound check
/// before a split.
#[test]
fn live_branch_weights_sum_to_one_within_each_family_after_pruning() -> Result<()> {
    let state = TwinState::fit(&series(200), at(199))?;
    let drivers = [
        family_driver("evt-guide-up", "guidance", "obj-ACME", 1.0),
        family_driver("evt-guide-beta", "guidance", "obj-BETA", 1.0),
        family_driver("evt-rates", "rates", "obj-ACME", -1.0),
    ];
    // 3 drivers x 3 sizes x 2 signs x 2 strengths, plus the baseline, plus
    // room for exactly one split.
    let bound = 3 * 3 * 2 * 2 + 1 + 1;
    let mut tree = FutureTree::grow(
        &state,
        &drivers,
        TreeConfig {
            max_branches: bound,
            ..config()
        },
    )?;
    let sum_to_one = |tree: &FutureTree| -> Result<()> {
        let families = tree.families()?;
        let mass: f64 = families.values().map(|f| f.mass).sum();
        assert!((mass - 1.0).abs() < 1e-9, "family masses sum to {mass}");
        for (family, partition) in &families {
            let within: f64 = partition.weights.values().sum();
            assert!(
                (within - 1.0).abs() < 1e-9,
                "family {family}'s weights sum to {within}"
            );
            for id in partition.weights.keys() {
                assert_eq!(&tree.branches[id].family, family, "{id} filed elsewhere");
                assert_eq!(tree.branches[id].status, BranchStatus::Live, "{id} pruned");
            }
        }
        Ok(())
    };
    let families = tree.families()?;
    assert_eq!(
        families.keys().map(String::as_str).collect::<Vec<_>>(),
        ["baseline", "guidance", "rates"],
        "the two guidance drivers are one family, and the baseline its own"
    );
    assert_eq!(families["guidance"].weights.len(), 24);
    sum_to_one(&tree)?;

    // Reality lands on one guidance world: the others are pruned.
    let target = tree.branches["evt-guide-up:s+6.00:p1.00"]
        .impact_bps
        .clone();
    let report = tree.observe(&ObservedMoves {
        at: at(260),
        moves_bps: target,
    })?;
    assert!(
        !report.pruned.is_empty(),
        "premise: reality pruned branches"
    );
    assert!(report.split.is_none(), "premise: reality was on a branch");
    sum_to_one(&tree)?;
    assert!(
        tree.families()?
            .values()
            .map(|f| f.weights.len())
            .sum::<usize>()
            < bound - 1,
        "pruned branches left their families"
    );

    // Reality far outside every branch splits one in, inheriting its nearest
    // parent's family; a second time, the tree is at its bound and does not.
    // The second lands far from the first split too, so only the bound
    // stops it splitting.
    let far = |at_s, sign: f64| ObservedMoves {
        at: at(at_s),
        moves_bps: BTreeMap::from([
            ("obj-ACME".to_string(), sign * 300.0),
            ("obj-BETA".to_string(), -sign * 300.0),
        ]),
    };
    let split = tree
        .observe(&far(320, 1.0))?
        .split
        .expect("premise: a split");
    let parent = tree.branches[&split]
        .parent
        .clone()
        .expect("a nearest parent");
    assert_eq!(tree.branches[&split].family, tree.branches[&parent].family);
    sum_to_one(&tree)?;
    assert!(
        tree.observe(&far(380, -1.0))?.split.is_none(),
        "split past the bound"
    );
    assert!(
        tree.branches.len() <= bound,
        "{} branches",
        tree.branches.len()
    );
    sum_to_one(&tree)?;

    // The records are partitioned the same way, and what the contract
    // refuses is listed rather than lost.
    let records = tree.world_branches_by_family();
    let live = tree.live().count();
    let recorded: usize = records.records.values().map(Vec::len).sum();
    assert_eq!(recorded + records.unrecorded.len(), live);
    for (family, branches) in &records.records {
        for record in branches {
            assert_eq!(&tree.branches[&record.branch_id].family, family);
        }
    }
    Ok(())
}

/// A driver cannot name an empty family or pose as the baseline: its
/// branches would be weighed as the world continuing. Mutation: accept any
/// name in `WorldDriver::family`.
#[test]
fn a_driver_naming_an_empty_or_reserved_family_is_refused_by_name() -> Result<()> {
    let state = TwinState::fit(&series(200), at(199))?;
    for family in ["", "  ", "baseline", "reality"] {
        let refused = FutureTree::grow(
            &state,
            &[family_driver("evt-x", family, "obj-ACME", 1.0)],
            config(),
        )
        .expect_err("a reserved or empty family was accepted");
        assert!(refused.message().contains("evt-x"), "{}", refused.message());
    }
    let named = family_driver("evt-x", "guidance", "obj-ACME", 1.0);
    let tree = FutureTree::grow(&state, &[named], config())?;
    assert!(tree.live().any(|b| b.family == "guidance"));
    Ok(())
}

/// Mutation: never split in `observe` — reality outside every branch
/// leaves the tree exploring only worlds it is not in.
#[test]
fn reality_outside_every_branch_splits_a_new_one_and_an_observation_before_the_fork_is_refused()
-> Result<()> {
    let state = TwinState::fit(&series(200), at(199))?;
    let mut tree = FutureTree::grow(&state, &[driver()], config())?;
    assert!(
        tree.observe(&ObservedMoves {
            at: at(199),
            moves_bps: BTreeMap::from([("obj-ACME".to_string(), 1.0)]),
        })
        .is_err(),
        "reality from the fork's own instant does not test a future"
    );
    let report = tree.observe(&ObservedMoves {
        at: at(300),
        moves_bps: BTreeMap::from([
            ("obj-ACME".to_string(), 50_000.0),
            ("obj-BETA".to_string(), -50_000.0),
        ]),
    })?;
    let split = report.split.expect("a world nobody grew is split in");
    let branch = &tree.branches[&split];
    assert_eq!(
        branch.provenance,
        Provenance::Synthetic,
        "still a hypothesis, never the record"
    );
    assert!(branch.parent.is_some());
    Ok(())
}

/// Mutation: make `FillCalibration::observe` a no-op, or have
/// `cost_model` ignore the bias — the spread stays where it was.
#[test]
fn after_fills_with_a_systematic_error_the_twins_spread_moves_toward_the_observed_fills()
-> Result<()> {
    let base = CostModel::liquid_equity();
    let mut calibration = FillCalibration::default();
    assert_eq!(
        calibration.cost_model(&base, "XLON"),
        base,
        "premise: uncalibrated is the base"
    );
    // The twin fills twelve basis points better than reality every time.
    for _ in 0..20 {
        calibration.observe("XLON", -12.0)?;
    }
    let calibrated = calibration.cost_model(&base, "XLON");
    assert!(
        calibrated.half_spread_bps > base.half_spread_bps + 10.0,
        "the twin now pays what reality charged: {} -> {}",
        base.half_spread_bps,
        calibrated.half_spread_bps
    );
    assert_eq!(
        calibration.cost_model(&base, "XPAR"),
        base,
        "another venue is untouched"
    );
    assert!(calibration.observe("XLON", f64::NAN).is_err());
    assert!(FillCalibration::new(0.0).is_err());
    Ok(())
}

/// R12-TICK-10121. A scenario world must say what it does to the book, and
/// the distribution it says it in must be a distribution: pruning removes
/// branches without redistributing their mass, so weights read straight off
/// the live probabilities would sum to less than one after the first
/// observation and understate every tail.
///
/// Mutation: drop the `/ mass` renormalisation in `portfolio_outcomes` — the
/// post-prune weights sum to the surviving mass and the sum assertion fails.
/// Mutation: ignore the position weight (use the raw impact) — the
/// hand-computed portfolio move no longer matches.
#[test]
fn a_scenario_world_yields_a_portfolio_outcome_distribution_whose_weights_sum_to_one() -> Result<()>
{
    let state = TwinState::fit(&series(200), at(199))?;
    let mut tree = FutureTree::grow(&state, &[driver()], config())?;
    let positions = BTreeMap::from([
        ("obj-ACME".to_string(), 0.5),
        ("obj-BETA".to_string(), -0.25),
    ]);

    let fresh = tree.portfolio_outcomes(&positions)?;
    assert_eq!(
        fresh.len(),
        tree.live().count(),
        "premise: one per live branch"
    );
    assert!(fresh.len() > 1, "premise: more than the baseline");
    assert!((fresh.iter().map(|o| o.weight).sum::<f64>() - 1.0).abs() < 1e-9);
    // The driver shocks ACME only, so a shocked branch moves the book by half
    // its ACME impact and the baseline by nothing.
    let shocked = tree
        .live()
        .find(|b| b.assumptions["shock_sigma"] > 1.5 && b.assumptions["propagation"] > 0.5)
        .expect("a +2 sigma propagated branch")
        .clone();
    let expected: f64 = 0.5 * shocked.impact_bps["obj-ACME"]
        - 0.25 * shocked.impact_bps.get("obj-BETA").copied().unwrap_or(0.0);
    assert!(
        expected.abs() > 1e-6,
        "premise: the shocked branch moves the book"
    );
    let found = fresh
        .iter()
        .find(|o| o.branch_id == shocked.id)
        .expect("the shocked branch has an outcome");
    assert!((found.portfolio_bps - expected).abs() < 1e-9);

    // Mass lost to pruning is not redistributed in the branch records, so
    // simulate it: halve every probability and the weights must still sum to
    // one. (`observe` happens to renormalise; the distribution must not rely
    // on that.)
    for branch in tree.branches.values_mut() {
        branch.probability *= 0.5;
    }
    let halved = tree.portfolio_outcomes(&positions)?;
    assert!((halved.iter().map(|o| o.weight).sum::<f64>() - 1.0).abs() < 1e-9);
    for branch in tree.branches.values_mut() {
        branch.probability *= 2.0;
    }

    // After reality prunes branches the survivors still carry all the mass.
    let report = tree.observe(&ObservedMoves {
        at: at(260),
        moves_bps: shocked.impact_bps.clone(),
    })?;
    assert!(!report.pruned.is_empty(), "premise: branches were pruned");
    let after = tree.portfolio_outcomes(&positions)?;
    assert!(after.len() < fresh.len());
    assert!((after.iter().map(|o| o.weight).sum::<f64>() - 1.0).abs() < 1e-9);

    // Refusals rather than a distribution of zeros.
    assert!(tree.portfolio_outcomes(&BTreeMap::new()).is_err());
    assert!(
        tree.portfolio_outcomes(&BTreeMap::from([("obj-NONE".to_string(), 1.0)]))
            .is_err(),
        "a book the twin does not model has no outcome"
    );
    assert!(
        tree.portfolio_outcomes(&BTreeMap::from([("obj-ACME".to_string(), f64::NAN)]))
            .is_err()
    );
    Ok(())
}

/// R12-TICK-2080: the twin's impact model is calibrated from the platform's
/// own fills, not only its spread. Fills whose error grows with the square
/// root of their participation — a purely size-dependent misfit, no bias —
/// move the impact coefficient toward what the fills charged and leave the
/// spread where it was; fills with no participation beside them move the
/// spread alone, as before.
///
/// Mutation: make `observe_with_participation` ignore the participation and
/// update the bias alone — the impact coefficient never moves and the first
/// assertion fails.
#[test]
fn after_fills_with_a_systematic_size_dependent_error_the_twins_impact_coefficient_moves_toward_the_observed_fills()
-> Result<()> {
    let base = CostModel::liquid_equity();
    let mut calibration = FillCalibration::default();
    // Reality charged 20 bps more impact per unit of sqrt(participation)
    // than the twin priced, on fills of four sizes, forty times over.
    let participations: [f64; 4] = [0.01, 0.04, 0.09, 0.16];
    for round in 0..40 {
        let participation = participations[round % participations.len()];
        let error_bps = -20.0 * participation.sqrt();
        calibration.observe_with_participation("XLON", error_bps, Some(participation))?;
    }
    let calibrated = calibration.cost_model(&base, "XLON");
    assert!(
        calibrated.impact_coefficient_bps > base.impact_coefficient_bps + 10.0,
        "the impact coefficient did not move toward the observed fills: {} -> {}",
        base.impact_coefficient_bps,
        calibrated.impact_coefficient_bps
    );
    assert!(
        (calibrated.half_spread_bps - base.half_spread_bps).abs() < 3.0,
        "a size-dependent error was charged to the spread: {} -> {}",
        base.half_spread_bps,
        calibrated.half_spread_bps
    );
    let venue = calibration.venue("XLON").expect("calibrated");
    assert_eq!(venue.participation_observations, 40);
    assert_eq!(venue.observations, 40);

    // Without participation the slope is untouched and the spread moves.
    let mut blind = FillCalibration::default();
    for _ in 0..20 {
        blind.observe_with_participation("XLON", -12.0, None)?;
    }
    let venue = blind.venue("XLON").expect("calibrated");
    assert_eq!(venue.impact_slope_bps, 0.0);
    assert_eq!(venue.participation_observations, 0);
    assert!(blind.cost_model(&base, "XLON").half_spread_bps > base.half_spread_bps + 10.0);

    assert!(
        calibration
            .observe_with_participation("XLON", -1.0, Some(1.5))
            .is_err(),
        "a fill that took more than the bar traded is refused"
    );

    // Fills that are one size for the twin's purposes — a share against bars
    // of slightly different volume — carry no size law however wild their
    // errors, and the slope holds still while the spread takes them. Before
    // the identifiability floor two such fills fitted a slope of thousands of
    // basis points per unit root and the twin's impact term left the earth.
    let mut one_size = FillCalibration::default();
    one_size.observe_with_participation("XLON", -30.0, Some(1.0e-6))?;
    one_size.observe_with_participation("XLON", 25.0, Some(2.0e-6))?;
    one_size.observe_with_participation("XLON", -40.0, Some(1.5e-6))?;
    let venue = one_size.venue("XLON").expect("calibrated");
    assert_eq!(venue.participation_observations, 3);
    assert_eq!(
        venue.impact_slope_bps, 0.0,
        "fills of one size fitted a size law: {}",
        venue.impact_slope_bps
    );
    assert!(
        (one_size.cost_model(&base, "XLON").impact_coefficient_bps - base.impact_coefficient_bps)
            .abs()
            < 1e-12
    );
    Ok(())
}
