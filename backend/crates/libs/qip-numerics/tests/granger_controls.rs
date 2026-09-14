//! The controlled Granger test — blueprint §9.2's method as the blueprint
//! actually names it, "Granger-style lead-lag **with controls**".
//!
//! The property every test here turns on: a lead-lag relationship that
//! exists only because two series share a persistent driver must survive an
//! uncontrolled test and fail a controlled one. If it fails both, the
//! fixture proves nothing about controls; if it survives both, the control
//! is not working. Each test therefore asserts the uncontrolled result
//! first, as its own premise.

use qip_numerics::stats::{granger_causality, granger_causality_controlling_for};

/// A deterministic linear congruential stream in `[-0.5, 0.5)`.
///
/// Deterministic rather than sampled so that a failure is reproducible and a
/// pass is not luck.
fn stream(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) - 0.5
    }
}

/// A persistent common driver, and three series loading on it
/// contemporaneously with independent noise.
///
/// Nothing in this process is a lagged relationship between two series. Any
/// lead-lag a test reports between them is therefore spurious by
/// construction — which is what makes it the right fixture for a control.
fn shared_driver(bars: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut noise = stream(0x2545_F491_4F6C_DD1D);
    let mut driver = Vec::with_capacity(bars);
    let mut level = 0.0;
    for _ in 0..bars {
        level = 0.8 * level + noise();
        driver.push(level);
    }
    let a: Vec<f64> = (0..bars).map(|t| driver[t] + 0.6 * noise()).collect();
    let b: Vec<f64> = (0..bars).map(|t| driver[t] + 0.6 * noise()).collect();
    (a, b, driver)
}

#[test]
fn a_lead_lag_that_is_only_a_shared_driver_survives_no_control_and_the_uncontrolled_test_finds_it()
{
    let (a, b, driver) = shared_driver(400);

    // The premise, and the whole point of asserting it: without this, a
    // controlled test reporting "not significant" would be evidence of
    // nothing at all. This is the failure §9.2's qualifier exists to
    // prevent, reproduced.
    let uncontrolled = granger_causality(&a, &b, 1).expect("well-formed series");
    assert!(
        uncontrolled.p_value < 0.01,
        "the premise: an uncontrolled test must manufacture this edge, p was {}",
        uncontrolled.p_value
    );

    let controlled =
        granger_causality_controlling_for(&a, &b, &[&driver], 1).expect("well-formed series");
    assert!(
        controlled.p_value > uncontrolled.p_value,
        "holding the common driver constant must weaken the apparent link: uncontrolled p {} \
         vs controlled p {}",
        uncontrolled.p_value,
        controlled.p_value
    );
    assert!(
        controlled.p_value >= 0.01,
        "and it must no longer clear the bar the platform writes edges at, p was {}",
        controlled.p_value
    );
}

#[test]
fn a_genuine_lagged_link_survives_a_control_that_is_not_its_cause() {
    let mut noise = stream(0x9E37_79B9_7F4A_7C15);
    let bars = 400;
    // An unrelated persistent series, used as a control. It has nothing to
    // do with the link below, so removing it must not remove the link.
    let mut unrelated = Vec::with_capacity(bars);
    let mut level = 0.0;
    for _ in 0..bars {
        level = 0.8 * level + noise();
        unrelated.push(level);
    }
    let cause: Vec<f64> = (0..bars).map(|_| noise()).collect();
    // A real one-bar transmission: the effect genuinely depends on the
    // cause's previous value.
    let effect: Vec<f64> = (0..bars)
        .map(|t| {
            if t == 0 {
                noise()
            } else {
                0.7 * cause[t - 1] + 0.3 * noise()
            }
        })
        .collect();

    // The premise: the uncontrolled test sees it.
    let uncontrolled = granger_causality(&cause, &effect, 1).expect("well-formed series");
    assert!(
        uncontrolled.p_value < 0.01,
        "the premise: this link is real and the uncontrolled test finds it, p was {}",
        uncontrolled.p_value
    );

    // The failure this prevents: a control that removes every edge is not a
    // control, it is a refusal, and it would read as "the graph was
    // spurious" while actually being "the method stopped working".
    let controlled =
        granger_causality_controlling_for(&cause, &effect, &[&unrelated], 1).expect("well-formed");
    assert!(
        controlled.p_value < 0.01,
        "an irrelevant control must not remove a real link, p was {}",
        controlled.p_value
    );
}

#[test]
fn the_cause_coefficient_keeps_its_index_whatever_number_of_controls_is_passed() {
    // A fixture in which the cause and the controls pull the effect in
    // *opposite* directions with different magnitudes, so that reading the
    // wrong column is arithmetically visible rather than merely different.
    //
    // This test asserted only `coefficient.abs() > 1e-9` at first, and a
    // mutation that moved the cause block to the end of the design — making
    // the read index land on a control — passed it unchanged. A noise
    // control has a non-zero coefficient too. That is the exact class of
    // defect mutation testing exists to catch, and it is recorded here
    // because the weak version looked entirely reasonable.
    let bars = 400;
    let mut noise = stream(0xDEAD_BEEF_CAFE_F00D);
    let cause: Vec<f64> = (0..bars).map(|_| noise()).collect();
    let first: Vec<f64> = (0..bars).map(|_| noise()).collect();
    let second: Vec<f64> = (0..bars).map(|_| noise()).collect();
    let third: Vec<f64> = (0..bars).map(|_| noise()).collect();
    // +0.8 on the cause's lag, and a large negative loading on each
    // control's lag.
    let effect: Vec<f64> = (0..bars)
        .map(|t| {
            if t == 0 {
                0.0
            } else {
                0.8 * cause[t - 1] - 0.9 * first[t - 1] - 0.7 * second[t - 1]
                    + 0.6 * third[t - 1]
                    + 0.05 * noise()
            }
        })
        .collect();

    // The premise: with the controls present, the design is well determined
    // and the cause's true loading is recoverable at all.
    let one = granger_causality_controlling_for(&cause, &effect, &[&first], 1)
        .expect("well-formed series");
    assert!(
        (one.coefficient - 0.8).abs() < 0.1,
        "the premise: with one control the cause's own loading is recovered near +0.8, got {}",
        one.coefficient
    );

    // The failure this prevents, and it is not hypothetical: the *sign* of
    // this coefficient is what `qip_world_model::granger` turns into
    // `TemporalPrecedence` or `InverseTemporalPrecedence`. A design in which
    // the cause block sat last would make this index read a control's
    // coefficient on every call that supplied one — here a strongly negative
    // one — and the graph would fill with edges pointed exactly backwards
    // while every p-value stayed entirely plausible.
    for controls in [
        vec![&first[..]],
        vec![&first[..], &second[..]],
        vec![&first[..], &second[..], &third[..]],
    ] {
        let count = controls.len();
        let test =
            granger_causality_controlling_for(&cause, &effect, &controls, 1).expect("well-formed");
        assert!(
            (test.coefficient - 0.8).abs() < 0.1,
            "with {count} control(s) the read index must still be the cause's own loading \
             (+0.8), not a control's; got {}",
            test.coefficient
        );
    }
}

#[test]
fn a_control_sampled_on_different_bars_is_refused_rather_than_truncated() {
    let (a, b, _) = shared_driver(300);
    let short = vec![0.1; 299];
    let refused = granger_causality_controlling_for(&a, &b, &[&short], 1);
    assert!(
        refused.is_err(),
        "a control on the wrong instants adjusts for the wrong thing and is refused, not trimmed"
    );

    // And the gate admits the right length — a gate that refuses everything
    // is not a working gate.
    let right = vec![0.1; 300];
    assert!(
        granger_causality_controlling_for(&a, &b, &[&right], 1).is_ok(),
        "a correctly sampled control is admitted"
    );
}

#[test]
fn a_non_finite_control_observation_is_refused_rather_than_filtered() {
    let (a, b, mut driver) = shared_driver(300);
    // The premise: this control is accepted before it is spoiled.
    assert!(
        granger_causality_controlling_for(&a, &b, &[&driver], 1).is_ok(),
        "the premise: the control is well formed to begin with"
    );
    driver[17] = f64::NAN;
    assert!(
        granger_causality_controlling_for(&a, &b, &[&driver], 1).is_err(),
        "a non-finite control is refused at the boundary, not silently dropped downstream"
    );
}

#[test]
fn the_uncontrolled_test_is_exactly_the_controlled_one_with_no_controls() {
    let (a, b, _) = shared_driver(250);
    let plain = granger_causality(&a, &b, 1).expect("well-formed");
    let empty = granger_causality_controlling_for(&a, &b, &[], 1).expect("well-formed");
    // The failure this prevents: two code paths computing "the same" test
    // and drifting apart, so that an edge's p-value depends on which
    // function the caller happened to reach for.
    assert_eq!(
        plain, empty,
        "the uncontrolled form must be the controlled one with an empty set, bit for bit"
    );
}

#[test]
fn too_many_controls_for_the_history_is_refused_rather_than_fitted_on_nothing() {
    let bars = 12;
    let mut noise = stream(7);
    let a: Vec<f64> = (0..bars).map(|_| noise()).collect();
    let b: Vec<f64> = (0..bars).map(|_| noise()).collect();
    let controls: Vec<Vec<f64>> = (0..8)
        .map(|_| (0..bars).map(|_| noise()).collect())
        .collect();
    let refs: Vec<&[f64]> = controls.iter().map(|c| c.as_slice()).collect();

    // The premise: with no controls this history is enough to fit.
    assert!(
        granger_causality_controlling_for(&a, &b, &[], 1).is_ok(),
        "the premise: the pair alone fits in this history"
    );
    // The failure this prevents: a regression with almost no residual
    // degrees of freedom reports a confident-looking p-value computed from
    // nothing.
    assert!(
        granger_causality_controlling_for(&a, &b, &refs, 1).is_err(),
        "controls that exhaust the degrees of freedom are refused, not fitted"
    );
}
