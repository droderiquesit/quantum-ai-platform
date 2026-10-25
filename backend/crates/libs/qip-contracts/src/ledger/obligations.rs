//! What the ledger of record says is withheld from deployable capital:
//! margin booked, collateral pledged, and cash owed and not yet settled
//! (R12-CAPITAL-4087). It lives in `qip-contracts` rather than `qip-capital`
//! (R12-CAPITAL-1099) because the ledger brain, `qip-ledgerd`, produces it:
//! ADR 0127 lets one brain meet another only through contracts, and the
//! ledger depending on the capital brain for its own read model was the one
//! edge that broke that.
//!
//! The survival waterfall in `qip-kernel` withholds margin, collateral and
//! settlement before it releases anything to opportunity. Until this module
//! its only source for the three was the cross-margin review, which can see
//! a margin requirement on the gross it carries and nothing else, so
//! collateral and settlement were derived as zero on every path — true of
//! the review, and a statement about the ledger that nobody had checked. A
//! pledge booked in `qip-ledgerd` left the trading account and the waterfall
//! released it to opportunity anyway; an accrual owed to a counterparty was
//! sized against as though it were cash.
//!
//! [`LedgerObligations`] is read from the ledger's own balances — the
//! `(Account, unit) -> debits minus credits` map that
//! `qip_ledgerd::LedgerStore::balances` returns — and nothing else, so the
//! figure is a fold of booked postings rather than a number a caller typed
//! in.
//!
//! # What each figure is, and what it is not
//!
//! * **Margin** is the sum of the `margin:<venue>` debit balances: what
//!   venues have required and the ledger has booked against an obligation.
//! * **Collateral** is the sum of the `collateral:<counterparty>` debit
//!   balances: units pledged out of a book. The kernel withholds only the
//!   part of it the margin does not already account for, so the same money
//!   is never withheld twice — that netting is the waterfall's, not this
//!   module's, because only the waterfall sees both claims.
//! * **Settlement** is the sum of the `accrual:<type>` **credit** balances:
//!   amounts the ledger has booked as owed over time and not yet cleared by
//!   a cash settlement (LEDGER-012). A debit accrual — something owed *to*
//!   the platform — is not counted and is not netted against what is owed:
//!   a receivable is not cash, and netting it would release capital on a
//!   promise.
//!
//! The ledger books every fill under `Settlement::Simulated`, which settles
//! at the fill, so an executed trade owes nothing later and contributes no
//! settlement here. A venue on a T+n cycle would book its unsettled cash as
//! an accrual and be counted; nothing is inferred for one that does not.
//!
//! # Refusals
//!
//! A negative margin or collateral balance is refused: the ledger itself
//! refuses to take a quantity-holding account below zero, so one here is a
//! defect, and reading it would release capital. An obligation in a unit
//! other than the one the waterfall sizes in is refused rather than skipped:
//! skipping it reads it as zero, and this view holds no conversion to say
//! what it is worth.

use super::Account;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Margin, collateral and unsettled cash, folded from ledger balances in one
/// unit at one instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerObligations {
    unit: String,
    as_of: Timestamp,
    margin: Decimal,
    collateral: Decimal,
    settlement: Decimal,
}

impl LedgerObligations {
    /// Fold the three figures out of `balances` (debits minus credits per
    /// account and unit), in `unit`, as the ledger stood at `as_of`.
    pub fn from_balances(
        balances: &BTreeMap<(Account, String), Decimal>,
        unit: &str,
        as_of: Timestamp,
    ) -> Result<Self> {
        if unit.trim().is_empty() {
            return Err(Error::invalid(
                "ledger obligations need the unit the survival waterfall sizes in",
            ));
        }
        let mut margin = Decimal::ZERO;
        let mut collateral = Decimal::ZERO;
        let mut settlement = Decimal::ZERO;
        for ((account, account_unit), balance) in balances {
            let (name, total, amount) = match account {
                Account::Margin { .. } => ("margin", &mut margin, *balance),
                Account::Collateral { .. } => ("collateral", &mut collateral, *balance),
                // Only what is owed: a credit balance, read as a positive
                // amount. A receivable is skipped, never netted.
                Account::Accrual { .. } if balance.is_negative() => {
                    ("settlement", &mut settlement, -*balance)
                }
                _ => continue,
            };
            if amount.is_zero() {
                continue;
            }
            if amount.is_negative() {
                return Err(Error::invalid(format!(
                    "the ledger holds {amount} {account_unit} at {account}; a {name} account \
                     below zero would release capital the survival waterfall withholds, and \
                     the ledger refuses to post one, so reconcile it before sizing"
                )));
            }
            if account_unit != unit {
                return Err(Error::invalid(format!(
                    "the ledger holds a {name} obligation of {amount} {account_unit} at \
                     {account}, and the survival waterfall sizes in {unit}; this view holds no \
                     conversion, and skipping it would read it as zero"
                )));
            }
            *total = total.checked_add(amount).ok_or_else(|| {
                Error::numeric(format!("the ledger's {name} obligations overflow"))
            })?;
        }
        Ok(Self {
            unit: unit.to_string(),
            as_of,
            margin,
            collateral,
            settlement,
        })
    }

    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// The instant the balances were read at.
    pub const fn as_of(&self) -> Timestamp {
        self.as_of
    }

    /// Margin the ledger has booked as required.
    pub const fn margin(&self) -> Decimal {
        self.margin
    }

    /// Units pledged as collateral out of the books.
    pub const fn collateral(&self) -> Decimal {
        self.collateral
    }

    /// Cash owed and not yet settled.
    pub const fn settlement(&self) -> Decimal {
        self.settlement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Decimal {
        Decimal::parse(text).unwrap()
    }

    fn at() -> Timestamp {
        Timestamp::from_secs(1_700_000_000)
    }

    fn balances(rows: &[(Account, &str, &str)]) -> BTreeMap<(Account, String), Decimal> {
        rows.iter()
            .map(|(account, unit, amount)| ((account.clone(), unit.to_string()), d(amount)))
            .collect()
    }

    fn collateral(counterparty: &str) -> Account {
        Account::Collateral {
            counterparty: counterparty.to_string(),
        }
    }

    fn accrual(kind: &str) -> Account {
        Account::Accrual {
            accrual_type: kind.to_string(),
        }
    }

    #[test]
    fn pledged_collateral_and_owed_accruals_are_folded_and_a_receivable_is_not_netted() {
        let book = balances(&[
            (
                Account::Margin {
                    venue: "XLON".to_string(),
                },
                "USD",
                "300",
            ),
            (collateral("prime"), "USD", "500"),
            (collateral("clearer"), "USD", "250"),
            (accrual("funding"), "USD", "-120"),
            // Owed to the platform: skipped, so the 120 owed is not reduced.
            (accrual("interest"), "USD", "80"),
            // Not an obligation account at all.
            (
                Account::Trading {
                    cell: "c".to_string(),
                    strategy: "s".to_string(),
                },
                "USD",
                "-10000",
            ),
        ]);
        let folded = LedgerObligations::from_balances(&book, "USD", at()).unwrap();
        assert_eq!(folded.margin(), d("300"));
        assert_eq!(folded.collateral(), d("750"));
        assert_eq!(folded.settlement(), d("120"));
    }

    #[test]
    fn an_obligation_in_another_unit_or_below_zero_is_refused_rather_than_read_as_zero() {
        let premise = balances(&[(collateral("prime"), "USD", "500")]);
        assert!(LedgerObligations::from_balances(&premise, "USD", at()).is_ok());

        let foreign = balances(&[(collateral("prime"), "EUR", "500")]);
        let refused = LedgerObligations::from_balances(&foreign, "USD", at()).unwrap_err();
        assert!(refused.message().contains("no conversion"), "{refused}");

        let negative = balances(&[(collateral("prime"), "USD", "-1")]);
        let refused = LedgerObligations::from_balances(&negative, "USD", at()).unwrap_err();
        assert!(refused.message().contains("below zero"), "{refused}");
    }
}
