//! Account bookkeeping: positions, cash and margin, kept exactly.
//!
//! The arithmetic is not reimplemented here. [`qip_portfolio::Portfolio`]
//! already holds the accounting identity this platform depends on —
//! `equity = cash + position value`, exact to the last unit after any sequence
//! of fills, with realised profit taken from lots rather than an average price
//! — and a broker adapter that kept its own parallel books would be a second
//! answer to the same question. So the ledger is a thin, honest wrapper: it
//! decides *what* to book and hands the arithmetic to the portfolio.
//!
//! Every fill must name an instrument the ledger has been told about. A fill in
//! an unknown instrument is refused rather than booked against a guessed
//! contract multiplier, because a multiplier guessed wrong is a position off by
//! a factor of a hundred that nothing downstream would question.
//!
//! Margin is Regulation T by default and stated in [`MarginPolicy`] rather than
//! buried: a requirement nobody can see is one nobody can argue with when it
//! stops a trade.

use crate::adapter::{CashBalance, MarginState, PositionSnapshot};
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, PortfolioId};
use qip_core::time::Timestamp;
use qip_core::{Currency, Decimal};
use qip_execution_engine::order::{Fill, Side};
use qip_financial::object::FinancialObject;
use qip_portfolio::portfolio::Portfolio;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the venue requires an account to hold against its positions.
///
/// Rates are [`Decimal`] rather than `f64` because they multiply money: a
/// margin call is decided to the cent, and a requirement that differs in the
/// ninth decimal between two runs is a call that appears and disappears.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginPolicy {
    /// Fraction of a long position's value required to open it.
    pub initial_long: Decimal,
    /// Fraction required to keep it.
    pub maintenance_long: Decimal,
    /// Fraction of a short position's value required to open it. Above one:
    /// a short is collateralised by the proceeds plus a margin on top.
    pub initial_short: Decimal,
    pub maintenance_short: Decimal,
}

impl Default for MarginPolicy {
    /// Regulation T for a cash-equity account.
    fn default() -> Self {
        Self {
            initial_long: Decimal::from_raw(500_000_000),
            maintenance_long: Decimal::from_raw(250_000_000),
            initial_short: Decimal::from_raw(1_500_000_000),
            maintenance_short: Decimal::from_raw(300_000_000),
        }
    }
}

impl MarginPolicy {
    /// A cash account: everything is paid for in full and nothing may be
    /// shorted on margin.
    pub fn cash_account() -> Self {
        Self {
            initial_long: Decimal::ONE,
            maintenance_long: Decimal::ONE,
            initial_short: Decimal::ONE,
            maintenance_short: Decimal::ONE,
        }
    }
}

/// One account's books.
#[derive(Clone, Debug)]
pub struct AccountLedger {
    account: String,
    portfolio: Portfolio,
    /// Instruments the ledger will book. A fill in anything else is refused.
    instruments: BTreeMap<String, FinancialObject>,
    /// Last price seen per instrument, used as the mark. A mark is a fact about
    /// a trade that happened, never an assumption.
    marks: BTreeMap<String, Decimal>,
    opening: Decimal,
    fills: Vec<Fill>,
    policy: MarginPolicy,
    as_of: Timestamp,
}

impl AccountLedger {
    pub fn new(
        account: impl Into<String>,
        currency: Currency,
        opening: Decimal,
        policy: MarginPolicy,
        at: Timestamp,
    ) -> Self {
        let account = account.into();
        Self {
            portfolio: Portfolio::new(
                PortfolioId::from_string(format!("account-{account}")),
                account.clone(),
                currency,
                opening,
                at,
            ),
            account,
            instruments: BTreeMap::new(),
            marks: BTreeMap::new(),
            opening,
            fills: Vec::new(),
            policy,
            as_of: at,
        }
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn policy(&self) -> MarginPolicy {
        self.policy
    }

    /// Tell the ledger about an instrument it may book.
    pub fn register(&mut self, object: FinancialObject) {
        self.instruments
            .insert(object.object_id.as_str().to_string(), object);
    }

    pub fn instrument(&self, object_id: &ObjectId) -> Option<&FinancialObject> {
        self.instruments.get(object_id.as_str())
    }

    pub fn knows(&self, object_id: &ObjectId) -> bool {
        self.instruments.contains_key(object_id.as_str())
    }

    /// Record a mark without a trade — a settlement price, a venue close.
    pub fn mark(&mut self, object_id: &ObjectId, price: Decimal) {
        self.marks.insert(object_id.as_str().to_string(), price);
    }

    /// Book a fill.
    ///
    /// Cash moves by exactly the fill value plus costs, which is what keeps the
    /// accounting identity exact rather than approximately right.
    pub fn apply(&mut self, object_id: &ObjectId, fill: &Fill, side: Side) -> Result<()> {
        let Some(object) = self.instruments.get(object_id.as_str()) else {
            return Err(Error::not_found(format!(
                "account {} was never told about {}, so a fill in it cannot be booked; registering \
                 the instrument is what supplies the contract multiplier and the currency",
                self.account,
                object_id.as_str()
            )));
        };
        if fill.quantity <= Decimal::ZERO {
            return Err(Error::invalid("a fill for zero quantity is not a fill"));
        }
        let signed = match side {
            Side::Buy => fill.quantity,
            Side::Sell => -fill.quantity,
        };
        self.portfolio.apply_fill(
            object,
            signed,
            fill.price,
            fill.costs,
            fill.at,
            Some(fill.order_id.as_str().to_string()),
        );
        self.marks
            .insert(object_id.as_str().to_string(), fill.price);
        self.fills.push(fill.clone());
        self.as_of = fill.at;
        Ok(())
    }

    /// Settled cash, with both ends of the identity so it can be reconciled.
    pub fn cash(&self) -> CashBalance {
        let valuation = self.portfolio.value(&self.marks, self.as_of);
        CashBalance {
            account: self.account.clone(),
            currency: self.portfolio.base_currency,
            settled: self.portfolio.cash,
            opening: self.opening,
            costs: self.portfolio.cumulative_costs,
            realised_pnl: valuation.realised_pnl,
            as_of: self.as_of,
        }
    }

    /// Every position that is not flat.
    pub fn positions(&self) -> Vec<PositionSnapshot> {
        self.portfolio
            .positions()
            .map(|position| {
                let mark = self.marks.get(position.object_id.as_str()).copied();
                PositionSnapshot {
                    object_id: position.object_id.clone(),
                    symbol: position.symbol.clone(),
                    quantity: position.quantity(),
                    average_price: position.average_price(),
                    realised_pnl: position.realised_pnl,
                    costs: position.total_costs,
                    mark,
                    market_value: mark.map(|mark| position.market_value(mark)),
                    as_of: position.updated_at,
                }
            })
            .collect()
    }

    /// Fills booked since `since`, or all of them.
    pub fn fills(&self, since: Option<Timestamp>) -> Vec<Fill> {
        match since {
            Some(since) => self
                .fills
                .iter()
                .filter(|fill| fill.at >= since)
                .cloned()
                .collect(),
            None => self.fills.clone(),
        }
    }

    /// The margin position, marked at what the ledger has seen trade.
    pub fn margin(&self, at: Timestamp) -> MarginState {
        let mut position_value = Decimal::ZERO;
        let mut gross = Decimal::ZERO;
        let mut initial = Decimal::ZERO;
        let mut maintenance = Decimal::ZERO;
        let mut unpriced = Vec::new();

        for position in self.portfolio.positions() {
            let Some(mark) = self.marks.get(position.object_id.as_str()).copied() else {
                // An unpriced position is excluded from the requirement rather
                // than treated as free. It is reported so the omission is
                // visible instead of flattering.
                unpriced.push(position.symbol.clone());
                continue;
            };
            let value = position.market_value(mark);
            let magnitude = value.abs();
            position_value += value;
            gross += magnitude;
            if position.quantity().is_negative() {
                initial += magnitude * self.policy.initial_short;
                maintenance += magnitude * self.policy.maintenance_short;
            } else {
                initial += magnitude * self.policy.initial_long;
                maintenance += magnitude * self.policy.maintenance_long;
            }
        }

        let cash = self.portfolio.cash;
        let equity = cash + position_value;
        MarginState {
            at,
            cash,
            position_value,
            equity,
            initial_requirement: initial,
            maintenance_requirement: maintenance,
            excess_liquidity: equity - initial,
            maintenance_excess: equity - maintenance,
            gross_exposure: gross,
            leverage: if equity.is_positive() {
                gross.to_f64() / equity.to_f64()
            } else {
                f64::INFINITY
            },
            unpriced,
        }
    }

    /// Check the books balance.
    ///
    /// A break here means a fill moved a position without moving cash, or the
    /// reverse, and every number downstream is wrong.
    pub fn verify(&self, at: Timestamp) -> Result<()> {
        self.portfolio.verify_accounting(&self.marks, at)
    }

    /// The portfolio itself, for callers that need the full accounting surface.
    pub fn portfolio(&self) -> &Portfolio {
        &self.portfolio
    }
}
