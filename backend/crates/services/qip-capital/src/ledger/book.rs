//! The books themselves: one per `(user, strategy)`, fed from the attribution.
//!
//! The entry the ledger takes is an [`AttributedFill`] — the amount the
//! centre's exact attribution said one strategy realised on one settled
//! position (ADR 0007: the decomposition closes to the last unit or the
//! settlement is refused, so what arrives here has already been proven to
//! add up). It is journalled here and not the fill record itself, because
//! the fill record is the cell's claim and the attribution is what the
//! centre accepted; a ledger fed from the claim would book fills the centre
//! refused, and two books would disagree about one event.
//!
//! Splitting a fill across users is where a second residual could appear,
//! so the split is checked the way the attribution is: the [`UserShare`]s
//! must sum to the fill's amount exactly, every user must hold a mandate,
//! and no user may appear twice. Any of those failing refuses the fill whole
//! — no book moves — because a fill half-booked is a fill nobody can find.

use super::cash::CashBalance;
use super::identity::UserId;
use super::mandate::Mandate;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the attribution said one strategy realised, ready to be booked to
/// whoever's capital it was trading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttributedFill {
    pub strategy: StrategyId,
    /// The attributed position this came from — the centre writes
    /// `cell/strategy/instrument` — so the entry can be traced back down the
    /// chain to the lot and from there to the fill.
    pub source: String,
    pub currency: Currency,
    /// Signed. A loss is booked as a loss.
    pub amount: Decimal,
}

/// One user's part of an attributed fill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserShare {
    pub user: UserId,
    pub amount: Decimal,
}

/// The key a book lives under. Ordered, so a report of every book comes out
/// the same on every machine.
pub type LedgerKey = (UserId, StrategyId);

/// One user's books at one strategy, by currency.
///
/// Carries counts rather than the entries themselves: the event log is the
/// record, and a ledger that kept every entry in memory would be the
/// unbounded working set the retention rule forbids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyBook {
    cash: BTreeMap<Currency, CashBalance>,
    /// Attributed fills booked here, so "none were booked" and "the balance
    /// happens to be zero" are different answers.
    entries: u64,
    last_entry_at: Option<Timestamp>,
}

impl StrategyBook {
    fn new() -> Self {
        Self {
            cash: BTreeMap::new(),
            entries: 0,
            last_entry_at: None,
        }
    }

    pub fn cash(&self, currency: Currency) -> Option<&CashBalance> {
        self.cash.get(&currency)
    }

    pub fn balances(&self) -> &BTreeMap<Currency, CashBalance> {
        &self.cash
    }

    pub fn entries(&self) -> u64 {
        self.entries
    }

    pub fn last_entry_at(&self) -> Option<Timestamp> {
        self.last_entry_at
    }

    fn cash_mut(&mut self, currency: Currency) -> &mut CashBalance {
        self.cash
            .entry(currency)
            .or_insert_with(|| CashBalance::new(currency))
    }
}

/// Per-user, per-strategy money state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UserLedger {
    mandates: BTreeMap<UserId, Mandate>,
    books: BTreeMap<LedgerKey, StrategyBook>,
    /// Fills journalled across every book, for the same reason each book
    /// counts its own.
    fills_journalled: u64,
}

impl UserLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// A ledger with one user — the desk — under [`Mandate::desk`].
    pub fn with_desk(user: UserId, capital: Decimal, currency: Currency) -> Result<Self> {
        let mut ledger = Self::new();
        ledger.enrol(user, Mandate::desk(capital, currency)?)?;
        Ok(ledger)
    }

    /// Give a user a mandate. Refuses a user who already has one: a
    /// mandate replaced in place is a mandate whose old terms nobody can
    /// recover, and the terms a fill was booked under are part of the
    /// attribution.
    pub fn enrol(&mut self, user: UserId, mandate: Mandate) -> Result<()> {
        if self.mandates.contains_key(&user) {
            return Err(Error::invalid(format!(
                "{user} already holds a mandate; a mandate is not replaced in place — \
                 record a new one under the change that supersedes it"
            )));
        }
        self.mandates.insert(user, mandate);
        Ok(())
    }

    pub fn mandate(&self, user: &UserId) -> Option<&Mandate> {
        self.mandates.get(user)
    }

    pub fn mandates(&self) -> &BTreeMap<UserId, Mandate> {
        &self.mandates
    }

    pub fn books(&self) -> &BTreeMap<LedgerKey, StrategyBook> {
        &self.books
    }

    pub fn book(&self, user: &UserId, strategy: &StrategyId) -> Option<&StrategyBook> {
        self.books.get(&(user.clone(), strategy.clone()))
    }

    pub fn balance(
        &self,
        user: &UserId,
        strategy: &StrategyId,
        currency: Currency,
    ) -> Option<&CashBalance> {
        self.book(user, strategy)
            .and_then(|book| book.cash(currency))
    }

    pub fn fills_journalled(&self) -> u64 {
        self.fills_journalled
    }

    /// Everything a user has funded into strategies, in the mandate's
    /// currency, across every book.
    fn funded_total(&self, user: &UserId, currency: Currency) -> Decimal {
        self.books
            .iter()
            .filter(|((owner, _), _)| owner == user)
            .filter_map(|(_, book)| book.cash(currency))
            .map(CashBalance::settled)
            .sum()
    }

    /// Move capital from a user's mandate into one strategy's book.
    ///
    /// Refuses a user without a mandate, a currency other than the
    /// mandate's, a non-positive amount, and any amount that would take the
    /// user's settled total across strategies past the mandate's investable
    /// capital — the liquidity floor is honoured here, at the one place
    /// capital enters a book, rather than checked by every reader.
    pub fn fund(
        &mut self,
        user: &UserId,
        strategy: &StrategyId,
        amount: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        let Some(mandate) = self.mandates.get(user) else {
            return Err(Error::denied(format!(
                "{user} holds no mandate; enrol one before funding a strategy"
            )));
        };
        let currency = mandate.currency();
        let investable = mandate.investable();
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "funding {strategy} with {amount} for {user} is refused; fund a positive amount"
            )));
        }
        let funded = self.funded_total(user, currency);
        if funded + amount > investable {
            return Err(Error::denied(format!(
                "funding {strategy} with {amount} {currency} would take {user}'s funded \
                 total from {funded} past the {investable} investable under the mandate ({} \
                 capital less a {} liquidity floor)",
                mandate.capital(),
                mandate.liquidity_floor()
            )));
        }
        let book = self
            .books
            .entry((user.clone(), strategy.clone()))
            .or_insert_with(StrategyBook::new);
        book.cash_mut(currency).credit(amount)?;
        book.last_entry_at = Some(at);
        Ok(())
    }

    /// Book an attributed fill to the users whose capital it was, exactly.
    ///
    /// Validates the whole split before any book moves: at least one share,
    /// every user enrolled, no user twice, and the shares summing to the
    /// fill's amount to the last unit. A split that fails any of these is
    /// refused with the difference named, and the ledger is as it was — the
    /// zero-residual rule of ADR 0007 applied one link further down the
    /// chain.
    pub fn journal(
        &mut self,
        fill: &AttributedFill,
        shares: &[UserShare],
        at: Timestamp,
    ) -> Result<()> {
        if shares.is_empty() {
            return Err(Error::invalid(format!(
                "the attributed fill of {} {} on {} names no user; a fill booked to nobody is \
                 a quantity nobody is attributed",
                fill.amount, fill.currency, fill.source
            )));
        }
        for (index, share) in shares.iter().enumerate() {
            if !self.mandates.contains_key(&share.user) {
                return Err(Error::denied(format!(
                    "the attributed fill on {} names {}, who holds no mandate; nothing was \
                     booked",
                    fill.source, share.user
                )));
            }
            if shares[..index]
                .iter()
                .any(|earlier| earlier.user == share.user)
            {
                return Err(Error::invalid(format!(
                    "the attributed fill on {} names {} twice; merge the shares before \
                     journalling, or one user is booked two amounts for one fill",
                    fill.source, share.user
                )));
            }
        }
        let shared: Decimal = shares.iter().map(|share| share.amount).sum();
        if shared != fill.amount {
            return Err(Error::invalid(format!(
                "the {} user share(s) of the attributed fill on {} sum to {shared} against an \
                 attributed {}; the difference of {} is an amount nobody is attributed, and \
                 the fill is refused rather than booked short",
                shares.len(),
                fill.source,
                fill.amount,
                fill.amount - shared
            )));
        }
        for share in shares {
            let book = self
                .books
                .entry((share.user.clone(), fill.strategy.clone()))
                .or_insert_with(StrategyBook::new);
            book.cash_mut(fill.currency).post_attributed(share.amount);
            book.entries += 1;
            book.last_entry_at = Some(at);
        }
        self.fills_journalled += 1;
        Ok(())
    }

    /// Book an attributed fill to one user whole — the desk, until users
    /// exist.
    pub fn journal_to(
        &mut self,
        user: &UserId,
        fill: &AttributedFill,
        at: Timestamp,
    ) -> Result<()> {
        self.journal(
            fill,
            &[UserShare {
                user: user.clone(),
                amount: fill.amount,
            }],
            at,
        )
    }

    /// Record a deposit the user says they have sent, against one
    /// strategy's book. Not available until [`Self::post_inflow`].
    pub fn expect_inflow(
        &mut self,
        user: &UserId,
        strategy: &StrategyId,
        reference: impl Into<String>,
        amount: Decimal,
        declared_at: Timestamp,
    ) -> Result<()> {
        let Some(mandate) = self.mandates.get(user) else {
            return Err(Error::denied(format!(
                "{user} holds no mandate; enrol one before declaring an inflow"
            )));
        };
        let currency = mandate.currency();
        self.books
            .entry((user.clone(), strategy.clone()))
            .or_insert_with(StrategyBook::new)
            .cash_mut(currency)
            .expect_inflow(reference, amount, declared_at)
    }

    /// The ledger says the deposit arrived.
    pub fn post_inflow(
        &mut self,
        user: &UserId,
        strategy: &StrategyId,
        reference: &str,
        at: Timestamp,
    ) -> Result<Decimal> {
        let Some(mandate) = self.mandates.get(user) else {
            return Err(Error::denied(format!(
                "{user} holds no mandate, so no inflow can be posted to them"
            )));
        };
        let currency = mandate.currency();
        let Some(book) = self.books.get_mut(&(user.clone(), strategy.clone())) else {
            return Err(Error::denied(format!(
                "{user} has no book at {strategy}, so no inflow under {reference} was expected"
            )));
        };
        let posted = book.cash_mut(currency).post_inflow(reference)?;
        book.last_entry_at = Some(at);
        Ok(posted)
    }
}
