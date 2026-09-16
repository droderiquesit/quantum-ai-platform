//! Putting intents that named no venue onto one venue, before they are netted.
//!
//! Blueprint §27.2, third row: *same instrument, different venues — not netted
//! by default, because those are different executions at different prices. **The
//! router may consolidate onto the best venue if strategies did not specify
//! one.***
//!
//! The half that was already built is the refusal to net across venues:
//! [`qip_contracts::intent::net`] groups on instrument, venue and
//! representation, so two venues stay two orders. That is right whenever a
//! strategy *chose* its venue. It is wrong when nobody did — and nobody did is
//! the ordinary case for a strategy that reasons about an instrument rather
//! than about a book. Left alone, two strategies that both wanted to buy the
//! same thing and neither of which cared where produce two orders, pay the
//! spread twice, telegraph the position, and can cross each other. That is the
//! self-trade §27 exists to make impossible, arriving through the one door the
//! netting key leaves open.
//!
//! So this runs **before** netting and does one thing: it replaces the
//! placeholder venue on those intents with the venue [`crate::router::Router`]
//! says is cheapest all-in, which makes them a single netting group. It never
//! changes a size, never changes a side, and never touches an intent that named
//! a venue.
//!
//! # Why a reserved identifier and not an `Option<VenueId>`
//!
//! Because the honest form of "no venue chosen" is a missing field on
//! [`Intent`], and that type lives in `qip-contracts`, which this lane does not
//! own. A reserved identifier is the form available here, and it is made safe
//! by the one rule this module never breaks: **an intent carrying
//! [`UNSPECIFIED_VENUE`] never comes out the other side.** Either it is
//! rewritten to a real venue, or it is removed and reported in
//! [`Consolidation::refused`]. [`Consolidation::intents`] is the only way to
//! read the result and the vector is private, so a caller that ignores the
//! refusals still cannot send an order to a venue literally named
//! `UNSPECIFIED`. Were `Intent` ever to gain an `Option<VenueId>`, this module
//! changes and nothing that reads it does.
//!
//! # What is deliberately refused rather than consolidated
//!
//! A cycle leg. §27.2's fourth row says a leg is never netted with directional
//! intent because it is part of an atomic set whose economics depend on every
//! leg landing where it was priced. Moving one to "the best venue" after the
//! cycle was priced breaks exactly the same economics as netting it would, and
//! more quietly, because the sizes still look right. A leg that arrives without
//! a venue is a bug in whatever produced it, and it is refused with a message
//! saying so.

use crate::health::HealthTracker;
use crate::ordertype::Urgency;
use crate::ratelimit::RateLedger;
use crate::router::{Router, RoutingRequest, VenueCandidate, VenueExclusion};
use qip_contracts::intent::{Intent, Representation};
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The venue identifier a strategy uses to say it chose no venue.
///
/// Reserved: no real venue may carry it, and nothing in this crate will send to
/// it. It exists so that "the strategy did not specify one" is expressible at
/// all in a type whose `venue` field is not optional.
pub const UNSPECIFIED_VENUE: &str = "UNSPECIFIED";

/// Whether this intent left the venue to the router.
pub fn is_unspecified(venue: &VenueId) -> bool {
    venue.as_str() == UNSPECIFIED_VENUE
}

/// The venue a group was consolidated onto, and what it was chosen on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsolidationDecision {
    pub object_id: ObjectId,
    pub representation: Representation,
    /// The venue every intent in the group now names.
    pub venue: VenueId,
    /// The side the group was priced on.
    pub side: BookSide,
    /// The quantity the comparison was made at — see [`Consolidator::consolidate`]
    /// for why it is the net and not the gross.
    pub quantity_priced: Decimal,
    /// The all-in price per unit the winning venue was chosen on, fees and the
    /// venue's own behaviour included. Not the quote.
    pub effective_price: Decimal,
    /// How many intents were moved onto it.
    pub intents_moved: usize,
    /// Every venue the router considered and did not pick, with its reason, so
    /// the choice can be argued with afterwards rather than only observed.
    pub rejected: Vec<VenueExclusion>,
    pub reason: String,
}

/// A group nobody could take, and therefore nobody may send.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsolidationRefusal {
    pub object_id: ObjectId,
    pub representation: Representation,
    /// The strategies whose intent was dropped, so the refusal is attributable
    /// to the callers that will not see their order.
    pub strategies: Vec<StrategyId>,
    /// The signed net that was being placed.
    pub net_size: Decimal,
    pub rejected: Vec<VenueExclusion>,
    pub reason: String,
}

/// What came out: intents that all name a real venue, and what did not.
#[derive(Clone, Debug, PartialEq)]
pub struct Consolidation {
    /// Private, and that is the guarantee. See the module documentation.
    intents: Vec<Intent>,
    pub decisions: Vec<ConsolidationDecision>,
    pub refused: Vec<ConsolidationRefusal>,
}

impl Consolidation {
    /// The intents to net, every one of which names a venue that exists.
    pub fn intents(&self) -> &[Intent] {
        &self.intents
    }

    /// The same, taken by value for the netting seam.
    pub fn into_intents(self) -> Vec<Intent> {
        self.intents
    }

    /// Whether anything was left unplaceable. A caller that ignores this has
    /// dropped orders, which is safe but must be visible.
    pub fn is_complete(&self) -> bool {
        self.refused.is_empty()
    }
}

/// The grouping key for intents that named no venue.
///
/// Instrument and representation, and deliberately not venue — the venue is
/// what is being decided. Ordered, because the decisions reach a journal and a
/// replay that reorders is not a replay.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UnspecifiedKey {
    object_id: ObjectId,
    representation: Representation,
}

/// Chooses one venue for the intents that chose none.
#[derive(Clone, Debug, PartialEq)]
pub struct Consolidator {
    router: Router,
    urgency: Urgency,
}

impl Consolidator {
    /// Build one around a router.
    ///
    /// The urgency is the consolidator's own, not a strategy's: what is being
    /// answered here is "which venue", and the order type the router picks on
    /// the way to that answer is discarded. It is stated rather than defaulted
    /// silently because it changes which venues are eligible at all — a venue
    /// offering only passive types drops out of an immediate comparison.
    pub fn new(router: Router, urgency: Urgency) -> Self {
        Self { router, urgency }
    }

    /// Consolidate, leaving every intent that named a venue exactly as it was.
    ///
    /// `at` is a parameter rather than a clock read, so the same intents
    /// against the same market consolidate the same way on a replay.
    ///
    /// # What the comparison is made on
    ///
    /// The **net**, not the gross. The gross is what the strategies asked for;
    /// the net is what a venue would actually see, because the rest cancels
    /// internally — and pricing a venue on size it will never be sent is how a
    /// venue with deep top-of-book wins an order that was always going to be
    /// small. When the net is zero the group cancels completely and no venue
    /// will see anything at all; a venue is still chosen, because that is what
    /// puts the two intents in one netting group and turns them into the
    /// internal cross rather than two orders, and it is chosen on the largest
    /// single contributor, which is the largest quantity any venue could have
    /// been asked for.
    pub fn consolidate(
        &self,
        intents: Vec<Intent>,
        candidates: &[VenueCandidate],
        health: &HealthTracker,
        rates: &RateLedger,
        at: Timestamp,
    ) -> Result<Consolidation> {
        let mut settled: Vec<Intent> = Vec::new();
        let mut groups: BTreeMap<UnspecifiedKey, Vec<Intent>> = BTreeMap::new();

        for intent in intents {
            if !is_unspecified(&intent.venue) {
                settled.push(intent);
                continue;
            }
            if let Some(cycle_id) = intent.cycle_id() {
                return Err(Error::invalid(format!(
                    "a leg of cycle {cycle_id} arrived without a venue, and consolidating it onto \
                     the cheapest one would break the cycle's economics as silently as netting it \
                     would: the sizes still close and the prices no longer do. Price the leg at \
                     the venue the cycle was found on and build it with CycleLeg::new naming that \
                     venue"
                )));
            }
            groups
                .entry(UnspecifiedKey {
                    object_id: intent.object_id.clone(),
                    representation: intent.representation,
                })
                .or_default()
                .push(intent);
        }

        let mut decisions = Vec::new();
        let mut refused = Vec::new();
        for (key, mut members) in groups {
            // Deterministic, because the priced quantity below is taken from
            // the largest member and ties are broken on this order.
            members.sort_by(|left, right| left.strategy.as_str().cmp(right.strategy.as_str()));
            let net_size = members
                .iter()
                .map(|intent| intent.signed_size)
                .fold(Decimal::ZERO, |a, b| a + b);
            let (side, quantity) = pricing_basis(net_size, &members);

            let request = RoutingRequest::new(
                // Never sent anywhere: the router needs a parent identity for
                // its own bookkeeping and this group has no order yet, by
                // design — §27.2 is decided before an order object exists.
                OrderId::from_string(format!("consolidation-{}", key.object_id.as_str())),
                key.object_id.clone(),
                side,
                quantity,
                self.urgency,
            );
            let decision = self.router.route(&request, candidates, health, rates, at)?;
            let Some(best) = best_slice(&decision.slices, side) else {
                refused.push(ConsolidationRefusal {
                    object_id: key.object_id,
                    representation: key.representation,
                    strategies: members
                        .iter()
                        .map(|intent| intent.strategy.clone())
                        .collect(),
                    net_size,
                    rejected: decision.exclusions,
                    reason: format!(
                        "no venue could take {quantity} on the {} side, so {} intents naming no \
                         venue are withheld rather than sent to a placeholder",
                        side.as_str(),
                        members.len()
                    ),
                });
                continue;
            };

            let venue = best.venue.clone();
            let moved = members.len();
            for intent in &mut members {
                intent.venue = venue.clone();
            }
            decisions.push(ConsolidationDecision {
                object_id: key.object_id,
                representation: key.representation,
                venue: venue.clone(),
                side,
                quantity_priced: quantity,
                effective_price: best.effective_price,
                intents_moved: moved,
                rejected: decision.exclusions,
                reason: format!(
                    "{moved} intents naming no venue are consolidated onto {} at an all-in {} \
                     per unit, so they net into one order instead of {moved}",
                    venue.as_str(),
                    best.effective_price
                ),
            });
            settled.append(&mut members);
        }

        // Deterministic output order, because this vector is what `net` groups
        // and what reaches the journal behind it.
        settled.sort_by(|left, right| {
            left.object_id
                .as_str()
                .cmp(right.object_id.as_str())
                .then_with(|| left.venue.as_str().cmp(right.venue.as_str()))
                .then_with(|| left.representation.cmp(&right.representation))
                .then_with(|| left.strategy.as_str().cmp(right.strategy.as_str()))
        });

        // The invariant, checked rather than trusted. It is the whole safety
        // argument for using a reserved identifier instead of an absent field,
        // and it costs one pass.
        if let Some(stray) = settled.iter().find(|intent| is_unspecified(&intent.venue)) {
            return Err(Error::invalid(format!(
                "consolidation left {} naming no venue, which must never reach the netting seam; \
                 this is a defect in Consolidator::consolidate rather than in its input",
                stray.strategy.as_str()
            )));
        }

        Ok(Consolidation {
            intents: settled,
            decisions,
            refused,
        })
    }
}

/// The side and quantity a group is priced on.
///
/// Split out because it is the one piece of judgement in the module and it
/// deserves to be read on its own: the net when there is one, and the largest
/// single contributor when the group cancels to nothing. Both are deterministic
/// given a contributor vector sorted by strategy id.
fn pricing_basis(net_size: Decimal, members: &[Intent]) -> (BookSide, Decimal) {
    if !net_size.is_zero() {
        let side = if net_size.is_positive() {
            BookSide::Ask
        } else {
            BookSide::Bid
        };
        return (side, net_size.abs());
    }
    let largest = members
        .iter()
        .max_by(|left, right| left.gross().cmp(&right.gross()));
    largest.map_or((BookSide::Ask, Decimal::ZERO), |intent| {
        let side = if intent.signed_size.is_positive() {
            BookSide::Ask
        } else {
            BookSide::Bid
        };
        (side, intent.gross())
    })
}

/// The best venue among the router's slices, by all-in price.
///
/// The router splits across venues when that is cheapest; consolidation wants
/// one venue, so the split is read as a ranking and the best rung taken. Buying
/// wants the lowest all-in price and selling the highest, and ties break on the
/// venue identifier so two runs over one market pick the same venue.
fn best_slice(
    slices: &[crate::router::RouteSlice],
    side: BookSide,
) -> Option<&crate::router::RouteSlice> {
    slices.iter().reduce(|best, slice| {
        if slice.effective_price == best.effective_price {
            if slice.venue.as_str() < best.venue.as_str() {
                slice
            } else {
                best
            }
        } else if side.is_better(slice.effective_price, best.effective_price) {
            slice
        } else {
            best
        }
    })
}
