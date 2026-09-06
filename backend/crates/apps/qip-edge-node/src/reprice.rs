//! Dynamic repricing on the node's venue seam: the caller
//! `qip_routing::reprice` was written for.
//!
//! That module decides that a resting limit child is stale and mints its
//! replacement — and sends nothing, on purpose. Its header names this loop
//! as the caller: once per book update, *after* the gateway's events have
//! been drained into the parent, so that a fill which arrived this pass is
//! booked before staleness is judged and the replacement carries the
//! remainder rather than a quantity that no longer exists. This module is
//! that caller, and nothing more: no quoting strategy, no fair value, no
//! inventory skew. A resting order the cell already sent gets withdrawn when
//! the touch moves past the declared threshold and its remainder re-sent at
//! the touch, once the venue has acknowledged the withdrawal.
//!
//! # Where it sits, and why beneath the cell
//!
//! The cell holds an intention as one [`OpenOrder`] under one id, and has no
//! path of its own to cancel a particular order or to place a replacement
//! for one; the only cancel it knows is the time-to-live withdrawal, and the
//! only placement is a strategy firing. The cell's record must also remain
//! *the* record — a replacement placed beside it would be an order the cell
//! never sent, which the reconciler rightly calls a break. So the wiring
//! sits beneath the cell's [`Placer`] seam. [`RequotingPlacer`] wraps the
//! simulated gateway; a repriced intention is a [`ParentOrder`] whose
//! children are the venue-level orders. The venue sees the cell's own id for
//! the original order and a fresh id for each replacement — the simulated
//! exchange refuses a reused client id outright, and `reprice.rs` explains
//! why a real venue would dedupe the replacement away as a retry — and the
//! wrapper maps the fresh ids back to the cell's id on every channel the
//! cell reads: execution reports, cancels, and the drop copy. The cell keeps
//! one id per intention; the venue's account and the cell's record still
//! describe the same fill.
//!
//! # One intention, one live order
//!
//! Three things hold it. The repricer refuses to mint a replacement until
//! the child is terminal; the parent's conservation arithmetic refuses to
//! attach a replacement whose quantity is still working; and this loop is
//! synchronous — the venue's cancel acknowledgement is in hand before the
//! replacement is constructed, on the same pass. The venue's own order
//! record is the witness a test asks, through
//! [`SimulatedGateway::venue_holds_open`].
//!
//! # What is refused rather than guessed
//!
//! * A cancel the venue refuses leaves the order exactly as it was and is
//!   reported as [`Requote::CancelRefused`]. Nothing new is sent.
//! * A cancel whose acknowledged remainder disagrees with what the drain
//!   booked means a fill the order-entry channel has not reported. The child
//!   is closed as the venue says, no replacement is minted, and the
//!   disagreement is reported as [`Requote::CancelDisagreed`]; the drop copy
//!   will surface the fill and the reconciler will halt the cell, which is
//!   the correct answer to a venue whose channels disagree.
//! * A replacement the venue rejects is reported as
//!   [`Requote::ReplacementRefused`] and its quantity released back to the
//!   parent. The intention is then held by the cell with nothing at the
//!   venue until its time to live; this loop does not retry, because a
//!   retry is a sending decision and this module makes none.
//! * A halted cell reprices nothing. `run_pass` reaches this module only
//!   past its halt check, so a halted node's requote count is flat by the
//!   same path that keeps its order count flat.

use crate::gateway::SimulatedGateway;
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_edge::cell::{Cell, ExecutionReport, OpenOrder, Placer};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::telemetry::CellMetrics;
use qip_routing::children::{ChildOrder, ParentOrder};
use qip_routing::ordertype::{RoutedOrderType, Touch};
use qip_routing::reprice::{RepriceDecision, RepricePolicy, Repricer, ThrottleScope};
use std::collections::BTreeMap;

/// The requote policy, as the deployment declares it:
/// `<tick>:<max drift in ticks>:<max drift in basis points>`.
pub const REPRICE_VARIABLE: &str = "QIP_REPRICE";

/// Read the requote policy, refusing anything but the stated form.
///
/// `None` is unset or blank: resting orders stay where they were sent until
/// their time to live, and the node says so in its production requirements.
/// The three numbers are the policy's own — the instrument's price
/// increment and the two drift thresholds — and the requote budgets keep
/// `RepricePolicy::new`'s conservative defaults. Every value is validated by
/// the policy itself, so a zero threshold or a zero tick is refused here at
/// start-up with the repricer's own reason rather than at the first pass.
pub fn parse_reprice(value: Option<&str>) -> Result<Option<RepricePolicy>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let form = || {
        Error::invalid(format!(
            "configuration: {REPRICE_VARIABLE}={value} is not `<tick>:<ticks>:<bps>`; write \
             `0.01:5:50` for a one-cent tick, stale at five ticks or fifty basis points behind \
             the touch, whichever binds first"
        ))
    };
    let mut parts = value.split(':');
    let (Some(tick), Some(ticks), Some(bps), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(form());
    };
    let tick = Decimal::parse(tick.trim()).ok_or_else(form)?;
    let ticks = ticks.trim().parse::<u32>().map_err(|_| form())?;
    let bps = bps.trim().parse::<f64>().map_err(|_| form())?;
    let policy = RepricePolicy::new(tick, ticks, bps);
    policy.validate().map_err(|error| {
        Error::invalid(format!(
            "configuration: {REPRICE_VARIABLE}={value} is refused: {}",
            error.message()
        ))
    })?;
    Ok(Some(policy))
}

/// What the requoter did to one intention on one pass. Holds are not
/// reported — an order left resting is the ordinary case — and everything
/// else is, because each of these is a sentence somebody has to be able to
/// finish about an order that is no longer where the cell sent it.
#[derive(Clone, Debug, PartialEq)]
pub enum Requote {
    /// The stale order was withdrawn and its remainder re-sent at the touch.
    Replaced {
        order_id: String,
        withdrawn: String,
        replacement: String,
        quantity: Decimal,
        price: Decimal,
    },
    /// The order filled entirely while its cancel was acknowledged, so
    /// there was no remainder to re-send.
    Completed { order_id: String, withdrawn: String },
    /// Stale, and the budget is spent. The named decision, not a skip.
    Throttled {
        order_id: String,
        scope: ThrottleScope,
        used: u32,
        budget: u32,
    },
    /// The venue refused the cancel; the order is exactly as it was.
    CancelRefused { order_id: String, reason: String },
    /// The venue's acknowledged remainder is not what the drain booked. No
    /// replacement was sent; the reconciler will find the fill.
    CancelDisagreed {
        order_id: String,
        booked: Decimal,
        acknowledged: Decimal,
    },
    /// The cancel was acknowledged and the replacement could not be sent.
    ReplacementRefused { order_id: String, reason: String },
    /// The cell's record could not be modelled as a parent order, so the
    /// order was left alone rather than repriced from a guess.
    Unmodelled { order_id: String, reason: String },
}

impl Requote {
    pub fn order_id(&self) -> &str {
        match self {
            Self::Replaced { order_id, .. }
            | Self::Completed { order_id, .. }
            | Self::Throttled { order_id, .. }
            | Self::CancelRefused { order_id, .. }
            | Self::CancelDisagreed { order_id, .. }
            | Self::ReplacementRefused { order_id, .. }
            | Self::Unmodelled { order_id, .. } => order_id,
        }
    }

    /// One line for the log.
    pub fn describe(&self) -> String {
        match self {
            Self::Replaced {
                order_id,
                withdrawn,
                replacement,
                quantity,
                price,
            } => format!(
                "order {order_id}: withdrew {withdrawn} and re-sent {quantity} at {price} as \
                 {replacement}"
            ),
            Self::Completed {
                order_id,
                withdrawn,
            } => format!(
                "order {order_id}: {withdrawn} filled entirely as its cancel was acknowledged; \
                 nothing to re-send"
            ),
            Self::Throttled {
                order_id,
                scope,
                used,
                budget,
            } => format!(
                "order {order_id}: stale, and the {} requote budget is spent ({used} of \
                 {budget}); it rests where it is",
                scope.as_str()
            ),
            Self::CancelRefused { order_id, reason } => {
                format!("order {order_id}: the venue refused the cancel and it stands: {reason}")
            }
            Self::CancelDisagreed {
                order_id,
                booked,
                acknowledged,
            } => format!(
                "order {order_id}: the cell had {booked} unfilled and the venue withdrew \
                 {acknowledged}; no replacement was sent and the drop copy will say what traded"
            ),
            Self::ReplacementRefused { order_id, reason } => format!(
                "order {order_id}: withdrawn, and the replacement was refused, so nothing rests \
                 for it until its time to live: {reason}"
            ),
            Self::Unmodelled { order_id, reason } => {
                format!("order {order_id}: left alone; its record could not be modelled: {reason}")
            }
        }
    }
}

/// A repriced intention: the cell's order as a parent, its venue-level
/// orders as children, and which child is at the venue now.
#[derive(Debug)]
struct Tracked {
    parent: ParentOrder,
    /// The child the venue holds open, or `None` once a replacement was
    /// refused — nothing rests for the intention until the cell withdraws it.
    live: Option<String>,
}

/// The requoter the pass loop holds: the repricer, the intentions it has
/// modelled, and the id map that keeps the cell's record whole.
#[derive(Debug)]
pub struct Requoter {
    repricer: Repricer,
    metrics: CellMetrics,
    /// Intentions modelled as parents, keyed by the cell's order id. Pruned
    /// to the cell's open orders on every pass, so it is bounded by the
    /// cell's own open-order cap.
    tracked: BTreeMap<String, Tracked>,
    /// Replacement id at the venue to the cell's order id. Only replacements
    /// appear here: an original order carries the cell's id at the venue.
    by_child: BTreeMap<String, String>,
}

impl Requoter {
    /// A requoter under `policy`, recording into the cell's registry.
    pub fn new(policy: RepricePolicy, metrics: CellMetrics) -> Result<Self> {
        Ok(Self {
            repricer: Repricer::new(policy)?,
            metrics,
            tracked: BTreeMap::new(),
            by_child: BTreeMap::new(),
        })
    }

    pub fn policy(&self) -> &RepricePolicy {
        self.repricer.policy()
    }

    /// The cell's order id a venue-level id belongs to, and the child it
    /// names, when this requoter has modelled it.
    fn locate(&self, venue_order_id: &str) -> Option<(String, String)> {
        if let Some(parent) = self.by_child.get(venue_order_id) {
            return Some((parent.clone(), venue_order_id.to_string()));
        }
        self.tracked
            .contains_key(venue_order_id)
            .then(|| (venue_order_id.to_string(), venue_order_id.to_string()))
    }

    /// Book a venue report into the parent and rename it to the cell's id.
    ///
    /// The report reaches the cell whatever the child's arithmetic says: a
    /// fill the venue reports is a fill, and the cell's own over-fill check
    /// is the one that breaks on it. What is refused here is only the
    /// parent's bookkeeping, and the report says so by its id alone.
    fn book(&mut self, report: &mut ExecutionReport) {
        let Some((order_id, child_id)) = self.locate(&report.order_id) else {
            return;
        };
        if let Some(child) = self
            .tracked
            .get_mut(&order_id)
            .and_then(|tracked| tracked.parent.child_mut(&child_id))
        {
            // A child already closed — cancelled this pass, filled past its
            // size — refuses the fill; the cell still books it, and the
            // disagreement is the cell's to find.
            let _ = child.apply_fill(report.quantity, report.price);
        }
        report.order_id = order_id;
    }

    /// The cell withdraws an intention by its own id; the venue is asked to
    /// withdraw whichever child it holds.
    fn cancel(
        &mut self,
        venue: &mut SimulatedGateway,
        order_id: &str,
        object_id: &ObjectId,
        venue_id: &VenueId,
        at: Timestamp,
    ) -> Result<Decimal> {
        let Some(tracked) = self.tracked.get_mut(order_id) else {
            return venue.cancel(order_id, object_id, venue_id, at);
        };
        let Some(live) = tracked.live.clone() else {
            // Nothing rests: the replacement was refused and reported when
            // it happened. What the cell has unfilled is what was never
            // re-sent, and that is what "withdrawn" honestly means here.
            return Ok(tracked.parent.outstanding());
        };
        let remaining = venue.cancel(&live, object_id, venue_id, at)?;
        if let Some(child) = tracked.parent.child_mut(&live) {
            let _ = child.cancel("withdrawn by the cell");
        }
        tracked.live = None;
        Ok(remaining)
    }

    /// Model an open order the cell holds as a parent with one working
    /// child at the venue, seeded with what the cell has already booked.
    fn model(order: &OpenOrder) -> Result<Tracked> {
        let mut parent = ParentOrder::new(
            OrderId::from_string(&order.order_id),
            order.object_id.clone(),
            order.side,
            order.quantity,
        )?;
        let mut child = ChildOrder::new(
            order.order_id.clone(),
            parent.order_id.clone(),
            order.venue.clone(),
            order.object_id.clone(),
            order.side,
            order.quantity,
            RoutedOrderType::Limit { price: order.price },
        )?;
        child.mark_working()?;
        if order.filled.is_positive() {
            // The cell's record carries the total and not the prices; the
            // child's consideration is never read here, so the sent price
            // stands in for it.
            child.apply_fill(order.filled, order.price)?;
        }
        parent.attach(child)?;
        Ok(Tracked {
            parent,
            live: Some(order.order_id.clone()),
        })
    }

    /// Consider every resting order the cell holds against the book it
    /// holds, once. Call after the gateway's events have been drained into
    /// the cell and never on a halted cell — see the module documentation.
    pub fn reprice(
        &mut self,
        cell: &Cell,
        venue: &mut SimulatedGateway,
        now: Timestamp,
    ) -> Vec<Requote> {
        let open = cell.open_orders();
        // Bounded by the cell: an intention the cell has settled is gone
        // from here on the same pass, with the replacement ids it minted.
        self.tracked
            .retain(|order_id, _| open.iter().any(|order| &order.order_id == order_id));
        self.by_child
            .retain(|_, order_id| self.tracked.contains_key(order_id));

        let mut requotes = Vec::new();
        for order in open
            .iter()
            .filter(|order| order.closed.is_none() && order.expires_at.is_some())
        {
            if !self.tracked.contains_key(&order.order_id) {
                match Self::model(order) {
                    Ok(tracked) => {
                        self.tracked.insert(order.order_id.clone(), tracked);
                    }
                    Err(error) => {
                        requotes.push(Requote::Unmodelled {
                            order_id: order.order_id.clone(),
                            reason: error.message().to_string(),
                        });
                        continue;
                    }
                }
            }
            let Some(tracked) = self.tracked.get_mut(&order.order_id) else {
                continue;
            };
            let Some(live) = tracked.live.clone() else {
                continue;
            };
            if tracked.parent.filled() != order.filled {
                // The parent's arithmetic and the cell's record disagree
                // about the same intention. Repricing from either would be
                // a guess at the remainder; the order stays where it is.
                requotes.push(Requote::Unmodelled {
                    order_id: order.order_id.clone(),
                    reason: format!(
                        "the cell has {} filled and the parent's children account for {}",
                        order.filled,
                        tracked.parent.filled()
                    ),
                });
                continue;
            }
            let Some(touch) = cell
                .liquidity()
                .get(&order.venue, &order.object_id)
                .and_then(|state| {
                    Some(Touch {
                        bid: state.best_bid()?.price,
                        ask: state.best_ask()?.price,
                    })
                })
            else {
                // A one-sided or unusable book prices nothing; the order
                // rests until the book can say where the touch is.
                continue;
            };
            let Some(child) = tracked.parent.child(&live).cloned() else {
                continue;
            };
            match self.repricer.consider(&child, touch, now) {
                RepriceDecision::Hold { .. } => {}
                RepriceDecision::Throttled {
                    scope,
                    used,
                    budget,
                    ..
                } => requotes.push(Requote::Throttled {
                    order_id: order.order_id.clone(),
                    scope,
                    used,
                    budget,
                }),
                RepriceDecision::CancelAndReplace { client_id, .. } => {
                    let requote = Self::replace(
                        &mut self.repricer,
                        &mut self.by_child,
                        &self.metrics,
                        tracked,
                        venue,
                        order,
                        &client_id,
                        touch,
                        now,
                    );
                    requotes.push(requote);
                }
            }
        }
        requotes
    }

    /// Carry one cancel-and-replace instruction to the venue: cancel, apply
    /// the acknowledgement to the child, and only then mint and send the
    /// replacement.
    #[allow(clippy::too_many_arguments)]
    fn replace(
        repricer: &mut Repricer,
        by_child: &mut BTreeMap<String, String>,
        metrics: &CellMetrics,
        tracked: &mut Tracked,
        venue: &mut SimulatedGateway,
        order: &OpenOrder,
        client_id: &str,
        touch: Touch,
        now: Timestamp,
    ) -> Requote {
        let order_id = order.order_id.clone();
        let acknowledged = match venue.cancel(client_id, &order.object_id, &order.venue, now) {
            Ok(remaining) => remaining,
            Err(error) => {
                repricer.abandon(client_id);
                return Requote::CancelRefused {
                    order_id,
                    reason: error.message().to_string(),
                };
            }
        };
        let booked = tracked
            .parent
            .child(client_id)
            .map_or(Decimal::ZERO, ChildOrder::remaining);
        // The venue has said its final word on the child whatever the
        // number was; nothing more will be reported on it.
        if let Some(child) = tracked.parent.child_mut(client_id) {
            let _ = child.cancel("repriced");
        }
        tracked.live = None;
        if acknowledged != booked {
            repricer.abandon(client_id);
            return Requote::CancelDisagreed {
                order_id,
                booked,
                acknowledged,
            };
        }
        let replacement =
            match repricer.on_cancel_acknowledged(&mut tracked.parent, client_id, touch) {
                Ok(Some(replacement)) => replacement,
                Ok(None) => {
                    return Requote::Completed {
                        order_id,
                        withdrawn: client_id.to_string(),
                    };
                }
                Err(error) => {
                    return Requote::ReplacementRefused {
                        order_id,
                        reason: error.message().to_string(),
                    };
                }
            };
        let RoutedOrderType::Limit { price } = replacement.order_type else {
            // The repricer mints limit children and nothing else; a venue
            // order of any other type is not one this loop knows how to send.
            release(tracked, &replacement.client_id, "not a limit order");
            return Requote::ReplacementRefused {
                order_id,
                reason: format!(
                    "the replacement {} is a {} order and only a limit rests",
                    replacement.client_id,
                    replacement.order_type.kind().as_str()
                ),
            };
        };
        let side: BookSide = replacement.side;
        if let Err(error) = venue.place(
            &replacement.client_id,
            &order.object_id,
            &order.venue,
            side,
            replacement.quantity,
            price,
            now,
        ) {
            release(tracked, &replacement.client_id, error.message());
            return Requote::ReplacementRefused {
                order_id,
                reason: error.message().to_string(),
            };
        }
        if let Some(child) = tracked.parent.child_mut(&replacement.client_id) {
            let _ = child.mark_working();
        }
        by_child.insert(replacement.client_id.clone(), order_id.clone());
        tracked.live = Some(replacement.client_id.clone());
        metrics.order_repriced(&order.venue);
        Requote::Replaced {
            order_id,
            withdrawn: client_id.to_string(),
            replacement: replacement.client_id,
            quantity: replacement.quantity,
            price,
        }
    }
}

/// A replacement that never reached the venue gives its quantity back to
/// the parent, so the intention's arithmetic still accounts for every share.
fn release(tracked: &mut Tracked, client_id: &str, reason: &str) {
    if let Some(child) = tracked.parent.child_mut(client_id) {
        let _ = child.reject(reason);
    }
}

/// The simulated gateway as the cell sees it through the requoter.
///
/// Every call the cell makes passes through here so that a replacement's
/// fresh venue id is mapped back to the cell's own on every channel. The
/// pass loop builds one per pass over the gateway and the requoter it
/// holds; with no requoter configured it forwards everything unchanged,
/// and no replacement id can exist for it to map.
#[derive(Debug)]
pub struct RequotingPlacer<'a> {
    venue: &'a mut SimulatedGateway,
    requoter: Option<&'a mut Requoter>,
}

impl<'a> RequotingPlacer<'a> {
    pub fn new(venue: &'a mut SimulatedGateway, requoter: Option<&'a mut Requoter>) -> Self {
        Self { venue, requoter }
    }

    /// The venue's clearing-ledger fills, with replacement ids mapped to
    /// the cell's — the drop copy must name the order the cell knows, or
    /// every fill on a replacement reconciles as a fill on an order the cell
    /// never sent.
    pub fn drain_drop_copies(&mut self) -> Vec<DropCopyFill> {
        let mut fills = self.venue.drain_drop_copies();
        if let Some(requoter) = self.requoter.as_deref() {
            for fill in &mut fills {
                if let Some((order_id, _)) = requoter.locate(&fill.order_id) {
                    fill.order_id = order_id;
                }
            }
        }
        fills
    }

    /// One round of repricing against the cell's books. Empty with no
    /// requoter.
    pub fn reprice(&mut self, cell: &Cell, now: Timestamp) -> Vec<Requote> {
        match self.requoter.as_deref_mut() {
            Some(requoter) => requoter.reprice(cell, &mut *self.venue, now),
            None => Vec::new(),
        }
    }
}

impl Placer for RequotingPlacer<'_> {
    fn is_simulated(&self) -> bool {
        self.venue.is_simulated()
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        // The cell's own id goes to the venue unchanged; only a replacement
        // ever carries a different one.
        self.venue
            .place(order_id, object_id, venue, side, quantity, price, at)
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        let mut reports = self.venue.execution_reports();
        if let Some(requoter) = self.requoter.as_deref_mut() {
            for report in &mut reports {
                requoter.book(report);
            }
        }
        reports
    }

    fn can_cancel(&self) -> bool {
        self.venue.can_cancel()
    }

    fn cancel(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        at: Timestamp,
    ) -> Result<Decimal> {
        match self.requoter.as_deref_mut() {
            Some(requoter) => requoter.cancel(&mut *self.venue, order_id, object_id, venue, at),
            None => self.venue.cancel(order_id, object_id, venue, at),
        }
    }

    fn required_configuration(&self) -> Vec<String> {
        self.venue.required_configuration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requote_policy_is_read_in_one_form_and_refused_in_every_other() {
        // Unset and blank are a node that never requotes, announced. The
        // stated form is accepted with the repricer's own validation behind
        // it, so `0.01:0:50` — a zero-tick threshold, which is chasing — is
        // refused at start-up with the repricer's reason rather than at the
        // first pass. Every other shape is refused naming the form.
        assert_eq!(parse_reprice(None).expect("unset is allowed"), None);
        assert_eq!(parse_reprice(Some("  ")).expect("blank is unset"), None);
        let policy = parse_reprice(Some("0.01:5:50"))
            .expect("the stated form is accepted")
            .expect("a value was given");
        assert_eq!(policy.max_drift_ticks, 5);
        for value in [
            "0.01",
            "0.01:5",
            "0.01:5:50:1",
            "cent:5:50",
            "0.01:five:50",
            "0.01:5:wide",
        ] {
            let error = match parse_reprice(Some(value)) {
                Ok(policy) => panic!("{REPRICE_VARIABLE}={value} was accepted as {policy:?}"),
                Err(error) => error,
            };
            assert!(
                error.message().starts_with("configuration:"),
                "{value}: {}",
                error.message()
            );
            assert!(
                error.message().contains("<tick>:<ticks>:<bps>"),
                "the refusal of {value} does not name the form: {}",
                error.message()
            );
        }
        let chasing = match parse_reprice(Some("0.01:0:50")) {
            Ok(policy) => panic!("a zero-tick threshold was accepted as {policy:?}"),
            Err(error) => error,
        };
        assert!(
            chasing.message().contains("chasing"),
            "the refusal does not carry the repricer's own reason: {}",
            chasing.message()
        );
    }
}
