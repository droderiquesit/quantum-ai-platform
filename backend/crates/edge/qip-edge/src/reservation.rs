//! The capital one region may commit in total, and what holds it between the
//! check and the order.
//!
//! Completion-plan item B12, and traceability row F6. The centre keeps a
//! reservation ledger and believes an amount is set aside per region; the
//! cell kept nothing at all. Every capital bound the cell held was per
//! *strategy* — one signed [`crate::VerifiedEnvelope`] each — and nothing
//! anywhere summed them, so four deployed strategies gave a cell four
//! envelopes' worth of authority bounded by no single number. Seven cells
//! that decide alone while partitioned (ADR 0008) could each spend to their
//! own total against a budget the centre had already promised elsewhere.
//!
//! [`RegionAllocation`] is the ledger: one amount for the whole region, held
//! before an order exists, refused — never reduced — when the hold cannot be
//! taken. Passing the check and taking the capital are the same operation, so
//! a second hold against the same capital cannot also pass. A check that
//! holds nothing is not a control; it is a race with a comforting name.
//!
//! [`RegionTable`] is the ledger as the blueprint (§26/§33) places it: one
//! table *per region*, given to every cell in that region by the composition
//! root, and consulted at the cell. Two cells under one grant then refuse
//! against the balance the other spent, without either asking the centre —
//! which is the only placement at which a partitioned cell can still refuse
//! its own second proposal. Before the table existed each cell owned its own
//! ledger, so two cells in one region could each spend the whole region.
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
//! Holds are scoped to one pass of `Cell::work` of the cell that took them
//! and lapse by that cell's pass number rather than by a duration. The cell's
//! passes are what a replay reproduces; a wall-clock validity would make the
//! same journal refuse differently on a slower machine. Each hold names its
//! owner because two cells sharing a table count passes independently: a
//! sweep that ignored the owner would return a hold the other cell took a
//! moment ago for an order it is still about to send.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// One live hold against the allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hold {
    /// Always positive; [`RegionAllocation::reserve`] refuses anything else.
    amount: Decimal,
    /// The owner's pass of `Cell::work` that took it. See the module doc for
    /// why the pass and not a duration.
    pass: u64,
}

/// What a hold is filed under: the cell that took it, then the cell's own
/// key. Two cells sharing a table both file `"{pass}:strategy:alpha"` and
/// must not collide on it, and a cell must not be able to commit or release
/// a hold that is not its own.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct HoldKey {
    owner: String,
    id: String,
}

/// What one region may commit, and every hold against it.
///
/// Iteration is over a [`BTreeMap`] so the sweep — and anything derived from
/// it — comes out in the same order on every machine. A replay that reorders
/// is not a replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionAllocation {
    free: Decimal,
    holds: BTreeMap<HoldKey, Hold>,
    committed: Decimal,
}

impl RegionAllocation {
    /// Open an allocation over an amount.
    ///
    /// Zero is permitted and means a region that sends nothing, which is the
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

    /// The sum of every live hold, whoever took it.
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

    /// Pass the check by taking the capital, on behalf of `owner`.
    ///
    /// There is no way to learn the allocation covers `amount` without
    /// simultaneously holding it, which is the whole point: a second
    /// strategy in the same pass — or a second cell under the same table —
    /// is *refused*, not clamped to what is left and not queued behind the
    /// first. Reducing it would spend the remainder of a budget the centre
    /// may already have promised elsewhere.
    pub fn reserve(
        &mut self,
        owner: &str,
        id: impl Into<String>,
        amount: Decimal,
        pass: u64,
    ) -> Result<()> {
        let key = HoldKey {
            owner: owner.to_string(),
            id: id.into(),
        };
        if key.owner.trim().is_empty() || key.id.trim().is_empty() {
            return Err(Error::invalid(
                "a region hold needs an owner and an id, or nothing can ever commit or release it",
            ));
        }
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "cannot hold {amount} of the region allocation; a hold holds a positive amount \
                 or it holds nothing"
            )));
        }
        if self.holds.contains_key(&key) {
            return Err(Error::invalid(format!(
                "{} already holds region capital under {}; commit or release it before holding \
                 again",
                key.owner, key.id
            )));
        }
        if amount > self.free {
            return Err(Error::denied(format!(
                "holding {amount} for {} needs more than the {} the region allocation has \
                 left; {} is already held by {} hold(s) and {} is committed — the capital is \
                 spoken for, so this is refused rather than reduced",
                key.id,
                self.free,
                self.held_total(),
                self.holds.len(),
                self.committed
            )));
        }
        // Bounded below by zero: the comparison above proved `amount` fits.
        self.free -= amount;
        self.holds.insert(key, Hold { amount, pass });
        Ok(())
    }

    /// Turn a hold into spend. The capital does not return here; see
    /// [`Self::return_committed`] for the one way it can.
    ///
    /// `None` for a hold that is not there under this owner. The caller
    /// cannot un-send the order this commits for, so refusing here would name
    /// a problem nobody could act on; the invariant that makes `None`
    /// impossible on the send path is stated at each call site in `Cell`.
    pub fn commit(&mut self, owner: &str, id: &str) -> Option<Decimal> {
        let hold = self.holds.remove(&key(owner, id))?;
        // Bounded by the opening amount, like the free balance it came from.
        self.committed += hold.amount;
        Some(hold.amount)
    }

    /// Give a hold back. The capital returns to the free balance.
    pub fn release(&mut self, owner: &str, id: &str) -> Option<Decimal> {
        let hold = self.holds.remove(&key(owner, id))?;
        // Conservation: this amount came out of this free balance.
        self.free += hold.amount;
        Some(hold.amount)
    }

    /// Give back capital that was committed for an order the venue never
    /// filled any of.
    ///
    /// The one path by which committed capital returns, and it is deliberately
    /// narrow: a rested order withdrawn whole at its time to live did not run,
    /// and billing the region for it would starve a disconnected cell on
    /// capital that never became a position ("bill what ran, not what was
    /// planned"). A partial fill is a position and takes no part of this: the
    /// caller returns nothing for it, which is the conservative direction.
    ///
    /// Refused, never floored, when more is returned than was ever committed:
    /// that is the caller's accounting having gone wrong, and a floor would
    /// make the region richer than its operator allocated.
    pub fn return_committed(&mut self, amount: Decimal) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "cannot return {amount} of committed region capital; a return is a positive \
                 amount or it is nothing"
            )));
        }
        if amount > self.committed {
            return Err(Error::invalid(format!(
                "cannot return {amount} to the region allocation when only {} was ever committed; \
                 the order's record and the allocation disagree, and the allocation is kept \
                 rather than widened",
                self.committed
            )));
        }
        // Both bounded: the comparison above proved `amount` fits, and the
        // free balance returns to at most its opening amount.
        self.committed -= amount;
        self.free += amount;
        Ok(())
    }

    /// Return every hold `owner` took before `pass` to the free balance, in
    /// id order.
    ///
    /// The backstop for a release site that was missed. A hold reaching here
    /// is a defect, not routine, which is why the caller journals each one.
    /// Scoped to the owner: another cell's holds are from another cell's
    /// pass count, and one of them may be mid-pass right now.
    pub fn sweep_before(&mut self, owner: &str, pass: u64) -> Vec<(String, Decimal)> {
        let due: Vec<String> = self
            .holds
            .iter()
            .filter(|(key, hold)| key.owner == owner && hold.pass < pass)
            .map(|(key, _)| key.id.clone())
            .collect();
        let mut swept = Vec::with_capacity(due.len());
        for id in due {
            if let Some(amount) = self.release(owner, &id) {
                swept.push((id, amount));
            }
        }
        swept
    }
}

fn key(owner: &str, id: &str) -> HoldKey {
    HoldKey {
        owner: owner.to_string(),
        id: id.to_string(),
    }
}

/// One region's allocation, shared by every cell the root assembles for that
/// region.
///
/// `Send + Sync` and free of I/O: a hold is a mutex acquisition and a
/// comparison, so it can sit on the order path of every cell in the region
/// without any of them waiting on anything outside the process. The table is
/// *given* to a cell (`Cell::with_region_table`), never reached for, because
/// a cell that chose its own table would be choosing how much it may risk.
///
/// A poisoned mutex — another cell's thread panicked while holding the lock —
/// is recovered rather than propagated, as every registry in
/// `qip-observability` does. The ledger inside cannot be torn: each mutation
/// is a comparison followed by writes with no panic point between them, so
/// what the next cell sees is a consistent balance, and refusing every hold
/// forever because a sibling crashed would be a region that stops for a
/// reason nothing journals.
#[derive(Clone, Debug)]
pub struct RegionTable {
    inner: Arc<Mutex<RegionAllocation>>,
}

impl RegionTable {
    /// Open a table over the region's amount. Refuses what
    /// [`RegionAllocation::new`] refuses.
    pub fn new(amount: Decimal) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(RegionAllocation::new(amount)?)),
        })
    }

    fn ledger(&self) -> MutexGuard<'_, RegionAllocation> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether two handles are the same table — the property a composition
    /// root's test asserts, since two tables that merely hold equal balances
    /// are exactly the defect a shared table exists to remove.
    pub fn shares_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn free(&self) -> Decimal {
        self.ledger().free()
    }

    pub fn held_total(&self) -> Decimal {
        self.ledger().held_total()
    }

    pub fn committed_total(&self) -> Decimal {
        self.ledger().committed_total()
    }

    /// See [`RegionAllocation::reserve`].
    pub fn reserve(
        &self,
        owner: &str,
        id: impl Into<String>,
        amount: Decimal,
        pass: u64,
    ) -> Result<()> {
        self.ledger().reserve(owner, id, amount, pass)
    }

    /// See [`RegionAllocation::commit`].
    pub fn commit(&self, owner: &str, id: &str) -> Option<Decimal> {
        self.ledger().commit(owner, id)
    }

    /// See [`RegionAllocation::release`].
    pub fn release(&self, owner: &str, id: &str) -> Option<Decimal> {
        self.ledger().release(owner, id)
    }

    /// See [`RegionAllocation::return_committed`].
    pub fn return_committed(&self, amount: Decimal) -> Result<()> {
        self.ledger().return_committed(amount)
    }

    /// See [`RegionAllocation::sweep_before`].
    pub fn sweep_before(&self, owner: &str, pass: u64) -> Vec<(String, Decimal)> {
        self.ledger().sweep_before(owner, pass)
    }
}

#[cfg(test)]
// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::dec;

    const CELL: &str = "london-1";

    #[test]
    fn a_second_hold_against_the_same_capital_is_refused_whole_rather_than_reduced() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        // The premise: the first hold was taken, and it moved the free
        // balance. Without this the refusal below would also pass against an
        // allocation that had refused the first hold too.
        allocation.reserve(CELL, "first", dec!("700"), 1)?;
        assert_eq!(allocation.free(), dec!("300"));

        let refused = allocation.reserve(CELL, "second", dec!("500"), 1);
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
        allocation.reserve(CELL, "one", dec!("400"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was taken"
        );

        assert_eq!(allocation.commit(CELL, "one"), Some(dec!("400")));
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
        allocation.reserve(CELL, "one", dec!("400"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was taken"
        );

        assert_eq!(allocation.release(CELL, "one"), Some(dec!("400")));
        assert_eq!(
            allocation.free(),
            dec!("1000"),
            "a released hold did not return its capital, so the allocation leaks on every refusal"
        );
        assert_eq!(allocation.committed_total(), Decimal::ZERO);
        assert_eq!(allocation.release(CELL, "one"), None);
        Ok(())
    }

    #[test]
    fn a_hold_from_an_earlier_pass_is_returned_by_the_next_passs_sweep() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve(CELL, "older", dec!("400"), 4)?;
        allocation.reserve(CELL, "current", dec!("100"), 5)?;
        assert_eq!(
            allocation.free(),
            dec!("500"),
            "the premise: both holds were taken"
        );

        let swept = allocation.sweep_before(CELL, 5);
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
    fn one_cells_sweep_leaves_another_cells_holds_standing() -> Result<()> {
        // Two cells share a table and count passes independently. A cell on
        // its fiftieth pass sweeping "everything before 50" would return the
        // hold a sibling on its first pass took a moment ago — for an order
        // that sibling is still about to send — and the sibling would then
        // commit a hold that no longer exists, spending nothing.
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve("london-1", "49:strategy:alpha", dec!("400"), 49)?;
        allocation.reserve("london-2", "1:strategy:alpha", dec!("100"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("500"),
            "the premise: both cells' holds were taken"
        );

        let swept = allocation.sweep_before("london-1", 50);
        assert_eq!(
            swept,
            vec![("49:strategy:alpha".to_string(), dec!("400"))],
            "the sweep did not return exactly the sweeping cell's own stale hold"
        );
        assert_eq!(
            allocation.held_total(),
            dec!("100"),
            "the sibling's live hold was swept by a cell that did not take it"
        );
        // And the sibling can still commit it, which is what a hold is for.
        assert_eq!(
            allocation.commit("london-2", "1:strategy:alpha"),
            Some(dec!("100"))
        );
        Ok(())
    }

    #[test]
    fn two_cells_may_file_the_same_hold_id_without_colliding() -> Result<()> {
        // Every cell keys its holds `"{pass}:strategy:{id}"`, so two cells on
        // the same pass running the same strategy file the same id. If the
        // table refused the second as a duplicate, the sibling would be turned
        // away for a reason that is not "the region is spent", and the journal
        // would say the strategy already held capital it never did.
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve("london-1", "1:strategy:alpha", dec!("400"), 1)?;
        allocation.reserve("london-2", "1:strategy:alpha", dec!("400"), 1)?;
        assert_eq!(allocation.free(), dec!("200"));
        assert_eq!(allocation.held_total(), dec!("800"));
        // Each cell can only release its own.
        assert_eq!(
            allocation.release("london-2", "1:strategy:alpha"),
            Some(dec!("400"))
        );
        assert_eq!(allocation.release("london-2", "1:strategy:alpha"), None);
        assert_eq!(allocation.held_total(), dec!("400"));
        Ok(())
    }

    #[test]
    fn returning_committed_capital_is_bounded_by_what_was_committed() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve(CELL, "one", dec!("400"), 1)?;
        assert_eq!(allocation.commit(CELL, "one"), Some(dec!("400")));
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was committed"
        );

        allocation.return_committed(dec!("400"))?;
        assert_eq!(
            allocation.free(),
            dec!("1000"),
            "an order withdrawn unfilled did not give its capital back"
        );
        assert_eq!(allocation.committed_total(), Decimal::ZERO);
        // More than was ever committed is refused, and nothing moves: the
        // region cannot be made richer than its operator allocated.
        assert!(
            allocation.return_committed(dec!("1")).is_err(),
            "a return past the committed total was admitted"
        );
        assert!(allocation.return_committed(Decimal::ZERO).is_err());
        assert_eq!(allocation.free(), dec!("1000"));
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
            allocation.reserve(CELL, "zero", Decimal::ZERO, 1).is_err(),
            "a hold of nothing was admitted, so an order of nothing could pass the gate"
        );
        assert!(
            allocation.reserve(CELL, "negative", dec!("-5"), 1).is_err(),
            "a negative hold was admitted, which would widen the free balance"
        );
        assert_eq!(allocation.free(), dec!("1000"));
        assert_eq!(allocation.held_total(), Decimal::ZERO);
        Ok(())
    }

    #[test]
    fn a_duplicate_hold_id_is_refused_rather_than_overwriting_the_first() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve(CELL, "one", dec!("400"), 1)?;
        assert_eq!(
            allocation.free(),
            dec!("600"),
            "the premise: the hold was taken"
        );

        assert!(
            allocation.reserve(CELL, "one", dec!("100"), 1).is_err(),
            "a second hold under the same id was admitted; releasing it would return only one \
             of the two amounts and the difference would leak"
        );
        // The free balance moved once, not twice: the second hold took nothing.
        assert_eq!(allocation.free(), dec!("600"));
        assert_eq!(allocation.held_total(), dec!("400"));
        Ok(())
    }

    #[test]
    fn a_table_handle_is_the_same_ledger_from_every_clone() -> Result<()> {
        // The property a composition root's test needs: a clone is a handle
        // on the one ledger, not a copy of its balance. Two equal balances in
        // two ledgers is the defect the table exists to remove.
        let table = RegionTable::new(dec!("1000"))?;
        let sibling = table.clone();
        assert!(table.shares_with(&sibling));
        sibling.reserve("london-2", "1:strategy:alpha", dec!("700"), 1)?;
        assert_eq!(
            table.free(),
            dec!("300"),
            "a hold through one handle was not seen through the other"
        );
        assert!(
            table
                .reserve("london-1", "1:strategy:alpha", dec!("500"), 1)
                .is_err(),
            "the first handle admitted 500 against the 300 the sibling left"
        );
        let separate = RegionTable::new(dec!("1000"))?;
        assert!(!table.shares_with(&separate));
        Ok(())
    }
}
