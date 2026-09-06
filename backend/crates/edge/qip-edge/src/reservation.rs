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
//! # Whose number this is
//!
//! Two numbers, and it matters which is which. The **ceiling** is the
//! operator's: the amount a composition root opens the table over, the most
//! this process will ever accept, a local backstop that can only narrow. The
//! **bound** is the centre's: the cell's disjoint share of its region's grant
//! (ADR 0039), applied by [`RegionAllocation::rebase`] from the grant
//! manifest of a signed policy payload and never wider than the ceiling. A
//! table opened by [`RegionAllocation::new`] starts bounded at its ceiling —
//! the shape every deployed node has today; one opened by
//! [`RegionAllocation::unfunded`] starts bounded at nothing and sends nothing
//! until the centre has named a share for it, which is "capital granted in
//! advance" read strictly.
//!
//! Before the bound existed the ceiling was the only number, and two nodes
//! under one regional grant held two operator-typed amounts nothing summed —
//! traceability F6's "operator discipline, not a structural guarantee". The
//! share closes that from the centre's side: it is computed from the
//! allocation plan, checked against the region's grant before anything ships,
//! and carried in the payload's `capital_grants` slot by naming the signed
//! envelopes it consists of, so the cell sums a fact it has already verified
//! rather than trusting a second claim about it.
//!
//! # A share is a cell's own, and only rises with the sequence
//!
//! A re-base is refused at or below the sequence last applied: a replayed
//! older payload carrying a wider share is exactly the widening ADR 0008
//! forbids, and refusing it here — as well as in `Cell::apply_policy` — keeps
//! the guarantee on the ledger rather than on one caller remembering to
//! check. A re-base is also refused from a second owner. The shared-table
//! shape (`Cell::with_region_table` handed to two cells) predates shares and
//! still works for cells nobody re-bases; but a share is one cell's disjoint
//! view, and two cells re-basing one table would leave it at whichever cell
//! spoke last rather than at either cell's share.
//!
//! One re-derivation is permitted *at* the applied sequence, by
//! [`RegionAllocation::rederive`]: the payload that names a cell's grants
//! arrives before the plan it names has deployed them, and a cell that could
//! only sum its manifest once would fund nothing until the next payload. The
//! re-derivation sums the same signed manifest against the grants the cell
//! now holds; it cannot name a grant the manifest did not, and the centre
//! ships a manifest only when its grants' gross fits the share it computed,
//! so the result is bounded by the same number the payload was.
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
    /// The operator's number: the most this ledger will ever be bounded to.
    /// See the module doc for why it is not the centre's.
    ceiling: Decimal,
    /// The effective bound: `held + committed + free` when no share has
    /// narrowed it below what is already spoken for. Never above `ceiling`.
    bound: Decimal,
    /// The sequence of the last share applied, and the cell that applied it.
    /// `None` until [`Self::rebase`] has run once.
    share: Option<AppliedShare>,
}

/// Who last re-based the ledger and with which payload sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AppliedShare {
    owner: String,
    sequence: u64,
}

/// What a re-base did to the ledger, for the journal.
///
/// `deficit` is what the share fell short of what the cell had already held
/// or committed. It is a stated ledger state, not an error: the input was a
/// valid share and the cell cannot un-send an order, so `free` is zero and the
/// shortfall is named rather than clamped away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rebase {
    /// The share as the centre named it, before the ceiling.
    pub share: Decimal,
    /// The bound the ledger now enforces: the share, or the ceiling if lower.
    pub bound: Decimal,
    /// Capital no hold is standing on, after the re-base.
    pub free: Decimal,
    /// By how much what was already spoken for exceeds the new bound; zero
    /// when it does not.
    pub deficit: Decimal,
}

impl RegionAllocation {
    /// Open an allocation over an amount, bounded at it from the start.
    ///
    /// Zero is permitted and means a region that sends nothing, which is the
    /// correct reading of "this region has no capital". A negative amount is
    /// refused rather than floored: it means the caller's own accounting has
    /// already gone wrong, and a floor would bury that.
    ///
    /// The amount is also the ceiling: a share the centre later names can
    /// narrow this ledger and never widen it past what the operator typed.
    pub fn new(free: Decimal) -> Result<Self> {
        Self::opened(free, free)
    }

    /// Open an allocation that funds nothing until the centre names a share.
    ///
    /// The ADR 0039 shape: `free` is zero, the ceiling is the operator's
    /// backstop, and the first [`Self::rebase`] is what lets the cell send. A
    /// cell opened this way and never granted to refuses every hold under
    /// `region_reservation`, which is the honest reading of "capital granted
    /// in advance" — a cell nobody has granted to has nothing in advance.
    /// Contrast [`Self::new`], which is every deployed node today and the
    /// owner's second decision in the ADR.
    pub fn unfunded(ceiling: Decimal) -> Result<Self> {
        Self::opened(Decimal::ZERO, ceiling)
    }

    fn opened(bound: Decimal, ceiling: Decimal) -> Result<Self> {
        if ceiling.is_negative() {
            return Err(Error::invalid(format!(
                "a region allocation cannot open over a negative amount ({ceiling}); give the \
                 cell the amount its region was actually allocated, or none at all"
            )));
        }
        Ok(Self {
            free: bound,
            holds: BTreeMap::new(),
            committed: Decimal::ZERO,
            ceiling,
            bound,
            share: None,
        })
    }

    /// Capital no hold is standing on.
    pub fn free(&self) -> Decimal {
        self.free
    }

    /// The bound the ledger enforces now: the last share applied, capped by
    /// the ceiling, or the opening amount if none has been.
    pub fn bound(&self) -> Decimal {
        self.bound
    }

    /// The operator's ceiling, which no share can raise.
    pub fn ceiling(&self) -> Decimal {
        self.ceiling
    }

    /// The sequence of the last share applied, if any was.
    pub fn share_sequence(&self) -> Option<u64> {
        self.share.as_ref().map(|share| share.sequence)
    }

    /// Re-base the ledger to the share the centre named for `owner`, under
    /// the payload sequence it arrived with.
    ///
    /// Three refusals, each named rather than corrected. A negative share is
    /// the centre's accounting gone wrong and is refused like a negative
    /// opening amount. A sequence at or below the last applied is a replay
    /// and is refused, not clamped: an older payload with a wider share is
    /// the widening the sequence discipline exists to stop, and the ledger
    /// keeps the guard itself so it does not depend on one caller checking.
    /// A second owner is refused: a share is one cell's disjoint view, and a
    /// table two cells both re-based would sit at whichever spoke last.
    ///
    /// The bound becomes `min(share, ceiling)`; `free` becomes what that
    /// leaves after every live hold and everything committed, or zero with
    /// the deficit stated when it leaves less than nothing. Nothing held or
    /// committed is touched — the cell cannot un-send an order.
    pub fn rebase(&mut self, owner: &str, share: Decimal, sequence: u64) -> Result<Rebase> {
        if owner.trim().is_empty() {
            return Err(Error::invalid(
                "a region share needs an owner, or nothing can tell one cell's share from a \
                 sibling's",
            ));
        }
        if share.is_negative() {
            return Err(Error::invalid(format!(
                "a region share cannot be negative ({share}); the centre's plan has gone wrong \
                 and the ledger is kept rather than corrected"
            )));
        }
        if let Some(applied) = &self.share {
            if applied.owner != owner {
                return Err(Error::denied(format!(
                    "this region table was re-based by {} and cannot be re-based by {owner}; a \
                     share is one cell's own, so a sibling must hold its own table",
                    applied.owner
                )));
            }
            if sequence <= applied.sequence {
                return Err(Error::denied(format!(
                    "region share sequence {sequence} is not newer than the applied {}; an old \
                     payload cannot re-base this cell's share",
                    applied.sequence
                )));
            }
        }
        Ok(self.apply_bound(owner, share, sequence))
    }

    /// Re-derive the share under the sequence already applied, for the same
    /// owner.
    ///
    /// The one case [`Self::rebase`]'s strict ordering cannot serve: the
    /// payload that names a cell's grants is applied *before* the plan it
    /// also names deploys them, so the manifest is summed against an empty
    /// set and the table narrows to nothing until the next payload. The
    /// grant then arrives, verified, and the cell can sum the same signed
    /// manifest again — same payload, same sequence, one more of the grants
    /// it names now held. This is not a second path to the bound: the inputs
    /// are the applied payload and the envelopes it names, both signed by
    /// the centre, and the centre ships a manifest only when their gross fits
    /// the share it computed. Refused, never applied, for any sequence other
    /// than the one applied and for a table no share has been applied to —
    /// a cell with no share has nothing to re-derive, and a different
    /// sequence is [`Self::rebase`]'s business with its own ordering guard.
    pub fn rederive(&mut self, owner: &str, share: Decimal, sequence: u64) -> Result<Rebase> {
        if share.is_negative() {
            return Err(Error::invalid(format!(
                "a region share cannot be negative ({share}); the centre's plan has gone wrong \
                 and the ledger is kept rather than corrected"
            )));
        }
        let Some(applied) = &self.share else {
            return Err(Error::denied(
                "no region share has been applied to this table, so there is nothing to \
                 re-derive; a share arrives with a policy payload and never from a grant alone",
            ));
        };
        if applied.owner != owner {
            return Err(Error::denied(format!(
                "this region table's share is {}'s and cannot be re-derived by {owner}",
                applied.owner
            )));
        }
        if applied.sequence != sequence {
            return Err(Error::denied(format!(
                "region share sequence {sequence} is not the applied {}; a share is re-derived \
                 only under the payload that named it",
                applied.sequence
            )));
        }
        Ok(self.apply_bound(owner, share, sequence))
    }

    /// The arithmetic shared by [`Self::rebase`] and [`Self::rederive`], past
    /// their guards: the bound becomes `min(share, ceiling)` and `free` what
    /// that leaves after everything spoken for, or zero with the deficit
    /// stated.
    fn apply_bound(&mut self, owner: &str, share: Decimal, sequence: u64) -> Rebase {
        let bound = share.min(self.ceiling);
        // Both addends are bounded by the ceiling this table opened over —
        // every hold was taken from `free`, every commitment from a hold —
        // so the sum cannot exceed twice a `Decimal` the operator typed.
        let spoken_for = self.held_total() + self.committed;
        let (free, deficit) = if spoken_for > bound {
            (Decimal::ZERO, spoken_for - bound)
        } else {
            (bound - spoken_for, Decimal::ZERO)
        };
        self.bound = bound;
        self.free = free;
        self.share = Some(AppliedShare {
            owner: owner.to_string(),
            sequence,
        });
        Rebase {
            share,
            bound,
            free,
            deficit,
        }
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

    /// Open a table that funds nothing until a share is applied. See
    /// [`RegionAllocation::unfunded`].
    pub fn unfunded(ceiling: Decimal) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(RegionAllocation::unfunded(ceiling)?)),
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

    /// See [`RegionAllocation::bound`].
    pub fn bound(&self) -> Decimal {
        self.ledger().bound()
    }

    /// See [`RegionAllocation::ceiling`].
    pub fn ceiling(&self) -> Decimal {
        self.ledger().ceiling()
    }

    /// See [`RegionAllocation::share_sequence`].
    pub fn share_sequence(&self) -> Option<u64> {
        self.ledger().share_sequence()
    }

    /// See [`RegionAllocation::rebase`].
    pub fn rebase(&self, owner: &str, share: Decimal, sequence: u64) -> Result<Rebase> {
        self.ledger().rebase(owner, share, sequence)
    }

    /// See [`RegionAllocation::rederive`].
    pub fn rederive(&self, owner: &str, share: Decimal, sequence: u64) -> Result<Rebase> {
        self.ledger().rederive(owner, share, sequence)
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
    fn a_share_at_or_below_the_applied_sequence_is_refused_not_clamped() -> Result<()> {
        // The ledger's own guard, independent of `Cell::apply_policy`'s: an
        // older payload with a wider share must not re-widen a cell the
        // centre has narrowed, whoever forgot to check the sequence first.
        let mut allocation = RegionAllocation::unfunded(dec!("5000"))?;
        assert_eq!(
            allocation.free(),
            Decimal::ZERO,
            "the premise: unfunded opens at nothing"
        );
        let first = allocation.rebase(CELL, dec!("3000"), 6)?;
        assert_eq!(first.bound, dec!("3000"));
        assert_eq!(
            allocation.free(),
            dec!("3000"),
            "the premise: sequence 6 funded the ledger"
        );
        let narrowed = allocation.rebase(CELL, dec!("100"), 7)?;
        assert_eq!(
            narrowed.bound,
            dec!("100"),
            "the premise: sequence 7 narrowed it"
        );

        for replay in [7, 6, 1] {
            let refused = allocation.rebase(CELL, dec!("4000"), replay);
            let error = match refused {
                Ok(rebase) => panic!("sequence {replay} re-based the ledger to {}", rebase.bound),
                Err(error) => error,
            };
            assert!(
                error.message().contains("not newer than the applied 7"),
                "the refusal did not name the applied sequence: {}",
                error.message()
            );
        }
        // Nothing moved: refused, not clamped to the applied share.
        assert_eq!(allocation.bound(), dec!("100"));
        assert_eq!(allocation.free(), dec!("100"));
        assert_eq!(allocation.share_sequence(), Some(7));
        Ok(())
    }

    #[test]
    fn a_share_is_rederived_only_under_the_sequence_that_named_it() -> Result<()> {
        // The node applies its payload before the plan it names deploys the
        // grants the manifest names, so the first sum is nothing. The grant
        // then lands and the same manifest is summed again under the same
        // sequence — and under no other, or the re-derivation would be a
        // second re-base path with no ordering guard.
        let mut allocation = RegionAllocation::unfunded(dec!("5000"))?;
        assert!(
            allocation.rederive(CELL, dec!("100"), 3).is_err(),
            "a table no share was applied to was re-derived from a grant alone"
        );
        assert_eq!(allocation.free(), Decimal::ZERO);
        let first = allocation.rebase(CELL, Decimal::ZERO, 3)?;
        assert_eq!(
            first.bound,
            Decimal::ZERO,
            "the premise: sequence 3 named nothing"
        );

        let rederived = allocation.rederive(CELL, dec!("100"), 3)?;
        assert_eq!(
            rederived.bound,
            dec!("100"),
            "the same sequence did not fund the ledger"
        );
        assert_eq!(allocation.free(), dec!("100"));
        assert_eq!(allocation.share_sequence(), Some(3));

        for other in [2, 4] {
            assert!(
                allocation.rederive(CELL, dec!("4000"), other).is_err(),
                "sequence {other} re-derived a share applied under sequence 3"
            );
        }
        assert!(
            allocation.rederive("london-2", dec!("4000"), 3).is_err(),
            "a sibling re-derived this cell's share"
        );
        assert!(allocation.rederive(CELL, dec!("-1"), 3).is_err());
        assert_eq!(
            allocation.bound(),
            dec!("100"),
            "a refused re-derivation moved the bound"
        );
        Ok(())
    }

    #[test]
    fn a_share_never_rises_past_the_ceiling_and_a_second_owner_cannot_rebase() -> Result<()> {
        // The ceiling is the operator's backstop: a share wider than it is
        // applied at the ceiling. And a share is one cell's own, so a table
        // two cells share cannot be re-based by both.
        let mut allocation = RegionAllocation::unfunded(dec!("500"))?;
        let rebase = allocation.rebase("london-1", dec!("9000"), 1)?;
        assert_eq!(
            rebase.share,
            dec!("9000"),
            "the premise: the share was the wide one"
        );
        assert_eq!(
            rebase.bound,
            dec!("500"),
            "the share rose past the operator's ceiling"
        );
        assert_eq!(allocation.free(), dec!("500"));
        let sibling = allocation.rebase("london-2", dec!("100"), 2);
        assert!(
            sibling.is_err(),
            "a second cell re-based a table that was the first cell's private view"
        );
        assert_eq!(
            allocation.bound(),
            dec!("500"),
            "the sibling's refused re-base moved the bound"
        );
        assert!(allocation.rebase(CELL, dec!("-1"), 3).is_err());
        assert!(allocation.rebase("", dec!("1"), 3).is_err());
        Ok(())
    }

    #[test]
    fn a_share_below_what_is_spoken_for_zeroes_free_and_states_the_deficit() -> Result<()> {
        let mut allocation = RegionAllocation::new(dec!("1000"))?;
        allocation.reserve(CELL, "held", dec!("300"), 1)?;
        allocation.reserve(CELL, "sent", dec!("400"), 1)?;
        assert_eq!(allocation.commit(CELL, "sent"), Some(dec!("400")));
        assert_eq!(
            allocation.free(),
            dec!("300"),
            "the premise: 700 is spoken for"
        );

        let rebase = allocation.rebase(CELL, dec!("500"), 1)?;
        assert_eq!(
            rebase.free,
            Decimal::ZERO,
            "free was not zeroed under a deficit"
        );
        assert_eq!(
            rebase.deficit,
            dec!("200"),
            "the deficit was not the shortfall"
        );
        assert!(!allocation.free().is_negative(), "free went negative");
        // Nothing held or committed moved: the cell cannot un-send.
        assert_eq!(allocation.held_total(), dec!("300"));
        assert_eq!(allocation.committed_total(), dec!("400"));
        // And a share that covers what is spoken for has no deficit.
        let covered = allocation.rebase(CELL, dec!("900"), 2)?;
        assert_eq!(covered.deficit, Decimal::ZERO);
        assert_eq!(covered.free, dec!("200"));
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
