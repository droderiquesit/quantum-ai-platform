//! The lattice rows' specialist members as the kernel feeds them
//! (WORLD-3086..3088): the regime path learned from the closes every pass
//! carries, and the catalyst and structural claims read from the world
//! model at the instant of the pass.
//!
//! Each test asserts its premise first and names the mutation that fails it.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};

use qip_core::time::{Duration, Timestamp};
use qip_kernel::lattice_stage::{
    CATALYST_WINDOW, LatticeInputs, LatticeStage, LatticeTarget, MACRO_PASS_THROUGH,
    MAX_CATALYST_MOVE, signals_from_world,
};
use qip_world_model::causal::{
    CausalEdge, CausalGraph, Mechanism, RETIREMENT_CONSECUTIVE_FAILURES,
};
use qip_world_model::lattice::LatticeHorizon;
use qip_world_model::now::WorldReading;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn day(n: i64) -> Timestamp {
    start().saturating_add(Duration::from_days(n))
}

/// Twelve daily closes that zigzag: up one per cent on odd days, down on
/// even days, so the regime label that follows the sign of the newest
/// return sees a positive drift under `bull` and a negative one under `bear`.
fn zigzag() -> Vec<(Timestamp, f64)> {
    let mut closes = vec![(day(0), 100.0)];
    for i in 1..=12 {
        let last = closes[i - 1].1;
        let next = if i % 2 == 1 { last * 1.01 } else { last * 0.99 };
        closes.push((day(i as i64), next));
    }
    closes
}

fn target(closes: Vec<(Timestamp, f64)>, regime: &str) -> LatticeTarget {
    LatticeTarget {
        subject: "obj-AAA".into(),
        closes,
        regime: regime.into(),
        held_quantity: 0.0,
        now_return: None,
    }
}

/// Run a stage over the zigzag one bar at a time, labelling each pass by the
/// sign of its newest return, then one more pass labelled `final_regime`
/// with the subject surprised so every row is issued out of turn. Returns
/// the weeks-months median of that final pass.
fn weeks_median_after_learning(final_regime: &str) -> (LatticeStage, f64) {
    let closes = zigzag();
    let mut stage = LatticeStage::new().unwrap();
    for k in 6..closes.len() {
        let history = closes[..=k].to_vec();
        let regime = if history[k].1 > history[k - 1].1 {
            "bull"
        } else {
            "bear"
        };
        stage.run(
            LatticeInputs {
                cycle: k as u64,
                targets: vec![target(history, regime)],
                surprised: BTreeSet::new(),
            },
            closes[k].0,
        );
    }
    let at = day(13);
    let review = stage.run(
        LatticeInputs {
            cycle: 99,
            targets: vec![target(closes, final_regime)],
            surprised: BTreeSet::from(["obj-AAA".to_string()]),
        },
        at,
    );
    let weeks = review
        .issued
        .iter()
        .find(|f| LatticeHorizon::of(f.horizon) == LatticeHorizon::WeeksMonths)
        .expect("the weeks-months row was issued out of turn");
    let qip_contracts::forecast::OutcomeDistribution::Quantiles { points } = &weeks.distribution
    else {
        panic!("quantiles")
    };
    (stage, points[1].value.to_f64())
}

/// WORLD-3087: the stage learns each regime label's drift from the closes it
/// is handed, on the default path, and the regime-path member projects it, so
/// the same closes forecast under `bear` sit below the same closes under
/// `bull`. Mutation: drop the regime drifts from `LatticeStage::context_for`.
#[test]
fn the_weeks_row_learns_each_regimes_drift_from_the_passes_closes_and_a_regime_change_moves_its_median()
 {
    let (stage, bull) = weeks_median_after_learning("bull");
    let (_, bear) = weeks_median_after_learning("bear");
    let drifts = stage.regime_drifts("obj-AAA");
    assert!(
        drifts.get("bull").is_some_and(|d| *d > 0.009)
            && drifts.get("bear").is_some_and(|d| *d < -0.009),
        "premise: both regimes' drifts were learned from the closes: {drifts:?}"
    );
    assert!(
        bull > bear,
        "the weeks-months median under bull ({bull}) sits above bear ({bear})"
    );
    assert!(
        bull - bear > 1.0,
        "a two per cent per day drift gap over thirty days moves the ensemble median by \
         more than a point: {bull} vs {bear}"
    );
}

fn edge(cause: &str, recorded: Timestamp) -> CausalEdge {
    CausalEdge::new(
        cause,
        "obj-AAA",
        Mechanism::SupplyChain,
        0.5,
        Duration::from_days(90),
        recorded,
    )
    .unwrap()
    .with_confidence(0.8)
    .unwrap()
}

/// WORLD-3088: the structural claims the quarters row reads are the live,
/// unretired edges into the subject as of the pass, so a claim retired by
/// three consecutive condition failures leaves the set and the row narrows
/// on the next pass; a claim retired after the instant asked about is still
/// read as live at that instant. Mutation: read `causal.edges()` instead of
/// `causal.incoming(subject, now)` in `signals_from_world`.
#[test]
fn a_claim_retired_by_its_condition_failures_leaves_the_structural_set_as_of_the_pass() {
    let mut causal = CausalGraph::new();
    causal.add(edge("kestrel", day(0)));
    causal.add(edge("rates", day(0)));
    let subjects = vec!["obj-AAA".to_string()];
    let before = signals_from_world(&causal, &[], &subjects, day(5));
    assert_eq!(
        before.structural_claims["obj-AAA"].len(),
        2,
        "premise: both claims are live"
    );
    for (i, claim) in before.structural_claims["obj-AAA"].iter().enumerate() {
        assert!((claim.transmission - 0.4).abs() < 1e-12, "{i}: {claim:?}");
        assert_eq!(claim.mechanism, "supply_chain");
    }
    for k in 0..RETIREMENT_CONSECUTIVE_FAILURES {
        causal
            .record_condition_failure("kestrel", "obj-AAA", "bear", day(10 + k as i64))
            .unwrap();
    }
    assert_eq!(
        causal.retired(day(20)).len(),
        1,
        "premise: kestrel's claim retired"
    );
    let after = signals_from_world(&causal, &[], &subjects, day(20));
    let causes: Vec<&str> = after.structural_claims["obj-AAA"]
        .iter()
        .map(|c| c.cause.as_str())
        .collect();
    assert_eq!(causes, vec!["rates"], "the retired claim is gone");
    let then = signals_from_world(&causal, &[], &subjects, day(11));
    assert_eq!(
        then.structural_claims["obj-AAA"].len(),
        2,
        "as of a day before the retirement, both were live"
    );
}

fn surprise(series: &str, value: f64, available: Timestamp) -> WorldReading {
    WorldReading {
        feature: "macro_surprise".into(),
        subject: series.into(),
        value,
        valid_at: available,
        available_at: available,
    }
}

/// WORLD-3086: a macro surprise that became knowable inside the catalyst
/// window is the hours-days row's pending catalyst, scaled by the stated
/// pass-through and capped; one older than the window, or not yet knowable
/// at the pass, is not. The largest of several wins. Mutation: drop the
/// window filter in `signals_from_world`.
#[test]
fn a_recent_macro_surprise_is_the_pending_catalyst_and_an_old_or_future_one_is_not() {
    let causal = CausalGraph::new();
    let now = day(10);
    let old = now.saturating_sub(CATALYST_WINDOW + Duration::from_hours(1));
    let recent = now.saturating_sub(Duration::from_hours(6));
    let future = now.saturating_add(Duration::from_hours(1));
    let none = signals_from_world(&causal, &[surprise("US.CPI.YOY", 0.3, old)], &[], now);
    assert!(none.pending_catalyst.is_none(), "an old surprise is spent");
    let not_yet = signals_from_world(&causal, &[surprise("US.CPI.YOY", 0.3, future)], &[], now);
    assert!(
        not_yet.pending_catalyst.is_none(),
        "a surprise not yet knowable is not read"
    );
    let some = signals_from_world(
        &causal,
        &[
            surprise("US.PPI.YOY", 0.1, recent),
            surprise("US.CPI.YOY", -0.3, recent),
            surprise("US.NFP", 9.0, old),
        ],
        &[],
        now,
    );
    let catalyst = some.pending_catalyst.expect("a recent surprise pends");
    assert_eq!(
        catalyst.source, "macro_surprise/US.CPI.YOY",
        "the largest recent one"
    );
    assert!((catalyst.expected_abs_move - 0.3 * MACRO_PASS_THROUGH).abs() < 1e-12);
    let huge = signals_from_world(&causal, &[surprise("US.CPI.YOY", 5.0, recent)], &[], now);
    assert!(
        (huge.pending_catalyst.unwrap().expected_abs_move - MAX_CATALYST_MOVE).abs() < 1e-12,
        "capped"
    );
    let _: BTreeMap<String, f64> = some.flow_imbalance;
}
