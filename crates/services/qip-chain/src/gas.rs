//! Gas as an execution cost.
//!
//! Gas is not a chain detail that belongs in an operations dashboard. It is a
//! per-attempt cost denominated in a volatile asset, paid whether or not the
//! transaction achieves anything, and on a small opportunity it is frequently
//! the largest single deduction. It therefore ends up where every other cost
//! ends up: as a [`Deduction`] against the gross edge, in the quote currency,
//! computed from an expected gas figure and a price rather than assumed.

use qip_contracts::{Deduction, DeductionKind};
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Money};
use serde::{Deserialize, Serialize};

/// Expected and worst-case gas for one named operation.
///
/// Two figures because the difference matters: a route sized on expected gas
/// that lands in the worst case has spent the edge it was sizing for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasProfile {
    pub operation: String,
    pub expected_gas: u64,
    pub worst_case_gas: u64,
}

impl GasProfile {
    pub fn new(operation: impl Into<String>, expected_gas: u64, worst_case_gas: u64) -> Result<Self> {
        let operation = operation.into();
        if expected_gas == 0 {
            return Err(Error::invalid(format!(
                "operation {operation} cannot cost zero gas"
            )));
        }
        if worst_case_gas < expected_gas {
            return Err(Error::invalid(format!(
                "operation {operation} has a worst case of {worst_case_gas} below its expected {expected_gas}"
            )));
        }
        Ok(Self {
            operation,
            expected_gas,
            worst_case_gas,
        })
    }
}

/// The gas price a transaction will actually pay under EIP-1559.
///
/// `max_fee` is a ceiling, not a bid: the transaction pays the base fee plus
/// its tip, capped by the ceiling. A transaction whose ceiling is below the
/// base fee is not expensive, it is not includable, and pricing it as though
/// it were is how a route plans around a leg that will never land.
pub fn effective_gas_price(
    base_fee: Decimal,
    max_fee_per_gas: Decimal,
    max_priority_fee_per_gas: Decimal,
) -> Result<Decimal> {
    if base_fee.is_negative() || max_fee_per_gas.is_negative() || max_priority_fee_per_gas.is_negative()
    {
        return Err(Error::invalid("gas prices cannot be negative"));
    }
    if max_fee_per_gas < base_fee {
        return Err(Error::invalid(format!(
            "a fee ceiling of {max_fee_per_gas} is below the base fee of {base_fee}; the transaction is not includable"
        )));
    }
    Ok(max_fee_per_gas.min(base_fee + max_priority_fee_per_gas))
}

/// What an operation costs to attempt, in native units and in quote currency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GasCost {
    pub operation: String,
    pub gas: u64,
    /// Native currency per unit of gas.
    pub price_per_gas: Decimal,
    /// Native currency burned.
    pub native_cost: Decimal,
    /// Quote currency per unit of the native currency.
    pub native_price: Decimal,
    pub cost: Money,
}

impl GasCost {
    /// Price an operation at a gas price and a native-token price.
    pub fn estimate(
        profile: &GasProfile,
        price_per_gas: Decimal,
        native_price: Decimal,
        currency: Currency,
    ) -> Result<Self> {
        Self::price(profile.operation.clone(), profile.expected_gas, price_per_gas, native_price, currency)
    }

    /// The same operation at its worst-case gas, which is what a limit is set
    /// against and what a pessimistic pre-trade check should use.
    pub fn worst_case(
        profile: &GasProfile,
        price_per_gas: Decimal,
        native_price: Decimal,
        currency: Currency,
    ) -> Result<Self> {
        Self::price(
            profile.operation.clone(),
            profile.worst_case_gas,
            price_per_gas,
            native_price,
            currency,
        )
    }

    fn price(
        operation: String,
        gas: u64,
        price_per_gas: Decimal,
        native_price: Decimal,
        currency: Currency,
    ) -> Result<Self> {
        if price_per_gas.is_negative() || native_price.is_negative() {
            return Err(Error::invalid("gas and native prices cannot be negative"));
        }
        let native_cost = Decimal::from_int(gas as i64)
            .checked_mul(price_per_gas)
            .ok_or_else(|| Error::numeric(format!("gas cost for {operation} overflowed")))?;
        let cost = native_cost
            .checked_mul(native_price)
            .ok_or_else(|| Error::numeric(format!("gas cost for {operation} overflowed")))?;
        Ok(Self {
            operation,
            gas,
            price_per_gas,
            native_cost,
            native_price,
            cost: Money::new(cost, currency),
        })
    }

    /// The cost as a deduction against an opportunity's gross edge.
    ///
    /// Gas is carried as [`DeductionKind::Funding`] — the cost of holding the
    /// position open across the chain rather than a fee the venue charges.
    pub fn deduction(&self) -> Result<Deduction> {
        Deduction::new(
            DeductionKind::Funding,
            self.cost.amount,
            format!(
                "{} gas for {} at {} per gas, native at {}",
                self.gas, self.operation, self.price_per_gas, self.native_price
            ),
        )
    }

    /// Cost per unit of a trade of `quantity`, for comparing against edge
    /// quoted per unit.
    pub fn per_unit(&self, quantity: Decimal) -> Result<Decimal> {
        self.cost
            .amount
            .checked_div(quantity)
            .ok_or_else(|| Error::invalid("gas cannot be amortised over a zero quantity"))
    }
}
