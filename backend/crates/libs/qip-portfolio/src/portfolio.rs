//! The portfolio: positions, cash, and the accounting that ties them together.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, ObjectId, PortfolioId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::asset_class::{AssetClass, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::universe::Universe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::exposure::ExposureBreakdown;
use crate::position::Position;

/// A portfolio valued at a point in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Valuation {
    pub at: Timestamp,
    pub cash: Decimal,
    /// Sum of position market values.
    pub position_value: Decimal,
    /// Cash plus position value. The portfolio's net worth.
    pub equity: Decimal,
    pub realised_pnl: Decimal,
    pub unrealised_pnl: Decimal,
    pub gross_exposure: Decimal,
    pub net_exposure: Decimal,
    pub leverage: f64,
    /// Positions with no price available at valuation time.
    pub unpriced: Vec<String>,
}

impl Valuation {
    /// Whether the accounting identity holds exactly.
    pub fn is_balanced(&self) -> bool {
        self.equity == self.cash + self.position_value
    }

    pub fn total_pnl(&self) -> Decimal {
        self.realised_pnl + self.unrealised_pnl
    }
}

/// A portfolio of positions funded by cash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Portfolio {
    pub portfolio_id: PortfolioId,
    pub name: String,
    pub base_currency: Currency,
    /// Settled cash in the base currency.
    pub cash: Decimal,
    positions: BTreeMap<String, Position>,
    /// Capital originally contributed, for return calculation.
    pub initial_capital: Decimal,
    /// Cumulative costs paid across the book.
    pub cumulative_costs: Decimal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Portfolio {
    pub fn new(
        portfolio_id: PortfolioId,
        name: impl Into<String>,
        base_currency: Currency,
        initial_capital: Decimal,
        at: Timestamp,
    ) -> Self {
        Self {
            portfolio_id,
            name: name.into(),
            base_currency,
            cash: initial_capital,
            positions: BTreeMap::new(),
            initial_capital,
            cumulative_costs: Decimal::ZERO,
            created_at: at,
            updated_at: at,
        }
    }

    pub fn position_count(&self) -> usize {
        self.positions.values().filter(|p| !p.is_flat()).count()
    }

    pub fn positions(&self) -> impl Iterator<Item = &Position> {
        self.positions.values().filter(|p| !p.is_flat())
    }

    /// Every position including flat ones, which retain their trade history.
    pub fn all_positions(&self) -> impl Iterator<Item = &Position> {
        self.positions.values()
    }

    pub fn position(&self, object_id: &ObjectId) -> Option<&Position> {
        self.positions.get(object_id.as_str())
    }

    pub fn position_mut(&mut self, object_id: &ObjectId) -> Option<&mut Position> {
        self.positions.get_mut(object_id.as_str())
    }

    /// Apply a fill, updating the position and cash.
    ///
    /// Returns the resulting position quantity. Cash moves by exactly the fill
    /// value plus costs, which is what keeps the accounting identity exact.
    pub fn apply_fill(
        &mut self,
        object: &FinancialObject,
        quantity: Decimal,
        price: Decimal,
        costs: Decimal,
        at: Timestamp,
        order_id: Option<String>,
    ) -> Decimal {
        let position = self
            .positions
            .entry(object.object_id.as_str().to_string())
            .or_insert_with(|| {
                Position::new(object.object_id.clone(), &object.symbol, at)
                    .with_multiplier(object.contract_multiplier)
            });

        let cash_flow = position.apply_fill(quantity, price, costs, at, order_id);
        self.cash += cash_flow;
        self.cumulative_costs += costs;
        self.updated_at = at;
        position.quantity()
    }

    /// Deposit or withdraw cash.
    pub fn adjust_cash(&mut self, amount: Decimal, at: Timestamp) {
        self.cash += amount;
        self.updated_at = at;
    }

    /// Credit a dividend or coupon.
    pub fn credit_income(
        &mut self,
        object_id: &ObjectId,
        per_unit: Decimal,
        at: Timestamp,
    ) -> Decimal {
        let Some(position) = self.positions.get(object_id.as_str()) else {
            return Decimal::ZERO;
        };
        let amount = position.quantity() * per_unit * position.contract_multiplier;
        self.cash += amount;
        self.updated_at = at;
        amount
    }

    /// Value the portfolio at the supplied prices.
    ///
    /// A position with no price is reported in `unpriced` and excluded from the
    /// valuation rather than valued at zero — a missing price is missing
    /// information, and treating it as worthless understates risk exactly when
    /// it matters.
    pub fn value(&self, prices: &BTreeMap<String, Decimal>, at: Timestamp) -> Valuation {
        let mut position_value = Decimal::ZERO;
        let mut unrealised = Decimal::ZERO;
        let mut realised = Decimal::ZERO;
        let mut gross = Decimal::ZERO;
        let mut net = Decimal::ZERO;
        let mut unpriced = Vec::new();

        for position in self.positions.values() {
            realised += position.realised_pnl;
            if position.is_flat() {
                continue;
            }
            match prices.get(position.object_id.as_str()) {
                Some(price) => {
                    let value = position.market_value(*price);
                    position_value += value;
                    unrealised += position.unrealised_pnl(*price);
                    gross += value.abs();
                    net += value;
                }
                None => unpriced.push(position.symbol.clone()),
            }
        }

        let equity = self.cash + position_value;
        Valuation {
            at,
            cash: self.cash,
            position_value,
            equity,
            realised_pnl: realised,
            unrealised_pnl: unrealised,
            gross_exposure: gross,
            net_exposure: net,
            leverage: if equity.is_positive() {
                gross.to_f64() / equity.to_f64()
            } else {
                f64::INFINITY
            },
            unpriced,
        }
    }

    /// Portfolio weights by market value as a fraction of equity.
    pub fn weights(
        &self,
        prices: &BTreeMap<String, Decimal>,
        at: Timestamp,
    ) -> BTreeMap<String, f64> {
        let valuation = self.value(prices, at);
        if !valuation.equity.is_positive() {
            return BTreeMap::new();
        }
        let equity = valuation.equity.to_f64();
        self.positions()
            .filter_map(|position| {
                let price = prices.get(position.object_id.as_str())?;
                Some((
                    position.object_id.as_str().to_string(),
                    position.market_value(*price).to_f64() / equity,
                ))
            })
            .collect()
    }

    /// Exposure along every axis, using the universe for classification.
    pub fn exposures(
        &self,
        universe: &Universe,
        prices: &BTreeMap<String, Decimal>,
    ) -> ExposureBreakdown {
        let mut breakdown = ExposureBreakdown::default();
        for position in self.positions() {
            let Some(price) = prices.get(position.object_id.as_str()) else {
                continue;
            };
            let value = position.market_value(*price);
            let Some(object) = universe.get(&position.object_id) else {
                // An unclassified instrument still counts toward totals; it is
                // bucketed as unknown rather than dropped, because dropping it
                // would understate gross exposure.
                breakdown.by_asset_class.add("unknown", value);
                continue;
            };
            breakdown
                .by_asset_class
                .add(object.asset_class.as_str(), value);
            breakdown.by_sector.add(object.sector.as_str(), value);
            breakdown.by_country.add(&object.geography, value);
            breakdown.by_currency.add(object.currency.as_str(), value);
            breakdown.by_issuer.add(
                object
                    .issuer
                    .clone()
                    .unwrap_or_else(|| object.symbol.clone()),
                value,
            );
            breakdown.by_venue.add(&object.venue, value);
            for (factor, loading) in &object.risk.factor_exposures.loadings {
                let contribution = Decimal::from_f64(*loading).unwrap_or(Decimal::ZERO) * value;
                breakdown.by_factor.add(factor, contribution);
            }
        }
        breakdown
    }

    /// Total return since inception, on contributed capital.
    pub fn total_return(&self, prices: &BTreeMap<String, Decimal>, at: Timestamp) -> f64 {
        if !self.initial_capital.is_positive() {
            return 0.0;
        }
        let equity = self.value(prices, at).equity;
        equity.to_f64() / self.initial_capital.to_f64() - 1.0
    }

    /// Positions in a given asset class.
    pub fn positions_in(&self, universe: &Universe, class: AssetClass) -> Vec<&Position> {
        self.positions()
            .filter(|p| {
                universe
                    .get(&p.object_id)
                    .is_some_and(|o| o.asset_class == class)
            })
            .collect()
    }

    /// Positions in a given sector.
    pub fn positions_in_sector(&self, universe: &Universe, sector: Sector) -> Vec<&Position> {
        self.positions()
            .filter(|p| {
                universe
                    .get(&p.object_id)
                    .is_some_and(|o| o.sector == sector)
            })
            .collect()
    }

    /// Remove flat positions that have no trade history worth keeping.
    pub fn compact(&mut self) {
        self.positions
            .retain(|_, p| !p.is_flat() || !p.closed_trades.is_empty());
    }

    /// Check the books balance at the supplied prices.
    ///
    /// Called after every batch of fills in the execution engine and in tests;
    /// a break here means a fill was applied to a position but not to cash, or
    /// the reverse, and every downstream number is wrong.
    pub fn verify_accounting(
        &self,
        prices: &BTreeMap<String, Decimal>,
        at: Timestamp,
    ) -> Result<()> {
        let valuation = self.value(prices, at);
        if !valuation.is_balanced() {
            return Err(Error::invalid(format!(
                "portfolio {} does not balance: equity {} != cash {} + positions {}",
                self.name, valuation.equity, valuation.cash, valuation.position_value
            )));
        }
        Ok(())
    }

    /// A serialisable snapshot.
    pub fn snapshot(&self, prices: &BTreeMap<String, Decimal>, at: Timestamp) -> PortfolioSnapshot {
        let valuation = self.value(prices, at);
        PortfolioSnapshot {
            portfolio_id: self.portfolio_id.clone(),
            name: self.name.clone(),
            base_currency: self.base_currency,
            valuation,
            weights: self.weights(prices, at),
            positions: self
                .positions()
                .map(|p| (p.symbol.clone(), p.quantity()))
                .collect(),
            at,
        }
    }
}

/// A point-in-time view of a portfolio, for the API and the event stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortfolioSnapshot {
    pub portfolio_id: PortfolioId,
    pub name: String,
    pub base_currency: Currency,
    pub valuation: Valuation,
    pub weights: BTreeMap<String, f64>,
    pub positions: BTreeMap<String, Decimal>,
    pub at: Timestamp,
}

impl EventBody for PortfolioSnapshot {
    const TOPIC: Topic = Topic::PnlUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.portfolio_id, self.at.as_nanos()))
    }
}
