//! The degenerate step in an equity series is skipped, not invented.
//!
//! `qip_numerics::stats::returns_over_signed_equity` is the workspace's one
//! statement of "equity marks to period returns". It is read by
//! `qip-kernel`'s `Platform`, whose returns feed `RiskState::with_tail_risk`
//! — volatility, value at risk and expected shortfall, all three of which are
//! read by limits that stop trading — and it is the rule
//! `qip_simulation_engine::backtest` computes a Sharpe ratio and a drawdown
//! with. Before it was one function it was three, and they disagreed on
//! exactly the steps below.
//!
//! `simple_returns` next to it is deliberately *not* this and must not be
//! substituted: it answers `0.0` from a zero base and sign-flips from a
//! negative one. Each test below therefore also asserts what the wrong answer
//! would have been, so that a reader can see the failure is arithmetic that
//! really happens and not a hypothetical.

use qip_numerics::stats::{returns_over_signed_equity, simple_returns};

#[test]
fn a_solvent_series_returns_one_figure_per_step() {
    // Premise: three marks, so two steps, and no mark is degenerate —
    // otherwise this would be a test of the skipping rule rather than of the
    // arithmetic it makes an exception to.
    let history = [100.0, 110.0, 99.0];
    assert!(history.iter().all(|equity| *equity > 0.0));

    let returns = returns_over_signed_equity(&history);
    assert_eq!(returns.len(), 2, "a step was dropped from a solvent series");
    assert!((returns[0] - 0.1).abs() < 1e-12, "{returns:?}");
    assert!((returns[1] + 0.1).abs() < 1e-12, "{returns:?}");
}

#[test]
fn a_step_out_of_a_wiped_out_book_is_dropped_rather_than_reported_as_flat() {
    // Zero, and then a recovery out of it. Premise: the neighbour answers
    // `0.0` here — "the book was flat" — which is an observation nobody made,
    // and a tail statistic fitted on invented calm reports less risk than the
    // book carries.
    let history = [100.0, 0.0, 50.0];
    assert_eq!(
        simple_returns(&history),
        vec![-1.0, 0.0],
        "the premise failed: the neighbour no longer fabricates the flat step"
    );

    let returns = returns_over_signed_equity(&history);
    // The first step is real and must survive: the ruin itself is a -100%.
    assert_eq!(
        returns.len(),
        1,
        "expected only the solvent step: {returns:?}"
    );
    assert!((returns[0] + 1.0).abs() < 1e-12, "{returns:?}");
    // Said explicitly, because the failure being prevented is a zero in this
    // position rather than a shorter list.
    assert!(
        !returns.contains(&0.0),
        "a step out of a zero book was reported as a flat one: {returns:?}"
    );
}

#[test]
fn a_step_from_a_negative_book_is_dropped_rather_than_sign_flipped() {
    // The arm no downstream guard catches, and the one the simulation engine
    // got wrong until 5ebf18c. From -50 to -25 the book recovered half its
    // deficit.
    let history = [-50.0, -25.0];
    // Premise: the wrong answer is finite arithmetic, not a NaN somebody
    // would have noticed. It reports the recovery as a loss of half.
    let sign_flipped = simple_returns(&history);
    assert_eq!(sign_flipped.len(), 1);
    assert!(sign_flipped[0].is_finite() && sign_flipped[0] < 0.0);

    let returns = returns_over_signed_equity(&history);
    assert!(
        returns.is_empty(),
        "a step from a negative book produced a return of {returns:?}, so a book that halved \
         its deficit is charted as having lost half of it"
    );
}

#[test]
fn an_ordinary_step_into_ruin_survives_and_only_the_step_out_of_it_is_dropped() {
    // The guard must be a guard and not a wall. A function returning an empty
    // vector for everything passes the three tests above at their degenerate
    // ends, and a book whose real losses were dropped would report no
    // volatility at the moment it had the most.
    let history = [100.0, 120.0, 0.0, 50.0, -100.0, -50.0, 200.0];
    let returns = returns_over_signed_equity(&history);
    // 100->120 kept, 120->0 kept (the ruin, a real -100%), 0->50 dropped,
    // 50->-100 kept (a real -300% out of a live book), -100->-50 dropped,
    // -50->200 dropped.
    assert_eq!(returns.len(), 3, "{returns:?}");
    assert!((returns[0] - 0.2).abs() < 1e-12, "{returns:?}");
    assert!((returns[1] + 1.0).abs() < 1e-12, "{returns:?}");
    assert!((returns[2] + 3.0).abs() < 1e-12, "{returns:?}");
}

#[test]
fn a_series_too_short_to_have_a_step_has_no_returns() {
    // The boundary `windows(2)` depends on. A fresh platform has one mark
    // after its first cycle, and a panic or a fabricated return there would
    // land on every deployment's opening cycle.
    assert!(returns_over_signed_equity(&[]).is_empty());
    assert!(returns_over_signed_equity(&[1_000_000.0]).is_empty());
}
