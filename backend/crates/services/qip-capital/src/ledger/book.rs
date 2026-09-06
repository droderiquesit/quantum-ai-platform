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
//!
//! [`UserLedger::pro_rata_shares`] produces such a split from the books
//! themselves: each user's part is in proportion to what they have at work
//! at the strategy, computed in integer arithmetic truncated toward zero,
//! and whatever the truncation leaves — at most one unit in the ninth
//! decimal per user — goes to the largest holder and is written into the
//! [`ProRataSplit`] as the `remainder`, so the shares sum to the fill by
//! construction and the unit that was moved is on the record rather than
//! in nobody's book.

use super::cash::CashBalance;
use super::eligibility::{Eligibility, EligibilityRecord, EligibilityRegistry, Ineligible};
use super::identity::{MandateId, UserId};
use super::mandate::Mandate;
use super::registry::MandateRegistry;
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

/// A fill split across users in proportion to what each has at work, with
/// the rounding remainder named rather than dropped.
///
/// `shares` sum to the fill exactly; `remainder` is the part of that sum
/// that truncation had left over and `remainder_to` is who received it —
/// the user with the largest entitlement, and the smaller `UserId` between
/// equals, so two machines splitting one fill agree on the unit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProRataSplit {
    /// In `UserId` order.
    pub shares: Vec<UserShare>,
    /// What every user had at work at the strategy, summed: the basis of
    /// every share.
    pub entitlement_total: Decimal,
    pub remainder: Decimal,
    pub remainder_to: UserId,
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
///
/// There is no empty ledger: one is opened with the desk, because the desk's
/// mandate is the ceiling every other mandate is admitted under and a ledger
/// with no ceiling would admit anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserLedger {
    registry: MandateRegistry,
    /// Who an operator has verified. Defaulted on deserialisation to a
    /// registry in which nobody is eligible, so a ledger stored before
    /// eligibility existed comes back refusing every funding rather than
    /// admitting everyone it used to.
    #[serde(default)]
    eligibility: EligibilityRegistry,
    books: BTreeMap<LedgerKey, StrategyBook>,
    /// Fills journalled across every book, for the same reason each book
    /// counts its own.
    fills_journalled: u64,
}

impl UserLedger {
    /// A ledger with one user — the desk — under [`Mandate::desk`],
    /// registered at the epoch: the desk is the holder that exists before
    /// any registration has a time.
    pub fn with_desk(user: UserId, capital: Decimal, currency: Currency) -> Result<Self> {
        Self::opened_by(
            user,
            Mandate::desk(capital, currency)?,
            Timestamp::from_secs(0),
        )
    }

    /// A ledger whose desk holds the given mandate as the ceiling.
    pub fn opened_by(desk: UserId, mandate: Mandate, at: Timestamp) -> Result<Self> {
        Ok(Self {
            registry: MandateRegistry::new(desk, mandate, at)?,
            eligibility: EligibilityRegistry::new(),
            books: BTreeMap::new(),
            fills_journalled: 0,
        })
    }

    /// Record an operator's eligibility decision about a mandate holder.
    ///
    /// Refuses a decision about a user who holds no mandate — eligibility is
    /// a statement about whose capital may be put to work, and a record
    /// about nobody's capital is one nothing can read — and whatever
    /// [`EligibilityRegistry::decide`] refuses. The operator is in the
    /// record; there is no way to call this with a bare flag.
    pub fn decide_eligibility(&mut self, record: EligibilityRecord) -> Result<()> {
        if self.registry.mandate(&record.user).is_none() {
            return Err(Error::denied(format!(
                "{} holds no mandate; register the mandate before deciding eligibility, or \
                 the decision is about nobody's capital",
                record.user
            )));
        }
        self.eligibility.decide(record)
    }

    /// The standing eligibility decisions.
    pub fn eligibility(&self) -> &EligibilityRegistry {
        &self.eligibility
    }

    /// Whether a user may have capital put to work at `now`, by name.
    ///
    /// The mandate first — its jurisdiction is what the eligibility is
    /// checked against — then [`EligibilityRegistry::admit`].
    pub fn eligibility_of(
        &self,
        user: &UserId,
        now: Timestamp,
    ) -> std::result::Result<&Eligibility, Ineligible> {
        let Some(mandate) = self.registry.mandate(user) else {
            return Err(Ineligible::NoMandate);
        };
        self.eligibility.admit(user, mandate.jurisdiction(), now)
    }

    /// Give a user a mandate, under the desk's ceiling. Every refusal is
    /// [`MandateRegistry::register`]'s: an id seen before, a user who
    /// already holds one, or a term the desk's mandate does not carry.
    pub fn enrol(
        &mut self,
        user: UserId,
        id: MandateId,
        mandate: Mandate,
        at: Timestamp,
    ) -> Result<()> {
        self.registry.register(user, id, mandate, at)
    }

    pub fn registry(&self) -> &MandateRegistry {
        &self.registry
    }

    pub fn desk(&self) -> &UserId {
        self.registry.desk()
    }

    pub fn mandate(&self, user: &UserId) -> Option<&Mandate> {
        self.registry.mandate(user)
    }

    pub fn mandates(&self) -> &BTreeMap<UserId, Mandate> {
        self.registry.mandates()
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
    pub(super) fn funded_total(&self, user: &UserId, currency: Currency) -> Decimal {
        self.books
            .iter()
            .filter(|((owner, _), _)| owner == user)
            .filter_map(|(_, book)| book.cash(currency))
            .map(CashBalance::settled)
            .sum()
    }

    /// What a user has settled at one strategy in one currency; zero for a
    /// book that does not exist.
    pub(super) fn funded_at(
        &self,
        user: &UserId,
        strategy: &StrategyId,
        currency: Currency,
    ) -> Decimal {
        self.balance(user, strategy, currency)
            .map_or(Decimal::ZERO, CashBalance::settled)
    }

    /// Move capital from a user's mandate into one strategy's book.
    ///
    /// Refuses a user without a mandate, a user the eligibility registry
    /// does not admit at `at` (by the [`Ineligible`] reason named), a
    /// non-positive amount, and any amount that would take the user's
    /// settled total across strategies past the mandate's investable
    /// capital — the liquidity floor and the eligibility are both honoured
    /// here, at the one place capital enters a book, rather than checked by
    /// every caller. A caller that consulted the registry first gets the
    /// same answer twice; one that did not is refused all the same.
    pub fn fund(
        &mut self,
        user: &UserId,
        strategy: &StrategyId,
        amount: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        let Some(mandate) = self.registry.mandate(user) else {
            return Err(Error::denied(format!(
                "{user} holds no mandate; enrol one before funding a strategy"
            )));
        };
        if let Err(why) = self.eligibility.admit(user, mandate.jurisdiction(), at) {
            return Err(Error::denied(why.describe(user)));
        }
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
            if self.registry.mandate(&share.user).is_none() {
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

    /// Split an attributed fill across the users with capital at work at
    /// its strategy, in proportion to what each has there, exactly.
    ///
    /// Each share is `fill × entitlement ÷ total` in integer arithmetic on
    /// the raw units, truncated toward zero, so no share is rounded up past
    /// its proportion. The truncations leave a remainder of at most one raw
    /// unit per user; it is assigned whole to the largest entitlement
    /// (smaller `UserId` between equals) and recorded in the split, so the
    /// shares sum to the fill by construction and the moved unit is on the
    /// record. Users whose settled cash at the strategy is not positive
    /// hold no entitlement and take no share — a negative book is a loss
    /// owed, not a claim on the next gain. Refused when nobody has capital
    /// at the strategy: a fill with no entitlement behind it is booked to
    /// the desk explicitly through [`Self::journal_to`], never silently.
    /// Refused on overflow rather than approximated.
    pub fn pro_rata_shares(&self, fill: &AttributedFill) -> Result<ProRataSplit> {
        let entitlements: Vec<(UserId, Decimal)> = self
            .books
            .iter()
            .filter(|((_, strategy), _)| *strategy == fill.strategy)
            .filter_map(|((user, _), book)| {
                book.cash(fill.currency)
                    .map(CashBalance::settled)
                    .filter(|settled| settled.is_positive())
                    .map(|settled| (user.clone(), settled))
            })
            .collect();
        if entitlements.is_empty() {
            return Err(Error::denied(format!(
                "no user has {} at work at {}, so the attributed fill on {} has no \
                 entitlement to split across; book it to the desk explicitly",
                fill.currency, fill.strategy, fill.source
            )));
        }
        let entitlement_total: Decimal = entitlements.iter().map(|(_, held)| *held).sum();
        let overflow = || {
            Error::numeric(format!(
                "splitting {} {} on {} across {} entitlements totalling {entitlement_total} \
                 overflows the ledger's arithmetic; the fill is refused rather than \
                 approximated",
                fill.amount,
                fill.currency,
                fill.source,
                entitlements.len()
            ))
        };
        let mut shares = Vec::with_capacity(entitlements.len());
        let mut allotted = Decimal::ZERO;
        for (user, held) in &entitlements {
            // Integer division truncates toward zero, in the raw units of
            // the fixed-point representation; every rounding that happens
            // here is downward in magnitude and named in the remainder.
            let scaled = fill
                .amount
                .raw()
                .checked_mul(held.raw())
                .ok_or_else(overflow)?;
            let amount = Decimal::from_raw(scaled / entitlement_total.raw());
            allotted = allotted.checked_add(amount).ok_or_else(overflow)?;
            shares.push(UserShare {
                user: user.clone(),
                amount,
            });
        }
        let remainder = fill.amount.checked_sub(allotted).ok_or_else(overflow)?;
        // The largest entitlement; `entitlements` is in `UserId` order, and
        // `max_by` returns the last maximum, so the comparison is reversed to
        // land on the first — the smaller id — between equals.
        let remainder_to = entitlements
            .iter()
            .rev()
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .map(|(user, _)| user.clone())
            .ok_or_else(|| {
                Error::invalid("an entitlement list proven non-empty has no largest entry")
            })?;
        if let Some(share) = shares.iter_mut().find(|share| share.user == remainder_to) {
            share.amount = share.amount.checked_add(remainder).ok_or_else(overflow)?;
        }
        Ok(ProRataSplit {
            shares,
            entitlement_total,
            remainder,
            remainder_to,
        })
    }

    /// Split an attributed fill pro rata and book it, returning the split
    /// so the remainder's destination is on the record with the entry.
    pub fn journal_pro_rata(
        &mut self,
        fill: &AttributedFill,
        at: Timestamp,
    ) -> Result<ProRataSplit> {
        let split = self.pro_rata_shares(fill)?;
        self.journal(fill, &split.shares, at)?;
        Ok(split)
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
        let Some(mandate) = self.registry.mandate(user) else {
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
        let Some(mandate) = self.registry.mandate(user) else {
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
