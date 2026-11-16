//! The pre-trade refusals an event contract carries (ADR 0658, R12-COVERAGE-5104).
//!
//! An event contract pays one unit if its declared outcome occurs and nothing
//! otherwise. The owner's 2026-10-31 ruling admitted the type on the condition
//! that it is fully funded with its loss bounded to the premium paid. Those are
//! properties of an order, so they are refused before an order exists:
//!
//! - a price outside `[0, 1]` is a payoff the contract cannot make;
//! - a sell with no long position behind it is a short, whose loss is not the
//!   premium but up to the whole unit per contract;
//! - a holding past one unit is not the bounded payoff the ruling admitted;
//! - a buy the cash does not cover is not fully funded.
//!
//! Without these a derivative-style order path would carry the type: it is not
//! a derivative (it has no underlying), so `derivative_gate` does not see it,
//! and `InstrumentType::is_derivative` names it `false` on purpose.

use std::collections::BTreeSet;

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_financial::asset_class::InstrumentType;
use qip_financial::universe::Universe;

use crate::platform::Platform;
use qip_execution_engine::order::Side;

/// The object ids of every event contract in `universe`.
pub(crate) fn event_contract_ids(universe: &Universe) -> BTreeSet<String> {
    universe
        .iter()
        .filter(|object| object.instrument_type == InstrumentType::EventContract)
        .map(|object| object.object_id.as_str().to_string())
        .collect()
}

/// The refusal, if any, for an order of `quantity` at `price` on a contract
/// when `held` units are already long and `cash` is free.
pub(crate) fn refusal(
    object_id: &str,
    side: Side,
    quantity: Decimal,
    price: Decimal,
    held: Decimal,
    cash: Decimal,
) -> Result<()> {
    if price.is_negative() || price > Decimal::ONE {
        return Err(Error::denied(format!(
            "{object_id} is an event contract whose payoff is one unit; a price of {price} is \
             outside [0, 1]. Quote it as a probability"
        )));
    }
    match side {
        Side::Sell => {
            if quantity > held {
                return Err(Error::denied(format!(
                    "{object_id} is an event contract and the desk holds {held}; selling \
                     {quantity} would be a short, whose loss is not bounded to a premium. Sell \
                     no more than is held"
                )));
            }
        }
        Side::Buy => {
            if held + quantity > Decimal::ONE {
                return Err(Error::denied(format!(
                    "{object_id} is an event contract bounded to one unit; holding {held} and \
                     buying {quantity} would exceed it. Size the order to what remains"
                )));
            }
            let premium = quantity * price;
            if premium > cash {
                return Err(Error::denied(format!(
                    "{object_id} is an event contract and must be fully funded; its premium \
                     {premium} exceeds the cash available, {cash}"
                )));
            }
        }
    }
    Ok(())
}

impl Platform {
    /// Refuse an order on an event contract that is not fully funded or is not
    /// bounded to one unit. Not an event contract: no claim.
    pub(crate) fn event_contract_gate(
        &self,
        object_id: &str,
        side: Side,
        quantity: Decimal,
        price: Decimal,
    ) -> Result<()> {
        if !self.event_contract_ids.contains(object_id) {
            return Ok(());
        }
        refusal(
            object_id,
            side,
            quantity,
            price,
            self.held_quantity(object_id),
            self.free_cash(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    #[test]
    fn a_buy_the_cash_does_not_cover_is_refused_as_not_fully_funded() {
        let outcome = refusal(
            "e",
            Side::Buy,
            dec!("1"),
            dec!("0.6"),
            dec!("0"),
            dec!("0.5"),
        );
        assert!(
            outcome.is_err_and(|e| e.message().contains("fully funded")),
            "an unfunded premium was admitted"
        );
        assert!(
            refusal(
                "e",
                Side::Buy,
                dec!("1"),
                dec!("0.5"),
                dec!("0"),
                dec!("0.5")
            )
            .is_ok(),
            "premise: a premise exactly covered by cash is admitted"
        );
    }
}
