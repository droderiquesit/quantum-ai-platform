//! Deciding where an order goes, and in what sizes.
//!
//! The mistake this exists to avoid is routing to the best quoted price. The
//! quote is one term of the cost; the fee is another and is often larger than
//! the difference being chosen between; a venue that rejects one order in
//! twenty charges a re-route on every order whether it rejects this one or not.
//! So every venue is compared on what the order would actually cost end to end,
//! and the quoted price is recorded next to it so the two can be seen to
//! disagree.
//!
//! Allocation is a greedy walk down the marginal cost curve. The parent is
//! offered to the venues in slices; each slice goes wherever the *next* unit is
//! cheapest, which is not the same as wherever the first one was — a venue is
//! only the best venue until its top of book runs out. That is what produces a
//! split without anyone asking for one.
//!
//! Two invariants hold whatever the market looks like:
//!
//! * **Nothing is created or lost.** Slices plus `unrouted` equal the request,
//!   exactly, in [`Decimal`]. Lot rounding moves quantity into `unrouted`; it
//!   never rounds it away. A router that leaks a share on a rounding boundary
//!   produces a position nobody ordered, and it does it once in ten thousand
//!   orders, which is the worst possible frequency.
//! * **Every venue not used says why.** An empty [`RoutingDecision::slices`] is
//!   never the whole answer; the exclusions are.

use crate::health::{HealthTracker, HealthVerdict};
use crate::ordertype::{OrderTypeSelection, RoutedOrderType, Touch, Urgency, select_order_type};
use crate::venue::VenueProfile;
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{OrderBook, Side};
use serde::{Deserialize, Serialize};

/// Map a book side to the aggressing side that consumes it.
const fn aggressor_for(side: BookSide) -> Side {
    match side {
        BookSide::Bid => Side::Sell,
        BookSide::Ask => Side::Buy,
    }
}

/// How finely the parent is offered to the venues.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouterSettings {
    /// Slices the order is walked out in.
    ///
    /// More slices track the marginal cost curve more closely and cost more to
    /// compute. Eight is enough to notice that a venue's top of book has run
    /// out, which is the thing the split exists to notice.
    pub slices: u32,
}

impl Default for RouterSettings {
    fn default() -> Self {
        Self { slices: 8 }
    }
}

/// An order to be placed, and how badly it is wanted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingRequest {
    pub parent: OrderId,
    pub object_id: ObjectId,
    /// The side of the book being consumed: `Ask` to buy, `Bid` to sell.
    pub side: BookSide,
    pub quantity: Decimal,
    pub urgency: Urgency,
    /// Whether a partial fill is useless to the caller.
    pub all_or_none: bool,
    /// The worst all-in price acceptable, fees included.
    ///
    /// A limit on the quoted price would be a limit on the wrong number.
    pub price_limit: Option<Decimal>,
}

impl RoutingRequest {
    pub fn new(
        parent: OrderId,
        object_id: ObjectId,
        side: BookSide,
        quantity: Decimal,
        urgency: Urgency,
    ) -> Self {
        Self {
            parent,
            object_id,
            side,
            quantity,
            urgency,
            all_or_none: false,
            price_limit: None,
        }
    }

    pub fn with_price_limit(mut self, limit: Decimal) -> Self {
        self.price_limit = Some(limit);
        self
    }

    pub fn all_or_none(mut self) -> Self {
        self.all_or_none = true;
        self
    }
}

/// A venue the order could go to, with everything needed to price it there.
#[derive(Clone, Debug, PartialEq)]
pub struct VenueCandidate {
    pub profile: VenueProfile,
    pub status: VenueStatus,
    pub book: OrderBook,
}

impl VenueCandidate {
    pub fn new(profile: VenueProfile, status: VenueStatus, book: OrderBook) -> Self {
        Self {
            profile,
            status,
            book,
        }
    }
}

/// Why a venue was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExclusionReason {
    /// Halted, closed, or unreachable.
    NotAccepting,
    /// Taken out of rotation by its own reject rate.
    Quarantined,
    /// Nothing resting on the side this order needs.
    NoDepth,
    /// The slice that reached it was below the venue's minimum.
    BelowMinimumSize,
    /// It accepts none of the order types this order could use.
    NoUsableOrderType,
    /// Its all-in price is worse than the caller's limit.
    PriceLimit,
}

impl ExclusionReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotAccepting => "not_accepting",
            Self::Quarantined => "quarantined",
            Self::NoDepth => "no_depth",
            Self::BelowMinimumSize => "below_minimum_size",
            Self::NoUsableOrderType => "no_usable_order_type",
            Self::PriceLimit => "price_limit",
        }
    }
}

/// A venue that was considered and passed over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueExclusion {
    pub venue: VenueId,
    pub reason: ExclusionReason,
    pub detail: String,
}

/// One venue's share of the parent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteSlice {
    pub venue: VenueId,
    pub quantity: Decimal,
    pub order_type: RoutedOrderType,
    /// Volume-weighted price from the book alone — what a quote comparison
    /// would have seen.
    pub quoted_price: Decimal,
    /// All-in price per unit: the quote, the fee, and what the venue's
    /// behaviour costs. The number the choice was actually made on.
    pub effective_price: Decimal,
    /// Signed: negative is a rebate.
    pub fee: Decimal,
    /// The priced cost of this venue's observed rejects and latency.
    pub health_cost: Decimal,
    pub reason: String,
}

/// Where the order is going, and why everywhere else is not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub parent: OrderId,
    pub requested: Decimal,
    pub slices: Vec<RouteSlice>,
    /// Quantity no venue could take, stated rather than dropped.
    pub unrouted: Decimal,
    pub exclusions: Vec<VenueExclusion>,
    /// The comparison, in the words it was made in.
    pub notes: Vec<String>,
}

impl RoutingDecision {
    pub fn routed(&self) -> Decimal {
        self.slices
            .iter()
            .fold(Decimal::ZERO, |sum, slice| sum + slice.quantity)
    }

    /// Whether the split adds back up to the parent, exactly.
    pub fn accounts_for_every_share(&self) -> bool {
        self.routed() + self.unrouted == self.requested
    }

    /// The same check as an error, for a caller that must not proceed without it.
    pub fn validate(&self) -> Result<()> {
        if !self.accounts_for_every_share() {
            return Err(Error::invalid(format!(
                "the split routes {} and leaves {} unrouted, which does not add up to {}",
                self.routed(),
                self.unrouted,
                self.requested
            )));
        }
        Ok(())
    }

    pub fn venues(&self) -> Vec<&VenueId> {
        self.slices.iter().map(|slice| &slice.venue).collect()
    }

    pub fn is_split(&self) -> bool {
        self.slices.len() > 1
    }
}

/// Everything the router needs to price one venue.
#[derive(Debug)]
struct Eligible<'a> {
    profile: &'a VenueProfile,
    book: &'a OrderBook,
    touch: Touch,
    selection: OrderTypeSelection,
    health_bps_f64: f64,
}

/// Chooses venues and sizes on net cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Router {
    settings: RouterSettings,
}

impl Router {
    pub fn new(settings: RouterSettings) -> Self {
        Self { settings }
    }

    pub fn settings(&self) -> &RouterSettings {
        &self.settings
    }

    /// Decide where the order goes.
    ///
    /// `at` is a parameter rather than a clock read, so the same market state
    /// and the same health history route the same way on a replay.
    pub fn route(
        &self,
        request: &RoutingRequest,
        candidates: &[VenueCandidate],
        health: &HealthTracker,
        at: Timestamp,
    ) -> Result<RoutingDecision> {
        if request.quantity <= Decimal::ZERO {
            return Err(Error::invalid(
                "a routing request needs a positive quantity",
            ));
        }

        let mut exclusions: Vec<VenueExclusion> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let eligible = self.eligible(request, candidates, health, at, &mut exclusions)?;

        let mut allocation = vec![Decimal::ZERO; eligible.len()];
        let mut price_limited: Vec<usize> = Vec::new();
        let mut remaining = request.quantity;

        let step = self.step_size(request.quantity);
        // One extra pass so the remainder left by an inexact division still
        // gets offered rather than falling into `unrouted` by default.
        for _ in 0..(self.settings.slices as usize + 1) {
            if remaining <= Decimal::ZERO {
                break;
            }
            let take = step.min(remaining);
            let Some(chosen) =
                self.cheapest_for(request, &eligible, &allocation, take, &mut price_limited)?
            else {
                break;
            };
            allocation[chosen] += take;
            remaining -= take;
        }

        // Lot sizes and minimums, applied after the shape of the split is
        // decided. Every share shaved off here is tracked, never dropped.
        let mut residual = remaining;
        for (index, quantity) in allocation.iter_mut().enumerate() {
            let profile = eligible[index].profile;
            let aligned = profile.round_to_lot(*quantity);
            let usable = if profile.accepts_size(aligned) {
                aligned
            } else {
                if aligned > Decimal::ZERO || *quantity > Decimal::ZERO {
                    exclusions.push(VenueExclusion {
                        venue: profile.venue.clone(),
                        reason: ExclusionReason::BelowMinimumSize,
                        detail: format!(
                            "a slice of {quantity} rounds to {aligned}, below the {} minimum",
                            profile.min_size
                        ),
                    });
                }
                Decimal::ZERO
            };
            residual += *quantity - usable;
            *quantity = usable;
        }

        // Offer what the rounding freed to whoever is cheapest for it.
        if residual > Decimal::ZERO
            && let Some(chosen) = self.cheapest_for(
                request,
                &eligible,
                &allocation,
                residual,
                &mut price_limited,
            )?
        {
            let profile = eligible[chosen].profile;
            let extra = profile.round_to_lot(residual);
            if extra > Decimal::ZERO && profile.accepts_size(allocation[chosen] + extra) {
                allocation[chosen] += extra;
                residual -= extra;
            }
        }

        // A venue the limit kept the order away from is a decision, and gets
        // said out loud like every other one.
        for index in &price_limited {
            if allocation.get(*index).is_some_and(|q| *q > Decimal::ZERO) {
                continue;
            }
            let profile = eligible[*index].profile;
            if exclusions.iter().any(|e| e.venue == profile.venue) {
                continue;
            }
            exclusions.push(VenueExclusion {
                venue: profile.venue.clone(),
                reason: ExclusionReason::PriceLimit,
                detail: format!(
                    "its all-in price is worse than the stated limit of {}",
                    request
                        .price_limit
                        .map_or_else(|| "none".to_string(), |limit| limit.to_string())
                ),
            });
        }

        let mut slices: Vec<RouteSlice> = Vec::new();
        for (index, quantity) in allocation.iter().enumerate() {
            if *quantity <= Decimal::ZERO {
                continue;
            }
            slices.push(self.slice_for(request, &eligible[index], *quantity)?);
        }
        // Deterministic order, so a replay produces the same child sequence.
        slices.sort_by(|a, b| a.venue.as_str().cmp(b.venue.as_str()));

        for candidate in &eligible {
            let priced = self
                .consideration(request, candidate, step.min(request.quantity))?
                .and_then(|cost| unit_price(request.side, cost, step.min(request.quantity)));
            notes.push(match priced {
                Some(price) => format!(
                    "{} would take a {step} slice at an all-in {price} using a {} order ({})",
                    candidate.profile.venue.as_str(),
                    candidate.selection.order_type.kind().as_str(),
                    candidate.selection.reason
                ),
                None => format!(
                    "{} could not take a {step} slice at all",
                    candidate.profile.venue.as_str()
                ),
            });
        }
        if residual > Decimal::ZERO {
            notes.push(format!(
                "{residual} could not be placed anywhere and is reported unrouted rather than rounded away"
            ));
        }

        let decision = RoutingDecision {
            parent: request.parent.clone(),
            requested: request.quantity,
            slices,
            unrouted: residual,
            exclusions,
            notes,
        };
        // The invariant is checked here rather than trusted, because the one
        // way it breaks is a rounding path nobody looked at.
        decision.validate()?;
        Ok(decision)
    }

    fn step_size(&self, quantity: Decimal) -> Decimal {
        let slices = Decimal::from_int(i64::from(self.settings.slices.max(1)));
        match quantity.checked_div(slices) {
            Some(step) if step > Decimal::ZERO => step,
            _ => quantity,
        }
    }

    fn eligible<'a>(
        &self,
        request: &RoutingRequest,
        candidates: &'a [VenueCandidate],
        health: &HealthTracker,
        at: Timestamp,
        exclusions: &mut Vec<VenueExclusion>,
    ) -> Result<Vec<Eligible<'a>>> {
        let mut eligible = Vec::new();
        for candidate in candidates {
            candidate.profile.validate()?;
            let venue = candidate.profile.venue.clone();

            if !candidate.status.accepts_orders() {
                exclusions.push(VenueExclusion {
                    venue,
                    reason: ExclusionReason::NotAccepting,
                    detail: format!(
                        "the venue is {} and an order sent there does not bounce, it disappears",
                        candidate.status.as_str()
                    ),
                });
                continue;
            }

            let assessment = health.assess(&venue, candidate.profile.typical_latency, at);
            if let HealthVerdict::Quarantined { until, reason } = &assessment.verdict {
                exclusions.push(VenueExclusion {
                    venue,
                    reason: ExclusionReason::Quarantined,
                    detail: format!("{reason}; out of rotation until {until}"),
                });
                continue;
            }

            let Some(touch) = Touch::from_book(&candidate.book) else {
                exclusions.push(VenueExclusion {
                    venue,
                    reason: ExclusionReason::NoDepth,
                    detail: "the book is one-sided, so there is no price to send against"
                        .to_string(),
                });
                continue;
            };

            let displayed = candidate
                .book
                .depth(aggressor_for(request.side), usize::MAX);
            if displayed <= Decimal::ZERO {
                exclusions.push(VenueExclusion {
                    venue,
                    reason: ExclusionReason::NoDepth,
                    detail: format!("nothing is resting on the {} side", request.side.as_str()),
                });
                continue;
            }

            let selection = match select_order_type(
                &candidate.profile,
                request.side,
                request.quantity,
                displayed,
                touch,
                request.urgency,
                request.all_or_none,
            ) {
                Ok(selection) => selection,
                Err(error) => {
                    exclusions.push(VenueExclusion {
                        venue,
                        reason: ExclusionReason::NoUsableOrderType,
                        detail: error.message().to_string(),
                    });
                    continue;
                }
            };

            eligible.push(Eligible {
                profile: &candidate.profile,
                book: &candidate.book,
                touch,
                selection,
                health_bps_f64: assessment.cost_bps_f64,
            });
        }
        Ok(eligible)
    }

    /// The venue whose next `take` is cheapest, or `None` if nobody can take it.
    fn cheapest_for(
        &self,
        request: &RoutingRequest,
        eligible: &[Eligible<'_>],
        allocation: &[Decimal],
        take: Decimal,
        price_limited: &mut Vec<usize>,
    ) -> Result<Option<usize>> {
        let mut best: Option<(usize, Decimal)> = None;
        for (index, candidate) in eligible.iter().enumerate() {
            let already = allocation[index];
            let Some(before) = self.consideration(request, candidate, already)? else {
                continue;
            };
            let Some(after) = self.consideration(request, candidate, already + take)? else {
                continue;
            };
            let marginal = after - before;
            let Some(price) = unit_price(request.side, marginal, take) else {
                continue;
            };
            if let Some(limit) = request.price_limit
                && request.side.is_better(limit, price)
            {
                if !price_limited.contains(&index) {
                    price_limited.push(index);
                }
                continue;
            }
            // A signed cost, so one comparison serves both sides: buying wants
            // the smallest outlay, selling the largest proceeds, and negating
            // the sell turns the second into the first.
            let Some(cost) = marginal.checked_div(take) else {
                continue;
            };
            if best.is_none_or(|(best_index, best_cost)| {
                cost < best_cost
                    || (cost == best_cost
                        && candidate.profile.venue.as_str()
                            < eligible[best_index].profile.venue.as_str())
            }) {
                best = Some((index, cost));
            }
        }
        Ok(best.map(|(index, _)| index))
    }

    /// Signed cost of taking `quantity` at one venue: positive is money out.
    ///
    /// `None` means the venue cannot supply that size, which is different from
    /// supplying it expensively and must not be flattened into a large number.
    fn consideration(
        &self,
        request: &RoutingRequest,
        candidate: &Eligible<'_>,
        quantity: Decimal,
    ) -> Result<Option<Decimal>> {
        if quantity <= Decimal::ZERO {
            return Ok(Some(Decimal::ZERO));
        }
        let order_type = candidate.selection.order_type;
        let price = if order_type.is_passive() {
            // A resting order trades at the price it posted. Whether it trades
            // at all is not knowable from the book, which is what `Urgency` is
            // for and what child-order management picks up when it does not.
            candidate.touch.resting(request.side)
        } else {
            let Some((vwap, filled)) = candidate.book.sweep(aggressor_for(request.side), quantity)
            else {
                return Ok(None);
            };
            if filled < quantity {
                return Ok(None);
            }
            vwap
        };

        let Some(notional) = quantity.checked_mul(price) else {
            return Err(Error::numeric("a venue's notional overflowed"));
        };
        let fee = candidate.profile.fees.fee(
            notional,
            order_type.liquidity(),
            candidate.profile.trailing_volume,
        );
        let health_cost = notional.apply_bps(candidate.health_bps_f64);
        let signed_notional = match request.side {
            BookSide::Ask => notional,
            BookSide::Bid => -notional,
        };
        Ok(Some(signed_notional + fee + health_cost))
    }

    fn slice_for(
        &self,
        request: &RoutingRequest,
        candidate: &Eligible<'_>,
        quantity: Decimal,
    ) -> Result<RouteSlice> {
        let order_type = candidate.selection.order_type;
        let quoted_price = if order_type.is_passive() {
            candidate.touch.resting(request.side)
        } else {
            candidate
                .book
                .sweep(aggressor_for(request.side), quantity)
                .map(|(vwap, _)| vwap)
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "{} was allocated {quantity} it cannot fill",
                        candidate.profile.venue.as_str()
                    ))
                })?
        };
        let notional = quantity
            .checked_mul(quoted_price)
            .ok_or_else(|| Error::numeric("a slice notional overflowed"))?;
        let fee = candidate.profile.fees.fee(
            notional,
            order_type.liquidity(),
            candidate.profile.trailing_volume,
        );
        let health_cost = notional.apply_bps(candidate.health_bps_f64);
        let consideration = match request.side {
            BookSide::Ask => notional + fee + health_cost,
            BookSide::Bid => -notional + fee + health_cost,
        };
        let effective_price = unit_price(request.side, consideration, quantity)
            .ok_or_else(|| Error::numeric("a slice's effective price is undefined"))?;

        Ok(RouteSlice {
            venue: candidate.profile.venue.clone(),
            quantity,
            order_type,
            quoted_price,
            effective_price,
            fee,
            health_cost,
            reason: format!(
                "quoted {quoted_price}, all-in {effective_price} after a {} fee of {fee}; {}",
                order_type.liquidity().as_str(),
                candidate.selection.reason
            ),
        })
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new(RouterSettings::default())
    }
}

/// Turn a signed consideration back into a price per unit.
///
/// Buying pays the consideration; selling receives it, so the sign flips. Doing
/// this in one place is what keeps a sell from being routed to the venue that
/// pays least.
fn unit_price(side: BookSide, consideration: Decimal, quantity: Decimal) -> Option<Decimal> {
    let per_unit = consideration.checked_div(quantity)?;
    Some(match side {
        BookSide::Ask => per_unit,
        BookSide::Bid => -per_unit,
    })
}
