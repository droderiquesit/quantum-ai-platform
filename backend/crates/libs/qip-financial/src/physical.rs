//! Blueprint §17.4's first two rows: what it costs to move a physical thing
//! from where it is to where it is worth more, and how much of it arrives.
//!
//! This is the "genuinely different cost model" §17.4 names, and the
//! difference is not the size of the numbers. A financial trade's cost is a
//! spread, a fee and an impact, and the quantity that settles is the quantity
//! that traded. A physical trade has two properties no
//! [`crate::costs::TransactionCostModel`] can express:
//!
//! * **Less arrives than left.** Spoilage, shrinkage and damage are a
//!   quantity loss compounding over the days in transit and the days in
//!   store, so the cost per *delivered* unit is not the cost per shipped
//!   unit, and a model that reports the second understates the first by
//!   exactly the loss.
//! * **Time is an input, not a latency.** Sea freight takes six weeks and
//!   customs clearance takes however long it takes; storage accrues over
//!   both, and spoilage compounds over both. A cost that does not read the
//!   calendar is a cost for a trade that settles instantly.
//!
//! [`LandedCost`] is therefore stated in two currencies of quantity — what
//! shipped, what was delivered, and what could be sold after returns — and
//! every component is named separately rather than rolled into one figure,
//! because "the freight was the problem" and "the duty was the problem" lead
//! a desk to two different places.
//!
//! # What this model is not, and this is the important part
//!
//! **Nothing in this platform ingests a freight rate, a duty schedule, a
//! spoilage rate or a marketplace fee table.** Every rate here arrives as a
//! declared term on [`LogisticsTerms`], supplied by whoever knows it. There
//! is no connector, no catalogue entry and no default: [`LogisticsTerms`] has
//! no `Default` implementation on purpose, because a default freight rate
//! would be a number this repository invented and then quoted back as though
//! it had measured it.
//!
//! [`Spoilage`] goes further and makes the provenance structural: a spoilage
//! rate must name its source, and a blank one is refused at construction. A
//! rate somebody read off a trade association's table is a fact with a
//! citation; the same figure with no citation is a guess that will be quoted
//! forward by everyone who reads it. The type will not hold the second.
//!
//! So this module has **no production caller and no default anything**, and
//! that is the honest state rather than a gap somebody forgot to close. What
//! would give it one is an ingestion path for carrier tariffs and a customs
//! schedule, which §17.4 itself says the row is "reachable with".
//!
//! # Money
//!
//! Every cost, price, rate and basis here is [`Decimal`]. There is no `f64`
//! in this module: a spoilage rate is a fraction of a quantity of goods, a
//! duty rate is a fraction of a declared value, and both are money's
//! arithmetic rather than a statistic. Routes iterate in the order they were
//! declared, which is the order the goods travel, and that order is checked
//! at construction rather than assumed.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The longest a route plus its storage may run.
///
/// Ten years. Spoilage compounds day by day and the loop that does it is
/// bounded by this; a route stated in days that ran to a hundred thousand
/// would be an arithmetic loop chosen by whoever wrote the terms. It is a
/// refusal rather than a cap, so a genuinely long-dated arrangement is told
/// to say so rather than silently costed over a decade.
pub const MAXIMUM_ROUTE_DAYS: u32 = 3_650;

/// How a leg moves.
///
/// Not decoration: the mode is what a reader needs to know why a leg takes
/// forty days, and it is the first thing anybody asks when a landed cost
/// comes out wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TransportMode {
    Sea,
    Rail,
    Road,
    Air,
}

impl TransportMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sea => "sea",
            Self::Rail => "rail",
            Self::Road => "road",
            Self::Air => "air",
        }
    }
}

/// One movement, from somewhere to somewhere else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub from: String,
    pub to: String,
    pub mode: TransportMode,
    /// Days in transit on this leg.
    pub days: u32,
    /// Freight charged per unit shipped on this leg.
    pub freight_per_unit: Decimal,
    /// Charged once for the shipment, whatever its size — a booking fee, a
    /// container, a customs broker's attendance.
    pub freight_per_shipment: Decimal,
}

impl Leg {
    /// The leg as a reader needs it: where, how, and for how long.
    ///
    /// The mode is on the leg so that it reaches [`LandedCost::route`] and
    /// through it [`LandedCost::describe`]. A mode recorded and never read
    /// would be a field that looks like information; it is the first thing
    /// anybody asks when a landed cost comes out wrong, because it is what
    /// explains a forty-day leg.
    pub fn describe(&self) -> String {
        format!(
            "{} -{}, {}d-> {}",
            self.from,
            self.mode.as_str(),
            self.days,
            self.to
        )
    }
}

/// What customs costs and how long it holds the goods.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Customs {
    /// Duty as a fraction of the declared value, in `[0, 1)`.
    pub duty_rate: Decimal,
    /// Charged once per shipment.
    pub clearance_fee: Decimal,
    /// Days the goods sit at the border. They spoil and they accrue storage
    /// while they do, which is why this is a separate figure from the legs'
    /// transit and not folded into one of them.
    pub clearance_days: u32,
}

/// A quantity loss rate, and where the rate came from.
///
/// The provenance is a field and not a comment, and it is refused when
/// blank. A spoilage rate is the one input in this module most likely to be
/// invented: freight has an invoice, duty has a schedule, and shrinkage has
/// whatever somebody remembers. A figure carried with its source is a figure
/// the next reader can check; the same figure alone is quoted forward for
/// ever by people who assume somebody measured it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spoilage {
    /// Fraction of the quantity still on hand lost per day, in `[0, 1)`.
    /// Compounding, because what spoils on day two spoils out of what
    /// survived day one.
    per_day: Decimal,
    /// Where the rate came from, in the words of whoever supplied it.
    source: String,
}

impl Spoilage {
    /// State a spoilage rate and its source.
    ///
    /// Refuses a rate outside `[0, 1)` and a blank source. One or more would
    /// mean the entire consignment is lost every day, which is not a rate;
    /// negative would mean the goods multiply in transit.
    pub fn new(per_day: Decimal, source: impl Into<String>) -> Result<Self> {
        if per_day.is_negative() || per_day >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "a spoilage rate of {per_day} per day is outside [0, 1); a rate of one loses the \
                 whole consignment on the first day and a negative one has the goods multiplying \
                 in transit — state the daily loss as a fraction"
            )));
        }
        let source = source.into();
        if source.trim().is_empty() {
            return Err(Error::invalid(
                "a spoilage rate must name where it came from; this platform measures no \
                 shrinkage, so a rate with no source is a figure somebody chose and every later \
                 reader will quote it forward as a measurement",
            ));
        }
        Ok(Self { per_day, source })
    }

    /// No loss at all — for a good that does not perish, which still has to
    /// say so.
    pub fn none(source: impl Into<String>) -> Result<Self> {
        Self::new(Decimal::ZERO, source)
    }

    pub fn per_day(&self) -> Decimal {
        self.per_day
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// What survives `days` of holding, out of `quantity`.
    ///
    /// Compounded day by day rather than `quantity * (1 - rate * days)`,
    /// which goes negative past `1 / rate` days and would report a
    /// consignment that arrived owing goods. The loop is bounded by
    /// [`MAXIMUM_ROUTE_DAYS`], checked by the caller before it starts.
    pub fn surviving(&self, quantity: Decimal, days: u32) -> Result<Decimal> {
        let retained = Decimal::ONE - self.per_day;
        let mut remaining = quantity;
        for _ in 0..days {
            remaining = remaining.checked_mul(retained).ok_or_else(|| {
                Error::numeric("the surviving quantity overflowed while compounding spoilage")
            })?;
        }
        Ok(remaining)
    }
}

/// What a marketplace takes out of the sale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketplaceFees {
    /// A fraction of the sale proceeds, in `[0, 1)`.
    pub ad_valorem: Decimal,
    /// Charged per unit sold.
    pub per_unit: Decimal,
    /// Charged once per consignment.
    pub per_consignment: Decimal,
}

/// Goods that come back, and what handling them costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Returns {
    /// Fraction of delivered units returned, in `[0, 1)`.
    pub rate: Decimal,
    /// Cost of taking one unit back — inbound freight, inspection,
    /// restocking. Charged on the returned units, not on the sold ones.
    pub cost_per_unit: Decimal,
    /// Fraction of a returned unit's value recovered on resale, in `[0, 1]`.
    /// One means a return is resold at full price; zero means it is scrap.
    pub recovery_rate: Decimal,
}

/// Everything declared about moving one consignment.
///
/// No `Default`. A default freight rate is a number this repository would
/// have invented; see the module documentation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogisticsTerms {
    /// The route, in the order the goods travel.
    pub route: Vec<Leg>,
    pub customs: Customs,
    pub spoilage: Spoilage,
    /// Storage charged per unit on hand per day, for every day of transit,
    /// clearance and warehousing.
    pub storage_per_unit_per_day: Decimal,
    /// Days in store at the destination before sale.
    pub storage_days: u32,
    pub fees: MarketplaceFees,
    pub returns: Returns,
}

/// What a consignment cost to land and to sell, component by component.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LandedCost {
    /// The origin of the first leg and the destination of the last, so the
    /// figure says what it costed.
    pub origin: String,
    pub destination: String,
    /// Every leg, in travel order, each naming its mode and its transit days.
    /// The one place a reader learns *why* a consignment took six weeks, and
    /// the reason [`Leg::mode`] is a field rather than decoration.
    pub route: Vec<String>,
    /// Days from the first leg's departure to the sale.
    pub elapsed_days: u32,

    pub quantity_shipped: Decimal,
    /// What arrived, after spoilage over transit and clearance.
    pub quantity_delivered: Decimal,
    /// What was sold, after returns out of what was delivered — net of the
    /// part of a return that is resold.
    pub quantity_sold: Decimal,

    /// What the goods cost at origin.
    pub goods: Decimal,
    pub freight: Decimal,
    pub duty: Decimal,
    pub clearance: Decimal,
    pub storage: Decimal,
    pub marketplace: Decimal,
    /// Handling the returns, net of what their resale recovered.
    pub returns: Decimal,
}

impl LandedCost {
    /// Everything paid out.
    pub fn total(&self) -> Decimal {
        self.goods
            + self.freight
            + self.duty
            + self.clearance
            + self.storage
            + self.marketplace
            + self.returns
    }

    /// Quantity that left and did not arrive.
    pub fn spoiled(&self) -> Decimal {
        self.quantity_shipped - self.quantity_delivered
    }

    /// Total cost divided over the units that arrived.
    ///
    /// Refuses rather than answering on a consignment where nothing arrived:
    /// a cost per unit on zero units is a division this model will not
    /// perform, and returning zero would report a free consignment.
    pub fn per_delivered_unit(&self) -> Result<Decimal> {
        self.total()
            .checked_div(self.quantity_delivered)
            .ok_or_else(|| {
                Error::numeric(
                    "nothing arrived, so there is no cost per delivered unit; the consignment's \
                     total cost is the figure to read",
                )
            })
    }

    /// Total cost divided over the units that were sold.
    pub fn per_sold_unit(&self) -> Result<Decimal> {
        self.total().checked_div(self.quantity_sold).ok_or_else(|| {
            Error::numeric(
                "nothing was sold, so there is no cost per sold unit; the consignment's total \
                 cost is the figure to read",
            )
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "{} shipped {} over {} day(s): {} arrived, {} sold, {} landed cost (goods {}, \
             freight {}, duty {}, clearance {}, storage {}, marketplace {}, returns {})",
            self.quantity_shipped,
            self.route.join(" then "),
            self.elapsed_days,
            self.quantity_delivered,
            self.quantity_sold,
            self.total(),
            self.goods,
            self.freight,
            self.duty,
            self.clearance,
            self.storage,
            self.marketplace,
            self.returns
        )
    }
}

/// What a physical arbitrage is worth once the goods have actually moved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicalArbitrage {
    pub cost: LandedCost,
    /// The price used at the destination, after the location and grade
    /// adjustments.
    pub realised_price: Decimal,
    /// Proceeds of the units sold at that price.
    pub proceeds: Decimal,
}

impl PhysicalArbitrage {
    /// Proceeds less everything paid. Negative is the whole point of the
    /// model: the gross spread that looked like an arbitrage.
    pub fn margin(&self) -> Decimal {
        self.proceeds - self.cost.total()
    }

    pub fn is_viable(&self) -> bool {
        self.margin().is_positive()
    }

    pub fn describe(&self) -> String {
        format!(
            "{} at {} is {} of proceeds against {} landed, a margin of {}; {}",
            self.cost.quantity_sold,
            self.realised_price,
            self.proceeds,
            self.cost.total(),
            self.margin(),
            if self.is_viable() {
                "viable"
            } else {
                "the spread does not survive landing it"
            }
        )
    }
}

impl LogisticsTerms {
    /// Days from departure to sale: every leg's transit, the border, and the
    /// warehouse.
    pub fn elapsed_days(&self) -> Result<u32> {
        let mut days: u32 = self.customs.clearance_days;
        for leg in &self.route {
            days = days.checked_add(leg.days).ok_or_else(|| {
                Error::numeric("the route's transit days overflowed; state a shorter route")
            })?;
        }
        days = days.checked_add(self.storage_days).ok_or_else(|| {
            Error::numeric("the route's total days overflowed; state a shorter route")
        })?;
        Ok(days)
    }

    /// Refuse terms that cannot describe a real movement.
    ///
    /// Every one of these is the caller's model being wrong rather than a
    /// transient condition, and each names what to do instead:
    ///
    /// * an empty route — a physical trade that moves nothing is a financial
    ///   trade, and costing it here would report freight of zero on a
    ///   consignment nobody shipped;
    /// * a route that does not join up, where one leg ends somewhere the next
    ///   does not begin. The refusal names both places. This is the mistake
    ///   an assembled route actually makes, and a model that quietly costed
    ///   the legs anyway would produce a plausible number for a journey that
    ///   cannot happen;
    /// * a negative freight, fee, duty, storage or handling figure — a cost
    ///   that pays you is a sign error, and it would show up as a landed cost
    ///   below the price of the goods;
    /// * a duty, fee, returns or recovery rate outside its bounds;
    /// * a route longer than [`MAXIMUM_ROUTE_DAYS`].
    pub fn validate(&self) -> Result<()> {
        let Some(first) = self.route.first() else {
            return Err(Error::invalid(
                "a consignment needs at least one leg; a physical trade that moves nothing is a \
                 financial trade, and this model would report freight of zero on it",
            ));
        };
        if first.from.trim().is_empty() {
            return Err(Error::invalid(
                "the first leg must name where the goods start; an unnamed origin makes the \
                 landed cost a figure about nowhere",
            ));
        }
        for window in self.route.windows(2) {
            let (before, after) = (&window[0], &window[1]);
            if before.to != after.from {
                return Err(Error::invalid(format!(
                    "the route does not join up: a leg ends at {} and the next begins at {}; \
                     insert the missing leg or correct the names — costing the legs as stated \
                     would price a journey that cannot be made",
                    before.to, after.from
                )));
            }
        }
        for leg in &self.route {
            if leg.to.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "the leg out of {} does not name where it ends",
                    leg.from
                )));
            }
            if leg.freight_per_unit.is_negative() || leg.freight_per_shipment.is_negative() {
                return Err(Error::invalid(format!(
                    "the leg {} to {} states a negative freight; a carrier that pays to take the \
                     goods is a sign error, and it would land the consignment below the price of \
                     the goods",
                    leg.from, leg.to
                )));
            }
        }
        Self::fraction("the duty rate", self.customs.duty_rate)?;
        Self::fraction("the marketplace ad valorem fee", self.fees.ad_valorem)?;
        Self::fraction("the returns rate", self.returns.rate)?;
        if self.returns.recovery_rate.is_negative() || self.returns.recovery_rate > Decimal::ONE {
            return Err(Error::invalid(format!(
                "the recovery rate on a return is {}, outside [0, 1]; one means a return resells \
                 at full price and zero means it is scrap — a rate above one would have returns \
                 making money",
                self.returns.recovery_rate
            )));
        }
        for (what, amount) in [
            ("the clearance fee", self.customs.clearance_fee),
            ("the storage rate", self.storage_per_unit_per_day),
            ("the marketplace per-unit fee", self.fees.per_unit),
            (
                "the marketplace per-consignment fee",
                self.fees.per_consignment,
            ),
            ("the cost of handling a return", self.returns.cost_per_unit),
        ] {
            if amount.is_negative() {
                return Err(Error::invalid(format!(
                    "{what} is {amount}; a cost that pays you is a sign error"
                )));
            }
        }
        let days = self.elapsed_days()?;
        if days > MAXIMUM_ROUTE_DAYS {
            return Err(Error::invalid(format!(
                "the route, clearance and storage come to {days} days against a bound of \
                 {MAXIMUM_ROUTE_DAYS}; a consignment held longer than that is a warehousing \
                 arrangement and should be costed as one"
            )));
        }
        Ok(())
    }

    fn fraction(what: &str, rate: Decimal) -> Result<()> {
        if rate.is_negative() || rate >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "{what} is {rate}, outside [0, 1); state it as a fraction of value"
            )));
        }
        Ok(())
    }

    /// Cost one consignment from origin to sale.
    ///
    /// `unit_cost` is what the goods cost where they start; `declared_value`
    /// is what customs charges duty on, per unit, which is not always the
    /// same figure and is a separate argument for that reason. `sale_price`
    /// is per unit at the destination, already adjusted for location basis
    /// and grade differential — see [`delivered_price`].
    ///
    /// Storage accrues on the quantity **still on hand**, day by day, rather
    /// than on the quantity shipped: a consignment that has lost a third of
    /// itself is not paying to store the third that is gone.
    pub fn cost(
        &self,
        quantity: Decimal,
        unit_cost: Decimal,
        declared_value_per_unit: Decimal,
    ) -> Result<LandedCost> {
        self.validate()?;
        if !quantity.is_positive() {
            return Err(Error::invalid(format!(
                "a consignment of {quantity} is not a consignment; ship a positive quantity"
            )));
        }
        if unit_cost.is_negative() || declared_value_per_unit.is_negative() {
            return Err(Error::invalid(
                "the goods' cost and their declared value are amounts and cannot be negative",
            ));
        }

        let goods = Self::mul("the cost of the goods", quantity, unit_cost)?;

        let mut freight = Decimal::ZERO;
        for leg in &self.route {
            freight += Self::mul("the freight on a leg", quantity, leg.freight_per_unit)?
                + leg.freight_per_shipment;
        }

        // Duty on the declared value of what arrives at the border, which is
        // what left less what spoiled in transit. Charging duty on the
        // shipped quantity would pay the border for goods that never reached
        // it.
        let transit_days: u32 = self.route.iter().map(|leg| leg.days).sum();
        let at_border = self.spoilage.surviving(quantity, transit_days)?;
        let declared = Self::mul(
            "the declared value at the border",
            at_border,
            declared_value_per_unit,
        )?;
        let duty = Self::mul("the duty", declared, self.customs.duty_rate)?;
        let clearance = self.customs.clearance_fee;

        // Storage on what is on hand, day by day, across transit, clearance
        // and the warehouse. One loop rather than three, because the
        // surviving quantity carries forward.
        let total_days = self.elapsed_days()?;
        let mut on_hand = quantity;
        let mut storage = Decimal::ZERO;
        let retained = Decimal::ONE - self.spoilage.per_day();
        for _ in 0..total_days {
            storage += Self::mul("the storage charge", on_hand, self.storage_per_unit_per_day)?;
            on_hand = on_hand.checked_mul(retained).ok_or_else(|| {
                Error::numeric("the quantity on hand overflowed while compounding spoilage")
            })?;
        }
        let delivered = on_hand;

        let returned = Self::mul("the returned quantity", delivered, self.returns.rate)?;
        let resold = Self::mul("the resold quantity", returned, self.returns.recovery_rate)?;
        let sold = delivered - returned + resold;
        let returns_cost = Self::mul(
            "the cost of handling returns",
            returned,
            self.returns.cost_per_unit,
        )?;

        let marketplace = Self::mul("the marketplace per-unit fee", sold, self.fees.per_unit)?
            + self.fees.per_consignment;

        let origin = self
            .route
            .first()
            .map(|leg| leg.from.clone())
            .unwrap_or_default();
        let destination = self
            .route
            .last()
            .map(|leg| leg.to.clone())
            .unwrap_or_default();

        Ok(LandedCost {
            origin,
            destination,
            route: self.route.iter().map(Leg::describe).collect(),
            elapsed_days: total_days,
            quantity_shipped: quantity,
            quantity_delivered: delivered,
            quantity_sold: sold,
            goods,
            freight,
            duty,
            clearance,
            storage,
            marketplace,
            returns: returns_cost,
        })
    }

    /// Cost the consignment and sell it, at a price already adjusted for
    /// where and what it is.
    ///
    /// The ad valorem marketplace fee is charged on the proceeds and so is
    /// added here rather than in [`Self::cost`], which does not know the
    /// price. It goes onto the cost's `marketplace` component, so `total`
    /// stays the total.
    pub fn arbitrage(
        &self,
        quantity: Decimal,
        unit_cost: Decimal,
        declared_value_per_unit: Decimal,
        sale_price: Decimal,
    ) -> Result<PhysicalArbitrage> {
        if sale_price.is_negative() {
            return Err(Error::invalid(
                "a sale price cannot be negative; a consignment nobody will take is a disposal \
                 cost and belongs in the returns terms",
            ));
        }
        let mut cost = self.cost(quantity, unit_cost, declared_value_per_unit)?;
        let proceeds = Self::mul("the proceeds", cost.quantity_sold, sale_price)?;
        cost.marketplace += Self::mul(
            "the marketplace ad valorem fee",
            proceeds,
            self.fees.ad_valorem,
        )?;
        Ok(PhysicalArbitrage {
            cost,
            realised_price: sale_price,
            proceeds,
        })
    }

    fn mul(what: &str, left: Decimal, right: Decimal) -> Result<Decimal> {
        left.checked_mul(right)
            .ok_or_else(|| Error::numeric(format!("{what} overflowed")))
    }
}

/// The price of a grade at a location, from a reference price and two bases.
///
/// §17.4's "delivery location" and "grade differential" rows, and they are
/// arithmetic rather than a model: a basis is a quoted difference, positive
/// or negative, and the only thing worth holding is that both are applied and
/// neither is silently dropped. A negative delivered price is refused —
/// a price below zero is a disposal, and a physical trade that pays somebody
/// to take the goods is a different trade with different terms.
pub fn delivered_price(
    reference: Decimal,
    location_basis: Decimal,
    grade_basis: Decimal,
) -> Result<Decimal> {
    let price = reference + location_basis + grade_basis;
    if price.is_negative() {
        return Err(Error::invalid(format!(
            "a reference of {reference} with a location basis of {location_basis} and a grade \
             basis of {grade_basis} prices the goods at {price}; a negative delivered price is a \
             disposal, which is a different trade — state it as one rather than as a sale"
        )));
    }
    Ok(price)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn leg(from: &str, to: &str, days: u32, per_unit: &str) -> Leg {
        Leg {
            from: from.to_string(),
            to: to.to_string(),
            mode: TransportMode::Sea,
            days,
            freight_per_unit: Decimal::parse(per_unit).unwrap_or(Decimal::ZERO),
            freight_per_shipment: Decimal::ZERO,
        }
    }

    fn terms(route: Vec<Leg>, spoilage: Spoilage) -> LogisticsTerms {
        LogisticsTerms {
            route,
            customs: Customs {
                duty_rate: Decimal::ZERO,
                clearance_fee: Decimal::ZERO,
                clearance_days: 0,
            },
            spoilage,
            storage_per_unit_per_day: Decimal::ZERO,
            storage_days: 0,
            fees: MarketplaceFees {
                ad_valorem: Decimal::ZERO,
                per_unit: Decimal::ZERO,
                per_consignment: Decimal::ZERO,
            },
            returns: Returns {
                rate: Decimal::ZERO,
                cost_per_unit: Decimal::ZERO,
                recovery_rate: Decimal::ZERO,
            },
        }
    }

    fn no_spoilage() -> Spoilage {
        Spoilage::none("a non-perishable good, stated by the desk").expect("a stated zero rate")
    }

    #[test]
    fn a_spoilage_rate_with_no_source_is_refused_and_the_same_rate_with_one_is_admitted() {
        // The one input in this module nobody measures. A rate carried with
        // its source is a figure the next reader can check; the same figure
        // alone gets quoted forward for ever by people who assume somebody
        // measured it. The admitting half matters as much — a gate that
        // refused every rate would just mean nobody used the model.
        let blank = Spoilage::new(dec!("0.01"), "   ");
        let message = blank
            .expect_err("a sourceless rate was admitted")
            .message()
            .to_string();
        assert!(
            message.contains("must name where it came from"),
            "the refusal did not ask for the source: {message}"
        );

        let sourced = Spoilage::new(dec!("0.01"), "carrier's 2026 tariff schedule, table 4")
            .expect("a sourced rate");
        assert_eq!(sourced.per_day(), dec!("0.01"));
        assert_eq!(sourced.source(), "carrier's 2026 tariff schedule, table 4");
    }

    #[test]
    fn a_spoilage_rate_of_one_or_more_is_refused_rather_than_clamped() {
        // One loses the whole consignment on day one and every day after,
        // which is not a rate. Clamped to 0.99 it becomes a plausible number
        // for a catastrophic good. Just under one is admitted, so the bound
        // is a bound.
        let message = Spoilage::new(Decimal::ONE, "somewhere")
            .expect_err("a rate of one was admitted")
            .message()
            .to_string();
        assert!(
            message.contains("outside [0, 1)"),
            "the refusal did not name the bound: {message}"
        );
        assert!(Spoilage::new(dec!("0.99"), "somewhere").is_ok());
        assert!(Spoilage::new(dec!("-0.01"), "somewhere").is_err());
    }

    #[test]
    fn spoilage_compounds_so_a_long_route_never_delivers_a_negative_quantity() {
        // The arithmetic this model exists to get right. A linear loss —
        // quantity * (1 - rate * days) — at 1% a day over two hundred days
        // reports minus one unit delivered, and a landed cost per unit
        // computed off it would be a negative number presented as a cost.
        // Compounded, the same route delivers a small positive quantity.
        let spoilage = Spoilage::new(dec!("0.01"), "the desk's own shrinkage log").expect("rate");
        let linear_would_be = dec!("100") * (Decimal::ONE - dec!("0.01") * dec!("200"));
        assert!(
            linear_would_be.is_negative(),
            "the premise is that the linear form goes negative here: {linear_would_be}"
        );

        let surviving = spoilage.surviving(dec!("100"), 200).expect("surviving");
        assert!(surviving.is_positive(), "{surviving}");
        assert!(surviving < dec!("15"), "{surviving}");
        // And a day's loss is a loss out of what survived the day before.
        let one_day = spoilage.surviving(dec!("100"), 1).expect("one day");
        let two_days = spoilage.surviving(dec!("100"), 2).expect("two days");
        assert_eq!(one_day, dec!("99"));
        assert_eq!(two_days, dec!("98.01"));
    }

    #[test]
    fn a_route_that_does_not_join_up_is_refused_and_a_route_that_does_is_costed() {
        // The mistake an assembled route actually makes. Costing the legs
        // anyway produces a perfectly plausible landed cost for a journey
        // that cannot be made, and nothing downstream could tell.
        let broken = terms(
            vec![
                leg("Santos", "Rotterdam", 20, "5"),
                leg("Hamburg", "Warsaw", 3, "2"),
            ],
            no_spoilage(),
        );
        let message = broken
            .cost(dec!("100"), dec!("50"), dec!("50"))
            .expect_err("a broken route was costed")
            .message()
            .to_string();
        assert!(
            message.contains("ends at Rotterdam and the next begins at Hamburg"),
            "the refusal did not name the gap: {message}"
        );

        let joined = terms(
            vec![
                leg("Santos", "Rotterdam", 20, "5"),
                leg("Rotterdam", "Warsaw", 3, "2"),
            ],
            no_spoilage(),
        );
        let cost = joined
            .cost(dec!("100"), dec!("50"), dec!("50"))
            .expect("a joined route costs");
        assert_eq!(cost.origin, "Santos");
        assert_eq!(cost.destination, "Warsaw");
        assert_eq!(cost.freight, dec!("700"));
        assert_eq!(cost.elapsed_days, 23);
        // The mode reaches the reader. `Leg::mode` is the first thing anybody
        // asks about when a landed cost comes out wrong — it is what explains
        // a twenty-day leg — and a mode stored on a leg that no output ever
        // names would be a field that looks like information and is not.
        assert_eq!(
            cost.route,
            vec![
                "Santos -sea, 20d-> Rotterdam".to_string(),
                "Rotterdam -sea, 3d-> Warsaw".to_string(),
            ]
        );
        assert!(
            cost.describe().contains("-sea, 20d->"),
            "the description did not name the mode: {}",
            cost.describe()
        );
    }

    #[test]
    fn duty_is_charged_on_what_reaches_the_border_and_not_on_what_was_shipped() {
        // Ten percent a day for ten days leaves about a third of the
        // consignment at the border. Charging duty on the shipped quantity
        // pays the border for goods that never reached it, and on a
        // perishable that is the difference between a viable trade and a
        // loss.
        let spoilage = Spoilage::new(dec!("0.1"), "the supplier's stated shrinkage").expect("rate");
        let mut declared = terms(vec![leg("Lagos", "Lisbon", 10, "0")], spoilage);
        declared.customs.duty_rate = dec!("0.2");

        let cost = declared
            .cost(dec!("1000"), dec!("1"), dec!("1"))
            .expect("cost");
        assert!(
            cost.quantity_delivered < dec!("400"),
            "the premise is heavy spoilage: {}",
            cost.quantity_delivered
        );
        // Shipped-quantity duty would be 1000 * 1 * 0.2 = 200.
        assert!(
            cost.duty < dec!("100"),
            "duty was charged on the shipped quantity: {}",
            cost.duty
        );
        assert!(
            cost.duty > dec!("60"),
            "duty was not charged at all: {}",
            cost.duty
        );
        assert_eq!(cost.spoiled(), dec!("1000") - cost.quantity_delivered);
    }

    #[test]
    fn a_gross_spread_that_looks_profitable_is_negative_once_the_goods_are_landed() {
        // The whole point of §17.4's second row. Buy at 50, sell at 62 — a
        // 24% gross spread that any financial cost model would wave through,
        // because its costs are basis points. Freight, duty, storage,
        // spoilage and the marketplace's cut take it under, and a desk that
        // read the gross spread would have shipped it.
        let spoilage =
            Spoilage::new(dec!("0.002"), "trade association shrinkage table").expect("rate");
        let terms = LogisticsTerms {
            route: vec![Leg {
                from: "Shenzhen".to_string(),
                to: "Felixstowe".to_string(),
                mode: TransportMode::Sea,
                days: 35,
                freight_per_unit: dec!("3"),
                freight_per_shipment: dec!("400"),
            }],
            customs: Customs {
                duty_rate: dec!("0.06"),
                clearance_fee: dec!("250"),
                clearance_days: 4,
            },
            spoilage,
            storage_per_unit_per_day: dec!("0.02"),
            storage_days: 20,
            fees: MarketplaceFees {
                ad_valorem: dec!("0.15"),
                per_unit: dec!("0.4"),
                per_consignment: dec!("50"),
            },
            returns: Returns {
                rate: dec!("0.08"),
                cost_per_unit: dec!("4"),
                recovery_rate: dec!("0.5"),
            },
        };

        let gross_spread = dec!("62") - dec!("50");
        assert!(
            gross_spread.is_positive(),
            "the premise is a positive gross spread"
        );

        let deal = terms
            .arbitrage(dec!("1000"), dec!("50"), dec!("50"), dec!("62"))
            .expect("arbitrage");
        assert!(
            !deal.is_viable(),
            "the landed cost did not eat the spread: {}",
            deal.describe()
        );
        assert!(deal.margin().is_negative(), "{}", deal.describe());
        // And the components are separate, so a desk can see which one did
        // it rather than being told only that the total was too big.
        assert!(deal.cost.freight.is_positive());
        assert!(deal.cost.duty.is_positive());
        assert!(deal.cost.storage.is_positive());
        assert!(deal.cost.marketplace.is_positive());
        assert!(deal.cost.returns.is_positive());
        assert!(deal.cost.spoiled().is_positive());

        // The same terms at a price that clears everything are viable, so
        // the model is a cost model and not a machine for refusing trades.
        let better = terms
            .arbitrage(dec!("1000"), dec!("50"), dec!("50"), dec!("95"))
            .expect("arbitrage");
        assert!(better.is_viable(), "{}", better.describe());
    }

    #[test]
    fn storage_is_charged_on_what_is_still_on_hand_and_never_on_what_has_spoiled() {
        // A consignment that has lost a third of itself is not paying to
        // store the third that is gone. Charging on the shipped quantity
        // overstates storage by the whole of the loss, which on a long route
        // for a perishable is most of the bill.
        let spoilage = Spoilage::new(dec!("0.05"), "the warehouse's own count").expect("rate");
        let mut perishable = terms(vec![leg("A", "B", 10, "0")], spoilage);
        perishable.storage_per_unit_per_day = dec!("1");
        let cost = perishable
            .cost(dec!("100"), dec!("0"), dec!("0"))
            .expect("cost");

        // Ten days at 1 per unit per day on a flat 100 units would be 1000.
        assert!(
            cost.storage < dec!("1000"),
            "storage was charged on the shipped quantity: {}",
            cost.storage
        );
        assert!(
            cost.storage > dec!("700"),
            "storage was not charged: {}",
            cost.storage
        );

        let sound = terms(vec![leg("A", "B", 10, "0")], no_spoilage());
        let mut sound = sound;
        sound.storage_per_unit_per_day = dec!("1");
        let flat = sound.cost(dec!("100"), dec!("0"), dec!("0")).expect("cost");
        assert_eq!(
            flat.storage,
            dec!("1000"),
            "a good that does not spoil pays the whole storage bill"
        );
    }

    #[test]
    fn a_route_longer_than_the_bound_is_refused_rather_than_costed_over_a_decade() {
        let long = terms(
            vec![leg("A", "B", MAXIMUM_ROUTE_DAYS + 1, "0")],
            no_spoilage(),
        );
        let message = long
            .cost(dec!("1"), dec!("1"), dec!("1"))
            .expect_err("an eleven-year route was costed")
            .message()
            .to_string();
        assert!(
            message.contains("against a bound of 3650"),
            "the refusal did not name the bound: {message}"
        );
        let at_bound = terms(vec![leg("A", "B", MAXIMUM_ROUTE_DAYS, "0")], no_spoilage());
        assert!(
            at_bound.cost(dec!("1"), dec!("1"), dec!("1")).is_ok(),
            "the bound itself was refused"
        );
    }

    #[test]
    fn a_negative_freight_is_refused_because_a_carrier_that_pays_you_is_a_sign_error() {
        let wrong = terms(vec![leg("A", "B", 1, "-5")], no_spoilage());
        let message = wrong
            .cost(dec!("1"), dec!("1"), dec!("1"))
            .expect_err("a negative freight was costed")
            .message()
            .to_string();
        assert!(
            message.contains("states a negative freight"),
            "the refusal did not name the freight: {message}"
        );
    }

    #[test]
    fn both_bases_reach_the_delivered_price_and_a_negative_one_is_refused() {
        // A basis silently dropped is the failure this guards: applying only
        // the location basis on a discounted grade prices the goods above
        // what they are worth, and the trade looks better than it is.
        assert_eq!(
            delivered_price(dec!("100"), dec!("-3"), dec!("-7")).expect("priced"),
            dec!("90")
        );
        assert_eq!(
            delivered_price(dec!("100"), dec!("3"), dec!("-7")).expect("priced"),
            dec!("96")
        );
        let message = delivered_price(dec!("10"), dec!("-4"), dec!("-9"))
            .expect_err("a negative price was returned")
            .message()
            .to_string();
        assert!(
            message.contains("a negative delivered price is a disposal"),
            "the refusal did not name the disposal: {message}"
        );
    }

    #[test]
    fn a_consignment_that_delivered_nothing_refuses_a_cost_per_unit_rather_than_answering_zero() {
        // A cost per unit on zero units would report a free consignment,
        // which is the most attractive possible answer to a question about a
        // total loss.
        let spoilage = Spoilage::new(dec!("0.9"), "the desk's own count").expect("rate");
        let total_loss = terms(vec![leg("A", "B", 400, "1")], spoilage);
        let cost = total_loss
            .cost(dec!("10"), dec!("1"), dec!("1"))
            .expect("cost");
        assert_eq!(
            cost.quantity_delivered,
            Decimal::ZERO,
            "the premise is that nothing arrived"
        );
        assert!(cost.total().is_positive(), "the freight was still paid");
        let message = cost
            .per_delivered_unit()
            .expect_err("a per-unit cost was returned on nothing")
            .message()
            .to_string();
        assert!(
            message.contains("nothing arrived"),
            "the refusal did not say why: {message}"
        );
    }
}
