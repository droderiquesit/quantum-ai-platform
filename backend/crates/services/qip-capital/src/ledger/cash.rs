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
//!
//! # Custody of the record: a balance serialises out and does not come back
//!
//! This platform holds no capital, so the only custody it can enforce is
//! custody of the record — the set of ways a number can appear in a user's
//! book must be closed, named, and every one of them gated. There are five,
//! all on [`super::UserLedger`]: `fund`, `journal`, `journal_to`,
//! `journal_pro_rata` and `post_inflow`. Each asks the mandate registry, the
//! eligibility registry, the investable ceiling or the exact-split rule
//! before it moves anything.
//!
//! There used to be a sixth, and nothing gated it. `CashBalance` derived
//! `Deserialize`, so
//! `serde_json::from_str::<CashBalance>(r#"{"currency":"USD","settled":"999999999",…}"#)`
//! returned a settled balance of any size that no funding, no operator's
//! eligibility decision, no product catalogue and no attributed fill had
//! produced. `ledger/entitlement.rs` states the rule that closes it — "a
//! record is evidence of what was decided, never an input that decides" — and
//! applies it to the entitlement while the money next door was reading itself
//! back in from a document. ADR 0021 refuses the path by which capital leaves
//! this platform; nothing in it permits a path by which capital arrives from a
//! file.
//!
//! So the money types here serialise and do not deserialise, and the guarantee
//! is the type system's rather than a check's:
//!
//! ```compile_fail
//! # use qip_capital::ledger::CashBalance;
//! // There is no `Deserialize for CashBalance`, and by the orphan rules no
//! // crate but this one can add it. A balance minted from a document is
//! // unrepresentable rather than refused.
//! let minted: CashBalance = serde_json::from_str(
//!     r#"{"currency":"USD","settled":"999999999","reserved":"0","expected":{}}"#,
//! )
//! .expect("a document is not a balance");
//! ```
//!
//! Serialising out still works, because a report of what the ledger holds is
//! the whole point of holding it:
//!
//! ```
//! # use qip_capital::ledger::CashBalance;
//! # use qip_core::Currency;
//! let empty = CashBalance::new(Currency::USD);
//! let document = serde_json::to_string(&empty).expect("a balance reports");
//! assert!(document.contains(r#""settled":"0""#));
//! ```

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Timestamp};
use serde::Serialize;
use std::collections::BTreeMap;

/// A deposit the user says is on its way.
///
/// Recorded so the desk can see what has been promised and match it when
/// something arrives; counted nowhere a position could be sized against.
///
/// Serialises and does not deserialise, for the reason this module's own
/// documentation gives: an inflow reaches a book through
/// [`CashBalance::expect_inflow`], which refuses a blank reference, a
/// non-positive amount and a reference already declared. A document that
/// wrote one straight into the map would meet none of those.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpectedInflow {
    pub amount: Decimal,
    pub declared_at: Timestamp,
}

/// One currency's cash at one strategy for one user.
///
/// Serialises and does not deserialise — see this module's documentation for
/// the way in that closed.
#[derive(Clone, Debug, PartialEq, Serialize)]
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
