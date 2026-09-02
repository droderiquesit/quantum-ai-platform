//! The feasibility gate: can this execute at my size, asked before anything
//! asks whether it should.
//!
//! Blueprint §18.1 puts this ahead of the profitability filter for a reason
//! that is arithmetic rather than taste. At small capital the binding
//! constraint is not whether an edge exists but whether the order that would
//! capture it is *expressible* — above the venue's minimum, on its lot and
//! tick grids, small enough that a fixed fee does not eat the edge, and no
//! larger than what rests at the touch. Every one of those is a cheaper
//! question than the profitability one, and a trade that fails any of them is
//! not a smaller trade; it is one the venue will reject, or one that loses
//! money on a fee schedule the edge calculation never saw.
//!
//! # Refuse, never round
//!
//! The blueprint allows "reject or round, per policy" on granularity. This
//! platform's policy is CLAUDE.md's second principle: a value silently
//! corrected is a caller bug that survives. An intent sized off the lot grid
//! is a strategy that does not know the venue's grid, and rounding it here
//! would let that strategy run indefinitely while the cell quietly traded a
//! different size from the one it reasoned about. So every rule here refuses,
//! names the rule, and the refusal is counted under its own gate literal — the
//! blueprint calls the distribution of feasibility refusals "the highest-value
//! observability signal at small capital", and a gate that rounded would have
//! nothing to chart.
//!
//! # What the gate knows, and from where
//!
//! Two sources, resolved field by field so neither has to be complete:
//!
//! * A [`VenueModel`] the composition root configures per venue — the venue's
//!   class, its lot, tick and minimum quantity (with per-instrument overrides,
//!   because a tick is a fact about an instrument's book and not about the
//!   venue), its minimum notional, its fixed fee per order and, for a chain
//!   venue, its gas cost per order.
//! * The centre's [`FeasibilityConstraints`] — item 11 of the policy payload,
//!   shipped on change — which overrides the tick, the minimum notional and
//!   the fee floor per venue whenever the slot is produced. This is that
//!   slot's first consumer; until now it was shipped and read by nothing.
//!
//! A venue with neither is checked for depth alone. That is stated rather
//! than hidden: the depth rule needs only the book, and the other rules need
//! a fact about the venue the cell has not been given. Nothing here invents
//! one, because a lot size guessed at is a rounding rule wearing a refusal's
//! clothes.
//!
//! # The fee floor, and why it fires only for a cycle
//!
//! §18.1's fee row compares "fixed fee component against expected gross
//! edge". An arbitrage cycle carries an edge — the scanner computed it — so a
//! cycle's legs are assessed together in [`assess_cycle_cost`]: the fixed fee
//! and gas of every leg, each as a fraction of that leg's own notional, are
//! summed and refused if they consume the edge as a fraction of the cycle's
//! start size. Fractions rather than money, because a triangular cycle quotes
//! its legs in three currencies and a fee in BTC cannot be subtracted from an
//! edge in USDT without a rate the gate would have to guess. Each leg's
//! notional is within the cycle's own edge of the start value, so the sum of
//! fractions is the fixed cost relative to the start value to that precision.
//!
//! A directional intent carries no edge estimate — a `Signal` names a
//! conviction and a size, not an expected return — so the fee floor is not
//! applied to it. Applying one would mean inventing the edge, and a gate that
//! compares a fee to a number nobody computed is not a control. The minimum
//! notional is the rule the operator sets in its place, and it is stated here
//! so nobody reads the omission as an oversight.

use qip_contracts::intent::Intent;
use qip_contracts::policy::FeasibilityConstraints;
use qip_contracts::venue::VenueClass;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use std::collections::BTreeMap;

/// The gate literal each rule refuses under. Literals, so the refusal series
/// stays bounded by the source and never by the market.
pub const GATE_MINIMUM_QUANTITY: &str = "feasibility_minimum_quantity";
pub const GATE_MINIMUM_NOTIONAL: &str = "feasibility_minimum_notional";
pub const GATE_LOT: &str = "feasibility_lot";
pub const GATE_TICK: &str = "feasibility_tick";
pub const GATE_DEPTH: &str = "feasibility_depth";
pub const GATE_FEE_FLOOR: &str = "feasibility_fee_floor";
pub const GATE_GAS_FLOOR: &str = "feasibility_gas_floor";
pub const GATE_CONSTRAINT: &str = "feasibility_constraint";

/// The grids an instrument's order must sit on, and the least it may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Granularity {
    lot_size: Decimal,
    tick_size: Decimal,
    minimum_quantity: Decimal,
}

impl Granularity {
    /// Refuses a lot or tick that is not positive: a grid of zero admits every
    /// value, which is no grid, and a negative one is a caller confusion.
    pub fn new(lot_size: Decimal, tick_size: Decimal, minimum_quantity: Decimal) -> Result<Self> {
        if !lot_size.is_positive() {
            return Err(Error::invalid(format!(
                "a lot size of {lot_size} is not a grid; state the venue's smallest quantity \
                 increment"
            )));
        }
        if !tick_size.is_positive() {
            return Err(Error::invalid(format!(
                "a tick size of {tick_size} is not a grid; state the venue's smallest price \
                 increment"
            )));
        }
        if minimum_quantity.is_negative() {
            return Err(Error::invalid(format!(
                "a minimum quantity of {minimum_quantity} is negative; zero means the venue \
                 states none"
            )));
        }
        Ok(Self {
            lot_size,
            tick_size,
            minimum_quantity,
        })
    }

    pub const fn lot_size(&self) -> Decimal {
        self.lot_size
    }

    pub const fn tick_size(&self) -> Decimal {
        self.tick_size
    }

    pub const fn minimum_quantity(&self) -> Decimal {
        self.minimum_quantity
    }
}

/// What the cell knows about executing at one venue.
///
/// Built by the composition root and installed through
/// `CellConfig::with_feasibility`. Every field bounds money, so every field
/// is exact, and every constructor refuses rather than defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct VenueModel {
    class: VenueClass,
    granularity: Granularity,
    minimum_notional: Decimal,
    /// Fixed fee per order, in the quote unit of the instrument traded.
    fee_floor: Decimal,
    /// Cost of landing one order on chain, in the same unit as the fee.
    /// `Some` exactly when the class is a chain venue: [`Self::new`] refuses
    /// the other three combinations.
    gas_floor: Option<Decimal>,
    /// Per-instrument grids, where the venue's default does not describe the
    /// instrument. A tick is a fact about a book, and a venue quoting BTC in
    /// whole dollars and ETH/BTC in ten-thousandths has two.
    instruments: BTreeMap<ObjectId, Granularity>,
}

impl VenueModel {
    /// A model for a venue of the given class.
    ///
    /// The gas floor is required for a chain venue and refused for any other:
    /// a decentralised exchange with no gas cost would let a cycle be priced
    /// as if landing an order were free, and a gas cost on a lit exchange is a
    /// number nobody computed being subtracted from an edge somebody did.
    pub fn new(
        class: VenueClass,
        granularity: Granularity,
        fee_floor: Decimal,
        gas_floor: Option<Decimal>,
    ) -> Result<Self> {
        if fee_floor.is_negative() {
            return Err(Error::invalid(format!(
                "a fee floor of {fee_floor} is negative; a rebate is not a floor"
            )));
        }
        let on_chain = matches!(class, VenueClass::DecentralisedExchange);
        match (on_chain, gas_floor) {
            (true, None) => {
                return Err(Error::invalid(
                    "a chain venue needs a gas floor: without one a cycle through it is priced \
                     as if landing an order cost nothing",
                ));
            }
            (false, Some(gas)) => {
                return Err(Error::invalid(format!(
                    "a gas floor of {gas} on a {} venue is a cost nothing there charges",
                    class.as_str()
                )));
            }
            (true, Some(gas)) if gas.is_negative() => {
                return Err(Error::invalid(format!(
                    "a gas floor of {gas} is negative; nobody is paid to land an order"
                )));
            }
            _ => {}
        }
        Ok(Self {
            class,
            granularity,
            minimum_notional: Decimal::ZERO,
            fee_floor,
            gas_floor,
            instruments: BTreeMap::new(),
        })
    }

    /// The least an order may be worth, in the quote unit.
    pub fn with_minimum_notional(mut self, minimum_notional: Decimal) -> Result<Self> {
        if minimum_notional.is_negative() {
            return Err(Error::invalid(format!(
                "a minimum notional of {minimum_notional} is negative; zero means the venue \
                 states none"
            )));
        }
        self.minimum_notional = minimum_notional;
        Ok(self)
    }

    /// Grids for one instrument that differ from the venue's default.
    #[must_use]
    pub fn with_instrument(mut self, object: ObjectId, granularity: Granularity) -> Self {
        self.instruments.insert(object, granularity);
        self
    }

    pub const fn class(&self) -> VenueClass {
        self.class
    }

    pub const fn fee_floor(&self) -> Decimal {
        self.fee_floor
    }

    pub const fn gas_floor(&self) -> Option<Decimal> {
        self.gas_floor
    }

    pub const fn minimum_notional(&self) -> Decimal {
        self.minimum_notional
    }

    /// The grids that govern `object` here.
    pub fn granularity_for(&self, object: &ObjectId) -> &Granularity {
        self.instruments.get(object).unwrap_or(&self.granularity)
    }
}

/// A rule that refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Infeasible {
    /// One of the `GATE_*` literals above.
    pub gate: &'static str,
    pub reason: String,
}

/// The fields the gate resolves from the two sources, per venue, per call.
///
/// Resolved rather than merged into a model: the policy slot can change on
/// any pass and the model cannot, and cloning a model to overlay three fields
/// on the hot path would be an allocation per intent for a lookup's worth of
/// work.
struct Effective {
    minimum_notional: Option<Decimal>,
    tick_size: Option<Decimal>,
    fee_floor: Option<Decimal>,
}

fn effective(
    model: Option<&VenueModel>,
    constraints: Option<&FeasibilityConstraints>,
    intent: &Intent,
) -> std::result::Result<Effective, Infeasible> {
    let venue = intent.venue.as_str();
    let from_policy = |field: &BTreeMap<String, Decimal>, name: &str| {
        match field.get(venue).copied() {
            // A constraint the centre shipped that no venue could have is a
            // payload bug, and using the model's value instead would hide it
            // behind an order that went out anyway.
            Some(value) if !value.is_positive() && name == "tick" => Err(Infeasible {
                gate: GATE_CONSTRAINT,
                reason: format!(
                    "the policy payload states a {name} of {value} for {venue}, which is not a \
                     grid; the constraint is refused rather than replaced"
                ),
            }),
            Some(value) if value.is_negative() => Err(Infeasible {
                gate: GATE_CONSTRAINT,
                reason: format!(
                    "the policy payload states a {name} of {value} for {venue}, which is \
                     negative; the constraint is refused rather than replaced"
                ),
            }),
            other => Ok(other),
        }
    };
    let (policy_minimum, policy_tick, policy_fee) = match constraints {
        Some(constraints) => (
            from_policy(&constraints.minimum_order, "minimum order")?,
            from_policy(&constraints.tick, "tick")?,
            from_policy(&constraints.fee_floor, "fee floor")?,
        ),
        None => (None, None, None),
    };
    Ok(Effective {
        minimum_notional: policy_minimum.or_else(|| model.map(VenueModel::minimum_notional)),
        tick_size: policy_tick
            .or_else(|| model.map(|model| model.granularity_for(&intent.object_id).tick_size())),
        fee_floor: policy_fee.or_else(|| model.map(VenueModel::fee_floor)),
    })
}

/// Whether `value` sits on the grid of `step`. Both positive.
fn on_grid(value: Decimal, step: Decimal) -> bool {
    value.floor_to_step(step) == value
}

/// Judge one intent against every rule that needs no edge estimate.
///
/// `touch_size` is what rests at the touch on the side the intent takes, or
/// `None` when the book has no such level. The caller reads it from the book
/// at the netting instant, because the gate must not reach for the book: it
/// is a pure function of what it is handed, so a replay judges the same
/// facts.
///
/// The rules run cheapest first and the first refusal is the answer. One
/// refusal per intent, not a list: the series this feeds counts *which* rule
/// binds, and an intent below the minimum that is also off-lot is an intent
/// below the minimum.
pub fn assess(
    model: Option<&VenueModel>,
    constraints: Option<&FeasibilityConstraints>,
    intent: &Intent,
    touch_size: Option<Decimal>,
) -> std::result::Result<(), Infeasible> {
    let quantity = intent.signed_size.abs();
    let price = intent.reference_price;
    let venue = intent.venue.as_str();
    let object = intent.object_id.as_str();
    let effective = effective(model, constraints, intent)?;

    if let Some(model) = model {
        let granularity = model.granularity_for(&intent.object_id);
        if quantity < granularity.minimum_quantity() {
            return Err(Infeasible {
                gate: GATE_MINIMUM_QUANTITY,
                reason: format!(
                    "{quantity} of {object} is below the {} minimum {venue} accepts",
                    granularity.minimum_quantity()
                ),
            });
        }
        if !on_grid(quantity, granularity.lot_size()) {
            return Err(Infeasible {
                gate: GATE_LOT,
                reason: format!(
                    "{quantity} of {object} is not a whole number of lots of {} at {venue}; the \
                     size is refused rather than rounded, because a strategy that does not know \
                     the grid would otherwise trade a size it never reasoned about",
                    granularity.lot_size()
                ),
            });
        }
    }

    if let Some(tick) = effective.tick_size
        && !on_grid(price, tick)
    {
        return Err(Infeasible {
            gate: GATE_TICK,
            reason: format!(
                "{price} is not on the {tick} tick grid for {object} at {venue}; the price is \
                 refused rather than rounded"
            ),
        });
    }

    if let Some(minimum) = effective.minimum_notional {
        let Some(notional) = quantity.checked_mul(price) else {
            return Err(Infeasible {
                gate: GATE_MINIMUM_NOTIONAL,
                reason: format!(
                    "{quantity} of {object} at {price} has a notional that cannot be represented"
                ),
            });
        };
        if notional < minimum {
            return Err(Infeasible {
                gate: GATE_MINIMUM_NOTIONAL,
                reason: format!(
                    "{quantity} of {object} at {price} is {notional} notional, below the \
                     {minimum} minimum {venue} accepts"
                ),
            });
        }
    }

    match touch_size {
        None => Err(Infeasible {
            gate: GATE_DEPTH,
            reason: format!(
                "nothing rests at the touch on the side {object} would take at {venue}"
            ),
        }),
        Some(resting) if quantity > resting => Err(Infeasible {
            gate: GATE_DEPTH,
            reason: format!(
                "{quantity} of {object} exceeds the {resting} resting at the touch at {venue}; \
                 the size is refused rather than reduced, because a reduced cycle leg is a \
                 position and a reduced directional order is a size nobody reasoned about"
            ),
        }),
        Some(_) => Ok(()),
    }
}

/// The fixed cost of sending this intent, as a fraction of its notional.
///
/// Fee floor plus gas floor, over `|size| × price`. A fraction so that legs
/// quoted in different currencies can be summed by [`assess_cycle_cost`]; see
/// the module comment for the precision of that sum. A venue with neither
/// source of a fee floor contributes nothing, which is stated by the `None`
/// arms rather than assumed by a default of zero.
pub fn fixed_cost_fraction(
    model: Option<&VenueModel>,
    constraints: Option<&FeasibilityConstraints>,
    intent: &Intent,
) -> std::result::Result<Decimal, Infeasible> {
    let effective = effective(model, constraints, intent)?;
    let fee = effective.fee_floor.unwrap_or(Decimal::ZERO);
    let gas = model
        .and_then(VenueModel::gas_floor)
        .unwrap_or(Decimal::ZERO);
    let fixed = fee + gas;
    if fixed.is_zero() {
        return Ok(Decimal::ZERO);
    }
    let notional = intent
        .signed_size
        .abs()
        .checked_mul(intent.reference_price)
        .filter(|notional| notional.is_positive());
    let Some(notional) = notional else {
        return Err(Infeasible {
            gate: GATE_FEE_FLOOR,
            reason: format!(
                "{} of {} at {} has no positive notional to charge a fixed fee against",
                intent.signed_size.abs(),
                intent.object_id.as_str(),
                intent.reference_price
            ),
        });
    };
    fixed.checked_div(notional).ok_or_else(|| Infeasible {
        gate: GATE_FEE_FLOOR,
        reason: format!("the fixed cost {fixed} over notional {notional} cannot be represented"),
    })
}

/// Whether a cycle's edge survives the fixed costs of its legs.
///
/// `fixed_cost_fraction` is the sum over legs of [`fixed_cost_fraction`];
/// `edge_fraction` is the cycle's net edge over its start quantity. Refused
/// when the costs meet or exceed the edge: a cycle that exactly pays its fees
/// is a cycle that pays fees to stand still. `gas` says whether any leg
/// landed on chain, so the refusal is counted under the gas gate when it did
/// — §18.1 asks for the gas threshold to be recorded separately, and a chain
/// cycle refused under the fee gate would hide which cost bound.
pub fn assess_cycle_cost(
    fixed_cost_fraction: Decimal,
    edge_fraction: Decimal,
    on_chain: bool,
) -> std::result::Result<(), Infeasible> {
    if fixed_cost_fraction < edge_fraction {
        return Ok(());
    }
    Err(Infeasible {
        gate: if on_chain {
            GATE_GAS_FLOOR
        } else {
            GATE_FEE_FLOOR
        },
        reason: format!(
            "fixed costs of {fixed_cost_fraction} of notional consume an edge of {edge_fraction} \
             of the start size; the cycle is refused whole"
        ),
    })
}

#[cfg(test)]
mod tests {
    //! One passing and one vetoing fixture per rule. The blueprint's delivery
    //! gate demands both, and for the reason the second half of the
    //! infrastructure rule gives: a gate that refuses everything is
    //! indistinguishable, by its refusals, from one that works.

    use super::*;
    use qip_contracts::signal::StrategyId;
    use qip_contracts::venue::VenueId;
    use qip_core::{Timestamp, dec};

    fn intent(size: &str, price: &str) -> Intent {
        Intent::new(
            StrategyId::new("alpha"),
            ObjectId::from_string("ACME"),
            VenueId::new("XLON"),
            Decimal::parse(size).expect("a decimal literal"),
            Decimal::parse(price).expect("a decimal literal"),
            Timestamp::from_secs(60),
        )
        .expect("a non-zero fixture size")
    }

    /// Lot 1, tick 0.01, minimum quantity 5, minimum notional 100, fee 0.5.
    fn model() -> VenueModel {
        VenueModel::new(
            VenueClass::Exchange,
            Granularity::new(dec!("1"), dec!("0.01"), dec!("5")).expect("a valid grid"),
            dec!("0.5"),
            None,
        )
        .expect("a lit venue takes no gas floor")
        .with_minimum_notional(dec!("100"))
        .expect("a positive minimum")
    }

    /// A size and price every rule of [`model`] admits, with depth to spare.
    fn admitted() -> Intent {
        intent("10", "100")
    }

    fn gate_of(outcome: std::result::Result<(), Infeasible>) -> Option<&'static str> {
        outcome.err().map(|infeasible| infeasible.gate)
    }

    #[test]
    fn the_admitted_fixture_passes_every_rule_so_each_veto_below_is_about_its_own_rule() {
        // The premise of every vetoing test in this module: a fixture that
        // was already refused for some other reason proves nothing about the
        // rule it names.
        let outcome = assess(Some(&model()), None, &admitted(), Some(dec!("500")));
        assert_eq!(outcome, Ok(()), "the passing fixture was refused");
    }

    #[test]
    fn a_size_below_the_minimum_quantity_is_refused_and_one_at_it_is_admitted() {
        let below = intent("4", "100");
        let at = intent("5", "100");
        assert!(below.signed_size < model().granularity_for(&below.object_id).minimum_quantity());
        assert_eq!(
            gate_of(assess(Some(&model()), None, &below, Some(dec!("500")))),
            Some(GATE_MINIMUM_QUANTITY)
        );
        // 5 × 100 = 500 notional, on the lot grid, at the minimum exactly.
        assert_eq!(assess(Some(&model()), None, &at, Some(dec!("500"))), Ok(()));
    }

    #[test]
    fn a_size_off_the_lot_grid_is_refused_rather_than_rounded() {
        let off = intent("10.5", "100");
        // Premise: it clears the minimum, so the lot rule is the one that
        // answers — and rounding would have produced 10, which passes.
        assert!(off.signed_size.abs() > dec!("5"));
        assert_eq!(
            gate_of(assess(Some(&model()), None, &off, Some(dec!("500")))),
            Some(GATE_LOT)
        );
        assert_eq!(
            assess(
                Some(&model()),
                None,
                &intent("-11", "100"),
                Some(dec!("500"))
            ),
            Ok(()),
            "a sell on the grid is admitted; the sign is not the grid's business"
        );
    }

    #[test]
    fn a_price_off_the_tick_grid_is_refused_rather_than_rounded() {
        let off = intent("10", "100.005");
        assert_eq!(
            gate_of(assess(Some(&model()), None, &off, Some(dec!("500")))),
            Some(GATE_TICK)
        );
        assert_eq!(
            assess(
                Some(&model()),
                None,
                &intent("10", "100.01"),
                Some(dec!("500"))
            ),
            Ok(())
        );
    }

    #[test]
    fn a_notional_below_the_minimum_is_refused_and_one_at_it_is_admitted() {
        // 9 × 10 = 90, below 100; 10 × 10 = 100, at it. Both clear the
        // minimum quantity of 5 and both are on the lot grid, so the notional
        // rule is the one deciding.
        let below = intent("9", "10");
        assert_eq!(
            gate_of(assess(Some(&model()), None, &below, Some(dec!("500")))),
            Some(GATE_MINIMUM_NOTIONAL)
        );
        assert_eq!(
            assess(Some(&model()), None, &intent("10", "10"), Some(dec!("500"))),
            Ok(())
        );
    }

    #[test]
    fn a_size_larger_than_the_touch_is_refused_rather_than_reduced() {
        let want = admitted();
        assert_eq!(
            gate_of(assess(Some(&model()), None, &want, Some(dec!("9")))),
            Some(GATE_DEPTH)
        );
        assert_eq!(
            gate_of(assess(Some(&model()), None, &want, None)),
            Some(GATE_DEPTH),
            "a side with nothing resting has no touch to take"
        );
        assert_eq!(
            assess(Some(&model()), None, &want, Some(dec!("10"))),
            Ok(())
        );
    }

    #[test]
    fn a_venue_with_no_model_and_no_constraints_is_checked_for_depth_alone() {
        // Stated in the module comment; asserted here so the comment cannot
        // drift. An off-lot, off-tick, tiny intent passes with no model, and
        // the depth rule still fires.
        let anything = intent("0.5", "100.005");
        assert_eq!(assess(None, None, &anything, Some(dec!("1"))), Ok(()));
        assert_eq!(
            gate_of(assess(None, None, &anything, Some(dec!("0.1")))),
            Some(GATE_DEPTH)
        );
    }

    fn constraints(venue: &str, tick: &str, minimum: &str, fee: &str) -> FeasibilityConstraints {
        let parse = |literal: &str| Decimal::parse(literal).expect("a decimal literal");
        FeasibilityConstraints {
            minimum_order: [(venue.to_string(), parse(minimum))].into_iter().collect(),
            fee_floor: [(venue.to_string(), parse(fee))].into_iter().collect(),
            tick: [(venue.to_string(), parse(tick))].into_iter().collect(),
        }
    }

    #[test]
    fn a_policy_constraint_overrides_the_model_and_a_bad_one_is_refused_not_replaced() {
        // The model admits 100.01 on a 0.01 tick; the centre says the tick
        // is now 0.1, and the intent is refused on the centre's grid.
        let want = intent("10", "100.01");
        assert_eq!(
            assess(Some(&model()), None, &want, Some(dec!("500"))),
            Ok(())
        );
        let coarser = constraints("XLON", "0.1", "100", "0.5");
        assert_eq!(
            gate_of(assess(
                Some(&model()),
                Some(&coarser),
                &want,
                Some(dec!("500"))
            )),
            Some(GATE_TICK)
        );
        // A constraint for another venue changes nothing here.
        let elsewhere = constraints("XNYS", "0.1", "100", "0.5");
        assert_eq!(
            assess(Some(&model()), Some(&elsewhere), &want, Some(dec!("500"))),
            Ok(())
        );
        // A tick of zero is not a grid, and the answer is a refusal under the
        // constraint gate — not the model's tick quietly used instead.
        let broken = constraints("XLON", "0", "100", "0.5");
        assert_eq!(
            gate_of(assess(
                Some(&model()),
                Some(&broken),
                &want,
                Some(dec!("500"))
            )),
            Some(GATE_CONSTRAINT)
        );
    }

    #[test]
    fn the_policy_constraints_apply_to_a_venue_with_no_model_at_all() {
        // Item 11 of the payload has to work for a venue the composition root
        // never modelled, or the slot's first consumer would depend on a
        // second source being present.
        let want = intent("10", "5");
        assert_eq!(assess(None, None, &want, Some(dec!("500"))), Ok(()));
        let minimum = constraints("XLON", "0.01", "100", "0");
        assert_eq!(
            gate_of(assess(None, Some(&minimum), &want, Some(dec!("500")))),
            Some(GATE_MINIMUM_NOTIONAL)
        );
    }

    #[test]
    fn the_fixed_cost_fraction_is_fee_plus_gas_over_notional() {
        // Fee 0.5 on 10 × 100 = 1000 notional is 0.0005.
        let fraction = fixed_cost_fraction(Some(&model()), None, &admitted());
        assert_eq!(fraction, Ok(dec!("0.0005")));
        // With no source of a fee at all the fraction is zero, and stated so.
        assert_eq!(
            fixed_cost_fraction(None, None, &admitted()),
            Ok(Decimal::ZERO)
        );
    }

    #[test]
    fn a_cycle_whose_fixed_costs_consume_its_edge_is_refused_under_the_fee_gate() {
        // Edge of 0.001 of start; costs of 0.0009 survive, 0.001 do not.
        assert_eq!(
            assess_cycle_cost(dec!("0.0009"), dec!("0.001"), false),
            Ok(())
        );
        assert_eq!(
            assess_cycle_cost(dec!("0.001"), dec!("0.001"), false)
                .err()
                .map(|i| i.gate),
            Some(GATE_FEE_FLOOR),
            "a cycle that exactly pays its fees pays fees to stand still"
        );
    }

    #[test]
    fn a_chain_cycle_refused_on_cost_is_counted_under_the_gas_gate() {
        assert_eq!(
            assess_cycle_cost(dec!("0.002"), dec!("0.001"), true)
                .err()
                .map(|i| i.gate),
            Some(GATE_GAS_FLOOR)
        );
        assert_eq!(
            assess_cycle_cost(dec!("0.0005"), dec!("0.001"), true),
            Ok(())
        );
    }

    #[test]
    fn a_chain_venue_needs_a_gas_floor_and_a_lit_venue_refuses_one() {
        let grid = Granularity::new(dec!("1"), dec!("0.01"), dec!("0")).expect("a valid grid");
        assert!(
            VenueModel::new(VenueClass::DecentralisedExchange, grid, dec!("0"), None).is_err(),
            "a chain venue with no gas floor prices landing an order as free"
        );
        assert!(
            VenueModel::new(VenueClass::Exchange, grid, dec!("0"), Some(dec!("1"))).is_err(),
            "a gas floor on a lit exchange is a cost nothing there charges"
        );
        let chain = VenueModel::new(
            VenueClass::DecentralisedExchange,
            grid,
            dec!("0"),
            Some(dec!("2")),
        )
        .expect("a chain venue with a gas floor");
        // Gas 2 on 10 × 100 = 1000 is 0.002 of notional.
        assert_eq!(
            fixed_cost_fraction(Some(&chain), None, &admitted()),
            Ok(dec!("0.002"))
        );
    }

    #[test]
    fn a_grid_that_is_not_positive_is_refused_at_construction() {
        assert!(Granularity::new(dec!("0"), dec!("0.01"), dec!("0")).is_err());
        assert!(Granularity::new(dec!("1"), dec!("-0.01"), dec!("0")).is_err());
        assert!(Granularity::new(dec!("1"), dec!("0.01"), dec!("-1")).is_err());
        assert!(model().with_minimum_notional(dec!("-1")).is_err());
    }

    #[test]
    fn a_per_instrument_grid_overrides_the_venue_default_for_that_instrument_only() {
        let fine = Granularity::new(dec!("0.001"), dec!("0.0001"), dec!("0")).expect("a grid");
        let venue_model = model().with_instrument(ObjectId::from_string("ACME"), fine);
        // ACME is now on the fine grid: 10.5 at 100.0001 passes.
        let fine_intent = intent("10.5", "100.0001");
        assert_eq!(
            assess(Some(&venue_model), None, &fine_intent, Some(dec!("500"))),
            Ok(())
        );
        // Another instrument at the same venue is still on the coarse one.
        let other = Intent::new(
            StrategyId::new("alpha"),
            ObjectId::from_string("OTHER"),
            VenueId::new("XLON"),
            dec!("10.5"),
            dec!("100"),
            Timestamp::from_secs(60),
        )
        .expect("a non-zero fixture size");
        assert_eq!(
            gate_of(assess(Some(&venue_model), None, &other, Some(dec!("500")))),
            Some(GATE_LOT)
        );
    }
}
