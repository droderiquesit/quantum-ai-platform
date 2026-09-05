//! The order management system.
//!
//! Every order goes through [`OrderManager::submit`], and that method is where
//! the platform's safety controls converge. In order:
//!
//! 1. The order must be well formed and trace to a proposal and a hypothesis.
//! 2. If the instrument has a [`crate::feasibility::VenueFeasibility`]
//!    installed through [`OrderManager::with_instrument_feasibility`], or
//!    failing that the destination venue has one installed through
//!    [`OrderManager::with_venue_feasibility`], the order
//!    must sit on its lot and tick grids and clear its minimums. This is
//!    `qip-edge`'s feasibility gate mirrored onto the central path: an
//!    off-lot or below-minimum order is a strategy that does not know the
//!    venue's grid, and it is refused here rather than allowed to ride a
//!    profitable strategy's order through pre-trade risk and out to a venue
//!    that would reject it, or silently trade a size nobody reasoned about.
//!    It reports through [`RefusalReason::Malformed`] rather than a variant
//!    of its own, naming the gate in the detail text: `RefusalReason` is
//!    matched exhaustively in `qip-kernel`, outside this crate's own scope,
//!    and a fourth field there is the same shape of defect a guessed grid
//!    would be — a fact invented at the point of use instead of asked for
//!    where it is owned. An infeasible order *is* malformed for the venue it
//!    named, which is the same reading the edge crate gives its own vetoes.
//! 3. The kill switch must not be tripped for its scope.
//! 4. The autonomy level must permit execution at all.
//! 5. A live venue additionally requires a live autonomy level *and* an
//!    available venue. Neither implies the other.
//! 6. Pre-trade risk must approve it, against the state it would produce.
//!
//! Every one of those is a refusal path, and each records why. A rejected
//! order that left no trace is indistinguishable from one that was never sent,
//! and the difference matters when reconstructing why a position was not put on.
//!
//! The ordering is deliberate: feasibility runs right after well-formedness
//! because it is the cheapest question with a venue-specific answer — the
//! same reasoning §18.1 gives for putting the edge crate's feasibility gate
//! ahead of its profitability filter — the kill switch is checked before the
//! autonomy level so that a stopped platform stays stopped even if the level
//! is misconfigured, and risk runs last so that its expensive projection is
//! not computed for an order that was never going to be sent.

use crate::broker::Broker;
use crate::feasibility::{self, VenueFeasibility};
use crate::order::{Fill, Order, OrderState, OrderType};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::time::Timestamp;
use qip_risk::limits::RiskState;
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_risk_engine::pretrade::{PreTradeChecker, PreTradeDecision, ProposedOrder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Why an order was refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RefusalReason {
    /// The order was not well formed — including, since `qip-edge`'s
    /// feasibility gate was mirrored onto this path, an order that cannot be
    /// expressed at the venue: off its lot or tick grid, or below a minimum
    /// it states. `detail` is prefixed `infeasible (<gate>):` in that case,
    /// naming the same `feasibility_*` gate literal the edge crate uses.
    Malformed { detail: String },
    /// The kill switch is tripped.
    Halted { scope: String, detail: String },
    /// The autonomy level does not permit execution.
    AutonomyTooLow {
        level: AutonomyLevel,
        required: AutonomyLevel,
    },
    /// A live venue was selected below a live autonomy level.
    LiveVenueBelowLiveAutonomy { level: AutonomyLevel, venue: String },
    /// The venue cannot be reached.
    VenueUnavailable { venue: String, detail: String },
    /// Pre-trade risk refused it.
    RiskRejected { reasons: Vec<String> },
    /// The venue refused it.
    VenueRejected { detail: String },
}

impl RefusalReason {
    pub fn describe(&self) -> String {
        match self {
            Self::Malformed { detail } => format!("malformed: {detail}"),
            Self::Halted { scope, detail } => {
                format!("trading is halted for {scope}: {detail}")
            }
            Self::AutonomyTooLow { level, required } => {
                format!("the autonomy level is {level}, and execution needs at least {required}")
            }
            Self::LiveVenueBelowLiveAutonomy { level, venue } => format!(
                "{venue} is a live venue and the autonomy level is {level}; live trading is disabled"
            ),
            Self::VenueUnavailable { venue, detail } => {
                format!("{venue} is unavailable: {detail}")
            }
            Self::RiskRejected { reasons } => format!("risk refused: {}", reasons.join("; ")),
            Self::VenueRejected { detail } => format!("the venue refused: {detail}"),
        }
    }

    /// Whether the refusal is a safety control rather than a transient fault.
    ///
    /// Distinguished because a safety refusal must never be retried
    /// automatically, and a transient one may be. `Malformed` — including a
    /// feasibility veto — is not a safety control by the same reasoning it
    /// already carried: the order itself needs to change, not the platform's
    /// posture, so there is nothing here for an automatic retry to trip over.
    pub const fn is_safety_control(&self) -> bool {
        matches!(
            self,
            Self::Halted { .. }
                | Self::AutonomyTooLow { .. }
                | Self::LiveVenueBelowLiveAutonomy { .. }
                | Self::RiskRejected { .. }
        )
    }

    /// The `feasibility_*` gate literal this refusal names, if it is a
    /// feasibility veto rather than any other malformation.
    ///
    /// Read back from the prefix [`OrderManager::submit`] wrote, and matched
    /// against the four literals exactly — `infeasible (<gate>):` with both
    /// delimiters — so that a counter keyed on the answer is bounded by the
    /// gate constants and cannot be minted by a reworded reason. It exists
    /// because the kernel counts refusals by the control that made them, and
    /// until it did an off-lot order and an order tracing to no hypothesis
    /// were the same bar on the same chart: `order-validation`. The edge plane
    /// charts its own feasibility vetoes under the gate literal, and a
    /// central refusal that could not be correlated with it was a control
    /// whose firing rate nobody could read.
    pub fn feasibility_gate(&self) -> Option<&'static str> {
        let Self::Malformed { detail } = self else {
            return None;
        };
        [
            feasibility::GATE_MINIMUM_QUANTITY,
            feasibility::GATE_MINIMUM_NOTIONAL,
            feasibility::GATE_LOT,
            feasibility::GATE_TICK,
        ]
        .into_iter()
        .find(|gate| detail.starts_with(&format!("infeasible ({gate}):")))
    }
}

/// What happened to a submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmissionResult {
    pub order_id: OrderId,
    pub at: Timestamp,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refusal: Option<RefusalReason>,
    pub fills: Vec<Fill>,
    /// The venue it went to, if it went anywhere.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub venue: Option<String>,
    /// Whether the fills came from a simulated venue.
    pub simulated: bool,
    /// If the order was resized by risk, what it was resized to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reduced_to: Option<Decimal>,
    /// Where the venue and the order disagreed about what happened.
    ///
    /// A venue that reports a fill the order refuses — an over-fill, a fill on
    /// a closed order — puts the book out of step with reality. Discarding
    /// that refusal is how the divergence becomes invisible, so it is carried
    /// here and counted on the manager instead.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub reconciliation_breaks: Vec<String>,
}

impl SubmissionResult {
    pub fn filled_quantity(&self) -> Decimal {
        self.fills
            .iter()
            .map(|fill| fill.quantity)
            .fold(Decimal::ZERO, |a, b| a + b)
    }
}

/// Manages the order lifecycle.
#[derive(Debug)]
pub struct OrderManager {
    orders: BTreeMap<String, Order>,
    checker: PreTradeChecker,
    /// Submissions that were refused, kept so a missing position can be
    /// explained.
    refusals: Vec<SubmissionResult>,
    /// Every venue/book disagreement seen, so a monitor can halt on one.
    reconciliation_breaks: Vec<String>,
    /// Feasibility grids by venue name, keyed on [`Broker::name`]. A venue
    /// with no entry is checked for nothing here — see
    /// `crate::feasibility`'s module comment for why that is the honest
    /// answer rather than a gap.
    feasibility: BTreeMap<String, VenueFeasibility>,
    /// Feasibility grids by instrument, keyed on the order's `object_id`, and
    /// consulted ahead of the venue's. A lot and a tick are facts about the
    /// instrument's listing, which is where a reference catalogue states
    /// them; a venue-wide grid is the coarser statement for a venue whose
    /// instruments all share one.
    instrument_feasibility: BTreeMap<String, VenueFeasibility>,
    sequence: u64,
}

impl OrderManager {
    pub fn new(checker: PreTradeChecker) -> Self {
        Self {
            orders: BTreeMap::new(),
            checker,
            refusals: Vec::new(),
            reconciliation_breaks: Vec::new(),
            feasibility: BTreeMap::new(),
            instrument_feasibility: BTreeMap::new(),
            sequence: 0,
        }
    }

    /// Install the lot/tick/minimum grid for one instrument, keyed on the
    /// exact string its `object_id` renders to.
    ///
    /// Takes precedence over [`OrderManager::with_venue_feasibility`] for
    /// that instrument, because the instrument's own record is the more
    /// specific claim: a venue-wide lot of one is true of most of an equity
    /// venue and false of the board lot a particular listing states, and the
    /// order that would be wrong is the one in that listing.
    #[must_use]
    pub fn with_instrument_feasibility(
        mut self,
        object_id: impl Into<String>,
        model: VenueFeasibility,
    ) -> Self {
        self.instrument_feasibility.insert(object_id.into(), model);
        self
    }

    /// The grid installed for one instrument, if any — so the stage that
    /// sizes a leg can express it in whole lots before this manager judges
    /// it, from the same grid, rather than from a second copy of the lot.
    pub fn instrument_feasibility(&self, object_id: &str) -> Option<&VenueFeasibility> {
        self.instrument_feasibility.get(object_id)
    }

    /// Install the lot/tick/minimum grid for one venue, keyed on the exact
    /// string a [`Broker::name`] returns.
    ///
    /// Opt in per venue rather than a single default: a grid guessed at for a
    /// venue nobody has modelled is a rounding rule wearing a refusal's
    /// clothes, and this platform refuses only what it actually knows.
    #[must_use]
    pub fn with_venue_feasibility(
        mut self,
        venue: impl Into<String>,
        model: VenueFeasibility,
    ) -> Self {
        self.feasibility.insert(venue.into(), model);
        self
    }

    /// Every venue/book disagreement recorded since assembly.
    ///
    /// Non-empty means the platform's positions may not match the venue's, and
    /// nothing downstream should be trusted until it is reconciled.
    pub fn reconciliation_breaks(&self) -> &[String] {
        &self.reconciliation_breaks
    }

    pub fn order(&self, order_id: &OrderId) -> Option<&Order> {
        self.orders.get(order_id.as_str())
    }

    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }

    pub fn open_orders(&self) -> Vec<&Order> {
        self.orders
            .values()
            .filter(|order| order.state.is_open())
            .collect()
    }

    pub fn refusals(&self) -> &[SubmissionResult] {
        &self.refusals
    }

    /// Refusals that were safety controls rather than transient faults.
    pub fn safety_refusals(&self) -> Vec<&SubmissionResult> {
        self.refusals
            .iter()
            .filter(|result| {
                result
                    .refusal
                    .as_ref()
                    .is_some_and(RefusalReason::is_safety_control)
            })
            .collect()
    }

    /// Every fill recorded, across all orders.
    pub fn fills(&self) -> Vec<&Fill> {
        self.orders
            .values()
            .flat_map(|order| order.fills.iter())
            .collect()
    }

    /// Whether any fill in the system came from a real venue.
    ///
    /// The question a reconciliation asks, answered from the fills themselves
    /// rather than from configuration — because configuration is exactly what
    /// gets confused between a test and a deployment.
    pub fn has_live_fills(&self) -> bool {
        self.fills().iter().any(|fill| !fill.simulated)
    }

    /// Submit an order. The single path to a venue.
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &mut self,
        mut order: Order,
        broker: &mut dyn Broker,
        autonomy: &AutonomyController,
        risk_state: &RiskState,
        axes: BTreeMap<String, String>,
        counterparty: Option<String>,
        at: Timestamp,
    ) -> SubmissionResult {
        let refuse = |order: &Order, reason: RefusalReason| SubmissionResult {
            order_id: order.order_id.clone(),
            at,
            accepted: false,
            refusal: Some(reason),
            fills: Vec::new(),
            venue: None,
            simulated: broker.is_simulated(),
            reduced_to: None,
            reconciliation_breaks: Vec::new(),
        };

        // 1. Well formed, and traceable.
        if let Err(error) = order.validate() {
            let result = refuse(
                &order,
                RefusalReason::Malformed {
                    detail: error.message().to_string(),
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 2. Feasibility, before anything spends effort on an order that
        //    cannot be expressed at this venue at all. Opt in per instrument,
        //    then per venue: an instrument with no grid at a venue with no
        //    grid is checked for nothing, which mirrors `qip-edge`'s stated
        //    behaviour for a venue it has not modelled either.
        if let Some(model) = self
            .instrument_feasibility
            .get(order.object_id.as_str())
            .or_else(|| self.feasibility.get(broker.name()))
            && let Err(infeasible) = feasibility::assess(model, &order)
        {
            let result = refuse(
                &order,
                RefusalReason::Malformed {
                    detail: format!("infeasible ({}): {}", infeasible.gate, infeasible.reason),
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 3. The kill switch, checked first among the safety controls so a
        //    stopped platform stays stopped even if the autonomy level is
        //    misconfigured.
        if autonomy.kill_switch().is_halted(&order.scope) {
            let detail = autonomy
                .kill_switch()
                .global_trip()
                .or_else(|| autonomy.kill_switch().scoped_trip(&order.scope))
                .map(|trip| format!("{} ({})", trip.reason, trip.tripped_by))
                .unwrap_or_else(|| "no reason recorded".to_string());
            let result = refuse(
                &order,
                RefusalReason::Halted {
                    scope: order.scope.clone(),
                    detail,
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 4. Execution must be permitted at all.
        let level = autonomy.level();
        if !level.executes() {
            let result = refuse(
                &order,
                RefusalReason::AutonomyTooLow {
                    level,
                    required: AutonomyLevel::PaperTrading,
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }

        // 5. A live venue needs a live level *and* an available venue. Neither
        //    implies the other, and treating either as sufficient is how an
        //    order reaches a market nobody intended it to reach.
        if !broker.is_simulated() {
            if !level.is_live() {
                let result = refuse(
                    &order,
                    RefusalReason::LiveVenueBelowLiveAutonomy {
                        level,
                        venue: broker.name().to_string(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
            if !broker.is_available() {
                let result = refuse(
                    &order,
                    RefusalReason::VenueUnavailable {
                        venue: broker.name().to_string(),
                        detail: broker.requirement(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
        }

        // 6. Pre-trade risk, against the state the order would produce.
        let proposed = ProposedOrder {
            object_id: order.object_id.clone(),
            quantity: order.quantity * Decimal::from_int(i64::from(order.side.sign())),
            reference_price: order.arrival_price,
            axes,
            counterparty,
            scope: order.scope.clone(),
        };
        let check = match self.checker.check(&proposed, risk_state, at) {
            Ok(check) => check,
            Err(error) => {
                let result = refuse(
                    &order,
                    RefusalReason::Malformed {
                        detail: error.message().to_string(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
        };

        let mut reduced_to = None;
        match &check.decision {
            PreTradeDecision::Rejected { reasons } => {
                let result = refuse(
                    &order,
                    RefusalReason::RiskRejected {
                        reasons: reasons.clone(),
                    },
                );
                self.record_refusal(order, at, result.clone());
                return result;
            }
            PreTradeDecision::Reduced {
                permitted_quantity, ..
            } => {
                let permitted = permitted_quantity.abs();
                order.quantity = permitted;
                reduced_to = Some(permitted);
            }
            PreTradeDecision::Approved => {}
        }

        // Cleared. Send it.
        if order
            .transition(OrderState::RiskApproved { at }, at)
            .is_err()
        {
            let result = refuse(
                &order,
                RefusalReason::Malformed {
                    detail: format!("order is already {}", order.state.as_str()),
                },
            );
            self.record_refusal(order, at, result.clone());
            return result;
        }
        // Every refused transition and every refused fill is recorded. The
        // state machine says these cannot happen from here; a break that
        // "cannot happen" and is discarded is one nobody ever finds out about.
        let mut breaks: Vec<String> = Vec::new();
        if let Err(error) = order.transition(
            OrderState::Working {
                at,
                venue: broker.name().to_string(),
            },
            at,
        ) {
            breaks.push(format!(
                "order {} could not be marked working: {}",
                order.order_id.as_str(),
                error.message()
            ));
        }

        match broker.submit(&order, at) {
            Ok(fills) => {
                for fill in &fills {
                    // A broker that reported the wrong simulation flag would
                    // let a paper fill be counted as real; the OMS does not
                    // take the broker's word for it.
                    let mut fill = fill.clone();
                    fill.simulated = broker.is_simulated();
                    let quantity = fill.quantity;
                    if let Err(error) = order.apply_fill(fill) {
                        breaks.push(format!(
                            "venue {} reported a fill of {} on order {} that the order refused: {}",
                            broker.name(),
                            quantity,
                            order.order_id.as_str(),
                            error.message()
                        ));
                    }
                }
                self.reconciliation_breaks.extend(breaks.iter().cloned());
                let result = SubmissionResult {
                    order_id: order.order_id.clone(),
                    at,
                    accepted: true,
                    refusal: None,
                    fills: order.fills.clone(),
                    venue: Some(broker.name().to_string()),
                    simulated: broker.is_simulated(),
                    reduced_to,
                    reconciliation_breaks: breaks,
                };
                self.orders
                    .insert(order.order_id.as_str().to_string(), order);
                result
            }
            Err(error) => {
                if let Err(transition) = order.transition(
                    OrderState::Rejected {
                        at,
                        reason: error.message().to_string(),
                    },
                    at,
                ) {
                    breaks.push(format!(
                        "order {} could not be marked rejected: {}",
                        order.order_id.as_str(),
                        transition.message()
                    ));
                }
                self.reconciliation_breaks.extend(breaks.iter().cloned());
                let result = SubmissionResult {
                    order_id: order.order_id.clone(),
                    at,
                    accepted: false,
                    refusal: Some(RefusalReason::VenueRejected {
                        detail: error.message().to_string(),
                    }),
                    fills: Vec::new(),
                    venue: Some(broker.name().to_string()),
                    simulated: broker.is_simulated(),
                    reduced_to,
                    reconciliation_breaks: breaks,
                };
                self.orders
                    .insert(order.order_id.as_str().to_string(), order);
                self.refusals.push(result.clone());
                result
            }
        }
    }

    /// Cancel a working order.
    pub fn cancel(
        &mut self,
        order_id: &OrderId,
        broker: &mut dyn Broker,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<()> {
        let Some(order) = self.orders.get_mut(order_id.as_str()) else {
            return Err(Error::not_found(format!("no order {}", order_id.as_str())));
        };
        if !order.state.is_open() {
            return Err(Error::invalid(format!(
                "order {} is {} and cannot be cancelled",
                order_id.as_str(),
                order.state.as_str()
            )));
        }
        broker.cancel(order, at)?;
        order.transition(
            OrderState::Cancelled {
                at,
                reason: reason.into(),
            },
            at,
        )
    }

    /// Cancel every open order in a scope, for a kill switch.
    ///
    /// Returns the ids cancelled and the ones that could not be, because a
    /// halt that silently left orders working would be worse than no halt.
    pub fn cancel_scope(
        &mut self,
        scope: &str,
        broker: &mut dyn Broker,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> (Vec<OrderId>, Vec<(OrderId, String)>) {
        let reason = reason.into();
        let targets: Vec<OrderId> = self
            .orders
            .values()
            .filter(|order| order.state.is_open() && order.scope == scope)
            .map(|order| order.order_id.clone())
            .collect();

        let mut cancelled = Vec::new();
        let mut failed = Vec::new();
        for order_id in targets {
            match self.cancel(&order_id, broker, reason.clone(), at) {
                Ok(()) => cancelled.push(order_id),
                Err(error) => failed.push((order_id, error.message().to_string())),
            }
        }
        (cancelled, failed)
    }

    /// A fresh order id, deterministic across a replay.
    pub fn next_order_id(&mut self, prefix: &str) -> OrderId {
        self.sequence += 1;
        OrderId::from_string(format!("{prefix}-{}", self.sequence))
    }

    fn record_refusal(&mut self, order: Order, at: Timestamp, result: SubmissionResult) {
        let mut order = order;
        if let Some(reason) = &result.refusal {
            let _ = order.transition(
                OrderState::Rejected {
                    at,
                    reason: reason.describe(),
                },
                at,
            );
        }
        self.orders
            .insert(order.order_id.as_str().to_string(), order);
        self.refusals.push(result);
    }
}

/// Choose an order type for a given size relative to available liquidity.
///
/// A market order in an illiquid name is how a small position becomes a large
/// loss, so anything above a few percent of daily volume is worked rather than
/// taken.
pub fn order_type_for(participation: f64, window_minutes: u32) -> OrderType {
    if !participation.is_finite() || participation <= 0.01 {
        OrderType::Market
    } else if participation <= 0.05 {
        OrderType::TimeWeighted {
            minutes: window_minutes,
        }
    } else if participation <= 0.15 {
        OrderType::VolumeWeighted {
            minutes: window_minutes,
        }
    } else {
        // Beyond fifteen percent of a day's volume, the order sets the price
        // rather than taking it, and a fixed participation rate is the only
        // sensible way to work it.
        OrderType::Participation { rate: 0.10 }
    }
}
