//! Legs that must complete together, or not at all.
//!
//! A multi-leg trade — a cross-venue arbitrage, a cross-asset spread, a hedged
//! entry — is one economic decision expressed as several orders. The orders can
//! fail independently. That is the entire problem this module exists for.
//!
//! # The failure being prevented
//!
//! Buy leg fills. Sell leg does not. The platform now holds a naked long
//! position it never decided to take, sized for an arbitrage that no longer
//! exists, and every single-order control upstream was satisfied: each leg
//! passed its own pre-trade check, each was inside its own notional limit, each
//! was individually correct. The exposure is created by the *gap between* them,
//! which no per-order control can see because no per-order control knows the
//! other leg exists.
//!
//! So the group is the unit that carries the risk, and [`LegGroup::leg_risk`]
//! is the number that must be bounded: the notional of what has filled minus
//! the notional of what has filled against it. A balanced group carries none
//! however large its legs are. A half-filled one carries all of it.
//!
//! # The invariant
//!
//! **A group that cannot complete is unwound, never abandoned.** Those are the
//! only two ends: every leg fills, or every leg that filled is reversed.
//! Abandoning a half-filled group would leave the position above in the book
//! with nothing recording that it was unintended — which is the state that
//! looks, to every downstream report, exactly like a position somebody chose.
//!
//! [`LegGroup::assess`] returns that decision and nothing else. It reads a
//! clock passed in, holds no I/O, and given the same group and instant returns
//! the same verdict — because a recovery path that behaves differently on
//! replay cannot be tested, and this one has to be.
//!
//! # What this module does not do
//!
//! It does not submit, cancel, or reverse anything. It decides *what should
//! happen* and hands back the orders that would do it. Submission goes through
//! [`crate::oms::OrderManager`] like every other order, so an unwind is
//! risk-checked, kill-switch-checked and recorded on the same path as the
//! trade that caused it. An unwind path with its own private route to a venue
//! would be a way around every control, opened for the one case that most
//! needs them.

use crate::order::{Fill, Order, OrderType, Side};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One leg of a group: the order, and what has filled of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub order: Order,
    /// Quantity filled so far, accumulated from [`LegGroup::record_fill`].
    pub filled: Decimal,
    /// Notional filled so far, at the prices actually paid.
    ///
    /// Accumulated rather than derived from `filled × arrival_price`: a leg
    /// filled in three prints at three prices has a notional none of those
    /// prices alone produces, and the leg risk below is a money figure.
    pub filled_notional: Decimal,
    /// Ids of every fill already folded into `filled`/`filled_notional`.
    ///
    /// A redelivered venue report carries the same fill id a second time.
    /// Without this, [`LegGroup::record_fill`] has no way to tell that print
    /// apart from a second, genuinely distinct one at the same size and
    /// price — it would double the leg's notional, which is exactly the
    /// quantity `leg_risk` is trusted to bound correctly.
    ///
    /// `#[serde(default)]` so a group persisted before this field existed
    /// still deserializes; the event log has to stay replayable across the
    /// change that added the check, not just after it.
    #[serde(default)]
    applied_fills: std::collections::BTreeSet<String>,
}

impl Leg {
    pub fn new(order: Order) -> Self {
        Self {
            order,
            filled: Decimal::ZERO,
            filled_notional: Decimal::ZERO,
            applied_fills: std::collections::BTreeSet::new(),
        }
    }

    /// Whether this leg has filled its full quantity.
    pub fn is_complete(&self) -> bool {
        self.filled >= self.order.quantity
    }

    /// What remains to trade on this leg.
    pub fn remaining(&self) -> Decimal {
        let remaining = self.order.quantity - self.filled;
        if remaining.is_positive() {
            remaining
        } else {
            Decimal::ZERO
        }
    }

    /// Signed filled notional: positive for a buy, negative for a sell.
    ///
    /// The sign is what makes the group's leg risk a cancellation rather than a
    /// sum. Two legs of equal notional on opposite sides net to nothing, which
    /// is the arithmetic statement of "the trade is balanced".
    fn signed_notional(&self) -> Decimal {
        match self.order.side {
            Side::Buy => self.filled_notional,
            Side::Sell => Decimal::ZERO - self.filled_notional,
        }
    }
}

/// Where a group is in its life.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupState {
    /// Legs exist; none has been submitted.
    Pending,
    /// At least one leg is working at a venue.
    Working,
    /// Every leg filled its full quantity. The only clean end.
    Complete { at: Timestamp },
    /// The group cannot complete and the filled legs are being reversed.
    Unwinding { at: Timestamp, reason: String },
    /// Every filled leg has a reversing order.
    Unwound { at: Timestamp },
    /// The group ended with nothing filled, so there was nothing to reverse.
    ///
    /// Distinct from `Unwound` on purpose. Both are safe ends and they mean
    /// different things: one cost two round trips and moved the book, the other
    /// cost nothing. A report that merged them could not tell an operator how
    /// often the platform is missing legs.
    Abandoned { at: Timestamp, reason: String },
}

impl GroupState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Working => "working",
            Self::Complete { .. } => "complete",
            Self::Unwinding { .. } => "unwinding",
            Self::Unwound { .. } => "unwound",
            Self::Abandoned { .. } => "abandoned",
        }
    }

    /// Whether the group has reached an end it cannot leave.
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Complete { .. } | Self::Unwound { .. } | Self::Abandoned { .. }
        )
    }
}

/// What the group should do next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Keep working the unfilled legs.
    Continue,
    /// Every leg is filled.
    Complete,
    /// Stop and reverse what filled, for the stated reason.
    Unwind { reason: String },
    /// Stop; nothing filled, so nothing to reverse.
    Abandon { reason: String },
}

/// A set of orders that express one decision and must end together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegGroup {
    pub group_id: String,
    pub legs: Vec<Leg>,
    /// The instant after which an incomplete group is given up on.
    ///
    /// Required, with no default. A multi-leg trade whose legs may sit
    /// unmatched indefinitely is a naked position with a plan to fix it later,
    /// and "later" is exactly the part that does not survive an outage.
    pub deadline: Timestamp,
    /// The most unmatched notional this group may carry while it works.
    ///
    /// Checked continuously rather than only at the deadline: a group whose
    /// first leg fills for far more than its second ever will is already the
    /// failure, and waiting for a clock to say so holds the exposure for the
    /// whole remaining window.
    pub max_leg_risk: Decimal,
    pub state: GroupState,
    /// Reversing orders issued, by the order id of the leg they reverse.
    unwinds: BTreeMap<String, OrderId>,
}

impl LegGroup {
    /// Assemble a group.
    ///
    /// Refuses a single-leg group. One order has no leg risk by construction —
    /// there is no gap between it and anything — so a "group" of one is a
    /// caller confusion, and admitting it would put an order on a path whose
    /// entire purpose is a risk it cannot have.
    pub fn new(
        group_id: impl Into<String>,
        orders: Vec<Order>,
        deadline: Timestamp,
        max_leg_risk: Decimal,
    ) -> Result<Self> {
        let group_id = group_id.into();
        if group_id.trim().is_empty() {
            return Err(Error::invalid(
                "a leg group needs an id; an unnamed group cannot be reconciled against the \
                 orders it issued",
            ));
        }
        if orders.len() < 2 {
            return Err(Error::invalid(format!(
                "leg group {group_id} has {} leg(s). A group exists to bound the risk between \
                 legs, and one order has no between; submit it as an ordinary order",
                orders.len()
            )));
        }
        if !max_leg_risk.is_positive() {
            return Err(Error::invalid(format!(
                "leg group {group_id} permits {max_leg_risk} of unmatched notional. A bound of \
                 zero or less can never be satisfied once any leg fills, so the group would \
                 unwind itself on its first print"
            )));
        }
        Ok(Self {
            group_id,
            legs: orders.into_iter().map(Leg::new).collect(),
            deadline,
            max_leg_risk,
            state: GroupState::Pending,
            unwinds: BTreeMap::new(),
        })
    }

    /// The unmatched notional the group is currently carrying.
    ///
    /// The signed sum of what has filled. Zero when the filled legs balance —
    /// which is the state the group is trying to reach and the only state in
    /// which stopping costs nothing.
    pub fn leg_risk(&self) -> Decimal {
        self.legs
            .iter()
            .fold(Decimal::ZERO, |sum, leg| sum + leg.signed_notional())
            .abs()
    }

    /// Whether every leg has filled in full.
    pub fn is_filled(&self) -> bool {
        self.legs.iter().all(Leg::is_complete)
    }

    /// Whether anything has filled at all.
    pub fn has_exposure(&self) -> bool {
        self.legs.iter().any(|leg| leg.filled.is_positive())
    }

    /// Record a fill against the leg that owns it.
    ///
    /// Keyed on the order id rather than an index: a fill arriving for an order
    /// this group does not hold is a routing mistake, and applying it to
    /// whatever leg happened to be at that index would put someone else's fill
    /// into this group's risk arithmetic.
    pub fn record_fill(&mut self, fill: &Fill) -> Result<()> {
        let group_id = self.group_id.clone();
        let leg = self
            .legs
            .iter_mut()
            .find(|leg| leg.order.order_id == fill.order_id)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "fill {} names order {}, which is not a leg of group {group_id}",
                    fill.fill_id.as_str(),
                    fill.order_id.as_str()
                ))
            })?;
        if !leg.applied_fills.insert(fill.fill_id.as_str().to_string()) {
            return Err(Error::invalid(format!(
                "fill {} was already applied to leg {}; a redelivered report is not a new fill",
                fill.fill_id.as_str(),
                fill.order_id.as_str()
            )));
        }
        leg.filled += fill.quantity;
        leg.filled_notional += fill.quantity * fill.price;
        if matches!(self.state, GroupState::Pending) {
            self.state = GroupState::Working;
        }
        Ok(())
    }

    /// Decide what the group should do, as of `now`.
    ///
    /// Pure: no clock is read and no I/O is performed, so the same group at the
    /// same instant always returns the same verdict. A recovery decision that
    /// varied between a live run and a replay could not be tested, and this is
    /// the decision that most needs testing.
    ///
    /// The order of the checks is the policy. Completion is checked first, so a
    /// group that filled everything in its final moment completes rather than
    /// being unwound by a deadline it met. Leg risk is checked before the
    /// deadline, because an over-exposed group is already the failure and
    /// waiting for the clock holds the exposure for the rest of the window.
    pub fn assess(&self, now: Timestamp) -> Verdict {
        if self.is_filled() {
            return Verdict::Complete;
        }

        let risk = self.leg_risk();
        if risk > self.max_leg_risk {
            return Verdict::Unwind {
                reason: format!(
                    "unmatched notional {risk} exceeds the {} this group may carry",
                    self.max_leg_risk
                ),
            };
        }

        if now >= self.deadline {
            return if self.has_exposure() {
                Verdict::Unwind {
                    reason: format!(
                        "the group did not complete by its deadline and carries {risk} unmatched"
                    ),
                }
            } else {
                Verdict::Abandon {
                    reason: "the group did not complete by its deadline and nothing filled"
                        .to_string(),
                }
            };
        }

        Verdict::Continue
    }

    /// The orders that would reverse everything filled.
    ///
    /// One per leg that filled, on the opposite side, for exactly the filled
    /// quantity — not the ordered quantity. Reversing the order size would
    /// close a position the group never took and open the mirror of it, which
    /// turns a recovery into a new naked trade.
    ///
    /// Market orders: an unwind is the platform reducing a risk it has decided
    /// it should not hold, and a limit price would make the reversal
    /// conditional on the market coming back to it. That is a way of holding
    /// the position while appearing to close it.
    ///
    /// Idempotent by construction. The reversing order's id is derived from the
    /// leg's, so calling this twice produces the same ids rather than a second
    /// set of orders that would double the reversal into a position opposite
    /// the one being closed.
    pub fn unwind_orders(&self, now: Timestamp) -> Vec<Order> {
        self.legs
            .iter()
            .filter(|leg| leg.filled.is_positive())
            .map(|leg| {
                let reversed = match leg.order.side {
                    Side::Buy => Side::Sell,
                    Side::Sell => Side::Buy,
                };
                // The average price actually paid, which is what the leg is
                // worth to reverse. Guarded because a fill of zero quantity
                // would divide by it.
                let arrival = if leg.filled.is_positive() {
                    leg.filled_notional / leg.filled
                } else {
                    leg.order.arrival_price
                };
                Order::new(
                    OrderId::from_string(format!("{}-unwind", leg.order.order_id.as_str())),
                    ObjectId::from_string(leg.order.object_id.as_str()),
                    reversed,
                    leg.filled,
                    OrderType::Market,
                    arrival,
                    leg.order.proposal_id.clone(),
                    leg.order.hypotheses.clone(),
                    leg.order.scope.clone(),
                    now,
                )
            })
            .collect()
    }

    /// Move the group to the end `assess` chose, recording the unwinds issued.
    ///
    /// Takes the reversing orders rather than producing them, so the caller
    /// that actually submitted them is the one that says which were issued. A
    /// group that recorded an unwind nobody submitted would report itself safe
    /// while the position was still open.
    pub fn settle(&mut self, verdict: &Verdict, issued: &[Order], now: Timestamp) -> Result<()> {
        if self.state.is_terminal() {
            return Err(Error::denied(format!(
                "leg group {} is already {} and cannot be settled again",
                self.group_id,
                self.state.as_str()
            )));
        }
        match verdict {
            Verdict::Continue => Ok(()),
            Verdict::Complete => {
                if !self.is_filled() {
                    return Err(Error::denied(format!(
                        "leg group {} was told to complete with {} of unmatched notional",
                        self.group_id,
                        self.leg_risk()
                    )));
                }
                self.state = GroupState::Complete { at: now };
                Ok(())
            }
            Verdict::Abandon { reason } => {
                if self.has_exposure() {
                    return Err(Error::denied(format!(
                        "leg group {} was told to abandon while carrying {} of unmatched \
                         notional; a group with exposure is unwound, never abandoned",
                        self.group_id,
                        self.leg_risk()
                    )));
                }
                self.state = GroupState::Abandoned {
                    at: now,
                    reason: reason.clone(),
                };
                Ok(())
            }
            Verdict::Unwind { reason } => {
                let expected = self.unwind_orders(now).len();
                if issued.len() != expected {
                    return Err(Error::denied(format!(
                        "leg group {} has {expected} filled leg(s) to reverse and {} reversing \
                         order(s) were issued; a group is not unwound until every filled leg has \
                         one",
                        self.group_id,
                        issued.len()
                    )));
                }
                for order in issued {
                    self.unwinds
                        .insert(order.order_id.as_str().to_string(), order.order_id.clone());
                }
                self.state = if expected == 0 {
                    GroupState::Abandoned {
                        at: now,
                        reason: reason.clone(),
                    }
                } else {
                    GroupState::Unwound { at: now }
                };
                Ok(())
            }
        }
    }

    /// The reversing orders this group has recorded as issued.
    pub fn unwinds(&self) -> Vec<&OrderId> {
        self.unwinds.values().collect()
    }

    /// Mark the group as unwinding, before the reversing orders are placed.
    ///
    /// Separate from [`Self::settle`] so the record shows the decision was made
    /// before the orders went out. A crash between the two leaves a group
    /// visibly `Unwinding` with no unwinds recorded, which is the state a
    /// recovery has to be able to find — rather than one still marked
    /// `Working`, which reads as a group that is fine.
    pub fn begin_unwind(&mut self, reason: impl Into<String>, now: Timestamp) -> Result<()> {
        if self.state.is_terminal() {
            return Err(Error::denied(format!(
                "leg group {} is already {}",
                self.group_id,
                self.state.as_str()
            )));
        }
        self.state = GroupState::Unwinding {
            at: now,
            reason: reason.into(),
        };
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_core::ids::FillId;

    /// Parse a decimal in a test fixture. `dec!` takes a literal only, and
    /// these helpers take the value as a parameter.
    fn d(value: &str) -> Decimal {
        Decimal::parse(value).expect("the fixture holds a well-formed decimal")
    }

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + offset)
    }

    fn order(id: &str, side: Side, quantity: &str, price: &str) -> Order {
        Order::new(
            OrderId::from_string(id),
            ObjectId::from_string("obj-ACME"),
            side,
            d(quantity),
            OrderType::Market,
            d(price),
            "prop-1",
            vec!["hyp-1".to_string()],
            "platform",
            at(0),
        )
    }

    fn pair_with_bound(max_leg_risk: &str) -> LegGroup {
        LegGroup::new(
            "grp-1",
            vec![
                order("ord-buy", Side::Buy, "100", "10"),
                order("ord-sell", Side::Sell, "100", "10"),
            ],
            at(60),
            d(max_leg_risk),
        )
        .expect("a two-leg group assembles")
    }

    fn pair() -> LegGroup {
        pair_with_bound("250")
    }

    /// Build a distinct fill each call, even for the same order.
    ///
    /// Several tests apply more than one print to a leg to exercise a volume-
    /// weighted average, and `record_fill` now refuses a fill id it has
    /// already seen — a fixture that derived the id from the order id alone
    /// would collide with itself on the second print and be refused as a
    /// redelivery, which is not the case under test.
    fn fill(order_id: &str, quantity: &str, price: &str) -> Fill {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Fill {
            fill_id: FillId::from_string(format!("fill-{order_id}-{sequence}")),
            order_id: OrderId::from_string(order_id),
            at: at(1),
            quantity: d(quantity),
            price: d(price),
            costs: Decimal::ZERO,
            venue: "SIM".to_string(),
            simulated: true,
        }
    }

    #[test]
    fn a_group_of_one_leg_is_refused_because_it_has_no_between() {
        let single = LegGroup::new(
            "grp-1",
            vec![order("a", Side::Buy, "1", "1")],
            at(60),
            dec!("1"),
        );
        let error = single.expect_err("a one-leg group was assembled");
        assert!(
            error.message().contains("has no between"),
            "the refusal does not say why one leg is not a group: {}",
            error.message()
        );
    }

    #[test]
    fn a_group_that_cannot_carry_any_exposure_is_refused_at_assembly() {
        // A bound of zero can never be satisfied once any leg fills, so the
        // group would unwind itself on its first print — an infinite loop of
        // entering and reversing, dressed as caution.
        let error = LegGroup::new(
            "grp-1",
            vec![
                order("a", Side::Buy, "1", "1"),
                order("b", Side::Sell, "1", "1"),
            ],
            at(60),
            Decimal::ZERO,
        )
        .expect_err("a zero risk bound was accepted");
        assert!(error.message().contains("unwind itself on its first print"));
    }

    #[test]
    fn a_balanced_group_carries_no_leg_risk_however_large_its_legs() {
        // The property that makes leg risk a cancellation rather than a sum. A
        // group with two thousand-unit legs on opposite sides is not twice as
        // risky as one with two hundred-unit legs; it is exactly as risky,
        // which is to say not at all.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        // The premise: one leg filled, so the group is genuinely exposed.
        assert_eq!(
            group.leg_risk(),
            dec!("1000"),
            "a lone filled leg carries its own notional"
        );

        group
            .record_fill(&fill("ord-sell", "100", "10"))
            .expect("sell");
        assert_eq!(
            group.leg_risk(),
            Decimal::ZERO,
            "two equal and opposite fills did not net to nothing"
        );
        assert!(group.is_filled());
        assert_eq!(group.assess(at(1)), Verdict::Complete);
    }

    #[test]
    fn a_half_filled_group_past_its_deadline_is_unwound_and_never_abandoned() {
        // The invariant. Abandoning here would leave a naked long in the book
        // with nothing recording that it was unintended — a position that
        // looks, to every downstream report, exactly like one somebody chose.
        //
        // The risk bound is deliberately generous. Written against the default
        // 250 bound, this test passed a mutation that made an exposed group
        // past its deadline abandon rather than unwind -- because 1000 of
        // unmatched notional trips the *risk* branch first, and the deadline
        // branch it claimed to be testing never ran. A test that names one
        // cause and exercises another guards the wrong thing.
        let mut group = pair_with_bound("5000");
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        // The premise: only the deadline can produce this verdict.
        assert!(
            group.leg_risk() <= group.max_leg_risk,
            "the risk bound is doing the work, not the deadline"
        );
        assert!(group.has_exposure(), "the premise failed: nothing filled");

        let verdict = group.assess(at(61));
        assert!(
            matches!(verdict, Verdict::Unwind { .. }),
            "a group holding 1000 of unmatched notional past its deadline returned {verdict:?}"
        );
        if let Verdict::Unwind { ref reason } = verdict {
            assert!(
                reason.contains("did not complete by its deadline"),
                "the unwind names the wrong cause: {reason}"
            );
        }

        // And abandoning it is refused outright, not merely not chosen.
        let refused = group.settle(
            &Verdict::Abandon {
                reason: "trying to walk away".to_string(),
            },
            &[],
            at(61),
        );
        assert!(
            refused
                .expect_err("a group with exposure was abandoned")
                .message()
                .contains("unwound, never abandoned")
        );
    }

    #[test]
    fn a_group_that_fills_everything_in_its_final_moment_completes_rather_than_unwinding() {
        // Ordering. Completion is checked before the deadline, so a group that
        // met its deadline is not punished for meeting it late.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        group
            .record_fill(&fill("ord-sell", "100", "10"))
            .expect("sell");
        assert_eq!(
            group.assess(at(3600)),
            Verdict::Complete,
            "a fully filled group past its deadline was not allowed to complete"
        );
    }

    #[test]
    fn a_group_over_its_risk_bound_unwinds_before_its_deadline() {
        // Leg risk is checked before the clock: an over-exposed group is
        // already the failure, and waiting for the deadline holds the exposure
        // for the rest of the window.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        // The premise: the deadline has not passed, so only the risk bound can
        // be producing this verdict.
        assert!(at(1) < group.deadline);
        match group.assess(at(1)) {
            Verdict::Unwind { reason } => assert!(
                reason.contains("exceeds"),
                "the unwind names the wrong cause: {reason}"
            ),
            other => panic!("1000 of unmatched notional against a 250 bound returned {other:?}"),
        }
    }

    #[test]
    fn an_empty_group_past_its_deadline_is_abandoned_and_not_unwound() {
        // The other safe end, and it must stay distinguishable: one cost two
        // round trips and moved the book, the other cost nothing.
        let group = pair();
        assert!(
            !group.has_exposure(),
            "the premise failed: something filled"
        );
        assert!(matches!(group.assess(at(61)), Verdict::Abandon { .. }));
    }

    #[test]
    fn unwinding_reverses_the_filled_quantity_and_not_the_ordered_quantity() {
        // Reversing the order size would close a position the group never took
        // and open the mirror of it — turning a recovery into a new naked
        // trade, in the direction opposite the one being closed.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "30", "10"))
            .expect("partial");

        let unwinds = group.unwind_orders(at(61));
        assert_eq!(unwinds.len(), 1, "only the filled leg needs reversing");
        assert_eq!(
            unwinds[0].quantity,
            dec!("30"),
            "the unwind used the ordered size"
        );
        assert_eq!(
            unwinds[0].side,
            Side::Sell,
            "the unwind did not reverse the side"
        );
    }

    #[test]
    fn unwinding_prices_the_reversal_at_what_was_actually_paid() {
        // A leg filled in several prints at several prices has an average none
        // of them alone produces, and the reversal is measured against it.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "50", "10"))
            .expect("first");
        group
            .record_fill(&fill("ord-buy", "50", "20"))
            .expect("second");

        let unwinds = group.unwind_orders(at(61));
        assert_eq!(unwinds[0].quantity, dec!("100"));
        assert_eq!(
            unwinds[0].arrival_price,
            dec!("15"),
            "the reversal is not priced at the average actually paid"
        );
    }

    #[test]
    fn the_same_group_produces_the_same_unwind_ids_twice() {
        // Idempotency. A second call producing a second set of ids would double
        // the reversal into a position opposite the one being closed.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");

        let first: Vec<String> = group
            .unwind_orders(at(61))
            .iter()
            .map(|o| o.order_id.as_str().to_string())
            .collect();
        let second: Vec<String> = group
            .unwind_orders(at(99))
            .iter()
            .map(|o| o.order_id.as_str().to_string())
            .collect();
        assert!(!first.is_empty(), "the premise failed: nothing to reverse");
        assert_eq!(
            first, second,
            "two calls produced different reversing orders"
        );
    }

    #[test]
    fn a_redelivered_fill_does_not_double_the_legs_notional() {
        // A retried report carrying the same fill id a second time must not
        // double the leg's filled notional -- that number is what leg_risk
        // is trusted to bound, and a phantom double-fill would report a
        // balanced group as exposed, or an exposed one as balanced, purely
        // from a redelivery nothing traded caused.
        let mut group = pair();
        let one = fill("ord-buy", "40", "10");
        group.record_fill(&one).expect("first application");
        assert_eq!(
            group.leg_risk(),
            dec!("400"),
            "the premise: one genuine fill left the group exposed"
        );

        let error = group
            .record_fill(&one)
            .expect_err("the same fill id was applied twice without complaint");
        assert!(
            error.message().contains("already applied"),
            "the refusal does not name the duplicate: {}",
            error.message()
        );
        assert_eq!(
            group.leg_risk(),
            dec!("400"),
            "a redelivered fill doubled the leg's notional"
        );
    }

    #[test]
    fn a_fill_for_an_order_the_group_does_not_hold_is_refused() {
        // Applying it to whatever leg sat at that index would put someone
        // else's fill into this group's risk arithmetic.
        let mut group = pair();
        let error = group
            .record_fill(&fill("ord-elsewhere", "100", "10"))
            .expect_err("a foreign fill was accepted");
        assert!(error.message().contains("is not a leg of group"));
    }

    #[test]
    fn a_group_is_not_unwound_until_every_filled_leg_has_a_reversing_order() {
        // Settling with fewer reversals than filled legs would mark the group
        // safe while a position was still open — the exact state the group
        // exists to make impossible.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        group
            .record_fill(&fill("ord-sell", "40", "10"))
            .expect("partial sell");

        let verdict = Verdict::Unwind {
            reason: "test".to_string(),
        };
        let error = group
            .settle(&verdict, &[], at(61))
            .expect_err("a group was unwound with no reversing orders");
        assert!(
            error
                .message()
                .contains("is not unwound until every filled leg has one"),
            "unexpected refusal: {}",
            error.message()
        );

        // With the full set, it settles.
        let issued = group.unwind_orders(at(61));
        assert_eq!(issued.len(), 2, "both legs filled and both need reversing");
        group.settle(&verdict, &issued, at(61)).expect("settles");
        assert!(matches!(group.state, GroupState::Unwound { .. }));
        assert_eq!(group.unwinds().len(), 2);
    }

    #[test]
    fn a_settled_group_cannot_be_settled_again() {
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        group
            .record_fill(&fill("ord-sell", "100", "10"))
            .expect("sell");
        group
            .settle(&Verdict::Complete, &[], at(2))
            .expect("completes");

        let error = group
            .settle(&Verdict::Complete, &[], at(3))
            .expect_err("a terminal group was settled twice");
        assert!(error.message().contains("cannot be settled again"));
    }

    #[test]
    fn completing_a_group_that_has_not_filled_everything_is_refused() {
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        let error = group
            .settle(&Verdict::Complete, &[], at(2))
            .expect_err("an unfilled group was completed");
        assert!(error.message().contains("unmatched notional"));
    }

    #[test]
    fn beginning_an_unwind_is_recorded_before_the_reversing_orders_go_out() {
        // A crash between the decision and the orders must leave a group
        // visibly Unwinding with no unwinds recorded — the state a recovery can
        // find — rather than one still marked Working, which reads as fine.
        let mut group = pair();
        group
            .record_fill(&fill("ord-buy", "100", "10"))
            .expect("buy");
        assert_eq!(group.state, GroupState::Working);

        group
            .begin_unwind("deadline passed", at(61))
            .expect("begins");
        assert!(matches!(group.state, GroupState::Unwinding { .. }));
        assert!(
            group.unwinds().is_empty(),
            "unwinds were recorded before any order was issued"
        );
        assert!(
            !group.state.is_terminal(),
            "an unwinding group is not finished; its reversals have not been placed"
        );
    }
}
