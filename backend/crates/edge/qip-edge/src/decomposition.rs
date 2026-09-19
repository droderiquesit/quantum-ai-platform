//! What a cycle does after one of its legs fills short (§32.1).
//!
//! §32.1's third mechanism: *"A partial fill at or above the minimum viable
//! fraction completes at reduced size rather than unwinding."* It is the
//! second of the five a cell can hold on its own — the first is
//! [`crate::dispersion`]'s pre-trade gate — and it is cell-local for the same
//! reason: it needs no timer, no dispatch thread and no venue message the
//! cell does not already receive.
//!
//! # The failure it closes
//!
//! `Cell::place_cycle` sent every leg of an admitted cycle at the size the
//! scanner priced, in plan order, whatever the venue had said about the legs
//! already out. So a first leg that filled six tenths of its size was
//! followed by a second leg at ten tenths, and the cycle that reached the
//! venues was not the cycle that was admitted: four tenths of the second leg
//! was an outright position, taken at a price chosen for an arbitrage that
//! did not exist at that size. Nothing noticed until the drop copy was
//! reconciled, by which time the position had been carried for however long
//! that took. The cell cannot unwind — its [`crate::cell::Placer`] can
//! withdraw a resting order where the gateway has a cancel path, and cannot
//! send the compensating order that would reverse a leg already filled — so
//! unwinding was never the alternative here. Sizing the rest of the cycle
//! to what the cycle has actually completed is.
//!
//! # Three outcomes, and the third is the one that is not a gate
//!
//! [`Decomposition::observe`] folds in what one leg completed and answers
//! with a [`Completion`]. A leg that filled in full leaves the fraction at
//! one and the rest of the cycle goes out exactly as it did before this
//! module existed — the half that distinguishes a mechanism from a new
//! limit. A leg that filled short at or above the minimum viable fraction
//! reduces every later leg to match. A leg that filled short of it cannot be
//! completed at a size worth completing, and the cycle stops: the legs
//! already sent are a position nobody chose, which is exactly the state
//! `Cell::break_cycle` exists for.
//!
//! **A leg the venue has said nothing about is [`Completion::Unanswered`],
//! and it does not reduce anything.** That distinction is available because
//! `Cell::confirm` breaks on an execution report of non-positive quantity, so
//! a booked fill is always positive and an order whose `filled` is zero is an
//! order no report has arrived for. A gateway with no order-entry channel at
//! all — [`crate::cell::Placer::execution_reports`] defaults to nothing, which
//! its own documentation calls the honest answer — would otherwise have every
//! one of its cycles read as a first leg that filled nothing, and the cell
//! would halt on each. Reading silence as a zero fill is inventing the
//! venue's answer; it is the same mistake as reading an unmeasured venue as a
//! fast one, which [`crate::dispersion`] names at length. What the discipline
//! gets instead is the same thing it gets there: the silence is counted, on
//! `qip_edge_cycle_legs_total{completion="unanswered"}`, so a cell whose
//! cycles are never decomposed because nothing ever answers does not read
//! like a cell whose cycles always complete whole.
//!
//! # Nothing here rounds a caller's size
//!
//! [`crate::feasibility`]'s standing policy is refuse, never round, and it is
//! about an intent a *strategy* handed in: a size off the venue's lot grid is
//! a strategy that does not know the grid, and correcting it would let that
//! strategy run for ever on a size it never reasoned about. The size computed
//! here has no such caller. It is the cell's own arithmetic on a fraction the
//! venue reported, and the venue's grid is a fact the cell holds, so the
//! largest whole number of lots the completed fraction supports is a size the
//! cell derived rather than a size it corrected. It is journaled with the
//! planned size and the fraction beside it, so the chain says what was
//! planned and what went out. Flooring, never rounding to nearest: a size
//! rounded up is a leg larger than the cycle in front of it can support,
//! which is the position this module exists to stop.

use crate::feasibility::Granularity;
use qip_core::Decimal;
use qip_core::error::{Error, Result};

/// The least fraction of a cycle's planned size that is worth completing.
///
/// Refused rather than clamped at construction, for the reason every other
/// bound in this crate is: this number decides when a control fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecompositionPolicy {
    minimum_viable: Decimal,
}

/// Half the planned size, as a stated policy rather than a measured one.
///
/// Like `RateLimits::depletion`'s band multiples, this number is a decision
/// nobody has calibrated against a venue, and it is written down as such so
/// that a later measurement can replace it without anybody having to work out
/// whether it was evidence. What it says is that half an arbitrage cycle is
/// still an arbitrage cycle and a tenth of one is a fee schedule: the fixed
/// costs `feasibility::assess_cycle_cost` weighed against the edge were
/// weighed at the planned size, and shrinking the size without re-weighing
/// them makes them a larger fraction of the same edge. A deployment that has
/// measured its fee schedule against its cycle sizes sets its own with
/// [`DecompositionPolicy::new`].
pub const DEFAULT_MINIMUM_VIABLE_FRACTION: Decimal = Decimal::from_raw(500_000_000);

impl Default for DecompositionPolicy {
    fn default() -> Self {
        Self {
            minimum_viable: DEFAULT_MINIMUM_VIABLE_FRACTION,
        }
    }
}

impl DecompositionPolicy {
    /// The policy, or the refusal naming what to set instead.
    pub fn new(minimum_viable: Decimal) -> Result<Self> {
        if !minimum_viable.is_positive() {
            return Err(Error::invalid(format!(
                "a minimum viable fraction of {minimum_viable} completes a cycle at any size a \
                 leg happens to fill, including none of it, so a leg that filled nothing would \
                 send the rest of the cycle at nothing; name the smallest fraction of its \
                 planned size this desk will carry a cycle at"
            )));
        }
        if minimum_viable > Decimal::ONE {
            return Err(Error::invalid(format!(
                "a minimum viable fraction of {minimum_viable} is more than the whole cycle, so \
                 no partial fill could ever reach it and every short leg would stop the cell; \
                 name a fraction at or below one"
            )));
        }
        Ok(Self { minimum_viable })
    }

    /// The smallest fraction of its planned size a cycle may complete at.
    pub const fn minimum_viable(self) -> Decimal {
        self.minimum_viable
    }
}

/// What one leg's fill says about the rest of its cycle.
///
/// Four arms rather than a boolean, because "the cycle carries on" has three
/// completely different meanings — the leg filled whole, the leg filled short
/// and the rest will follow it down, or the venue has not answered at all —
/// and an operator reading a journal needs to know which one carried it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// The venue has reported nothing on this leg. The fraction is untouched
    /// and the rest of the cycle goes out at its planned size.
    Unanswered,
    /// The venue reported the whole leg filled.
    Whole,
    /// The leg filled short, at or above the minimum viable fraction. The
    /// carried fraction is what every later leg is sized to.
    Reduced { fraction: Decimal },
    /// The leg filled short of the minimum viable fraction. There is no size
    /// at which the rest of the cycle is worth sending.
    Unviable { fraction: Decimal, minimum: Decimal },
}

// There is deliberately no `carries_on` predicate here. One was written and
// removed before this shipped, because nothing outside the tests called it:
// the decision to stop a cycle is taken by [`Decomposition::size_for`] on the
// *next* leg rather than on this verdict, and it has to be, since a last leg
// that completes short has no next leg to refuse and stopping a cycle that is
// already whole would be wrong. A predicate that reads as the decision and is
// consulted by nothing is the shape of control this repository has shipped
// once before and now refuses to.
impl Completion {
    /// The label this outcome is counted under. A source-file literal per
    /// arm, so `qip_edge_cycle_legs_total{completion}` is bounded by this
    /// enum and never by anything a venue said.
    ///
    /// `short` rather than `decomposed`: the series counts a leg by what
    /// *that leg* completed, and the decomposition is what happens to the
    /// legs behind it. A label naming the consequence would put the first
    /// short leg of a cycle and the two reduced ones that follow it under the
    /// same word, and those are the two facts an operator is trying to tell
    /// apart.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unanswered => "unanswered",
            Self::Whole => "whole",
            Self::Reduced { .. } => "short",
            Self::Unviable { .. } => "unviable",
        }
    }
}

/// The size a leg still to be sent goes out at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegSize {
    /// Nothing has completed short; the leg goes out exactly as planned.
    Planned(Decimal),
    /// The leg goes out smaller, on the venue's own lot grid.
    Decomposed { planned: Decimal, size: Decimal },
    /// No size on the venue's grid is both expressible and at or above the
    /// minimum viable fraction of the planned leg. Nothing goes out.
    Unviable { planned: Decimal, reason: String },
}

/// The fraction of its planned size one cycle can still complete at.
///
/// Held for the length of a single `Cell::place_cycle` and dropped with it:
/// nothing about one cycle's fills carries into the next.
#[derive(Clone, Copy, Debug)]
pub struct Decomposition {
    policy: DecompositionPolicy,
    /// Starts at one and only ever falls — the minimum over every leg the
    /// venues have answered on. Monotone on purpose: a later leg that filled
    /// whole says nothing about the earlier one that did not, and letting the
    /// fraction recover would size a leg above what the cycle in front of it
    /// can support.
    fraction: Decimal,
}

impl Decomposition {
    /// A cycle that has completed nothing short yet.
    pub const fn new(policy: DecompositionPolicy) -> Self {
        Self {
            policy,
            fraction: Decimal::ONE,
        }
    }

    /// The fraction of its planned size the cycle can still complete at.
    pub const fn fraction(self) -> Decimal {
        self.fraction
    }

    /// The policy in force.
    pub const fn policy(self) -> DecompositionPolicy {
        self.policy
    }

    /// Fold in what one leg completed.
    ///
    /// `sent` is what went to the venue — the decomposed size, not the
    /// planned one, so a cycle already reduced measures each later leg
    /// against what it actually asked for and the fractions compound rather
    /// than double-counting. `filled` is what the venue has reported against
    /// it.
    ///
    /// A `sent` of zero is refused rather than answered: no leg the cell
    /// sends is ever zero — [`Self::size_for`] refuses that size before a
    /// leg exists — so a zero here is a caller that has lost track of which
    /// order it is asking about, and a fraction over zero is the answer that
    /// would hide it.
    pub fn observe(&mut self, sent: Decimal, filled: Decimal) -> Result<Completion> {
        if !sent.is_positive() {
            return Err(Error::invalid(format!(
                "a leg of {sent} was never sent, so there is no fraction of it to complete the \
                 rest of the cycle at; pass the quantity the venue was asked for"
            )));
        }
        if filled.is_negative() {
            return Err(Error::invalid(format!(
                "a leg cannot have filled {filled}; pass what the venue has reported traded"
            )));
        }
        if filled.is_zero() {
            return Ok(Completion::Unanswered);
        }
        if filled >= sent {
            return Ok(Completion::Whole);
        }
        let Some(leg) = filled.checked_div(sent) else {
            return Err(Error::numeric(format!(
                "a fill of {filled} against a leg of {sent} has no representable fraction"
            )));
        };
        self.fraction = self.fraction.min(leg);
        if self.fraction < self.policy.minimum_viable() {
            return Ok(Completion::Unviable {
                fraction: self.fraction,
                minimum: self.policy.minimum_viable(),
            });
        }
        Ok(Completion::Reduced {
            fraction: self.fraction,
        })
    }

    /// The size a leg planned at `planned` goes out at, on `granularity`.
    ///
    /// `granularity` is `None` for a venue the cell holds no model of — the
    /// feasibility gate judges such a venue for depth alone, and this does
    /// the same: the reduced size is used as computed, because a lot size
    /// guessed at here would be the rounding rule that module refuses to
    /// write.
    pub fn size_for(self, planned: Decimal, granularity: Option<&Granularity>) -> LegSize {
        if !planned.is_positive() {
            return LegSize::Unviable {
                planned,
                reason: format!("a planned leg of {planned} is not a size the cell can send"),
            };
        }
        if self.fraction >= Decimal::ONE {
            return LegSize::Planned(planned);
        }
        let Some(scaled) = planned.checked_mul(self.fraction) else {
            return LegSize::Unviable {
                planned,
                reason: format!(
                    "{planned} at {} of its planned size has no representable quantity",
                    self.fraction
                ),
            };
        };
        let size = match granularity {
            Some(grid) => scaled.floor_to_step(grid.lot_size()),
            None => scaled,
        };
        if !size.is_positive() {
            return LegSize::Unviable {
                planned,
                reason: format!(
                    "{planned} at {} of its planned size is {scaled}, and the largest whole lot \
                     inside it is nothing",
                    self.fraction
                ),
            };
        }
        if let Some(grid) = granularity
            && size < grid.minimum_quantity()
        {
            return LegSize::Unviable {
                planned,
                reason: format!(
                    "{size} is below the {} minimum this venue accepts, so the cycle cannot be \
                     completed at {} of its planned size",
                    grid.minimum_quantity(),
                    self.fraction
                ),
            };
        }
        // Flooring to the grid can take the leg below the fraction the policy
        // admitted — a planned leg of three lots at six tenths is 1.8 lots and
        // goes out as one, which is a third. The realised fraction is what the
        // policy is asked about, because the realised fraction is what the
        // cycle actually completes at.
        let realised = size.checked_div(planned);
        match realised {
            Some(realised) if realised >= self.policy.minimum_viable() => {
                LegSize::Decomposed { planned, size }
            }
            Some(realised) => LegSize::Unviable {
                planned,
                reason: format!(
                    "the largest whole lot inside {} of {planned} is {size}, which is {realised} \
                     of the planned leg and below the {} this desk will complete a cycle at",
                    self.fraction,
                    self.policy.minimum_viable()
                ),
            },
            None => LegSize::Unviable {
                planned,
                reason: format!("{size} against a planned {planned} has no representable fraction"),
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn d(value: &str) -> Decimal {
        Decimal::parse(value).expect("a literal decimal in a test")
    }

    fn half() -> DecompositionPolicy {
        DecompositionPolicy::default()
    }

    fn lots(lot: &str, minimum: &str) -> Granularity {
        Granularity::new(d(lot), d("0.01"), d(minimum)).expect("a grid in a test")
    }

    #[test]
    fn a_leg_the_venue_has_not_answered_on_leaves_the_rest_of_the_cycle_at_its_planned_size()
    -> Result<()> {
        // The failure this prevents, and it is not hypothetical for any
        // gateway in this tree: `Placer::execution_reports` defaults to
        // nothing. Reading that as "the leg filled nothing" would make every
        // cycle at such a gateway stop after its first leg.
        let mut decomposition = Decomposition::new(half());
        assert_eq!(
            decomposition.observe(d("100"), Decimal::ZERO)?,
            Completion::Unanswered,
            "a leg with no report was read as a leg that filled nothing"
        );
        assert_eq!(
            decomposition.fraction(),
            Decimal::ONE,
            "silence moved the fraction the rest of the cycle is sized at"
        );
        assert_eq!(
            decomposition.size_for(d("40"), None),
            LegSize::Planned(d("40")),
            "a later leg was reduced on the strength of a report that never arrived"
        );
        Ok(())
    }

    #[test]
    fn a_leg_that_filled_short_sizes_every_later_leg_to_what_it_actually_completed() -> Result<()> {
        let mut decomposition = Decomposition::new(half());
        assert_eq!(
            decomposition.observe(d("100"), d("60"))?,
            Completion::Reduced { fraction: d("0.6") },
            "six tenths of a leg was not read as six tenths of the cycle"
        );
        assert_eq!(
            decomposition.size_for(d("50"), Some(&lots("0.001", "0.001"))),
            LegSize::Decomposed {
                planned: d("50"),
                size: d("30")
            },
            "the later leg went out at a size the completed leg cannot support"
        );
        Ok(())
    }

    #[test]
    fn the_fraction_a_cycle_completes_at_only_ever_falls_across_its_legs() -> Result<()> {
        // The monotonicity, not any one value. A fraction that recovered
        // would size a third leg above what the first leg — the one that is
        // already at the venue and cannot be added to — can support.
        let mut decomposition = Decomposition::new(half());
        assert_eq!(
            decomposition.observe(d("100"), d("60"))?,
            Completion::Reduced { fraction: d("0.6") },
            "the premise failed: the first short leg did not reduce the cycle"
        );
        assert_eq!(
            decomposition.observe(d("30"), d("30"))?,
            Completion::Whole,
            "a leg that filled everything asked of it was not read as whole"
        );
        assert_eq!(
            decomposition.fraction(),
            d("0.6"),
            "a whole leg raised the fraction an earlier short leg had set"
        );
        assert_eq!(
            decomposition.observe(d("18"), d("9"))?,
            Completion::Reduced { fraction: d("0.5") },
            "a second short leg did not compound onto the first"
        );
        assert_eq!(
            decomposition.fraction(),
            d("0.5"),
            "the fraction did not fall to the worst leg the cycle has had"
        );
        Ok(())
    }

    #[test]
    fn a_leg_that_filled_below_the_minimum_viable_fraction_stops_the_cycle_rather_than_sizing_it()
    -> Result<()> {
        let mut decomposition = Decomposition::new(half());
        let completion = decomposition.observe(d("100"), d("49.9"))?;
        assert_eq!(
            completion,
            Completion::Unviable {
                fraction: d("0.499"),
                minimum: d("0.5")
            },
            "a leg that completed under half the cycle was sized down rather than stopped"
        );
        assert!(
            matches!(completion, Completion::Unviable { .. }),
            "an unviable completion said the rest of the cycle could still be sent"
        );
        // The boundary itself, because a minimum tested only well inside and
        // well outside is satisfied by any threshold between the two.
        let mut exact = Decomposition::new(half());
        assert!(
            !matches!(
                exact.observe(d("100"), d("50"))?,
                Completion::Unviable { .. }
            ),
            "a leg that completed exactly the minimum was stopped"
        );
        Ok(())
    }

    #[test]
    fn a_reduced_leg_is_floored_to_the_venues_lot_grid_and_never_rounded_up_to_it() -> Result<()> {
        // Rounding to nearest would send 2 lots where the completed leg
        // supports 1.8 — a tenth of the cycle taken as an outright position
        // by the control that exists to stop exactly that.
        let mut decomposition = Decomposition::new(DecompositionPolicy::new(d("0.25"))?);
        assert_eq!(
            decomposition.observe(d("100"), d("60"))?,
            Completion::Reduced { fraction: d("0.6") },
            "the premise failed: the leg was not reduced at all"
        );
        assert_eq!(
            decomposition.size_for(d("3"), Some(&lots("1", "1"))),
            LegSize::Decomposed {
                planned: d("3"),
                size: d("1")
            },
            "1.8 lots did not floor to one whole lot"
        );
        Ok(())
    }

    #[test]
    fn a_leg_whose_largest_whole_lot_falls_under_the_minimum_viable_fraction_is_unviable()
    -> Result<()> {
        // The case flooring creates and the fraction alone hides: six tenths
        // clears the policy, and one lot of three does not.
        let mut decomposition = Decomposition::new(half());
        assert_eq!(
            decomposition.observe(d("100"), d("60"))?,
            Completion::Reduced { fraction: d("0.6") },
            "the premise failed: the completion did not clear the policy"
        );
        let size = decomposition.size_for(d("3"), Some(&lots("1", "1")));
        assert!(
            matches!(size, LegSize::Unviable { .. }),
            "one lot of a planned three was sent as six tenths of the cycle: {size:?}"
        );
        Ok(())
    }

    #[test]
    fn a_reduced_leg_below_the_venues_minimum_quantity_is_unviable_rather_than_sent() -> Result<()>
    {
        let mut decomposition = Decomposition::new(DecompositionPolicy::new(d("0.25"))?);
        decomposition.observe(d("100"), d("30"))?;
        let size = decomposition.size_for(d("10"), Some(&lots("0.1", "5")));
        assert!(
            matches!(size, LegSize::Unviable { .. }),
            "a three-unit leg was sent at a venue whose minimum is five: {size:?}"
        );
        Ok(())
    }

    #[test]
    fn a_policy_that_could_not_fire_is_refused_at_configuration() {
        assert!(
            DecompositionPolicy::new(Decimal::ZERO).is_err(),
            "a minimum viable fraction of zero completes a cycle at any size at all"
        );
        assert!(
            DecompositionPolicy::new(Decimal::parse("-0.5").unwrap_or(Decimal::ZERO)).is_err(),
            "a negative minimum viable fraction is not a fraction"
        );
        assert!(
            DecompositionPolicy::new(Decimal::parse("1.5").unwrap_or(Decimal::ZERO)).is_err(),
            "a minimum viable fraction above one stops every cycle that fills short"
        );
        assert!(
            DecompositionPolicy::new(Decimal::ONE).is_ok(),
            "a desk that will only ever complete a whole cycle is a policy, not an error"
        );
    }

    #[test]
    fn a_leg_the_cell_never_sent_is_refused_rather_than_divided_by() {
        let mut decomposition = Decomposition::new(half());
        assert!(
            decomposition.observe(Decimal::ZERO, Decimal::ZERO).is_err(),
            "a leg of zero was answered rather than refused"
        );
        assert!(
            decomposition.observe(d("100"), d("-1")).is_err(),
            "a negative fill was folded into the fraction"
        );
    }
}
