//! What a cell must be shown before it resumes after a crash (§36.3).
//!
//! Blueprint §36.3's node-crash row, in its own words: *"That region dark;
//! reconciles against every venue before resuming"*, and §48's degradation
//! matrix spells out what against — *"Reconciles against every venue
//! including resting orders and quotes before resuming"*.
//!
//! # The failure this exists to stop
//!
//! A cell rebuilds its books from the feed on every start and its journal
//! chains onto genesis, so a restarted process knows nothing about the
//! orders the dead one left resting at a venue. Until this existed the new
//! process simply began trading: it raised signals, netted them and sent
//! orders, while the venue was still holding size for it that nothing in
//! this platform could see, withdraw or attribute a fill on. That is a
//! position with no owner, and the reconciler could never find it — the
//! cell's side of the comparison was empty and stayed empty, so the two
//! records agreed on nothing and therefore agreed.
//!
//! # Why the cell does not arm this itself
//!
//! Only the composition root knows whether this process is a restart. The
//! node reads that from its own journal store — a store holding a previous
//! session is a previous run of this cell — and arms the discipline before
//! the health surface binds. A cell that armed itself on every start would
//! stop every genuinely first start of every node for ever, waiting for an
//! account of a venue it has never sent anything to, which is a gate that
//! cannot open rather than a control that fires.
//!
//! # What clears it, and what does not
//!
//! Each venue clears when the *venue's own* account of what it holds open
//! for this cell agrees with the cell's record. Nothing the cell computes
//! about itself can clear it: a cell that cleared its own gate would be
//! asserting the thing it was asked to prove. A disagreement does not clear
//! it either — it is a reconciliation break, which halts the cell and is
//! never auto-corrected (§36.3's own row for a break, and §48's "human
//! investigation").

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use std::collections::{BTreeMap, BTreeSet};

/// The most orders one venue's account may name.
///
/// The same bound as the cell's own open-order set, and for the same
/// reason: the account is compared against that set order by order, and an
/// account claiming more orders than the cell could ever have held is not an
/// account of this cell.
pub const MAX_ACCOUNT_ORDERS: usize = crate::cell::MAX_OPEN_ORDERS;

/// One venue's own statement of what it is holding open for this cell.
///
/// Built by the composition root from whatever channel the venue offers,
/// never by the cell: a cell that synthesised this from the orders it
/// remembers sending would be comparing a record with itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueAccount {
    venue: VenueId,
    /// Order id to the quantity the venue still holds open on it. Ordered,
    /// because the comparison's findings reach a journal entry and a replay
    /// that reorders is not a replay.
    open: BTreeMap<String, Decimal>,
    /// Quotes the venue holds live for this cell. Counted rather than
    /// listed: §48 asks for "resting orders and quotes", and a quote is not
    /// an order the cell can attribute a fill on — what matters is whether
    /// the venue is holding any, which is the number an operator needs
    /// before they decide a restarted cell is safe to resume.
    quotes: usize,
    at: Timestamp,
}

impl VenueAccount {
    /// An account with nothing open. The ordinary answer, and the one that
    /// lets a restarted cell resume.
    pub fn empty(venue: VenueId, at: Timestamp) -> Self {
        Self {
            venue,
            open: BTreeMap::new(),
            quotes: 0,
            at,
        }
    }

    /// Add one order the venue says it is still holding.
    ///
    /// Refuses a quantity that is not positive and an empty id, rather than
    /// dropping either: an account entry with no quantity would be compared
    /// against the cell's record and found to agree with nothing, and a
    /// silent drop is how a resting order the venue really holds becomes a
    /// resting order nobody reconciles against.
    pub fn with_open(mut self, order_id: impl Into<String>, remaining: Decimal) -> Result<Self> {
        let order_id = order_id.into();
        if order_id.trim().is_empty() {
            return Err(Error::invalid(format!(
                "{} reported an order it holds open with no id; an order nobody can name cannot \
                 be matched against the cell's record, so supply the venue's own id for it",
                self.venue.as_str()
            )));
        }
        if !remaining.is_positive() {
            return Err(Error::invalid(format!(
                "{} reported order {order_id} open for {remaining}; an order with nothing \
                 remaining is not open, so leave it out of the account or report what is left",
                self.venue.as_str()
            )));
        }
        if self.open.len() >= MAX_ACCOUNT_ORDERS {
            return Err(Error::invalid(format!(
                "{} reported more than {MAX_ACCOUNT_ORDERS} orders open for this cell, which is \
                 more than it could ever have held open at once; this is not an account of this \
                 cell",
                self.venue.as_str()
            )));
        }
        self.open.insert(order_id, remaining);
        Ok(self)
    }

    /// State how many quotes the venue holds live for this cell.
    pub fn with_quotes(mut self, quotes: usize) -> Self {
        self.quotes = quotes;
        self
    }

    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub const fn open(&self) -> &BTreeMap<String, Decimal> {
        &self.open
    }

    pub const fn quotes(&self) -> usize {
        self.quotes
    }

    pub const fn at(&self) -> Timestamp {
        self.at
    }
}

/// The venues a restarted cell must still be shown before it forms an order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeDiscipline {
    reason: String,
    pending: BTreeSet<String>,
}

impl ResumeDiscipline {
    /// Require an account from every venue named.
    ///
    /// Refuses an empty venue list and an empty reason. A discipline over no
    /// venue is satisfied the instant it is armed — the control that reads
    /// as protection and cannot fire — and a discipline with no stated
    /// reason leaves an operator looking at a node that is refusing every
    /// pass with nothing to act on.
    pub fn new(
        reason: impl Into<String>,
        venues: impl IntoIterator<Item = VenueId>,
    ) -> Result<Self> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(Error::invalid(
                "a resume discipline with no stated reason refuses every pass and tells an \
                 operator nothing; name why this cell must reconcile before it resumes",
            ));
        }
        let pending: BTreeSet<String> = venues
            .into_iter()
            .map(|venue| venue.as_str().to_string())
            .collect();
        if pending.is_empty() {
            return Err(Error::invalid(
                "a resume discipline naming no venue is satisfied before it is armed, so it \
                 would let a restarted cell trade against venues it never reconciled with; name \
                 the venues this cell may reach",
            ));
        }
        Ok(Self { reason, pending })
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The venues still to answer, in order.
    pub fn pending(&self) -> Vec<String> {
        self.pending.iter().cloned().collect()
    }

    pub fn is_satisfied(&self) -> bool {
        self.pending.is_empty()
    }

    /// Record that `venue` answered and its account agreed.
    ///
    /// Returns whether this venue was one the discipline was waiting for, so
    /// a caller can tell a venue that cleared from one that answered twice.
    pub fn answered(&mut self, venue: &VenueId) -> bool {
        self.pending.remove(venue.as_str())
    }

    /// Whether `venue` is one this discipline is still waiting for.
    pub fn awaits(&self, venue: &VenueId) -> bool {
        self.pending.contains(venue.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venue() -> VenueId {
        VenueId::new("XLON")
    }

    fn d(literal: &str) -> Decimal {
        Decimal::parse(literal).expect("a test decimal parses")
    }

    #[test]
    fn a_discipline_naming_no_venue_is_refused_because_it_would_be_satisfied_at_birth() {
        // This repository's standing example is a limit that could never
        // fire; this is the same shape from the other side — a gate that is
        // open the instant it is armed. A restarted cell holding one would
        // report that it had reconciled with every venue it was asked about,
        // having been asked about none.
        let refusal = ResumeDiscipline::new("restart", Vec::new())
            .expect_err("a discipline over no venue proves nothing");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("satisfied before it is armed"),
            "the refusal should say why: {}",
            refusal.message()
        );
        // The half that proves it admits a good value.
        let armed = ResumeDiscipline::new("restart", [venue()]).expect("one venue is a discipline");
        assert!(!armed.is_satisfied());
    }

    #[test]
    fn a_discipline_with_no_stated_reason_is_refused() {
        let refusal = ResumeDiscipline::new("  ", [venue()])
            .expect_err("a discipline that refuses every pass must say why");
        assert_eq!(refusal.code(), "invalid");
    }

    #[test]
    fn a_discipline_is_satisfied_only_once_every_venue_has_answered() {
        let mut discipline =
            ResumeDiscipline::new("restart", [venue(), VenueId::new("XNYS")]).expect("two venues");
        // Premise: both venues really are pending, or the assertions below
        // would pass on an empty set.
        assert_eq!(discipline.pending(), vec!["XLON", "XNYS"]);
        assert!(discipline.answered(&venue()));
        assert!(
            !discipline.is_satisfied(),
            "one venue answering is not every venue answering"
        );
        assert!(
            !discipline.answered(&venue()),
            "a second answer from the same venue clears nothing"
        );
        assert!(discipline.answered(&VenueId::new("XNYS")));
        assert!(discipline.is_satisfied());
    }

    #[test]
    fn an_account_entry_with_nothing_remaining_is_refused_rather_than_dropped() {
        // A dropped entry is a resting order the venue holds and the cell
        // never compares against — exactly the order this discipline exists
        // to find.
        let refusal = VenueAccount::empty(venue(), Timestamp::from_secs(10))
            .with_open("ORD-1", Decimal::ZERO)
            .expect_err("an order with nothing left is not open");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("is not open"),
            "the refusal should say why: {}",
            refusal.message()
        );
        let account = VenueAccount::empty(venue(), Timestamp::from_secs(10))
            .with_open("ORD-1", d("5"))
            .expect("a positive remainder is admitted");
        assert_eq!(account.open().len(), 1);
    }

    #[test]
    fn an_account_order_with_no_id_is_refused() {
        let refusal = VenueAccount::empty(venue(), Timestamp::from_secs(10))
            .with_open("   ", d("5"))
            .expect_err("an order nobody can name matches nothing");
        assert_eq!(refusal.code(), "invalid");
    }
}
