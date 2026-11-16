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
//! Every check counts what earlier orders have already claimed: orders
//! admitted by `order_for` and not yet submitted, and orders resting open at
//! the OMS. Without that, two buys in one pass each pass against the same cash
//! and the same unit of headroom, and together they are neither funded nor
//! bounded. The reservations are dropped at the start of every cycle, so an
//! order abandoned before submission cannot lock a contract past its pass.
//!
//! An event contract is not a derivative (it has no underlying), so
//! `derivative_gate` does not see it and `InstrumentType::is_derivative` names
//! it `false` on purpose.

use std::collections::BTreeSet;

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::InstrumentType;
use qip_financial::universe::Universe;

use crate::platform::Platform;

/// The object ids of every event contract in `universe`.
pub(crate) fn event_contract_ids(universe: &Universe) -> BTreeSet<String> {
    universe
        .iter()
        .filter(|object| object.instrument_type == InstrumentType::EventContract)
        .map(|object| object.object_id.as_str().to_string())
        .collect()
}

/// What earlier orders on one contract have already claimed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Committed {
    pub buy: Decimal,
    pub sell: Decimal,
    pub cash: Decimal,
}

/// An order admitted by `order_for` and not yet submitted.
#[derive(Clone, Debug)]
pub(crate) struct Reservation {
    pub object_id: String,
    pub side: Side,
    pub quantity: Decimal,
    pub premium: Decimal,
}

/// The refusal, if any, for an order of `quantity` at `price` on a contract
/// when `held` units are long, `cash` is free and `committed` is claimed.
pub(crate) fn refusal(
    object_id: &str,
    side: Side,
    quantity: Decimal,
    price: Decimal,
    held: Decimal,
    cash: Decimal,
    committed: Committed,
) -> Result<()> {
    if price.is_negative() || price > Decimal::ONE {
        return Err(Error::denied(format!(
            "{object_id} is an event contract whose payoff is one unit; a price of {price} is \
             outside [0, 1]. Quote it as a probability"
        )));
    }
    match side {
        Side::Sell => {
            let sellable = held - committed.sell;
            if quantity > sellable {
                return Err(Error::denied(format!(
                    "{object_id} is an event contract and only {sellable} is unclaimed (held \
                     {held}, {} already being sold); selling {quantity} would be a short, whose \
                     loss is not bounded to a premium. Sell no more than is unclaimed",
                    committed.sell
                )));
            }
        }
        Side::Buy => {
            if held + committed.buy + quantity > Decimal::ONE {
                return Err(Error::denied(format!(
                    "{object_id} is an event contract bounded to one unit; holding {held} with {} \
                     already being bought, buying {quantity} would exceed it. Size the order to \
                     what remains",
                    committed.buy
                )));
            }
            let premium = quantity * price;
            let available = cash - committed.cash;
            if premium > available {
                return Err(Error::denied(format!(
                    "{object_id} is an event contract and must be fully funded; its premium \
                     {premium} exceeds the {available} left of cash {cash} after {} committed to \
                     earlier event-contract buys",
                    committed.cash
                )));
            }
        }
    }
    Ok(())
}

impl Platform {
    /// What orders other than `excluding` have claimed: units on `object_id`
    /// only, but premium cash across every event contract, because the cash
    /// is one pool. Counted are orders admitted and not yet submitted, and
    /// orders open at the OMS.
    fn event_commitments(&self, object_id: &str, excluding: Option<&str>) -> Committed {
        let mut claimed = Committed::default();
        for (id, reservation) in &self.pending_event_orders {
            if Some(id.as_str()) == excluding {
                continue;
            }
            let same = reservation.object_id == object_id;
            match reservation.side {
                Side::Buy => {
                    claimed.cash += reservation.premium;
                    if same {
                        claimed.buy += reservation.quantity;
                    }
                }
                Side::Sell if same => claimed.sell += reservation.quantity,
                Side::Sell => {}
            }
        }
        for order in self.orders().open_orders() {
            let id = order.order_id.to_string();
            if !self.event_contract_ids.contains(order.object_id.as_str())
                || Some(id.as_str()) == excluding
                || self.pending_event_orders.contains_key(&id)
            {
                continue;
            }
            let same = order.object_id.as_str() == object_id;
            let remaining = order.remaining_quantity();
            match order.side {
                Side::Buy => {
                    claimed.cash += remaining * order.arrival_price;
                    if same {
                        claimed.buy += remaining;
                    }
                }
                Side::Sell if same => claimed.sell += remaining,
                Side::Sell => {}
            }
        }
        claimed
    }

    /// Refuse an order on an event contract that is not fully funded or is not
    /// bounded to one unit. Not an event contract: no claim. `own` is the
    /// order's own id when it is already reserved, so it is not counted
    /// against itself.
    pub(crate) fn event_contract_gate(
        &self,
        object_id: &str,
        side: Side,
        quantity: Decimal,
        price: Decimal,
        own: Option<&str>,
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
            self.event_commitments(object_id, own),
        )
    }

    /// Reserve what an admitted order claims until it is submitted.
    pub(crate) fn reserve_event_order(&mut self, order_id: String, reservation: Reservation) {
        self.pending_event_orders.insert(order_id, reservation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    #[test]
    fn a_buy_the_cash_does_not_cover_is_refused_as_not_fully_funded() {
        let none = Committed::default();
        let outcome = refusal(
            "e",
            Side::Buy,
            dec!("1"),
            dec!("0.6"),
            dec!("0"),
            dec!("0.5"),
            none,
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
                dec!("0.5"),
                none
            )
            .is_ok(),
            "premise: a premium exactly covered by cash is admitted"
        );
    }

    #[test]
    fn cash_an_earlier_admission_claimed_is_not_spent_twice() {
        let earlier = Committed {
            buy: dec!("0.5"),
            sell: dec!("0"),
            cash: dec!("0.3"),
        };
        // Premise: against the same cash alone the second buy fits.
        assert!(
            refusal(
                "e",
                Side::Buy,
                dec!("0.5"),
                dec!("0.6"),
                dec!("0"),
                dec!("0.5"),
                Committed::default()
            )
            .is_ok()
        );
        let outcome = refusal(
            "e",
            Side::Buy,
            dec!("0.5"),
            dec!("0.6"),
            dec!("0"),
            dec!("0.5"),
            earlier,
        );
        assert!(
            outcome.is_err_and(|e| e.message().contains("earlier event-contract buys")),
            "the second buy was funded from cash the first had claimed"
        );
    }
}
