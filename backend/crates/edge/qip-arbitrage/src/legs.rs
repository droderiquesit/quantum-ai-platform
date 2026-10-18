//! From a planned cycle to the legs the cell's netting seam accepts.
//!
//! Blueprint §27 makes one rule of the cell: **no strategy sends an order.**
//! Everything the cell wants done arrives at one seam as a
//! `Vec<qip_contracts::intent::Intent>`, is netted there, gated there, and
//! sent from there. The arbitrage family is not an exception to that rule; it
//! is the family the rule's hardest clause is about. §27.2: a cycle leg is
//! part of an atomic set, it is never netted with directional flow, and it
//! carries a no-net flag. This module is where the scanner's output takes on
//! that flag — and it takes it on by type, because
//! [`qip_contracts::intent::CycleLeg`] has no nettable form to fall into.
//!
//! # What becomes a leg
//!
//! One [`qip_contracts::edge::LegStep`] of the planned trade becomes one
//! [`CycleLeg`], in the plan's order. The plan already decided which leg goes
//! first and which are optional dust; this module does not re-decide either.
//! Transfers are not legs — [`crate::plan::LegPlanner`] realises them as
//! prefunded inventory, so they never appear in `steps()` and never become an
//! order, which is correct: a transfer is not something a venue can fill.
//!
//! # The sign, stated once because it once inverted
//!
//! A leg's `side` is **the side of the book consumed**
//! ([`crate::graph::EdgeKind::Trade`]): consuming offers *buys*, consuming bids
//! *sells*. The cell now signs its own intents by the same rule — taking the
//! ask is positive, hitting the bid is negative — but it did not always: until
//! the sign was corrected at the seam where an intent is made, `Bid` was a buy
//! in the cell and the two conventions were opposite, and an adapter that
//! copied the leg's side into a signed size by that old rule would have
//! emitted every leg backwards. A cycle that sells what it meant to buy at
//! each step still closes, so nothing downstream would notice. The mapping
//! therefore lives in [`signed_size`] and nowhere else, and a test holds it
//! against the fixture whose sides are known rather than against the cell.
//!
//! # What is wired, and what is not
//!
//! The adapter has its caller. `qip_edge::arbitrage::ArbitrageDesk` holds the
//! scanner, and `qip_edge::Cell::work` re-quotes the desk's graph from the
//! cell's own books, scans it between the strategy loop's intent collection
//! and `net`, takes each surviving opportunity through
//! [`Opportunity::cycle_legs`], and sends the legs by the cell's one order
//! path — never through `net`, which the type already made impossible and
//! the cell now never attempts. Every leg passes the cell's feasibility gate
//! first (blueprint §18.1), and a leg that cannot execute at its size vetoes
//! the cycle whole.
//!
//! What remains further off is unchanged by that. §31's cross-region paths
//! (3–6) need a mirror edge class the graph does not have, and when it does
//! the coordination is policy distributed in advance — a reference price, a
//! threshold, a direction gate per region — and never a message at
//! execution time. A cell's graph holds the cell's own venues (ADR 0008), so
//! every cycle the desk can see today is path 1, 2 or 7, executed alone. And
//! A cycle whose later leg a venue refuses still halts the cell, but no
//! longer leaves what filled standing: [`unwind_plan`] turns the legs the
//! venue reported filled into the offsetting orders that close them, and the
//! cell sends those through its own order path before it halts. The fill
//! path that sentence once said the cell lacked is the order-entry channel's
//! execution reports.

use crate::scan::Opportunity;
use qip_contracts::edge::LegStep;
use qip_contracts::intent::CycleLeg;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use std::collections::BTreeMap;

impl Opportunity {
    /// A reproducible identity for this cycle at this instant.
    ///
    /// The path kind, then every leg as `object@venue/side` in plan order,
    /// then the instant the cycle was opened at, in nanoseconds. Two scans of
    /// the same market at the same instant produce the same id — a replay
    /// must — and the same cycle taken again a moment later produces a
    /// different one, because they are two atomic sets and a journal that
    /// gave them one name could not tell which leg belonged to which.
    ///
    /// The strategy is deliberately not part of it. The cycle is a fact about
    /// the market; which strategy claims it is a fact about the cell, and it
    /// travels on the intent rather than in the cycle's name.
    pub fn cycle_id(&self, opened_at: Timestamp) -> String {
        let path: Vec<String> = self
            .planned
            .plan
            .steps()
            .iter()
            .map(|step| {
                format!(
                    "{}@{}/{}",
                    step.object_id.as_str(),
                    step.venue.as_str(),
                    step.side.as_str()
                )
            })
            .collect();
        format!(
            "{}:{}@{}",
            self.pricing.kind.as_str(),
            path.join(">"),
            opened_at.as_nanos()
        )
    }

    /// The legs of this cycle as no-net intents, in plan order.
    ///
    /// `strategy` is the deployment that will own the fills, `opened_at` the
    /// instant the cycle is being committed — the scan's `now`, in practice —
    /// and `valid_until` the deadline after which an unfilled leg is a
    /// stranded position rather than a pending one. A deadline at or before
    /// the opening instant is refused: a leg born expired would be sent and
    /// immediately abandoned, which is the half-filled cycle the plan exists
    /// to prevent, produced on purpose.
    ///
    /// The legs carry no feature inputs. A scan's evidence is the books it
    /// walked, and the book snapshots behind a [`crate::LiquiditySource`] have
    /// an age and an observation count but no revision number an attribution
    /// could cite. The caller that has the source may attach what it knows
    /// through [`CycleLeg::with_inputs`]; this adapter does not invent one.
    pub fn cycle_legs(
        &self,
        strategy: &StrategyId,
        opened_at: Timestamp,
        valid_until: Timestamp,
    ) -> Result<Vec<CycleLeg>> {
        if valid_until <= opened_at {
            return Err(Error::invalid(format!(
                "a cycle opened at {} cannot have its legs expire at {}; a leg born expired is \
                 sent and abandoned in the same breath, which strands the rest of the set",
                opened_at.as_nanos(),
                valid_until.as_nanos()
            )));
        }
        let cycle_id = self.cycle_id(opened_at);
        self.planned
            .plan
            .steps()
            .iter()
            .map(|step| {
                CycleLeg::new(
                    cycle_id.clone(),
                    strategy.clone(),
                    step.object_id.clone(),
                    step.venue.clone(),
                    signed_size(step),
                    step.reference_price,
                    valid_until,
                )
            })
            .collect()
    }
}

/// The signed size the netting seam expects, from the side of the book the
/// leg consumes: offers consumed is a buy and positive, bids consumed is a
/// sell and negative. See the module comment for why this is stated here
/// rather than copied from the cell's rule, which once inverted it.
fn signed_size(step: &LegStep) -> Decimal {
    match step.side {
        BookSide::Ask => step.quantity,
        BookSide::Bid => -step.quantity,
    }
}

/// One order that closes what a broken cycle's filled legs left open at one
/// venue on one instrument: `side` is the book it takes from (a buy takes
/// the ask), `quantity` is positive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnwindOrder {
    pub venue: VenueId,
    pub object_id: ObjectId,
    pub side: BookSide,
    pub quantity: Decimal,
}

/// The offsetting orders that return a broken cycle's filled legs to flat.
///
/// `filled` is each leg the venue reported filling — venue, instrument, the
/// side the leg took, and the quantity filled — and nothing planned: a leg
/// that went out and never filled is a resting order to withdraw, not a
/// position to offset, and offsetting a planned size would open the very
/// position the unwind exists to close. Fills are netted per venue and
/// instrument first, so two legs that traded the same book in opposite
/// directions offset each other rather than producing two orders, and a
/// book whose fills net to zero produces none. The result is in
/// `(venue, instrument)` order so a replay sends the same orders in the
/// same order.
pub fn unwind_plan(
    filled: impl IntoIterator<Item = (VenueId, ObjectId, BookSide, Decimal)>,
) -> Vec<UnwindOrder> {
    let mut net: BTreeMap<(VenueId, ObjectId), Decimal> = BTreeMap::new();
    for (venue, object_id, side, quantity) in filled {
        let signed = match side {
            BookSide::Ask => quantity,
            BookSide::Bid => -quantity,
        };
        *net.entry((venue, object_id)).or_insert(Decimal::ZERO) += signed;
    }
    net.into_iter()
        .filter(|(_, held)| !held.is_zero())
        .map(|((venue, object_id), held)| UnwindOrder {
            venue,
            object_id,
            // Long is closed by selling into the bid, short by buying the ask.
            side: if held.is_positive() {
                BookSide::Bid
            } else {
                BookSide::Ask
            },
            quantity: held.abs(),
        })
        .collect()
}

#[cfg(test)]
mod unwind_tests {
    use super::*;

    fn d(literal: &str) -> Decimal {
        Decimal::parse(literal).unwrap_or(Decimal::ZERO)
    }

    #[test]
    fn filled_legs_net_per_book_and_each_open_book_gets_one_opposite_order() {
        let x = VenueId::new("X");
        let eth = ObjectId::from_string("ETHUSDT");
        let btc = ObjectId::from_string("BTCUSDT");
        let plan = unwind_plan([
            (x.clone(), eth.clone(), BookSide::Ask, d("2")),
            (x.clone(), btc.clone(), BookSide::Bid, d("0.1")),
            (x.clone(), eth.clone(), BookSide::Bid, d("0.5")),
        ]);
        assert_eq!(
            plan,
            vec![
                UnwindOrder {
                    venue: x.clone(),
                    object_id: btc,
                    side: BookSide::Ask,
                    quantity: d("0.1"),
                },
                UnwindOrder {
                    venue: x,
                    object_id: eth,
                    side: BookSide::Bid,
                    quantity: d("1.5"),
                },
            ]
        );
    }

    #[test]
    fn fills_that_net_to_flat_need_no_order() {
        let x = VenueId::new("X");
        let eth = ObjectId::from_string("ETHUSDT");
        let plan = unwind_plan([
            (x.clone(), eth.clone(), BookSide::Ask, d("1")),
            (x, eth, BookSide::Bid, d("1")),
        ]);
        assert!(plan.is_empty(), "{plan:?}");
    }
}
