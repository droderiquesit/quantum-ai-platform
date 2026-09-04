//! The capital one cell may commit in total, and what holds it between the
//! check and the order.
//!
//! Completion-plan item B12. The centre keeps a reservation ledger and
//! believes an amount is set aside per region; the cell kept nothing at all.
//! Every capital bound the cell held was per *strategy* — one signed
//! [`crate::VerifiedEnvelope`] each — and nothing anywhere summed them, so
//! four deployed strategies gave a cell four envelopes' worth of authority
//! bounded by no single number. Seven cells that decide alone while
//! partitioned (ADR 0008) could each spend to their own total against a
//! budget the centre had already promised elsewhere.
//!
//! [`RegionAllocation`] is the cell's side of that: one amount for the whole
//! cell, held before an order exists, refused — never reduced — when the hold
//! cannot be taken. Passing the check and taking the capital are the same
//! operation, so a second hold against the same capital cannot also pass. A
//! check that holds nothing is not a control; it is a race with a comforting
//! name.
//!
//! # Why the discipline is reproduced rather than reused
//!
//! `qip_capital::reservation::ReservationLedger` already implements it, and
//! `qip-edge` may not reach it: the acceptance test
//! `no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`
//! asserts that no crate in a cell reaches `qip-capital`, because a cell that
//! could would be able to widen its own bound. Taking the dependency would
//! mean deleting the test that makes ADR 0008's safety argument true.
//!
//! # What this is not
//!
//! It is not the centre's authority, and a comment claiming otherwise would
//! be the most damaging sentence in this file. Nothing on any wire the cell
//! receives carries a region allocation: a `CapitalEnvelope` is keyed on
//! (strategy, cell), and the policy payload's twelve slots carry no
//! per-region amount. The number here is one a composition root is *given* by
//! its operator. That makes it a local backstop — it can only narrow what the
//! signed envelopes already allow, never widen it — and a cell nobody hands
//! one to behaves exactly as every cell does today. Making it the centre's
//! number needs a signed field on the wire and a producer at the centre, in
//! two crates this one may not edit.
//!
//! # The clock is the pass, not the wall
//!
//! Holds are scoped to one pass of `Cell::work` and lapse by pass number
//! rather than by a duration. The cell's passes are what a replay reproduces;
//! a wall-clock validity would make the same journal refuse differently on a
//! slower machine.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// One live hold against the allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hold {
    /// Always positive; [`RegionAllocation::reserve`] refuses anything else.
    amount: Decimal,
    /// The pass of `Cell::work` that took it. See the module doc for why the
    /// pass and not a duration.
    pass: u64,
}

/// What one cell may commit, and every hold against it.
///
/// Iteration is over a [`BTreeMap`] so the sweep — and anything derived from
/// it — comes out in the same order on every machine. A replay that reorders
/// is not a replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionAllocation {
    free: Decimal,
    holds: BTreeMap<String, Hold>,
    committed: Decimal,
}

impl RegionAllocation {
    /// Open an allocation over an amount.
    ///
    /// Zero is permitted and means a cell that sends nothing, which is the
    /// correct reading of "this region has no capital". A negative amount is
    /// refused rather than floored: it means the caller's own accounting has
    /// already gone wrong, and a floor would bury that.
    pub fn new(free: Decimal) -> Result<Self> {
        if free.is_negative() {
            return Err(Error::invalid(format!(
                "a region allocation cannot open over a negative amount ({free}); give the cell \
                 the amount its region was actually allocated, or none at all"
            )));
        }
        Ok(Self {
            free,
            holds: BTreeMap::new(),
            committed: Decimal::ZERO,
        })
    }

    /// Capital no hold is standing on.
    pub fn free(&self) -> Decimal {
        self.free
    }

    /// The sum of every live hold.
    pub fn held_total(&self) -> Decimal {
        self.holds
            .values()
            .map(|hold| hold.amount)
            .fold(Decimal::ZERO, |left, right| left + right)
    }

    /// Capital that became an order and left the allocation.
    pub fn committed_total(&self) -> Decimal {
        self.committed
    }

    /// Pass the check by taking the capital.
    ///
    /// There is no way to learn the allocation covers `amount` without
    /// simultaneously holding it, which is the whole point: a second
    /// strategy in the same pass is *refused*, not clamped to what is left
    /// and not queued behind the first. Reducing it would spend the
    /// remainder of a budget the centre may already have promised elsewhere.
    pub fn reserve(&mut self, id: impl Into<String>, amount: Decimal, pass: u64) -> Result<()> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::invalid(
                "a region hold needs an id, or nothing can ever commit or release it",
            ));
        }
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "cannot hold {amount} of the region allocation; a hold holds a positive amount \
                 or it holds nothing"
            )));
        }
        if self.holds.contains_key(&id) {
            return Err(Error::invalid(format!(
                "{id} already holds region capital; commit or release it before holding again"
            )));
        }
        if amount > self.free {
            return Err(Error::denied(format!(
                "holding {amount} for {id} needs more than the {} the region allocation has \
                 left; {} is already held by {} hold(s) — the capital is committed or spoken \
                 for, so this is refused rather than reduced",
                self.free,
                self.held_total(),
                self.holds.len()
            )));
        }
        // Bounded below by zero: the comparison above proved `amount` fits.
        self.free -= amount;
        self.holds.insert(id, Hold { amount, pass });
        Ok(())
    }

    /// Turn a hold into spend. The capital does not return.
    ///
    /// `None` for a hold that is not there. The caller cannot un-send the
    /// order this commits for, so refusing here would name a problem nobody
    /// could act on; the invariant that makes `None` impossible on the send
    /// path is stated at each call site in `Cell`.
    pub fn commit(&mut self, id: &str) -> Option<Decimal> {
        let hold = self.holds.remove(id)?;
        // Bounded by the opening amount, like the free balance it came from.
        self.committed += hold.amount;
        Some(hold.amount)
    }

    /// Give a hold back. The capital returns to the free balance.
    pub fn release(&mut self, id: &str) -> Option<Decimal> {
        let hold = self.holds.remove(id)?;
        // Conservation: this amount came out of this free balance.
        self.free += hold.amount;
        Some(hold.amount)
    }

    /// Return every hold older than `pass` to the free balance, in id order.
    ///
    /// The backstop for a release site that was missed. A hold reaching here
    /// is a defect, not routine, which is why the caller journals each one.
    pub fn sweep_before(&mut self, pass: u64) -> Vec<(String, Decimal)> {
        let due: Vec<String> = self
            .holds
            .iter()
            .filter(|(_, hold)| hold.pass < pass)
            .map(|(id, _)| id.clone())
            .collect();
        let mut swept = Vec::with_capacity(due.len());
        for id in due {
            if let Some(amount) = self.release(&id) {
                swept.push((id, amount));
            }
        }
        swept
    }
}

#[cfg(test)]
// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::dec;

    #[test]
    fn a_second_hold_against_the_same_capital_is_refused_whole_rather_than_reduced() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        // The premise: the first hold was taken, and it moved the free
        // balance. Without this the refusal below would also pass against an
        // allocation that had refused the first hold too.
        allocation.reserve("first", dec!("700"), 1)?;
        assert_eq!(allocation.free(), dec!("300"));

        let refused = allocation.reserve("second", dec!("500"), 1);
        let error = match refused {
            Ok(()) => panic!("the allocation admitted 500 against a free balance of 300"),
            Err(error) => error,
        };
        // Named so an operator reading the journal learns how short it was,
        // not merely that something was short.
        assert!(
            error.message().contains("300"),
            "the refusal did not name what the allocation had left: {}",
            error.message()
        );
        // The refusal is whole: nothing of the 500 was taken.
        assert_eq!(
            allocation.free(),
            dec!("300"),
            "a refused hold moved the free balance, so it was reduced rather than refused"
        );
        assert_eq!(allocation.held_total(), dec!("700"));
        Ok(())
    }

    #[test]
    fn committing_a_hold_does_not_return_its_capital_to_the_free_balance() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve("one", dec!("400"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was taken"
        );

        assert_eq!(allocation.commit("one"), Some(dec!("400")));
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "committed capital came back to the free balance, so the cell could spend it twice"
        );
        assert_eq!(allocation.committed_total(), dec!("400"));
        assert_eq!(allocation.held_total(), Decimal::ZERO);
        Ok(())
    }

    #[test]
    fn releasing_a_hold_returns_exactly_what_it_took() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve("one", dec!("400"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was taken"
        );

        assert_eq!(allocation.release("one"), Some(dec!("400")));
        assert_eq!(
            allocation.free(),
            dec!("1000"),
            "a released hold did not return its capital, so the allocation leaks on every refusal"
        );
        assert_eq!(allocation.committed_total(), Decimal::ZERO);
        assert_eq!(allocation.release("one"), None);
        Ok(())
    }

    #[test]
    fn a_hold_from_an_earlier_pass_is_returned_by_the_next_passs_sweep() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve("older", dec!("400"), 4)?;
        allocation.reserve("current", dec!("100"), 5)?;
        assert_eq!(
            allocation.free(),
            dec!("500"),
            "the premise: both holds were taken"
        );

        let swept = allocation.sweep_before(5);
        assert_eq!(
            swept,
            vec![("older".to_string(), dec!("400"))],
            "the sweep did not return exactly the hold that outlived its pass"
        );
        assert_eq!(allocation.free(), dec!("900"));
        // The current pass's own hold is untouched: sweeping it would take
        // capital back from an order the same pass is still about to send.
        assert_eq!(allocation.held_total(), dec!("100"));
        Ok(())
    }

    #[test]
    fn a_region_allocation_cannot_open_over_a_negative_amount() {
        let error = match RegionAllocation::new(dec!("-1")) {
            Ok(_) => panic!("an allocation opened over a negative amount"),
            Err(error) => error,
        };
        assert!(
            error.message().contains("negative"),
            "the refusal did not say what was wrong: {}",
            error.message()
        );
        // Zero is a different fact and is permitted: a region with no capital.
        assert!(RegionAllocation::new(Decimal::ZERO).is_ok());
    }

    #[test]
    fn a_hold_of_a_non_positive_amount_is_refused() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        assert!(
            allocation.reserve("zero", Decimal::ZERO, 1).is_err(),
            "a hold of nothing was admitted, so an order of nothing could pass the gate"
        );
        assert!(
            allocation.reserve("negative", dec!("-5"), 1).is_err(),
            "a negative hold was admitted, which would widen the free balance"
        );
        assert_eq!(allocation.free(), dec!("1000"));
        assert_eq!(allocation.held_total(), Decimal::ZERO);
        Ok(())
    }

    #[test]
    fn a_duplicate_hold_id_is_refused_rather_than_overwriting_the_first() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve("one", dec!("400"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was taken"
        );

        assert!(
            allocation.reserve("one", dec!("100"), 1).is_err(),
            "a second hold under the same id was admitted; releasing it would return only one \
             of the two amounts and the difference would leak"
        );
        // The free balance moved once, not twice: the second hold took nothing.
        assert_eq!(allocation.free(), dec!("600"));
        assert_eq!(allocation.held_total(), dec!("400"));
        Ok(())
    }
}
