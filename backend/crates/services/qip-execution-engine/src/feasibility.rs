//! The central feasibility gate: can this order be expressed at this venue,
//! asked before pre-trade risk ever projects the position it would produce.
//!
//! `qip-edge`'s cell has carried this gate since its own `feasibility.rs`
//! (blueprint §18.1): at small capital the binding constraint is not whether
//! an edge exists but whether the order that would capture it sits on the
//! venue's lot and tick grids and clears its minimum size, and every one of
//! those is cheaper to ask than the profitability question. The central path
//! had no equivalent — [`crate::oms::OrderManager::submit`] ran a well-formed
//! order straight through the kill switch, the autonomy gate and pre-trade
//! risk with no check that the venue could accept the size or price at all,
//! so a strategy that reasoned in fractional or off-tick units rode every
//! other control it happened to satisfy. This module is that missing gate,
//! adapted to what the central path actually has on hand.
//!
//! # Refuse, never round
//!
//! Same rule as the edge crate, restated because it is the reason this module
//! exists rather than a `Decimal::floor_to_step` call at the point of use:
//! CLAUDE.md's second principle is that a value silently corrected is a
//! caller bug that survives. An order sized off the lot grid is a strategy
//! that does not know the venue's grid, and rounding it here would let that
//! strategy run indefinitely while the book quietly recorded a different size
//! from the one it reasoned about. Every rule below refuses and names the
//! rule; none of them adjusts the order and lets it through.
//!
//! # What the gate knows, and why it is opt in per venue
//!
//! [`VenueFeasibility`] is built by the composition root, keyed by venue name,
//! and installed through [`crate::oms::OrderManager::with_venue_feasibility`].
//! A venue nobody has modelled is checked for nothing here — not because the
//! gate assumes it is fine, but because a lot size or minimum guessed at is a
//! rounding rule wearing a refusal's clothes, and this platform's rule is to
//! refuse only what it actually knows. That mirrors the edge crate's own
//! stated position on a venue with no model at all.
//!
//! # Why this differs from the edge gate's shape
//!
//! The edge gate reads a resting book and a scanner's edge estimate to run a
//! depth rule and a fee-floor rule; the central path, at the point
//! [`assess`] runs, has neither — an [`crate::order::Order`] carries a
//! quantity, a type and an arrival price, not a book snapshot or a
//! profitability estimate. So this gate keeps the two rules that ask
//! questions the order itself can answer — the lot grid and the minimum
//! size — and adds the tick and minimum-notional rules the edge gate also
//! carries, applied only where an order states a price at all: a market
//! order takes whatever the venue quotes, so there is no submitted price for
//! a tick rule to judge, exactly as the edge crate declines to apply its fee
//! floor to an intent that carries no edge estimate rather than inventing
//! one.

use crate::order::{Order, OrderType};
use qip_core::Decimal;
use qip_core::error::{Error, Result};

/// The gate literal each rule refuses under, identical to the edge crate's
/// names for the same control so a refusal on either plane correlates under
/// one vocabulary.
pub const GATE_MINIMUM_QUANTITY: &str = "feasibility_minimum_quantity";
pub const GATE_MINIMUM_NOTIONAL: &str = "feasibility_minimum_notional";
pub const GATE_LOT: &str = "feasibility_lot";
pub const GATE_TICK: &str = "feasibility_tick";

/// What the central path knows about executing at one venue.
///
/// Every field bounds either a size or a price, so every field is exact, and
/// the constructor refuses rather than defaults: a lot size of zero would
/// admit every quantity, which is no grid at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VenueFeasibility {
    lot_size: Decimal,
    /// `None` where the venue states no tick, e.g. a venue quoted only in
    /// whole units of its own instrument.
    tick_size: Option<Decimal>,
    minimum_quantity: Decimal,
    minimum_notional: Decimal,
}

impl VenueFeasibility {
    /// Refuses a lot that is not positive, a stated tick that is not
    /// positive, or a minimum that is negative.
    pub fn new(
        lot_size: Decimal,
        tick_size: Option<Decimal>,
        minimum_quantity: Decimal,
        minimum_notional: Decimal,
    ) -> Result<Self> {
        if !lot_size.is_positive() {
            return Err(Error::invalid(format!(
                "a lot size of {lot_size} is not a grid; state the venue's smallest quantity \
                 increment"
            )));
        }
        if let Some(tick) = tick_size
            && !tick.is_positive()
        {
            return Err(Error::invalid(format!(
                "a tick size of {tick} is not a grid; state the venue's smallest price increment \
                 or omit it if the venue states none"
            )));
        }
        if minimum_quantity.is_negative() {
            return Err(Error::invalid(format!(
                "a minimum quantity of {minimum_quantity} is negative; zero means the venue \
                 states none"
            )));
        }
        if minimum_notional.is_negative() {
            return Err(Error::invalid(format!(
                "a minimum notional of {minimum_notional} is negative; zero means the venue \
                 states none"
            )));
        }
        Ok(Self {
            lot_size,
            tick_size,
            minimum_quantity,
            minimum_notional,
        })
    }

    pub const fn lot_size(&self) -> Decimal {
        self.lot_size
    }

    pub const fn tick_size(&self) -> Option<Decimal> {
        self.tick_size
    }

    pub const fn minimum_quantity(&self) -> Decimal {
        self.minimum_quantity
    }

    pub const fn minimum_notional(&self) -> Decimal {
        self.minimum_notional
    }
}

/// A rule that refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Infeasible {
    /// One of the `GATE_*` literals above.
    pub gate: &'static str,
    pub reason: String,
}

/// Whether `value` sits on the grid of `step`.
fn on_grid(value: Decimal, step: Decimal) -> bool {
    value.floor_to_step(step) == value
}

/// Judge one order against the venue's grid, before anything downstream
/// treats its size or price as real.
///
/// Rules run cheapest first and the first refusal is the answer, matching the
/// edge gate: one refusal per order, not a list, because the series this
/// feeds counts *which* rule bound and an order below the minimum that is
/// also off-lot is an order below the minimum.
pub fn assess(model: &VenueFeasibility, order: &Order) -> std::result::Result<(), Infeasible> {
    let quantity = order.quantity;
    let object = order.object_id.as_str();

    if quantity < model.minimum_quantity {
        return Err(Infeasible {
            gate: GATE_MINIMUM_QUANTITY,
            reason: format!(
                "{quantity} of {object} is below the {} minimum this venue accepts",
                model.minimum_quantity
            ),
        });
    }

    if !on_grid(quantity, model.lot_size) {
        return Err(Infeasible {
            gate: GATE_LOT,
            reason: format!(
                "{quantity} of {object} is not a whole number of lots of {}; the size is \
                 refused rather than rounded, because a strategy that does not know the grid \
                 would otherwise trade a size it never reasoned about",
                model.lot_size
            ),
        });
    }

    // A market order — and every worked type that takes the market's price
    // rather than stating one — has no submitted price for the tick rule to
    // judge; only a limit order states one.
    if let (OrderType::Limit { price }, Some(tick)) = (order.order_type, model.tick_size)
        && !on_grid(price, tick)
    {
        return Err(Infeasible {
            gate: GATE_TICK,
            reason: format!(
                "{price} is not on the {tick} tick grid for {object}; the price is refused \
                 rather than rounded"
            ),
        });
    }

    if model.minimum_notional.is_positive() {
        let Some(notional) = quantity.checked_mul(order.arrival_price) else {
            return Err(Infeasible {
                gate: GATE_MINIMUM_NOTIONAL,
                reason: format!(
                    "{quantity} of {object} at {} has a notional that cannot be represented",
                    order.arrival_price
                ),
            });
        };
        if notional < model.minimum_notional {
            return Err(Infeasible {
                gate: GATE_MINIMUM_NOTIONAL,
                reason: format!(
                    "{quantity} of {object} at {} is {notional} notional, below the {} minimum \
                     this venue accepts",
                    order.arrival_price, model.minimum_notional
                ),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    //! One passing and one vetoing fixture per rule, exactly as the edge
    //! module's own suite is structured: the blueprint's delivery gate wants
    //! both, because a gate that refuses everything looks, by its refusals
    //! alone, indistinguishable from one that works.

    use super::*;
    use crate::order::Side;
    use qip_core::dec;
    use qip_core::ids::{ObjectId, OrderId};
    use qip_core::time::Timestamp;

    fn order(quantity: &str, order_type: OrderType, price: &str) -> Order {
        Order::new(
            OrderId::from_string("ord-1"),
            ObjectId::from_string("ACME"),
            Side::Buy,
            Decimal::parse(quantity).expect("a decimal literal"),
            order_type,
            Decimal::parse(price).expect("a decimal literal"),
            "prop-1",
            vec!["hyp-1".to_string()],
            "momentum",
            Timestamp::from_secs(60),
        )
    }

    /// Lot 1, tick 0.01, minimum quantity 5, minimum notional 100.
    fn model() -> VenueFeasibility {
        VenueFeasibility::new(dec!("1"), Some(dec!("0.01")), dec!("5"), dec!("100"))
            .expect("a valid model")
    }

    /// A market order at a size and arrival price every rule of [`model`]
    /// admits.
    fn admitted() -> Order {
        order("10", OrderType::Market, "100")
    }

    fn gate_of(outcome: std::result::Result<(), Infeasible>) -> Option<&'static str> {
        outcome.err().map(|infeasible| infeasible.gate)
    }

    #[test]
    fn the_admitted_fixture_passes_every_rule_so_each_veto_below_is_about_its_own_rule() {
        // The premise every vetoing test in this module relies on: a fixture
        // already refused for some other reason proves nothing about the
        // rule it names.
        assert_eq!(assess(&model(), &admitted()), Ok(()));
    }

    #[test]
    fn a_size_below_the_minimum_quantity_is_refused_and_one_at_it_is_admitted() {
        let below = order("4", OrderType::Market, "100");
        let at = order("5", OrderType::Market, "100");
        assert!(below.quantity < model().minimum_quantity());
        assert_eq!(
            gate_of(assess(&model(), &below)),
            Some(GATE_MINIMUM_QUANTITY)
        );
        // 5 x 100 = 500 notional, on the lot grid, at the minimum exactly.
        assert_eq!(assess(&model(), &at), Ok(()));
    }

    #[test]
    fn a_size_off_the_lot_grid_is_refused_rather_than_rounded() {
        let off = order("10.5", OrderType::Market, "100");
        // Premise: it clears the minimum quantity, so the lot rule is the
        // one that answers — and rounding would have produced 10, which
        // passes.
        assert!(off.quantity > model().minimum_quantity());
        assert_eq!(gate_of(assess(&model(), &off)), Some(GATE_LOT));
        assert_eq!(
            assess(&model(), &order("11", OrderType::Market, "100")),
            Ok(())
        );
    }

    #[test]
    fn a_limit_price_off_the_tick_grid_is_refused_rather_than_rounded() {
        let off = order(
            "10",
            OrderType::Limit {
                price: dec!("100.005"),
            },
            "100",
        );
        assert_eq!(gate_of(assess(&model(), &off)), Some(GATE_TICK));
        let at = order(
            "10",
            OrderType::Limit {
                price: dec!("100.01"),
            },
            "100",
        );
        assert_eq!(assess(&model(), &at), Ok(()));
    }

    #[test]
    fn a_market_order_is_never_judged_against_the_tick_grid() {
        // A market order states no price of its own; the venue supplies one
        // at execution, and there is nothing here for the tick rule to
        // check. `admitted()`'s arrival price of 100 sits on the grid
        // anyway, so use one that would fail the rule if it were applied to
        // demonstrate the rule truly does not run.
        let off_tick_arrival = order("10", OrderType::Market, "100.005");
        assert_eq!(assess(&model(), &off_tick_arrival), Ok(()));
    }

    #[test]
    fn a_notional_below_the_minimum_is_refused_and_one_at_it_is_admitted() {
        // 9 x 10 = 90, below 100; 10 x 10 = 100, at it. Both clear the
        // minimum quantity of 5 and both are on the lot grid, so the
        // notional rule is the one deciding.
        let below = order("9", OrderType::Market, "10");
        assert_eq!(
            gate_of(assess(&model(), &below)),
            Some(GATE_MINIMUM_NOTIONAL)
        );
        assert_eq!(
            assess(&model(), &order("10", OrderType::Market, "10")),
            Ok(())
        );
    }

    #[test]
    fn a_zero_minimum_notional_states_the_venue_has_none_rather_than_refusing_everything() {
        let no_minimum =
            VenueFeasibility::new(dec!("1"), None, dec!("0"), dec!("0")).expect("a valid model");
        // A tiny order that would fail every other model's notional rule
        // passes, because this venue states no minimum at all.
        assert_eq!(
            assess(&no_minimum, &order("1", OrderType::Market, "0.01")),
            Ok(())
        );
    }

    #[test]
    fn a_grid_that_is_not_positive_is_refused_at_construction() {
        assert!(VenueFeasibility::new(dec!("0"), None, dec!("0"), dec!("0")).is_err());
        assert!(VenueFeasibility::new(dec!("1"), Some(dec!("0")), dec!("0"), dec!("0")).is_err());
        assert!(VenueFeasibility::new(dec!("1"), None, dec!("-1"), dec!("0")).is_err());
        assert!(VenueFeasibility::new(dec!("1"), None, dec!("0"), dec!("-1")).is_err());
    }
}
