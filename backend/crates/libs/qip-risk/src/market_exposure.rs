//! Book-level basis and gap exposure, and the finding each one produces
//! (R12-RISK-4095, ADR 0690).
//!
//! The market-risk row names price, volatility, gap, basis, Greeks, factor and
//! causal-driver exposure. Every one but basis and gap had a figure and a
//! limit; those two had a per-candidate input (`basis_volatility`) and a stress
//! world, and no number a reader of the book could be shown. A risk nobody
//! measures cannot be limited, and a limit with no figure behind it is the
//! `MaxExpectedShortfall` defect, so this module supplies the figure first and
//! the decision second.
//!
//! # What is measured
//!
//! * **Basis**: for each held hedge pair, `notional × basis_volatility ×
//!   sqrt(holding_days) × BASIS_QUANTILE` — the very expression
//!   `hedge_search` prices a planned hedge with, so the book figure and the
//!   plan figure cannot drift apart. The book figure is the plain sum: pairs
//!   are not assumed to diversify, which is the restrictive reading.
//! * **Gap**: the loss fraction of the worst stress world named by the caller
//!   (the stress library's tail-risk-event world is the intended one),
//!   multiplied by the base it is a fraction of (the book's equity, for the stress
//!   library's tail worlds).
//!
//! # What is decided
//!
//! [`assess`] turns a figure and an optional bound into a [`Finding`]. With no
//! bound the finding is [`Verdict::Unbounded`], never a pass: an unbounded
//! exposure is recorded as such so that "nobody stated a bound" and "inside the
//! bound" cannot be read as the same thing. A figure that is not finite or is
//! negative is refused, not clamped.
//!
//! Statistics are `f64`; money stays `Decimal` at the boundary of whoever calls
//! this, and the crossing is the `to_f64` the caller performs.

use crate::hedge_search::BASIS_QUANTILE;
use qip_core::{Error, Result};

/// One held hedge pair: a position carried against the exposure it stands in
/// for, and how far the two can drift apart.
#[derive(Clone, Debug, PartialEq)]
pub struct BasisPosition {
    /// The pair's name, carried into the finding so a reader sees which pair.
    pub label: String,
    /// Absolute notional of the hedge leg, in the book currency.
    pub notional: f64,
    /// Daily volatility of the gap between the hedge and its exposure.
    pub basis_volatility: f64,
}

/// A stress world's discontinuous move, as the stress library states it.
#[derive(Clone, Debug, PartialEq)]
pub struct GapWorld {
    /// The world's name.
    pub name: String,
    /// Loss as a fraction of the notional it hits, in `[0, 1]`.
    pub loss_fraction: f64,
}

/// Which exposure a finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Exposure {
    /// Hedge-to-exposure basis, summed over held pairs.
    Basis,
    /// Discontinuous (gap) loss of the worst named stress world.
    Gap,
}

impl Exposure {
    /// The name journaled and charted.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Basis => "basis",
            Self::Gap => "gap",
        }
    }
}

/// The outcome of weighing a figure against a bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The figure is within the stated bound.
    Within,
    /// The figure exceeds the stated bound: a refusal in a limit's hands.
    Breach,
    /// No bound was stated. Recorded, never treated as within.
    Unbounded,
}

/// A recorded reading of one market exposure.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// Which exposure.
    pub exposure: Exposure,
    /// The measured figure, in the book currency.
    pub figure: f64,
    /// The bound it was weighed against, if one was stated.
    pub bound: Option<f64>,
    /// The verdict.
    pub verdict: Verdict,
    /// What was measured and how, for the journal.
    pub detail: String,
}

fn finite_non_negative(what: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(Error::invalid(format!(
            "{what} is {value}; it must be a finite figure of at least zero, and it is refused \
             rather than clamped because a corrected input is a caller bug that survives"
        )));
    }
    Ok(())
}

/// The book's basis exposure: the sum over `positions` of
/// `notional × basis_volatility × sqrt(holding_days) × BASIS_QUANTILE`.
///
/// An empty book has no basis and measures zero; that is a measurement, since
/// no pair is held. A position with a negative or non-finite input refuses the
/// whole measurement, so one malformed pair cannot be summed away.
pub fn book_basis_exposure(positions: &[BasisPosition], holding_days: f64) -> Result<f64> {
    if !holding_days.is_finite() || holding_days <= 0.0 {
        return Err(Error::invalid(format!(
            "holding days is {holding_days}; basis is priced over a positive horizon"
        )));
    }
    let mut total = 0.0_f64;
    for position in positions {
        finite_non_negative("a basis position's notional", position.notional)?;
        finite_non_negative(
            "a basis position's basis volatility",
            position.basis_volatility,
        )?;
        total +=
            position.notional * position.basis_volatility * holding_days.sqrt() * BASIS_QUANTILE;
    }
    if !total.is_finite() {
        return Err(Error::numeric(
            "the book's basis exposure overflowed; refusing a figure that is not finite",
        ));
    }
    Ok(total)
}

/// The book's gap exposure: the worst `loss_fraction × gross_notional` over
/// `worlds`.
///
/// Refuses an empty list of worlds: with none named there is no gap figure,
/// and zero would read as protection.
pub fn book_gap_exposure(worlds: &[GapWorld], gross_notional: f64) -> Result<(f64, String)> {
    finite_non_negative("the gross notional", gross_notional)?;
    if worlds.is_empty() {
        return Err(Error::invalid(
            "no stress world was named, so there is no gap figure; name the tail-risk-event \
             world rather than reading an absent measurement as zero",
        ));
    }
    let mut worst: Option<(f64, &str)> = None;
    for world in worlds {
        if !world.loss_fraction.is_finite() || !(0.0..=1.0).contains(&world.loss_fraction) {
            return Err(Error::invalid(format!(
                "stress world {} states a loss fraction of {}, outside [0, 1]",
                world.name, world.loss_fraction
            )));
        }
        let loss = world.loss_fraction * gross_notional;
        if worst.is_none_or(|(current, _)| loss > current) {
            worst = Some((loss, world.name.as_str()));
        }
    }
    let (loss, name) = worst.unwrap_or((0.0, ""));
    Ok((loss, name.to_string()))
}

/// Weigh a measured `figure` against an optional `bound`.
///
/// No bound yields [`Verdict::Unbounded`]. A bound that is not finite or is
/// negative is refused.
pub fn assess(
    exposure: Exposure,
    figure: f64,
    bound: Option<f64>,
    detail: impl Into<String>,
) -> Result<Finding> {
    finite_non_negative("the measured exposure", figure)?;
    let verdict = match bound {
        None => Verdict::Unbounded,
        Some(bound) => {
            finite_non_negative("the exposure bound", bound)?;
            if figure > bound {
                Verdict::Breach
            } else {
                Verdict::Within
            }
        }
    };
    Ok(Finding {
        exposure,
        figure,
        bound,
        verdict,
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_is_priced_with_the_expression_the_hedge_search_uses() {
        let positions = [BasisPosition {
            label: "es-future".into(),
            notional: 1_000_000.0,
            basis_volatility: 0.005,
        }];
        let figure = book_basis_exposure(&positions, 4.0).unwrap_or(f64::NAN);
        let expected = 1_000_000.0 * 0.005 * 2.0 * BASIS_QUANTILE;
        assert!((figure - expected).abs() < 1e-9, "{figure} vs {expected}");
    }

    #[test]
    fn two_pairs_are_summed_and_never_netted() {
        let pair = |n: f64| BasisPosition {
            label: "p".into(),
            notional: n,
            basis_volatility: 0.01,
        };
        let one = book_basis_exposure(&[pair(100.0)], 1.0).unwrap_or(f64::NAN);
        let two = book_basis_exposure(&[pair(100.0), pair(100.0)], 1.0).unwrap_or(f64::NAN);
        assert!((two - 2.0 * one).abs() < 1e-9);
        assert!(one > 0.0);
    }

    #[test]
    fn a_malformed_pair_refuses_the_whole_measurement() {
        let bad = BasisPosition {
            label: "bad".into(),
            notional: -1.0,
            basis_volatility: 0.01,
        };
        assert!(book_basis_exposure(&[bad], 1.0).is_err());
        assert!(book_basis_exposure(&[], 0.0).is_err());
    }

    #[test]
    fn the_gap_figure_is_the_worst_named_world() {
        let worlds = [
            GapWorld {
                name: "mild".into(),
                loss_fraction: 0.05,
            },
            GapWorld {
                name: "tail-risk-event".into(),
                loss_fraction: 0.40,
            },
        ];
        let (loss, name) = book_gap_exposure(&worlds, 1_000.0).unwrap_or((f64::NAN, String::new()));
        assert!((loss - 400.0).abs() < 1e-9);
        assert_eq!(name, "tail-risk-event");
        assert!(book_gap_exposure(&[], 1_000.0).is_err());
    }

    #[test]
    fn no_stated_bound_is_recorded_as_unbounded_and_never_as_within() {
        let finding =
            assess(Exposure::Gap, 10.0, None, "d").unwrap_or_else(|_| unreachable_finding());
        assert_eq!(finding.verdict, Verdict::Unbounded);
        let within =
            assess(Exposure::Gap, 10.0, Some(10.0), "d").unwrap_or_else(|_| unreachable_finding());
        assert_eq!(within.verdict, Verdict::Within);
        let breach = assess(Exposure::Basis, 10.5, Some(10.0), "d")
            .unwrap_or_else(|_| unreachable_finding());
        assert_eq!(breach.verdict, Verdict::Breach);
    }

    fn unreachable_finding() -> Finding {
        Finding {
            exposure: Exposure::Basis,
            figure: f64::NAN,
            bound: None,
            verdict: Verdict::Within,
            detail: String::new(),
        }
    }
}
