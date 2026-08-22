//! Arbitrage within a single market.
//!
//! If every outcome of a market can be bought for less than the payoff, the
//! set is a locked profit: exactly one of them pays, and the outcomes are
//! exhaustive by construction. The interesting part is not detecting it — the
//! sum of the touch prices does that — but sizing it, because the touch is
//! usually a few contracts deep and the profit at the touch is not the profit
//! on the size anyone would want to do.
//!
//! So the walk here consumes the books level by level, taking the largest
//! slice every leg can fill at once and stopping the moment the combined price
//! stops being profitable or any leg runs out of depth. The quantity it
//! reports is therefore executable against the book it was given, and the
//! reported profit is what that quantity actually makes.

use qip_contracts::{BookSide, Deduction, DeductionKind, LegPlan, LegStep, NetEdge};
use qip_core::error::{Error, Result};
use qip_core::Decimal;
use qip_market::book::{BookLevel, OrderBook};
use serde::{Deserialize, Serialize};

use crate::market::{EventMarket, OutcomeId};
use crate::pricing::{implied_from_ask, implied_from_bid, SumDeviation};

/// Which side of the complete set is mispriced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetArbitrageKind {
    /// Every outcome can be bought for less than the payoff.
    UnderpricedSet,
    /// Every outcome can be sold for more than a complete set costs to mint.
    OverpricedSet,
}

impl SetArbitrageKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::UnderpricedSet => "underpriced_set",
            Self::OverpricedSet => "overpriced_set",
        }
    }
}

/// A locked profit in one market, sized against the book that offers it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetArbitrage {
    pub kind: SetArbitrageKind,
    /// Contracts of the complete set. Never more than the thinnest leg holds.
    pub quantity: Decimal,
    /// What the legs cost in total, fees included.
    pub cost: Decimal,
    /// What the position returns in total, fees deducted.
    pub proceeds: Decimal,
    /// Volume-weighted price paid or received per outcome, in outcome order.
    pub leg_prices: Vec<(OutcomeId, Decimal)>,
    /// Whether the size was limited by depth rather than by price.
    ///
    /// A depth-limited arbitrage is one where more size exists at a worse
    /// price; a price-limited one has been taken to exhaustion.
    pub depth_limited: bool,
    pub edge: NetEdge,
    pub plan: LegPlan,
}

impl SetArbitrage {
    /// Profit after every deduction. The only number worth acting on.
    pub fn profit(&self) -> Decimal {
        self.edge.net()
    }
}

/// Sum of the probabilities the touch prices imply.
///
/// Reported rather than normalised: the deviation is the venue's fee model
/// made visible, and a deviation that moves is either an opportunity or a
/// broken assumption about the fees.
pub fn implied_sum(market: &EventMarket, books: &[(OutcomeId, &OrderBook)]) -> Result<SumDeviation> {
    let mut ask_sum = Decimal::ZERO;
    let mut bid_sum = Decimal::ZERO;
    for outcome in market.outcomes() {
        let book = book_for(books, &outcome.id)?;
        let ask = book.best_ask().ok_or_else(|| {
            Error::not_found(format!("outcome {} has no offer to price", outcome.id))
        })?;
        let bid = book.best_bid().ok_or_else(|| {
            Error::not_found(format!("outcome {} has no bid to price", outcome.id))
        })?;
        ask_sum += implied_from_ask(ask.price, &market.fees, market.payoff())?.value();
        bid_sum += implied_from_bid(bid.price, &market.fees, market.payoff())?.value();
    }
    Ok(SumDeviation { ask_sum, bid_sum })
}

/// Detect and size the complete-set arbitrage in a market.
///
/// Both directions are checked. For a binary market this is the complement
/// arbitrage — yes and no together costing less than one payoff — which is the
/// same computation with two legs rather than many.
pub fn set_arbitrage(
    market: &EventMarket,
    books: &[(OutcomeId, &OrderBook)],
) -> Result<Option<SetArbitrage>> {
    if let Some(found) = walk(market, books, SetArbitrageKind::UnderpricedSet)? {
        return Ok(Some(found));
    }
    walk(market, books, SetArbitrageKind::OverpricedSet)
}

/// A cursor over one side of one book.
///
/// Walking levels rather than reading the touch is what makes every size in
/// this module executable: the cursor cannot hand out depth that is not there.
pub(crate) struct Depth<'a> {
    levels: &'a [BookLevel],
    index: usize,
    consumed: Decimal,
}

impl<'a> Depth<'a> {
    pub(crate) fn new(levels: &'a [BookLevel]) -> Self {
        Self {
            levels,
            index: 0,
            consumed: Decimal::ZERO,
        }
    }

    pub(crate) fn current(&self) -> Option<&BookLevel> {
        self.levels.get(self.index)
    }

    pub(crate) fn remaining(&self) -> Decimal {
        self.current()
            .map_or(Decimal::ZERO, |level| level.size - self.consumed)
    }

    pub(crate) fn take(&mut self, quantity: Decimal) {
        self.consumed += quantity;
        if self.remaining() <= Decimal::ZERO {
            self.index += 1;
            self.consumed = Decimal::ZERO;
        }
    }
}

fn walk(
    market: &EventMarket,
    books: &[(OutcomeId, &OrderBook)],
    kind: SetArbitrageKind,
) -> Result<Option<SetArbitrage>> {
    let outcomes = market.outcomes();
    let payoff = market.payoff();
    let fees = market.fees;

    let mut cursors: Vec<Depth<'_>> = Vec::with_capacity(outcomes.len());
    for outcome in &outcomes {
        let book = book_for(books, &outcome.id)?;
        cursors.push(Depth::new(match kind {
            SetArbitrageKind::UnderpricedSet => &book.asks,
            SetArbitrageKind::OverpricedSet => &book.bids,
        }));
    }

    let mut quantity = Decimal::ZERO;
    let mut gross = Decimal::ZERO;
    let mut fees_paid = Decimal::ZERO;
    let mut notional = Decimal::ZERO;
    let mut leg_notional = vec![Decimal::ZERO; outcomes.len()];
    let mut depth_limited = false;

    loop {
        let mut combined = Decimal::ZERO;
        let mut slice: Option<Decimal> = None;
        for cursor in &cursors {
            let Some(level) = cursor.current() else {
                depth_limited = true;
                break;
            };
            combined += level.price;
            let available = cursor.remaining();
            slice = Some(slice.map_or(available, |smallest: Decimal| smallest.min(available)));
        }
        let (Some(slice), false) = (slice, depth_limited) else {
            break;
        };
        if !slice.is_positive() {
            break;
        }

        // Per contract of the complete set, before size.
        let (unit_gross, unit_fee) = match kind {
            SetArbitrageKind::UnderpricedSet => (
                fees.net_payoff(payoff) - combined,
                fees.taker_cost(combined) + fees.settlement_cost(payoff),
            ),
            SetArbitrageKind::OverpricedSet => (combined - payoff, fees.taker_cost(combined)),
        };
        if unit_gross - unit_fee <= Decimal::ZERO {
            // The next slice is not profitable, so the size is set by price
            // rather than by depth.
            break;
        }

        quantity += slice;
        gross += unit_gross * slice;
        fees_paid += unit_fee * slice;
        notional += combined * slice;
        for (position, cursor) in cursors.iter_mut().enumerate() {
            if let Some(level) = cursor.current() {
                leg_notional[position] += level.price * slice;
            }
            cursor.take(slice);
        }
    }

    if !quantity.is_positive() {
        return Ok(None);
    }

    let mut leg_prices = Vec::with_capacity(outcomes.len());
    let mut steps = Vec::with_capacity(outcomes.len());
    for (position, outcome) in outcomes.iter().enumerate() {
        let average = leg_notional[position]
            .checked_div(quantity)
            .ok_or_else(|| Error::numeric("an arbitrage leg has no size to price"))?;
        leg_prices.push((outcome.id.clone(), average));
        steps.push(LegStep {
            object_id: outcome.object_id.clone(),
            venue: market.venue.clone(),
            // Buying a set lifts every offer; selling one hits every bid.
            side: match kind {
                SetArbitrageKind::UnderpricedSet => BookSide::Ask,
                SetArbitrageKind::OverpricedSet => BookSide::Bid,
            },
            quantity,
            reference_price: average,
            // Every leg is mandatory and simultaneous: a set missing a leg is
            // not a hedged position, it is a directional one nobody chose.
            order: 0,
            optional: false,
        });
    }

    let edge = NetEdge::gross(gross, quantity)?
        .deduct(Deduction::new(
            DeductionKind::Fees,
            fees_paid,
            format!(
                "{}bp taker on {notional} of legs and {}bp settlement on the payoff",
                fees.taker_bps, fees.settlement_bps
            ),
        )?)
        .deduct(Deduction::new(
            DeductionKind::Spread,
            Decimal::ZERO,
            "already paid: the legs are priced by walking the book, not at the touch",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Slippage,
            Decimal::ZERO,
            "none: the size is bounded by the resting depth it was computed from",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Latency,
            Decimal::ZERO,
            "not modelled here; the execution layer knows its own round trip",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Funding,
            Decimal::ZERO,
            "none on a fully funded set held to resolution",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Collateral,
            Decimal::ZERO,
            "the set is its own collateral until settlement",
        )?)
        .deduct(Deduction::new(
            DeductionKind::Uncertainty,
            Decimal::ZERO,
            "none: one outcome of a complete set pays by construction, whatever the resolution is",
        )?);

    let cost = match kind {
        SetArbitrageKind::UnderpricedSet => notional + fees.taker_cost(notional),
        SetArbitrageKind::OverpricedSet => payoff * quantity,
    };
    let proceeds = cost + edge.net();

    Ok(Some(SetArbitrage {
        kind,
        quantity,
        cost,
        proceeds,
        leg_prices,
        depth_limited,
        edge,
        plan: LegPlan::new(steps)?,
    }))
}

fn book_for<'a>(books: &[(OutcomeId, &'a OrderBook)], outcome: &OutcomeId) -> Result<&'a OrderBook> {
    books
        .iter()
        .find(|(id, _)| id == outcome)
        .map(|(_, book)| *book)
        .ok_or_else(|| {
            Error::not_found(format!(
                "outcome {outcome} has no book; a partial set is not an arbitrage"
            ))
        })
}
