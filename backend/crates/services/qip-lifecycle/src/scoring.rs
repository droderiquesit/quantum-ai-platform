//! The one place this crate turns a return series into a Sharpe ratio.
//!
//! Blueprint rule 27: the backtest runner links the production crates and
//! never reimplements them, so that a backtest and a live run cannot diverge
//! because they are the same code. The holdout gate honours that by calling
//! [`qip_simulation_engine::validation::deflated_sharpe`] and
//! [`assess_overfitting`]; the demotion monitor calls [`assess_overfitting`]
//! on live returns. The scaled gate, alone, had written `mean / stddev` out
//! by hand. It agreed with the engine today; nothing made it keep agreeing.
//!
//! Both helpers here delegate to the engine rather than restate its formula.
//! [`periodic_sharpe`] is [`assess_overfitting`] over a single fold, which
//! is the engine's own `mean / stddev` with its own zero-variance guard.
//! [`annualised_sharpe`] scales by the same `sqrt(max(periods, 1))` that
//! [`deflated_sharpe`](qip_simulation_engine::validation::deflated_sharpe)
//! applies to its `observed` figure, and the parity test in
//! `tests/lifecycle.rs` holds the two equal to the last bit.

use qip_core::error::Result;
use qip_simulation_engine::validation::assess_overfitting;

/// Sharpe ratio per observation period, exactly as the simulation engine
/// computes it: `mean / sample stddev`, and zero where the returns do not
/// vary.
pub fn periodic_sharpe(returns: &[f64]) -> Result<f64> {
    let fold = std::slice::from_ref(&returns);
    // One fold on each side, so the report's out-of-sample figure is the
    // Sharpe of `returns` and nothing is averaged with it.
    let report = assess_overfitting(&[fold[0].to_vec()], &[fold[0].to_vec()])?;
    Ok(report.out_of_sample_sharpe)
}

/// The periodic Sharpe scaled to a year, on the scale
/// [`qip_simulation_engine::validation::DeflatedSharpe::observed`] reports.
///
/// `periods_per_year` below one is treated as one, as the engine does: a
/// series sampled less than once a year has nothing to annualise.
pub fn annualised_sharpe(returns: &[f64], periods_per_year: f64) -> Result<f64> {
    Ok(periodic_sharpe(returns)? * periods_per_year.max(1.0).sqrt())
}
