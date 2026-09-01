//! Holding capital between the check and the trade.
//!
//! The failure this module removes is gap-matrix item 10: a proposal that
//! passed a capital check held nothing, so two proposals sized in the same
//! cycle could each pass against the same free balance and the book could be
//! committed twice over — each check individually correct and their sum a
//! position nobody approved. A check that holds nothing is not a control; it
//! is a race with a comforting name.
//!
//! [`ReservationLedger`] makes passing the check and holding the capital the
//! same operation. [`ReservationLedger::reserve`] succeeds only by moving the
//! amount out of the free balance, so the second proposal against the same
//! capital is *refused* — not clamped to what is left, not queued behind the
//! first — with a message naming what would free the capital. From there a
//! reservation ends in exactly one of three ways:
//!
//! * [`ReservationLedger::commit`] — the proposal was released to execution,
//!   and the capital becomes an allocation rather than returning.
//! * [`ReservationLedger::release`] — the proposal was vetoed or withdrawn,
//!   and the capital returns to the free balance.
//! * **Expiry** — nobody did either, and the hold lapses so an abandoned
//!   proposal cannot pin capital forever. Expiry is judged against the
//!   [`Timestamp`] the caller passes, like every clock in this crate, so a
//!   replay reproduces the same refusals.
//!
//! Fail closed at the join: an expired or unknown reservation cannot be
//! committed. A commit that succeeded against a lapsed hold would spend
//! capital the free balance already counts as available — the double-spend
//! this module exists to prevent, reintroduced at its own back door.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One live hold on the free balance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reservation {
    /// How much is held. Always positive; [`ReservationLedger::reserve`]
    /// refuses anything else.
    pub amount: Decimal,
    pub reserved_at: Timestamp,
    /// When the hold lapses. At this instant the reservation is already
    /// expired, matching how an envelope is not live at its own expiry.
    pub expires_at: Timestamp,
}

impl Reservation {
    /// Whether the hold has lapsed at `now`.
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }
}

/// The free balance and every hold against it.
///
/// Serializable so the state can be journalled and replayed; iteration is over
/// a [`BTreeMap`] so anything derived from it — the expiry sweep, a report —
/// comes out in the same order on every machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReservationLedger {
    free: Decimal,
    reservations: BTreeMap<String, Reservation>,
    committed: Decimal,
}

impl ReservationLedger {
    /// Open a ledger over a free balance.
    ///
    /// Zero is permitted — a ledger with nothing free refuses every
    /// reservation, which is the correct behaviour for an empty book — but a
    /// negative balance is refused rather than floored, because it means the
    /// caller's own accounting has already gone wrong and a floor would bury
    /// that.
    pub fn new(free: Decimal) -> Result<Self> {
        if free.is_negative() {
            return Err(Error::invalid(format!(
                "a reservation ledger cannot open over a negative free balance ({free}); \
                 reconcile the book before holding capital against it"
            )));
        }
        Ok(Self {
            free,
            reservations: BTreeMap::new(),
            committed: Decimal::ZERO,
        })
    }

    /// Capital not held by any reservation, as of `now`.
    ///
    /// Sweeps lapsed holds first, so the answer is what a reservation made at
    /// `now` could actually take rather than what the last mutation left.
    pub fn free(&mut self, now: Timestamp) -> Decimal {
        self.expire_due(now);
        self.free
    }

    /// The sum of every hold still recorded, lapsed or not.
    pub fn reserved_total(&self) -> Decimal {
        self.reservations
            .values()
            .map(|r| r.amount)
            .fold(Decimal::ZERO, |a, b| a + b)
    }

    /// Capital that passed through [`Self::commit`] and left the ledger.
    pub fn committed_total(&self) -> Decimal {
        self.committed
    }

    /// The hold recorded under `id`, if any — lapsed or not.
    pub fn reservation(&self, id: &str) -> Option<&Reservation> {
        self.reservations.get(id)
    }

    /// Return every lapsed hold to the free balance.
    ///
    /// Called from every entry point that takes a timestamp, so the caller
    /// never has to remember a sweep — but also public, so a housekeeping pass
    /// can record what lapsed. Returns the expired holds in id order.
    pub fn expire_due(&mut self, now: Timestamp) -> Vec<(String, Decimal)> {
        let due: Vec<String> = self
            .reservations
            .iter()
            .filter(|(_, r)| r.is_expired(now))
            .map(|(id, _)| id.clone())
            .collect();
        let mut expired = Vec::with_capacity(due.len());
        for id in due {
            if let Some(reservation) = self.reservations.remove(&id) {
                // Cannot overflow: every held amount came out of this same
                // free balance, so returning it restores a value the field
                // has already represented.
                self.free += reservation.amount;
                expired.push((id, reservation.amount));
            }
        }
        expired
    }

    /// Pass the capital check by taking the capital.
    ///
    /// This is the whole point of the module: there is no way to learn that
    /// the free balance covers `amount` without simultaneously holding it, so
    /// a second reservation against the same capital cannot also pass. The
    /// refusal is a refusal — the caller resizes, releases something, or
    /// waits; nothing is clamped and nothing queues.
    pub fn reserve(
        &mut self,
        id: impl Into<String>,
        amount: Decimal,
        now: Timestamp,
        validity: Duration,
    ) -> Result<()> {
        self.expire_due(now);
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::invalid(
                "a reservation needs an id, or nothing can ever commit or release it",
            ));
        }
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "cannot reserve {amount}; a reservation holds a positive amount or it holds \
                 nothing"
            )));
        }
        if validity.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a reservation must be valid for a positive duration; one that expires at or \
                 before its own creation holds nothing",
            ));
        }
        if self.reservations.contains_key(&id) {
            return Err(Error::invalid(format!(
                "{id} already holds a reservation; commit or release it before reserving again"
            )));
        }
        if amount > self.free {
            return Err(Error::denied(format!(
                "reserving {amount} for {id} needs more than the {} free; {} is already held \
                 by {} reservation(s) — release one, let one expire, or resize the proposal",
                self.free,
                self.reserved_total(),
                self.reservations.len()
            )));
        }
        self.free -= amount;
        self.reservations.insert(
            id,
            Reservation {
                amount,
                reserved_at: now,
                expires_at: now.saturating_add(validity),
            },
        );
        Ok(())
    }

    /// Convert a hold into an allocation. The capital does not return.
    ///
    /// Fails closed on both the ways a commit could spend capital the free
    /// balance already counts: an unknown id is refused rather than treated
    /// as already-committed, and a lapsed hold is refused *and returned to
    /// the free balance*, because at its expiry the capital stopped being
    /// held whether or not a sweep had run yet.
    pub fn commit(&mut self, id: &str, now: Timestamp) -> Result<Decimal> {
        let Some(reservation) = self.reservations.get(id) else {
            return Err(Error::denied(format!(
                "no reservation named {id} exists; reserve before committing, and note that a \
                 lapsed hold is removed at its expiry"
            )));
        };
        if reservation.is_expired(now) {
            let expires_at = reservation.expires_at;
            self.expire_due(now);
            return Err(Error::denied(format!(
                "the reservation for {id} expired at {expires_at} and its capital has returned \
                 to the free balance; reserve again before committing"
            )));
        }
        // Removal cannot miss: the borrow above proved the key present and
        // nothing ran in between.
        let Some(reservation) = self.reservations.remove(id) else {
            return Err(Error::denied(format!(
                "the reservation for {id} vanished between check and commit"
            )));
        };
        // Bounded by the opening balance, like the free field it came from.
        self.committed += reservation.amount;
        Ok(reservation.amount)
    }

    /// Give a hold back. The capital returns to the free balance.
    ///
    /// Releasing a hold that has lapsed but not yet been swept succeeds and
    /// returns the same capital the sweep would have — a veto racing the
    /// expiry clock should not error on the loser. An unknown id is refused,
    /// because a release that "succeeds" against nothing turns a typo into a
    /// clean audit trail.
    pub fn release(&mut self, id: &str, now: Timestamp) -> Result<Decimal> {
        let Some(reservation) = self.reservations.remove(id) else {
            self.expire_due(now);
            return Err(Error::denied(format!(
                "no reservation named {id} exists to release; a lapsed hold returns its own \
                 capital at expiry"
            )));
        };
        // Same conservation argument as the sweep: this amount came out of
        // this free balance.
        self.free += reservation.amount;
        self.expire_due(now);
        Ok(reservation.amount)
    }
}
