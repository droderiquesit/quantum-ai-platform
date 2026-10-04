//! The Capital Brain's own books: where every unit of liquidity sits, how much
//! must stay unspent, how much leverage a strategy may carry, which book owes
//! which, and which financing function an entity may use at all.
//!
//! Four small controls share this module because each one is a refusal that
//! was missing and each guards the same fact: capital the platform does not
//! have must not be promised.
//!
//! * [`CapitalBook`] (CAPITAL-005, CAPITAL-021). Total liquidity is divided
//!   into assigned uses and an uncommitted reserve. The reserve is *derived*
//!   (`total - assigned`), never stored, so no unit can be unassigned or
//!   assigned twice by construction. A commitment that would take the reserve
//!   below the floor is refused, not trimmed to fit: a grant quietly shrunk to
//!   fit is a grant whose holder believes they hold more than they do.
//! * [`LeverageBook`] (CAPITAL-004). A leverage ceiling is an explicit,
//!   journaled decision per strategy and region. An allocation with no
//!   decision is refused (fail closed), because "no decision" must not read as
//!   "unbounded".
//! * [`InternalFunding`] (CAPITAL-014). A funding between two of the
//!   platform's books is a receivable on one side and a payable of the same
//!   amount on the other, and only a recorded repayment reduces either.
//! * [`FinancingPermissions`] (CAPITAL-019). A regulated function runs only
//!   for an (entity, counterparty, jurisdiction) with a recorded permission;
//!   the empty registry refuses everything.
//!
//! Nothing here moves money or contacts a counterparty; these are the
//! controls a mover must pass first.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What committed liquidity is being used for. The fourth use in the
/// requirement, the uncommitted reserve, is the remainder and so has no
/// variant: it cannot be assigned to, only left over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Use {
    /// Committed under a capital grant.
    Grant,
    /// Placed at a venue or region ahead of demand.
    Placement,
    /// Posted as collateral.
    Collateral,
    /// Routed to idle-cash yield.
    Yield,
}

/// Total liquidity, the floor that must stay uncommitted, and every
/// assignment against it.
#[derive(Debug, Clone)]
pub struct CapitalBook {
    total: Decimal,
    floor: Decimal,
    assigned: BTreeMap<String, (Use, Decimal)>,
}

impl CapitalBook {
    /// Open a book. A floor above the total could never be met, so it is
    /// refused rather than leaving a book that refuses everything silently.
    pub fn new(total: Decimal, floor: Decimal) -> Result<Self> {
        if total.is_negative() || floor.is_negative() {
            return Err(Error::invalid(
                "total liquidity and the uncommitted floor must not be negative; \
                 reconcile the balances before opening the book",
            ));
        }
        if floor > total {
            return Err(Error::invalid(format!(
                "the uncommitted floor {floor} exceeds total liquidity {total}; \
                 lower the floor or fund the book first"
            )));
        }
        Ok(Self {
            total,
            floor,
            assigned: BTreeMap::new(),
        })
    }

    /// Total liquidity the book divides.
    pub fn total(&self) -> Decimal {
        self.total
    }

    /// Sum of everything assigned to a use.
    pub fn committed(&self) -> Decimal {
        self.assigned
            .values()
            .fold(Decimal::ZERO, |acc, (_, amount)| acc + *amount)
    }

    /// Liquidity assigned to no use: the uncommitted reserve.
    pub fn uncommitted(&self) -> Decimal {
        self.total - self.committed()
    }

    /// Amount assigned to one use.
    pub fn assigned_to(&self, kind: Use) -> Decimal {
        self.assigned
            .values()
            .filter(|(u, _)| *u == kind)
            .fold(Decimal::ZERO, |acc, (_, amount)| acc + *amount)
    }

    /// Commit liquidity under a unique id. Refused, never reduced, when it
    /// would leave less than the floor uncommitted.
    pub fn commit(&mut self, id: &str, kind: Use, amount: Decimal) -> Result<()> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "commitment {id} must be a positive amount, got {amount}"
            )));
        }
        if self.assigned.contains_key(id) {
            return Err(Error::invalid(format!(
                "commitment {id} is already assigned; release it before assigning \
                 the same capital to another use"
            )));
        }
        let remaining = self.uncommitted() - amount;
        if remaining < self.floor {
            return Err(Error::denied(format!(
                "commitment {id} of {amount} would leave {remaining} uncommitted, below the \
                 floor of {}; commit less, release something, or have the Capital Brain \
                 lower the floor",
                self.floor
            )));
        }
        self.assigned.insert(id.to_owned(), (kind, amount));
        Ok(())
    }

    /// Return a commitment to the uncommitted reserve.
    pub fn release(&mut self, id: &str) -> Result<Decimal> {
        self.assigned
            .remove(id)
            .map(|(_, amount)| amount)
            .ok_or_else(|| Error::not_found(format!("no commitment {id} to release")))
    }
}

/// One recorded leverage decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeverageDecision {
    /// Strategy the ceiling applies to.
    pub strategy: String,
    /// Region the ceiling applies to.
    pub region: String,
    /// Maximum gross notional per unit of capital.
    pub max_leverage: Decimal,
    /// When the decision was taken.
    pub decided_at: Timestamp,
}

/// Leverage decisions, append-only, latest per (strategy, region) in force.
#[derive(Debug, Clone, Default)]
pub struct LeverageBook {
    journal: Vec<LeverageDecision>,
}

impl LeverageBook {
    /// Empty book: every allocation is refused until a decision exists.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every decision ever recorded, in order, for the event log.
    pub fn journal(&self) -> &[LeverageDecision] {
        &self.journal
    }

    /// Record a decision. A ceiling of zero or less would forbid all use of
    /// the capital and is refused as a likely mistake.
    pub fn decide(
        &mut self,
        strategy: &str,
        region: &str,
        max_leverage: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if !max_leverage.is_positive() {
            return Err(Error::invalid(format!(
                "leverage for {strategy}/{region} must be positive, got {max_leverage}; \
                 withhold the allocation instead of deciding a zero ceiling"
            )));
        }
        self.journal.push(LeverageDecision {
            strategy: strategy.to_owned(),
            region: region.to_owned(),
            max_leverage,
            decided_at: at,
        });
        Ok(())
    }

    /// The decision currently in force.
    pub fn current(&self, strategy: &str, region: &str) -> Option<&LeverageDecision> {
        self.journal
            .iter()
            .rev()
            .find(|d| d.strategy == strategy && d.region == region)
    }

    /// Refuse an allocation whose implied leverage (`notional / capital`)
    /// exceeds the decision, or for which no decision exists.
    pub fn check(
        &self,
        strategy: &str,
        region: &str,
        notional: Decimal,
        capital: Decimal,
    ) -> Result<()> {
        let decision = self.current(strategy, region).ok_or_else(|| {
            Error::denied(format!(
                "no leverage decision recorded for {strategy}/{region}; the Capital Brain \
                 must decide one before capital is allocated"
            ))
        })?;
        if !capital.is_positive() {
            return Err(Error::invalid(format!(
                "allocation for {strategy}/{region} carries no capital, so its leverage is undefined"
            )));
        }
        // Compare by multiplication so no rounding in a quotient can admit a breach.
        let ceiling = decision
            .max_leverage
            .checked_mul(capital)
            .ok_or_else(|| Error::numeric("leverage ceiling overflowed"))?;
        if notional > ceiling {
            return Err(Error::denied(format!(
                "allocation for {strategy}/{region} implies notional {notional} on capital \
                 {capital}, above the recorded leverage of {}; reduce the notional",
                decision.max_leverage
            )));
        }
        Ok(())
    }
}

/// One internal funding and what is still owed on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    /// Book that lent the capital (holds the receivable).
    pub funder: String,
    /// Book that received it (holds the payable).
    pub funded: String,
    /// Amount originally funded.
    pub principal: Decimal,
    /// Amount repaid so far.
    pub repaid: Decimal,
}

impl Obligation {
    /// What remains owed.
    pub fn outstanding(&self) -> Decimal {
        self.principal - self.repaid
    }
}

/// Internal fundings between the platform's own books.
#[derive(Debug, Clone, Default)]
pub struct InternalFunding {
    obligations: BTreeMap<String, Obligation>,
}

impl InternalFunding {
    /// Empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a funding: one receivable and one payable of the same amount.
    pub fn fund(&mut self, id: &str, funder: &str, funded: &str, amount: Decimal) -> Result<()> {
        if funder == funded {
            return Err(Error::invalid(format!(
                "funding {id} names {funder} on both sides; a book cannot owe itself"
            )));
        }
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "funding {id} must be a positive amount, got {amount}"
            )));
        }
        if self.obligations.contains_key(id) {
            return Err(Error::invalid(format!(
                "funding {id} already exists; repay it or use a new id"
            )));
        }
        self.obligations.insert(
            id.to_owned(),
            Obligation {
                funder: funder.to_owned(),
                funded: funded.to_owned(),
                principal: amount,
                repaid: Decimal::ZERO,
            },
        );
        Ok(())
    }

    /// Record a repayment; the only way an obligation shrinks.
    pub fn repay(&mut self, id: &str, amount: Decimal) -> Result<Decimal> {
        let obligation = self
            .obligations
            .get_mut(id)
            .ok_or_else(|| Error::not_found(format!("no funding {id} to repay")))?;
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "repayment of {id} must be positive, got {amount}"
            )));
        }
        if amount > obligation.outstanding() {
            return Err(Error::invalid(format!(
                "repayment {amount} exceeds the {} outstanding on {id}; repay at most that",
                obligation.outstanding()
            )));
        }
        obligation.repaid += amount;
        Ok(obligation.outstanding())
    }

    /// Whether an obligation has been repaid in full.
    pub fn is_extinguished(&self, id: &str) -> bool {
        self.obligations
            .get(id)
            .is_some_and(|o| o.outstanding().is_zero())
    }

    /// Outstanding receivable held by a book.
    pub fn receivable(&self, book: &str) -> Decimal {
        self.obligations
            .values()
            .filter(|o| o.funder == book)
            .fold(Decimal::ZERO, |a, o| a + o.outstanding())
    }

    /// Outstanding payable held by a book.
    pub fn payable(&self, book: &str) -> Decimal {
        self.obligations
            .values()
            .filter(|o| o.funded == book)
            .fold(Decimal::ZERO, |a, o| a + o.outstanding())
    }

    /// Every book named by any obligation.
    pub fn books(&self) -> BTreeSet<&str> {
        self.obligations
            .values()
            .flat_map(|o| [o.funder.as_str(), o.funded.as_str()])
            .collect()
    }

    /// Whether receivables equal payables across all books.
    pub fn is_balanced(&self) -> bool {
        let books = self.books();
        let r = books
            .iter()
            .fold(Decimal::ZERO, |a, b| a + self.receivable(b));
        let p = books.iter().fold(Decimal::ZERO, |a, b| a + self.payable(b));
        r == p
    }
}

/// A regulated function of the Capital Bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FinancingFunction {
    /// FX funding.
    FxFunding,
    /// Borrow or lend.
    BorrowLend,
    /// Repo.
    Repo,
    /// Credit line.
    CreditLine,
    /// Deposit-taking.
    DepositTaking,
    /// Lending to third parties.
    ThirdPartyLending,
    /// Brokerage.
    Brokerage,
    /// Derivatives dealing.
    DerivativesDealing,
}

impl FinancingFunction {
    /// Every function, so a property test can sweep all of them.
    pub const ALL: [FinancingFunction; 8] = [
        Self::FxFunding,
        Self::BorrowLend,
        Self::Repo,
        Self::CreditLine,
        Self::DepositTaking,
        Self::ThirdPartyLending,
        Self::Brokerage,
        Self::DerivativesDealing,
    ];
}

type PermissionKey = (FinancingFunction, String, String, String);

/// Recorded permissions. Empty by default, which refuses everything.
#[derive(Debug, Clone, Default)]
pub struct FinancingPermissions {
    granted: BTreeSet<PermissionKey>,
}

impl FinancingPermissions {
    /// Empty registry: every function refuses.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a permission for exactly one combination.
    pub fn permit(
        &mut self,
        function: FinancingFunction,
        entity: &str,
        counterparty: &str,
        jurisdiction: &str,
    ) {
        self.granted.insert((
            function,
            entity.to_owned(),
            counterparty.to_owned(),
            jurisdiction.to_owned(),
        ));
    }

    /// Gate a call. Must run before any counterparty is contacted.
    pub fn require(
        &self,
        function: FinancingFunction,
        entity: &str,
        counterparty: &str,
        jurisdiction: &str,
    ) -> Result<()> {
        let key = (
            function,
            entity.to_owned(),
            counterparty.to_owned(),
            jurisdiction.to_owned(),
        );
        if self.granted.contains(&key) {
            Ok(())
        } else {
            Err(Error::denied(format!(
                "{function:?} is not permitted for entity {entity} with counterparty \
                 {counterparty} in {jurisdiction}; record that permission, with legal \
                 sign-off, before calling it"
            )))
        }
    }
}
