//! Currency at a strategy for a user, and what of it can actually be spent.
//!
//! The failure this file prevents is the one blueprint §43.3 writes into the
//! definition of `ExpectedInflow`: "never available until the ledger says
//! so". A deposit the user says they have sent is a claim; a balance that
//! counted the claim would size positions against money that may never
//! arrive, and the first fill against it would be the platform lending to the
//! user without anyone deciding to. [`CashBalance::available`] is therefore
//! settled cash less reservations and nothing else; expected inflows are
//! held beside the balance, visible, and excluded until
//! [`CashBalance::post_inflow`] is called by whatever reconciled them.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A deposit the user says is on its way.
///
/// Recorded so the desk can see what has been promised and match it when
/// something arrives; counted nowhere a position could be sized against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExpectedInflow {
    pub amount: Decimal,
    pub declared_at: Timestamp,
}

/// One currency's cash at one strategy for one user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CashBalance {
    currency: Currency,
    /// Cash the ledger has said is here: funded, posted, or realised.
    settled: Decimal,
    /// Settled cash held against a proposal that has not resolved.
    reserved: Decimal,
    /// Keyed by the reference the user supplied, so the same claim made
    /// twice is refused rather than counted twice.
    expected: BTreeMap<String, ExpectedInflow>,
}

impl CashBalance {
    pub fn new(currency: Currency) -> Self {
        Self {
            currency,
            settled: Decimal::ZERO,
            reserved: Decimal::ZERO,
            expected: BTreeMap::new(),
        }
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    /// What the ledger has said is here, held or not.
    pub fn settled(&self) -> Decimal {
        self.settled
    }

    pub fn reserved(&self) -> Decimal {
        self.reserved
    }

    /// What could be spent now: settled less reserved. An expected inflow
    /// is not in this number, whatever the user has declared.
    pub fn available(&self) -> Decimal {
        self.settled - self.reserved
    }

    /// The sum of every inflow declared and not yet posted — reported so it
    /// is visible, and never added to anything.
    pub fn expected_total(&self) -> Decimal {
        self.expected.values().map(|inflow| inflow.amount).sum()
    }

    pub fn expected_inflows(&self) -> &BTreeMap<String, ExpectedInflow> {
        &self.expected
    }

    /// Record that the user says a deposit is coming.
    ///
    /// Refuses a non-positive amount, a blank reference, and a reference
    /// already declared: a second declaration under the same reference is
    /// either a retry, which changes nothing, or a different deposit under a
    /// reused reference, which reconciliation could never tell apart.
    pub fn expect_inflow(
        &mut self,
        reference: impl Into<String>,
        amount: Decimal,
        declared_at: Timestamp,
    ) -> Result<()> {
        let reference = reference.into();
        if reference.trim().is_empty() {
            return Err(Error::invalid(
                "an expected inflow needs a reference, or reconciliation has nothing to match",
            ));
        }
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "an expected inflow of {amount} is refused; a deposit is a positive amount"
            )));
        }
        if self.expected.contains_key(&reference) {
            return Err(Error::invalid(format!(
                "an inflow under the reference {reference} is already expected; a second \
                 declaration is either a retry or a reused reference, and the ledger cannot \
                 tell which"
            )));
        }
        self.expected.insert(
            reference,
            ExpectedInflow {
                amount,
                declared_at,
            },
        );
        Ok(())
    }

    /// The ledger says the deposit arrived: move it from expected to settled.
    ///
    /// The one path by which an expectation becomes money. Refuses a
    /// reference nobody declared, because posting an inflow with no
    /// expectation behind it is a credit from nowhere.
    pub fn post_inflow(&mut self, reference: &str) -> Result<Decimal> {
        let Some(inflow) = self.expected.remove(reference) else {
            return Err(Error::denied(format!(
                "no inflow under the reference {reference} was expected; declare it before \
                 posting it, or the credit has no claim behind it"
            )));
        };
        self.settled += inflow.amount;
        Ok(inflow.amount)
    }

    /// The deposit is not coming: drop the expectation. Nothing else moves.
    pub fn cancel_inflow(&mut self, reference: &str) -> Result<Decimal> {
        self.expected
            .remove(reference)
            .map(|inflow| inflow.amount)
            .ok_or_else(|| {
                Error::denied(format!(
                    "no inflow under the reference {reference} was expected, so there is \
                     nothing to cancel"
                ))
            })
    }

    /// Credit settled cash: funding from the mandate, or a positive
    /// attribution. Refuses a non-positive amount; a debit is
    /// [`Self::debit`] and a signed attribution is [`Self::post_attributed`].
    pub fn credit(&mut self, amount: Decimal) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "a credit of {amount} is refused; credit a positive amount or debit instead"
            )));
        }
        self.settled += amount;
        Ok(())
    }

    /// Spend settled cash that no reservation holds.
    ///
    /// Refused, not floored, when `amount` exceeds [`Self::available`], and
    /// the refusal names the expected inflows so the caller sees exactly why
    /// the money they were told about is not here.
    pub fn debit(&mut self, amount: Decimal) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "a debit of {amount} is refused; debit a positive amount or credit instead"
            )));
        }
        let available = self.available();
        if amount > available {
            return Err(Error::denied(format!(
                "a debit of {amount} {} exceeds the {available} available ({} settled, {} \
                 reserved); {} is expected and not available until the ledger posts it",
                self.currency,
                self.settled,
                self.reserved,
                self.expected_total()
            )));
        }
        self.settled -= amount;
        Ok(())
    }

    /// Hold available cash against a proposal.
    pub fn reserve(&mut self, amount: Decimal) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "a reservation of {amount} holds nothing and is refused"
            )));
        }
        let available = self.available();
        if amount > available {
            return Err(Error::denied(format!(
                "reserving {amount} {} exceeds the {available} available; {} is expected and \
                 not available until the ledger posts it",
                self.currency,
                self.expected_total()
            )));
        }
        self.reserved += amount;
        Ok(())
    }

    /// Give a hold back.
    pub fn release(&mut self, amount: Decimal) -> Result<()> {
        if !amount.is_positive() || amount > self.reserved {
            return Err(Error::invalid(format!(
                "releasing {amount} against {} reserved is refused; a release returns part or \
                 all of what is held and nothing more",
                self.reserved
            )));
        }
        self.reserved -= amount;
        Ok(())
    }

    /// Book what the attribution said this strategy realised for this user.
    ///
    /// Signed and unbounded below: a realised loss larger than the settled
    /// cash leaves the balance negative, which is a fact about what happened
    /// and is recorded as one. Flooring it would hide a loss the user owes,
    /// which is the last thing a per-user ledger may do.
    pub fn post_attributed(&mut self, amount: Decimal) {
        self.settled += amount;
    }
}
