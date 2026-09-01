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
//! # What this is not yet
//!
//! It is the adapter and not the wiring. Nothing deployed calls
//! [`Opportunity::cycle_legs`]: the scanner is constructed by no composition
//! root, and the cell's work loop runs compiled strategy programs and never
//! consults an arbitrage graph. The next slice belongs to the cell and inserts
//! a scan between the strategy loop's intent collection and `net`, so that
//! legs and directional intents meet at the one seam the blueprint names.
//! §31's cross-region paths (3–6) are further off than that: the graph has no
//! mirror edge class to express them, and when it does the coordination is
//! policy distributed in advance — a reference price, a threshold, a
//! direction gate per region — and never a message at execution time. A
//! cell's graph holds the cell's own venues (ADR 0008), so every cycle this
//! module can see today is path 1, 2 or 7, executed alone.

use crate::scan::Opportunity;
use qip_contracts::edge::LegStep;
use qip_contracts::intent::CycleLeg;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};

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
