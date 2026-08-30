//! Choosing an order type for a size, a book and a deadline.
//!
//! The thresholds here are the ones already used in
//! `qip-execution-engine`'s scheduling helper, and for the same reason: a
//! market order in a thin book is how a small position becomes a large loss.
//! That helper decides how to *spread* an order over time; this one decides
//! what to send at each venue right now. Both answer to participation — the
//! size against the liquidity actually on offer — because that ratio is what
//! decides whether an order takes the price or sets it.
//!
//! Urgency is the second axis and the one that cannot be inferred. A patient
//! order should rest and collect the spread; an order that has to be done now
//! should cross and pay it. Nothing in the book says which of those a caller
//! wants, so it is asked for rather than guessed.

use crate::venue::{Liquidity, VenueProfile};
use qip_contracts::message::BookSide;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_market::book::OrderBook;
use serde::{Deserialize, Serialize};

/// Participation at or below which an order disappears into the book.
const LIGHT_PARTICIPATION: f64 = 0.01;
/// Participation at or below which an order can still be worked quickly.
const WORKABLE_PARTICIPATION: f64 = 0.05;
/// Participation past which the order sets the price rather than taking it.
const HEAVY_PARTICIPATION: f64 = 0.15;
/// How far through the touch a protective limit is placed, in basis points.
///
/// Wide enough that an order which was going to fill still fills, narrow enough
/// that a book which has emptied underneath it does not.
const PROTECTIVE_TOLERANCE_BPS: f64 = 10.0;

/// The shape of an order, without its prices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderTypeKind {
    Market,
    Limit,
    Peg,
    ImmediateOrCancel,
    FillOrKill,
}

impl OrderTypeKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::Peg => "peg",
            Self::ImmediateOrCancel => "ioc",
            Self::FillOrKill => "fok",
        }
    }
}

/// What a pegged order tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PegReference {
    /// Midpoint of the touch.
    Mid,
    /// The near touch: rest where the queue is.
    Near,
    /// The far touch: rest where the trade happens.
    Far,
}

/// An order type with the prices that make it actionable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RoutedOrderType {
    /// Take whatever the book offers, at whatever it costs.
    Market,
    /// Rest at `price` and wait.
    Limit { price: Decimal },
    /// Rest at a price that follows the book.
    Peg {
        reference: PegReference,
        offset: Decimal,
    },
    /// Take what is there at `limit` or better, cancel the rest.
    ImmediateOrCancel { limit: Decimal },
    /// All of it at `limit` or better, or none of it.
    FillOrKill { limit: Decimal },
}

impl RoutedOrderType {
    pub const fn kind(&self) -> OrderTypeKind {
        match self {
            Self::Market => OrderTypeKind::Market,
            Self::Limit { .. } => OrderTypeKind::Limit,
            Self::Peg { .. } => OrderTypeKind::Peg,
            Self::ImmediateOrCancel { .. } => OrderTypeKind::ImmediateOrCancel,
            Self::FillOrKill { .. } => OrderTypeKind::FillOrKill,
        }
    }

    /// Whether the order rests rather than crosses.
    ///
    /// Decides which side of the fee schedule applies, which is frequently
    /// larger than the price difference the router is choosing between.
    pub const fn is_passive(&self) -> bool {
        matches!(self, Self::Limit { .. } | Self::Peg { .. })
    }

    pub const fn liquidity(&self) -> Liquidity {
        if self.is_passive() {
            Liquidity::Maker
        } else {
            Liquidity::Taker
        }
    }

    /// The worst price the order will accept, where it names one.
    pub const fn limit_price(&self) -> Option<Decimal> {
        match self {
            Self::Limit { price } => Some(*price),
            Self::ImmediateOrCancel { limit } | Self::FillOrKill { limit } => Some(*limit),
            Self::Market | Self::Peg { .. } => None,
        }
    }
}

/// How much the caller is willing to pay for certainty of completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Urgency {
    /// Rest and collect the spread. Completion is not promised.
    Patient,
    /// Work it, crossing only where crossing is cheap.
    Normal,
    /// Cross where it is affordable; work only what would move the price.
    Urgent,
    /// Done now, at whatever the book holds.
    Immediate,
}

impl Urgency {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Patient => "patient",
            Self::Normal => "normal",
            Self::Urgent => "urgent",
            Self::Immediate => "immediate",
        }
    }
}

/// Both touches, which is the least a price decision can be made from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Touch {
    pub bid: Decimal,
    pub ask: Decimal,
}

impl Touch {
    /// Read the touch off a book, refusing a one-sided market.
    ///
    /// A one-sided book cannot price a peg, a mid or a protective limit, and
    /// inventing the missing side is how an order gets sent at a price nobody
    /// quoted.
    pub fn from_book(book: &OrderBook) -> Option<Self> {
        Some(Self {
            bid: book.best_bid()?.price,
            ask: book.best_ask()?.price,
        })
    }

    pub fn mid(&self) -> Option<Decimal> {
        (self.bid + self.ask).checked_div(Decimal::from_int(2))
    }

    /// The price this order would rest at without crossing.
    ///
    /// Buying rests on the bid, selling rests on the offer. `side` names the
    /// side of the book being consumed, so it is the opposite of the side the
    /// order rests on — the inversion worth writing down once.
    pub const fn resting(&self, side: BookSide) -> Decimal {
        match side {
            BookSide::Ask => self.bid,
            BookSide::Bid => self.ask,
        }
    }

    /// The price this order would have to cross to.
    pub const fn crossing(&self, side: BookSide) -> Decimal {
        match side {
            BookSide::Ask => self.ask,
            BookSide::Bid => self.bid,
        }
    }
}

/// An order type, the reasoning, and whether the venue forced a compromise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderTypeSelection {
    pub order_type: RoutedOrderType,
    /// The size as a share of the liquidity on offer. A statistic.
    pub participation_f64: f64,
    /// What was wanted before the venue's supported list was consulted.
    pub preferred: OrderTypeKind,
    /// Whether the venue could not take the preferred type.
    pub degraded: bool,
    pub reason: String,
}

/// Choose the order type to send to one venue.
///
/// `displayed` is the liquidity actually on offer on the side being consumed,
/// not the whole book: resting size on the far side of the spread is not
/// liquidity for this order.
#[allow(clippy::too_many_arguments)]
pub fn select_order_type(
    profile: &VenueProfile,
    side: BookSide,
    quantity: Decimal,
    displayed: Decimal,
    touch: Touch,
    urgency: Urgency,
    all_or_none: bool,
) -> Result<OrderTypeSelection> {
    if quantity <= Decimal::ZERO {
        return Err(Error::invalid("an order type needs a positive quantity"));
    }

    let participation_f64 = if displayed > Decimal::ZERO {
        quantity
            .checked_div(displayed)
            .map_or(f64::INFINITY, Decimal::to_f64)
    } else {
        // Nothing showing is not the same as a small order in a deep book, and
        // treating it as unknown rather than as zero keeps a market order from
        // being sent into an empty book.
        f64::INFINITY
    };

    let resting = touch.resting(side);
    let crossing = touch.crossing(side);
    let protective = protective_limit(crossing, side)?;

    let (preferred, reason) = if all_or_none {
        (
            RoutedOrderType::FillOrKill { limit: protective },
            "the caller cannot use a partial fill, so a partial fill is refused rather than managed",
        )
    } else if !participation_f64.is_finite() {
        (
            RoutedOrderType::Limit { price: resting },
            "there is no displayed liquidity to size against, so nothing is sent unpriced",
        )
    } else {
        match urgency {
            Urgency::Patient => (
                RoutedOrderType::Peg {
                    reference: PegReference::Mid,
                    offset: Decimal::ZERO,
                },
                "patient, so the order rests at the midpoint and collects the spread instead of paying it",
            ),
            Urgency::Normal => {
                if participation_f64 <= LIGHT_PARTICIPATION {
                    (
                        RoutedOrderType::Market,
                        "small against the displayed size, so crossing costs the spread and nothing else",
                    )
                } else if participation_f64 <= WORKABLE_PARTICIPATION {
                    (
                        RoutedOrderType::ImmediateOrCancel { limit: protective },
                        "large enough to walk the book, so it takes what is there at a price it chose",
                    )
                } else {
                    (
                        RoutedOrderType::Limit { price: resting },
                        "large against the displayed size, so it rests rather than setting the price",
                    )
                }
            }
            Urgency::Urgent => {
                if participation_f64 <= WORKABLE_PARTICIPATION {
                    (
                        RoutedOrderType::Market,
                        "urgent and small enough that crossing is affordable",
                    )
                } else if participation_f64 <= HEAVY_PARTICIPATION {
                    (
                        RoutedOrderType::ImmediateOrCancel { limit: protective },
                        "urgent, but large enough that the price it pays has to be bounded",
                    )
                } else {
                    (
                        RoutedOrderType::Limit { price: resting },
                        "urgent, but past the point where taking the book would move it against the rest of the order",
                    )
                }
            }
            Urgency::Immediate => {
                if participation_f64 <= HEAVY_PARTICIPATION {
                    (
                        RoutedOrderType::Market,
                        "immediate, and the book can absorb it",
                    )
                } else {
                    (
                        RoutedOrderType::ImmediateOrCancel { limit: protective },
                        "immediate, but not at any price: it takes what is there and reports the rest",
                    )
                }
            }
        }
    };

    // A venue that does not support the type is not a reason to send it anyway.
    // Degrading is recorded rather than silent, because the fallback is a
    // different trade from the one that was chosen.
    let preferred_kind = preferred.kind();
    if profile.supports(preferred_kind) {
        return Ok(OrderTypeSelection {
            order_type: preferred,
            participation_f64,
            preferred: preferred_kind,
            degraded: false,
            reason: reason.to_string(),
        });
    }

    let fallback = fallback_for(profile, preferred, resting, protective).ok_or_else(|| {
        Error::unavailable(format!(
            "{} supports none of the order types this order could use",
            profile.venue.as_str()
        ))
    })?;
    Ok(OrderTypeSelection {
        order_type: fallback,
        participation_f64,
        preferred: preferred_kind,
        degraded: true,
        reason: format!(
            "{reason}; {} does not accept a {} order, so a {} is sent instead",
            profile.venue.as_str(),
            preferred_kind.as_str(),
            fallback.kind().as_str()
        ),
    })
}

/// A limit set through the crossing touch, so it fills but not at any price.
fn protective_limit(crossing: Decimal, side: BookSide) -> Result<Decimal> {
    let tolerance = crossing.abs().apply_bps(PROTECTIVE_TOLERANCE_BPS);
    let limit = match side {
        BookSide::Ask => crossing + tolerance,
        BookSide::Bid => crossing - tolerance,
    };
    if limit <= Decimal::ZERO {
        return Err(Error::invalid(
            "a protective limit came out at or below zero, which means the touch is unusable",
        ));
    }
    Ok(limit)
}

/// The nearest supported substitute, in decreasing order of faithfulness.
fn fallback_for(
    profile: &VenueProfile,
    preferred: RoutedOrderType,
    resting: Decimal,
    protective: Decimal,
) -> Option<RoutedOrderType> {
    let ladder: [RoutedOrderType; 4] = if preferred.is_passive() {
        [
            RoutedOrderType::Limit { price: resting },
            RoutedOrderType::Peg {
                reference: PegReference::Near,
                offset: Decimal::ZERO,
            },
            RoutedOrderType::ImmediateOrCancel { limit: protective },
            RoutedOrderType::Market,
        ]
    } else {
        [
            RoutedOrderType::ImmediateOrCancel { limit: protective },
            RoutedOrderType::FillOrKill { limit: protective },
            RoutedOrderType::Limit { price: protective },
            RoutedOrderType::Market,
        ]
    };
    ladder.into_iter().find(|candidate| {
        candidate.kind() != preferred.kind() && profile.supports(candidate.kind())
    })
}
