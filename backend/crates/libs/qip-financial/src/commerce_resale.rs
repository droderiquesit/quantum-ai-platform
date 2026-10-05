//! The commerce plane's back half: per-unit attributes, the Resale Router,
//! resale pricing and SKU-level learning (COMMERCE-005, -007, -019, -020).
//!
//! Pure and paper-only, like [`crate::commerce`]: nothing here performs I/O
//! or reaches a venue, it chooses and records.

use crate::commerce::{CommercePlane, FeasibilityInput, UnitState};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn mul(what: &str, a: Decimal, b: Decimal) -> Result<Decimal> {
    a.checked_mul(b)
        .ok_or_else(|| Error::numeric(format!("{what} overflowed")))
}

fn div(what: &str, a: Decimal, b: Decimal) -> Result<Decimal> {
    a.checked_div(b)
        .ok_or_else(|| Error::numeric(format!("{what} could not be divided")))
}

// ----------------------------------------------------- Unit attributes

/// The twelve attributes the layer tracks per unit.
pub const UNIT_ATTRIBUTES: [&str; 12] = [
    "sku_identity",
    "condition",
    "location",
    "inventory_state",
    "shipping",
    "insurance",
    "customs_duties",
    "tax",
    "marketplace_fees",
    "payment_chargeback_risk",
    "storage",
    "returns",
];

/// An attribute is either stated or explicitly absent with a reason. There is
/// no third state that a default could fill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attr {
    Value(String),
    Absent { reason: String },
}

/// A unit's full attribute set, complete by construction.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitAttributes {
    unit: String,
    attrs: BTreeMap<String, Attr>,
}

impl UnitAttributes {
    /// Refuses a record missing any of the twelve names, naming it; refuses a
    /// name outside the twelve; refuses blank values and blank reasons.
    pub fn new(unit: &str, attrs: BTreeMap<String, Attr>) -> Result<Self> {
        if unit.trim().is_empty() {
            return Err(Error::invalid("a tracked unit needs an id"));
        }
        for name in attrs.keys() {
            if !UNIT_ATTRIBUTES.contains(&name.as_str()) {
                return Err(Error::invalid(format!(
                    "{name} is not one of the twelve tracked attributes"
                )));
            }
        }
        for name in UNIT_ATTRIBUTES {
            match attrs.get(name) {
                None => {
                    return Err(Error::invalid(format!(
                        "unit {unit} lacks {name}; state it or mark it Absent with a reason"
                    )));
                }
                Some(Attr::Value(v)) if v.trim().is_empty() => {
                    return Err(Error::invalid(format!("{name} is blank; state it")));
                }
                Some(Attr::Absent { reason }) if reason.trim().is_empty() => {
                    return Err(Error::invalid(format!(
                        "{name} is absent with no reason; say why"
                    )));
                }
                Some(_) => {}
            }
        }
        Ok(Self {
            unit: unit.to_string(),
            attrs,
        })
    }

    pub fn unit(&self) -> &str {
        &self.unit
    }

    pub fn get(&self, name: &str) -> Option<&Attr> {
        self.attrs.get(name)
    }
}

// ------------------------------------------------------- Resale router

/// The three places a unit can be resold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VenueKind {
    Marketplace,
    Auction,
    Direct,
}

/// What one venue offers for one unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueQuote {
    pub kind: VenueKind,
    pub venue: String,
    pub expected_price: Decimal,
    /// Fraction of the price the venue keeps, in `[0, 1)`.
    pub fee_rate: Decimal,
    pub fixed_fee: Decimal,
    pub days_to_sale: u32,
}

/// The figures compared for one venue, kept so the choice can be audited.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueFigures {
    pub kind: VenueKind,
    pub venue: String,
    pub net_proceeds: Decimal,
    pub carry: Decimal,
    /// Net proceeds less carry; the number the choice maximises.
    pub score: Decimal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueChoice {
    pub chosen: VenueFigures,
    /// Every venue considered, in venue-kind then name order.
    pub rationale: Vec<VenueFigures>,
}

/// Choose the venue with the best net proceeds after the cost of holding the
/// unit until it sells. Pure: the same quotes give the same choice on replay,
/// and a tie goes to the earlier venue kind, then the earlier name.
pub fn route_resale(
    cost_basis: Decimal,
    daily_capital_cost: Decimal,
    quotes: &[VenueQuote],
) -> Result<VenueChoice> {
    if quotes.is_empty() {
        return Err(Error::invalid(
            "the resale router needs at least one venue quote",
        ));
    }
    if cost_basis.is_negative() || daily_capital_cost.is_negative() {
        return Err(Error::invalid(
            "a cost basis and a cost of capital cannot be negative",
        ));
    }
    let mut figures = Vec::new();
    for q in quotes {
        if q.venue.trim().is_empty()
            || q.expected_price.is_negative()
            || q.fixed_fee.is_negative()
            || q.fee_rate.is_negative()
            || q.fee_rate >= Decimal::ONE
        {
            return Err(Error::invalid(format!(
                "the quote for venue {:?} is malformed; fix it rather than routing on it",
                q.venue
            )));
        }
        let net_proceeds =
            q.expected_price - mul("the venue fee", q.expected_price, q.fee_rate)? - q.fixed_fee;
        let carry = mul(
            "the carry",
            mul("the carry", cost_basis, daily_capital_cost)?,
            Decimal::from(i64::from(q.days_to_sale)),
        )?;
        figures.push(VenueFigures {
            kind: q.kind,
            venue: q.venue.clone(),
            net_proceeds,
            carry,
            score: net_proceeds - carry,
        });
    }
    figures.sort_by(|a, b| (a.kind, &a.venue).cmp(&(b.kind, &b.venue)));
    // The first maximum in sorted order wins, which is the tie-break.
    let mut best = 0;
    for (i, f) in figures.iter().enumerate() {
        if f.score > figures[best].score {
            best = i;
        }
    }
    Ok(VenueChoice {
        chosen: figures[best].clone(),
        rationale: figures,
    })
}

// ------------------------------------------------------ Resale pricing

/// A unit on offer and every price it has been offered at.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub unit: String,
    pub price: Decimal,
    /// The market price the current price was set from.
    pub market_price: Decimal,
    pub changed_day: u32,
    /// `(day, price, reason)` for every publication, the first included.
    pub history: Vec<(u32, Decimal, String)>,
}

/// Prices listed inventory and reprices it while it stays unsold.
#[derive(Clone, Debug)]
pub struct ResaleDesk {
    /// Never price below this; a repricing that would is refused.
    floor_per_unit: Decimal,
    /// Days unsold before the price is marked down.
    markdown_after_days: u32,
    /// Fraction taken off at each markdown, in `(0, 1)`.
    markdown_rate: Decimal,
    listed: BTreeMap<String, Listed>,
}

impl ResaleDesk {
    pub fn new(
        floor_per_unit: Decimal,
        markdown_after_days: u32,
        markdown_rate: Decimal,
    ) -> Result<Self> {
        if floor_per_unit.is_negative()
            || markdown_after_days == 0
            || !markdown_rate.is_positive()
            || markdown_rate >= Decimal::ONE
        {
            return Err(Error::invalid(
                "a resale desk needs a non-negative floor, a positive markdown interval and a markdown rate in (0, 1)",
            ));
        }
        Ok(Self {
            floor_per_unit,
            markdown_after_days,
            markdown_rate,
            listed: BTreeMap::new(),
        })
    }

    pub fn listed(&self, unit: &str) -> Option<&Listed> {
        self.listed.get(unit)
    }

    fn require_held(plane: &CommercePlane, unit: &str) -> Result<()> {
        match plane.ledger().units().get(unit).map(|u| &u.state) {
            Some(UnitState::Held) => Ok(()),
            Some(state) => Err(Error::invalid(format!(
                "unit {unit} is {state:?}; only a held, unsold unit is listed or repriced"
            ))),
            None => Err(Error::not_found(format!(
                "unit {unit} is not in the ledger; receive it first"
            ))),
        }
    }

    fn check_floor(&self, price: Decimal) -> Result<()> {
        if price < self.floor_per_unit {
            return Err(Error::denied(format!(
                "a price of {price} is under the floor of {}; hold the listing or lower the floor deliberately",
                self.floor_per_unit
            )));
        }
        Ok(())
    }

    pub fn list(
        &mut self,
        plane: &CommercePlane,
        unit: &str,
        market_price: Decimal,
        day: u32,
    ) -> Result<Decimal> {
        Self::require_held(plane, unit)?;
        if self.listed.contains_key(unit) {
            return Err(Error::invalid(format!("unit {unit} is already listed")));
        }
        if !market_price.is_positive() {
            return Err(Error::invalid("a market price must be positive"));
        }
        self.check_floor(market_price)?;
        self.listed.insert(
            unit.to_string(),
            Listed {
                unit: unit.to_string(),
                price: market_price,
                market_price,
                changed_day: day,
                history: vec![(
                    day,
                    market_price,
                    "initial price from the market".to_string(),
                )],
            },
        );
        Ok(market_price)
    }

    /// Publish a new price when the market moved or the unit has sat unsold
    /// past the markdown interval; `Ok(None)` means neither happened. A unit
    /// that has been sold, or is reserved, is refused rather than repriced.
    pub fn reprice(
        &mut self,
        plane: &CommercePlane,
        unit: &str,
        market_price: Decimal,
        day: u32,
    ) -> Result<Option<Decimal>> {
        Self::require_held(plane, unit)?;
        if !market_price.is_positive() {
            return Err(Error::invalid("a market price must be positive"));
        }
        let (current, last_market, since) = {
            let l = self.listed.get(unit).ok_or_else(|| {
                Error::not_found(format!("unit {unit} is not listed; list it first"))
            })?;
            if day < l.changed_day {
                return Err(Error::invalid("a repricing cannot predate the last change"));
            }
            (l.price, l.market_price, day - l.changed_day)
        };
        let (new, reason) = if market_price != last_market {
            (market_price, "the market price moved".to_string())
        } else if since >= self.markdown_after_days {
            (
                current - mul("the markdown", current, self.markdown_rate)?,
                format!("{since} days unsold; marked down"),
            )
        } else {
            return Ok(None);
        };
        self.check_floor(new)?;
        if let Some(l) = self.listed.get_mut(unit) {
            l.price = new;
            l.market_price = market_price;
            l.changed_day = day;
            l.history.push((day, new, reason));
        }
        Ok(Some(new))
    }
}

// ------------------------------------------------- SKU-level economics

/// A completed purchase-and-resale cycle, the fact learning is built from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CycleEvent {
    pub sku: String,
    pub realised_cost: Decimal,
    pub resale_price: Decimal,
    pub days_to_sale: u32,
    pub returned: bool,
}

/// What the completed cycles of one SKU add up to.
#[derive(Clone, Debug, PartialEq)]
pub struct SkuEconomics {
    pub cycles: u32,
    pub total_cost: Decimal,
    pub total_resale: Decimal,
    pub total_days: u32,
    pub returns: u32,
}

/// Replay cycle events into per-SKU records. A record is a pure function of
/// the events, so replaying the same log reproduces it exactly.
pub fn replay_economics(events: &[CycleEvent]) -> Result<BTreeMap<String, SkuEconomics>> {
    let mut out: BTreeMap<String, SkuEconomics> = BTreeMap::new();
    for e in events {
        if e.sku.trim().is_empty() || e.realised_cost.is_negative() || e.resale_price.is_negative()
        {
            return Err(Error::invalid(
                "a completed cycle needs a SKU and non-negative cost and resale price",
            ));
        }
        let s = out.entry(e.sku.clone()).or_insert(SkuEconomics {
            cycles: 0,
            total_cost: Decimal::ZERO,
            total_resale: Decimal::ZERO,
            total_days: 0,
            returns: 0,
        });
        s.cycles += 1;
        s.total_cost += e.realised_cost;
        s.total_resale += e.resale_price;
        s.total_days += e.days_to_sale;
        s.returns += u32::from(e.returned);
    }
    Ok(out)
}

impl FeasibilityInput {
    /// The same assessment drawn on realised figures: expected resale, days on
    /// market (whole days, rounded down) and the return rate come from the
    /// SKU's completed cycles; every other input and every refusal is
    /// unchanged.
    pub fn with_learned(&self, e: &SkuEconomics) -> Result<FeasibilityInput> {
        if e.cycles == 0 {
            return Err(Error::invalid(
                "a SKU record with no cycles teaches nothing",
            ));
        }
        let n = Decimal::from(i64::from(e.cycles));
        let mut next = self.clone();
        next.resale = vec![(div("the mean resale", e.total_resale, n)?, Decimal::ONE)];
        next.days_on_market = Some(e.total_days / e.cycles);
        if let Some(t) = next.terms.as_mut() {
            t.returns.rate = div("the return rate", Decimal::from(i64::from(e.returns)), n)?;
        }
        Ok(next)
    }
}
