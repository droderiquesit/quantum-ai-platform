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
//!
//! # The expiry had no ceiling, so the third ending was optional
//!
//! The third bullet above is a promise — "cannot pin capital forever" — and
//! until [`MAXIMUM_RESERVATION_VALIDITY`] existed a caller could void it
//! through this module's own front door, silently. [`ReservationLedger::reserve`]
//! computed the expiry with `Timestamp::saturating_add`, so a validity larger
//! than the remaining range of the clock *clamped* to [`Timestamp::MAX`] —
//! the value [`qip_core::Timestamp`] documents as the sentinel meaning "no
//! upper bound". A hold taken in 2023 with an unbounded validity was still
//! live a century later, `free` stayed at zero, and every subsequent
//! reservation was refused for want of capital that nothing would ever
//! release. A control that stops the platform sizing positions, while
//! reading as the control working, is the worst shape a refusal can take.
//!
//! Two things close it, and both can fire. A validity past the ceiling is
//! **refused**, not truncated, because a caller told their hold was granted
//! and silently given a shorter one is a caller who will believe the wrong
//! expiry. And the expiry arithmetic is checked rather than saturating, so a
//! `now` near the end of the clock's range refuses too — `Timestamp::MAX` is
//! a real constructible value that point-in-time views already pass around.
//!
//! The ceiling is a day, and it is not a number picked to fit its callers.
//! Both production callers already assume it: the kernel holds a constructed
//! proposal for twenty-four hours, and `qip_kernel::exploration`'s
//! `PROBE_VALIDITY` is "a day, matching the hold a constructed proposal
//! takes, so exploration capital cannot be pinned for longer than
//! return-seeking capital can". This makes that shared assumption structural
//! instead of a coincidence between two call sites. [`crate::envelope`] took
//! the same decision one module over, for the same reason and with the same
//! shape: expiry is the only revocation mechanism either type has, so the
//! ceiling on it is the whole of the guarantee.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The longest a reservation may hold capital before it lapses.
///
/// A day. See this module's documentation for why a ceiling has to exist at
/// all, and why this is the day both production callers already assume
/// rather than a bound chosen to admit them.
pub const MAXIMUM_RESERVATION_VALIDITY: Duration = Duration::from_hours(24);

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
    ///
    /// `validity` is bounded by [`MAXIMUM_RESERVATION_VALIDITY`] and a longer
    /// one is refused rather than shortened, for the reason this module's
    /// documentation gives: expiry is the only thing that frees a hold nobody
    /// resolves, so an unbounded validity is an unbounded hold.
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
        if validity > MAXIMUM_RESERVATION_VALIDITY {
            return Err(Error::denied(format!(
                "a reservation may not hold capital for longer than {:.1} hour(s), and {id} \
                 asked for {:.1}; expiry is the only thing that frees a hold nobody commits \
                 or releases, so a longer one is refused rather than truncated — hold for \
                 less, or re-reserve when this one lapses",
                MAXIMUM_RESERVATION_VALIDITY.as_secs_f64() / 3600.0,
                validity.as_secs_f64() / 3600.0
            )));
        }
        // Checked rather than saturating: a saturating expiry lands on
        // `Timestamp::MAX`, which is the sentinel for "no upper bound", and
        // the hold would pin its capital for the rest of the clock's range.
        let Some(expires_at) = now
            .as_nanos()
            .checked_add(validity.as_nanos())
            .map(Timestamp::from_nanos)
        else {
            return Err(Error::numeric(format!(
                "a reservation for {id} taken at {now} and valid for {:.1} hour(s) expires \
                 past the end of the clock; reserve at an instant the expiry can be \
                 represented from",
                validity.as_secs_f64() / 3600.0
            )));
        };
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
                expires_at,
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

impl ReservationLedger {
    /// Re-anchor the free balance to the book's tracked equity.
    ///
    /// The ledger holds capital *between* checks; the equity it holds against
    /// moves with every fill. Two independent claims about one balance is the
    /// standing failure, so the kernel calls this once per sizing pass and the
    /// identity is explicit: free = equity − active holds. Committed capital
    /// does not appear in the identity because a commit spends into the book —
    /// the equity already carries what it became.
    ///
    /// A drawdown can leave the holds exceeding equity. The state that keeps
    /// is the safe one — free goes to zero, so every new reservation is
    /// refused — and the shortfall is returned as an error so the caller puts
    /// it on the record rather than discovering it as a quiet run of refusals.
    pub fn resync_free(&mut self, equity: Decimal, now: Timestamp) -> Result<()> {
        self.expire_due(now);
        let reserved = self.reserved_total();
        let free = equity - reserved;
        if free.is_negative() {
            self.free = Decimal::ZERO;
            return Err(Error::invalid(format!(
                "the active holds ({reserved}) exceed tracked equity ({equity}); the free \
                 balance is floored at zero and new reservations will be refused until holds \
                 expire or release"
            )));
        }
        self.free = free;
        Ok(())
    }
}
