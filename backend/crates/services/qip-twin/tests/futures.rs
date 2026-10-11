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
