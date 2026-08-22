//! Cross-venue consistency between two markets on the same proposition.
//!
//! The same question priced at 0.61 on one venue and 0.66 on another is only
//! an arbitrage if it really is the same question. In practice it usually is
//! not: one venue resolves on the announcement, the other on the effective
//! date; one voids an ambiguous outcome, the other resolves it as no. Trading
//! those against each other is not arbitrage, it is a short position in the
//! difference between two documents nobody read.
//!
//! So the pair is a type with a constructor that refuses. Two markets whose
//! [`crate::resolution::ResolutionCriteria`] digests differ cannot be made
//! into a [`CrossMarketPair`] at all, and a difference of resolution source —
//! the same question, two authorities who can disagree — is admitted only
//! against a haircut the caller has to name.

use qip_contracts::{BookSide, Deduction, DeductionKind, LegPlan, LegStep, NetEdge, VenueId};
use qip_core::error::{Error, Result};
use qip_core::Decimal;
use qip_market::book::OrderBook;
use serde::{Deserialize, Serialize};

use crate::arbitrage::Depth;
use crate::market::{EventMarket, OutcomeId};
use crate::resolution::PropositionDifference;

/// Two markets established to be on the same proposition.
#[derive(Clone, Debug)]
pub struct CrossMarketPair<'a> {
    left: &'a EventMarket,
    right: &'a EventMarket,
    /// Present when the two resolve from different authorities.
    source_divergence: Option<(String, String)>,
    /// What the caller charges itself, per contract, for that divergence.
    haircut_per_contract: Decimal,
}

impl<'a> CrossMarketPair<'a> {
    /// Pair two markets, refusing anything that is not the same contract.
    ///
    /// The refusal is the point. Every structural difference — the criteria,
    /// the resolution time, the payoff, the rule for an undetermined outcome —
    /// makes these different instruments, and no fee model rescues a spread
    /// between different instruments.
    pub fn new(left: &'a EventMarket, right: &'a EventMarket) -> Result<Self> {
        let differences = left.proposition.differences(&right.proposition);
        let structural: Vec<&str> = differences
            .iter()
            .filter(|difference| difference.is_structural())
            .map(PropositionDifference::as_str)
            .collect();
        if !structural.is_empty() {
            return Err(Error::invalid(format!(
                "markets {} and {} differ in {}; they are not the same proposition and must not be arbitraged against each other",
                left.market_id,
                right.market_id,
                structural.join(", ")
            )));
        }
        let source_divergence = differences
            .iter()
            .any(|difference| matches!(difference, PropositionDifference::Source))
            .then(|| {
                (
                    left.proposition.source.name.clone(),
                    right.proposition.source.name.clone(),
                )
            });
        Ok(Self {
            left,
            right,
            source_divergence,
            haircut_per_contract: Decimal::ZERO,
        })
    }

    /// State what a difference of resolution authority is worth per contract.
    pub fn with_source_haircut(mut self, per_contract: Decimal) -> Result<Self> {
        if per_contract.is_negative() {
            return Err(Error::invalid("a haircut cannot be negative"));
        }
        self.haircut_per_contract = per_contract;
        Ok(self)
    }

    /// The two authorities, when they differ.
    pub fn source_divergence(&self) -> Option<&(String, String)> {
        self.source_divergence.as_ref()
    }

    pub const fn left(&self) -> &EventMarket {
        self.left
    }

    pub const fn right(&self) -> &EventMarket {
        self.right
    }

    /// Buy the cheap venue and sell the rich one, sized against both books.
    ///
    /// The outcome must be the same outcome on both sides — matched by its
    /// criteria digest rather than by its label, since "Yes" on two venues is
    /// two strings and says nothing.
    pub fn arbitrage(
        &self,
        outcome: &OutcomeId,
        left_book: &OrderBook,
        right_book: &OrderBook,
    ) -> Result<Option<CrossVenueArbitrage>> {
        let left_outcome = self.left.outcome(outcome)?;
        let right_outcome = self.right.outcome(outcome)?;
        if left_outcome.digest() != right_outcome.digest() {
            return Err(Error::invalid(format!(
                "outcome {outcome} resolves on different criteria at {} and {}",
                self.left.venue, self.right.venue
            )));
        }
        if self.source_divergence.is_some() && self.haircut_per_contract.is_zero() {
            return Err(Error::invalid(format!(
                "markets {} and {} resolve from different sources; state the haircut that divergence is worth before trading the spread",
                self.left.market_id, self.right.market_id
            )));
        }

        // Both directions: buy left / sell right, and the reverse.
        let forward = self.direction(outcome, self.left, left_book, self.right, right_book)?;
        let reverse = self.direction(outcome, self.right, right_book, self.left, left_book)?;
        Ok(match (forward, reverse) {
            (Some(forward), Some(reverse)) => {
                if forward.edge.net() >= reverse.edge.net() {
                    Some(forward)
                } else {
                    Some(reverse)
                }
            }
            (found, None) | (None, found) => found,
        })
    }

    fn direction(
        &self,
        outcome: &OutcomeId,
        buy_market: &EventMarket,
        buy_book: &OrderBook,
        sell_market: &EventMarket,
        sell_book: &OrderBook,
    ) -> Result<Option<CrossVenueArbitrage>> {
        let mut asks = Depth::new(&buy_book.asks);
        let mut bids = Depth::new(&sell_book.bids);

        let mut quantity = Decimal::ZERO;
        let mut gross = Decimal::ZERO;
        let mut fees_paid = Decimal::ZERO;
        let mut buy_notional = Decimal::ZERO;
        let mut sell_notional = Decimal::ZERO;
        let mut depth_limited = false;

        loop {
            let (Some(ask), Some(bid)) = (asks.current(), bids.current()) else {
                depth_limited = true;
                break;
            };
            let unit_gross = bid.price - ask.price;
            let unit_fee = buy_market.fees.taker_cost(ask.price)
                + sell_market.fees.taker_cost(bid.price)
                + self.haircut_per_contract;
            if unit_gross - unit_fee <= Decimal::ZERO {
                break;
            }
            let slice = asks.remaining().min(bids.remaining());
            if !slice.is_positive() {
                break;
            }
            quantity += slice;
            gross += unit_gross * slice;
            fees_paid += unit_fee * slice;
            buy_notional += ask.price * slice;
            sell_notional += bid.price * slice;
            asks.take(slice);
            bids.take(slice);
        }

        if !quantity.is_positive() {
            return Ok(None);
        }
        let buy_price = buy_notional
            .checked_div(quantity)
            .ok_or_else(|| Error::numeric("a cross-venue leg has no size to price"))?;
        let sell_price = sell_notional
            .checked_div(quantity)
            .ok_or_else(|| Error::numeric("a cross-venue leg has no size to price"))?;

        let haircut = self.haircut_per_contract * quantity;
        let trading_fees = fees_paid - haircut;
        let edge = NetEdge::gross(gross, quantity)?
            .deduct(Deduction::new(
                DeductionKind::Fees,
                trading_fees,
                format!(
                    "{}bp taker at {} and {}bp taker at {}",
                    buy_market.fees.taker_bps,
                    buy_market.venue,
                    sell_market.fees.taker_bps,
                    sell_market.venue
                ),
            )?)
            .deduct(Deduction::new(
                DeductionKind::Uncertainty,
                haircut,
                match &self.source_divergence {
                    Some((left, right)) => format!(
                        "the two venues resolve from {left} and {right}, which can disagree"
                    ),
                    None => "none: both venues resolve from the same source on identical criteria"
                        .to_string(),
                },
            )?)
            .deduct(Deduction::new(
                DeductionKind::Spread,
                Decimal::ZERO,
                "already paid: both legs are priced by walking their books",
            )?)
            .deduct(Deduction::new(
                DeductionKind::Slippage,
                Decimal::ZERO,
                "none: the size is bounded by the resting depth on both sides",
            )?)
            .deduct(Deduction::new(
                DeductionKind::Latency,
                Decimal::ZERO,
                "not modelled here; the execution layer knows its own round trip",
            )?)
            .deduct(Deduction::new(
                DeductionKind::Funding,
                Decimal::ZERO,
                "none: both legs are fully funded to resolution",
            )?)
            .deduct(Deduction::new(
                DeductionKind::Collateral,
                Decimal::ZERO,
                "the long leg collateralises the short until both resolve",
            )?);

        let plan = LegPlan::new(vec![
            LegStep {
                object_id: buy_market.outcome(outcome)?.object_id.clone(),
                venue: buy_market.venue.clone(),
                side: BookSide::Ask,
                quantity,
                reference_price: buy_price,
                order: 0,
                optional: false,
            },
            LegStep {
                object_id: sell_market.outcome(outcome)?.object_id.clone(),
                venue: sell_market.venue.clone(),
                side: BookSide::Bid,
                quantity,
                reference_price: sell_price,
                order: 0,
                optional: false,
            },
        ])?;

        Ok(Some(CrossVenueArbitrage {
            buy_venue: buy_market.venue.clone(),
            sell_venue: sell_market.venue.clone(),
            quantity,
            buy_price,
            sell_price,
            depth_limited,
            edge,
            plan,
        }))
    }
}

/// The same contract, bought on one venue and sold on another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossVenueArbitrage {
    pub buy_venue: VenueId,
    pub sell_venue: VenueId,
    pub quantity: Decimal,
    /// Volume-weighted, not the touch.
    pub buy_price: Decimal,
    pub sell_price: Decimal,
    pub depth_limited: bool,
    pub edge: NetEdge,
    pub plan: LegPlan,
}

impl CrossVenueArbitrage {
    pub fn profit(&self) -> Decimal {
        self.edge.net()
    }
}
