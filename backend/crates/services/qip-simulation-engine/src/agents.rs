//! Typed counterparty agents: deterministic order-flow generators inside the
//! synthetic market.
//!
//! **Every behaviour in this module is synthetic. None of it is calibrated
//! against real fills, because the platform has never had a real fill to
//! calibrate against** (ADR 0003: it never submits a live order). A run that
//! uses these agents carries [`FlowCalibration::NotCalibrated`] in its record
//! and there is no other variant, so a report cannot present the counterparty
//! model as calibrated without inventing a field the record does not have.
//! The blueprint's own warning applies verbatim: uncalibrated, a market
//! simulator with adaptive agents is "confident expensive error". What the
//! agents *are* good for is the same thing the injected conditions in
//! [`crate::conditions`] are good for — putting a strategy through a market
//! that pushes back in a stated way and reading what it does.
//!
//! # Why flow at all
//!
//! Recorded history cannot respond to an order. Every fill in a replay is
//! free: nothing was ahead in the queue, nothing traded ahead of the move, and
//! nothing took the depth first. The five agents here each model one way a
//! real counterparty makes a fill cost something, and each one's rule is
//! stated on its constructor so a reader can see exactly which assumption
//! produced which order. They are the five the blueprint names; the failure
//! each models is in its doc.
//!
//! # Determinism
//!
//! The flow is a pure function of the run seed, the agents and the price
//! path, generated once when the agents are attached
//! ([`crate::market::MarketSimulator::with_agents`]) and again whenever the
//! condition schedule changes, because an agent withdraws under stress and
//! must see the same stress the fills will. Agents are held in a `BTreeMap`
//! keyed on their names, so the order in which they draw from the seeded
//! stream — and therefore the flow — cannot depend on the order a caller
//! declared them in. The generated records are part of
//! [`crate::market::SimulationRun::digest`].
//!
//! # Where an agent may look
//!
//! An agent reads the path only through a [`PathWindow`], which refuses a
//! read of any observation whose `known_at` lies beyond the window's declared
//! information horizon. Four of the five agents hold a horizon of zero: they
//! see what the market has printed and nothing else. The informed agent holds
//! the horizon it was constructed with, which is the whole of what makes it
//! informed — and it is refused one step past that horizon exactly as the
//! others are refused one step past now. The refusal is the point: an agent
//! that could read the path freely would produce flow that looked like
//! foreknowledge, and a strategy scored against it would be scored against
//! the future.
//!
//! # Arithmetic
//!
//! Returns and thresholds are statistics and are `f64`; quantities and prices
//! are money and are [`Decimal`]. The crossing happens in two places, each
//! marked: a path point's `Decimal` price becomes a [`PathObservation`]'s
//! `f64` when the window is built, and an agent's `f64` decision becomes a
//! `Decimal` quantity in [`FlowAction`] when the order is generated.

use crate::conditions::Regime;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::Timestamp;
use qip_market::book::Side;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The statement every run carrying agent flow must carry with it.
///
/// One string, in one place, so the record, the summary and the type's own
/// documentation cannot drift into three claims about calibration.
pub const NOT_CALIBRATED_STATEMENT: &str =
    "synthetic counterparty behaviour, not calibrated against real fills: none exist";

/// What the counterparty model was calibrated against.
///
/// There is one variant. A `Calibrated` arm would need real fills to name,
/// and the platform has none; adding the arm is the moment someone has to
/// produce them. Serialised as the statement itself rather than as a token,
/// so the record reads as the sentence and a record carrying any other
/// sentence does not decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowCalibration {
    NotCalibrated,
}

impl FlowCalibration {
    pub const fn statement(self) -> &'static str {
        match self {
            Self::NotCalibrated => NOT_CALIBRATED_STATEMENT,
        }
    }
}

impl Serialize for FlowCalibration {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.statement())
    }
}

impl<'de> Deserialize<'de> for FlowCalibration {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == NOT_CALIBRATED_STATEMENT {
            Ok(Self::NotCalibrated)
        } else {
            Err(serde::de::Error::custom(format!(
                "a counterparty flow record must state {NOT_CALIBRATED_STATEMENT:?}; got {text:?}"
            )))
        }
    }
}

/// The five counterparty behaviours the blueprint names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Passive,
    Informed,
    Momentum,
    Competitor,
    Maker,
}

impl AgentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Informed => "informed",
            Self::Momentum => "momentum",
            Self::Competitor => "competitor",
            Self::Maker => "maker",
        }
    }
}

/// One agent's rule, with its parameters.
///
/// Constructed only through [`CounterpartyAgent`]'s named constructors, which
/// validate every parameter; there is no way to hold a rule with a negative
/// clip or a horizon that was never stated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behaviour {
    /// **Passive liquidity.** Rests `size` at the calm touch on both sides
    /// every step, refreshing the queue, and with probability
    /// `participation` sends an uninformed clip of `size` on a side chosen by
    /// a coin flip. Withdraws entirely — no quote, no clip — under any
    /// injected condition.
    ///
    /// Models two failures a replay hides. A queue: in a replay nothing is
    /// ahead of the strategy's resting order, so every passive fill is
    /// instant; here the passive agent's size is in front of it. And
    /// uninformed flow: without it every counterparty is informed and the
    /// simulator overstates adverse selection. Its withdrawal under stress is
    /// the blueprint's own line — passive liquidity "withdraws under stress"
    /// — so the fill probability it teaches collapses exactly when the
    /// strategy needs it.
    Passive { size: Decimal, participation: f64 },
    /// **Informed flow.** Reads the path's return from now to `horizon` steps
    /// ahead — the one licensed read of the future in this module — and
    /// takes `clip` toward the move when its magnitude exceeds `threshold`.
    /// Sends nothing when the move within the horizon is smaller than that.
    ///
    /// Models adverse selection honestly: the counterparty that trades with a
    /// resting order does so because it knows something, and the move it
    /// knew about arrives after the fill. Its information stops at the
    /// horizon; a read one step beyond is refused by [`PathWindow`].
    Informed {
        clip: Decimal,
        horizon: usize,
        threshold: f64,
    },
    /// **Momentum follower.** Reads the trailing return over `lookback`
    /// steps and takes `clip` in its direction when its magnitude exceeds
    /// `threshold`; nothing otherwise.
    ///
    /// Models impact and reflexivity: flow that arrives *after* a move, in
    /// its direction, taking the depth on the side a strategy needs to get
    /// out — the crowded exit. It reads only what has printed.
    Momentum {
        clip: Decimal,
        lookback: usize,
        threshold: f64,
    },
    /// **Competing arbitrageur.** Runs the same trailing-return signal a
    /// momentum strategy runs, at `lookback` steps, and takes `clip`
    /// multiplied by the number of consecutive steps the signal has held,
    /// capped at `crowd_limit` — the crowd grows while the signal persists
    /// and is gone the step it flips.
    ///
    /// Models crowding: the same signal in more hands than yours, arriving in
    /// the same step, so the depth an order was sized against is consumed
    /// before it arrives — and whether an edge survives that.
    Competitor {
        clip: Decimal,
        lookback: usize,
        threshold: f64,
        crowd_limit: usize,
    },
    /// **Market maker.** Quotes `size` on both sides at the calm mid plus
    /// and minus `half_spread_bps` — never inside the calm touch — and skews
    /// on inventory: its inventory is the negative of every other agent's
    /// taker flow at the instrument, and the side it does not want to trade
    /// is widened by `skew_bps` scaled by inventory over `max_inventory`.
    /// Withdraws under any injected condition and once its inventory exceeds
    /// `max_inventory`.
    ///
    /// Models how spreads respond to flow: a fixed spread never shows a
    /// strategy the cost of its own footprint. Because the maker never
    /// improves on the calm touch, a skew appears only as widening, and the
    /// simulator stays no more generous than the calm book.
    Maker {
        size: Decimal,
        half_spread_bps: f64,
        skew_bps: f64,
        max_inventory: Decimal,
    },
}

impl Behaviour {
    pub const fn kind(&self) -> AgentKind {
        match self {
            Self::Passive { .. } => AgentKind::Passive,
            Self::Informed { .. } => AgentKind::Informed,
            Self::Momentum { .. } => AgentKind::Momentum,
            Self::Competitor { .. } => AgentKind::Competitor,
            Self::Maker { .. } => AgentKind::Maker,
        }
    }

    /// The steps ahead of now this behaviour is licensed to read.
    pub const fn information_horizon(&self) -> usize {
        match self {
            Self::Informed { horizon, .. } => *horizon,
            _ => 0,
        }
    }

    /// The rule in one sentence, for the run record.
    pub fn describe(&self) -> String {
        match self {
            Self::Passive {
                size,
                participation,
            } => format!(
                "rests {size} at the calm touch both sides; with probability {participation:.2} takes {size} on a coin-flipped side; withdraws under any condition"
            ),
            Self::Informed {
                clip,
                horizon,
                threshold,
            } => format!(
                "reads the return {horizon} step(s) ahead and takes {clip} toward it beyond {:.2}%; refused beyond {horizon}",
                threshold * 100.0
            ),
            Self::Momentum {
                clip,
                lookback,
                threshold,
            } => format!(
                "takes {clip} in the direction of the trailing {lookback}-step return beyond {:.2}%",
                threshold * 100.0
            ),
            Self::Competitor {
                clip,
                lookback,
                threshold,
                crowd_limit,
            } => format!(
                "takes {clip} × run length (≤{crowd_limit}) in the direction of the trailing {lookback}-step return beyond {:.2}%",
                threshold * 100.0
            ),
            Self::Maker {
                size,
                half_spread_bps,
                skew_bps,
                max_inventory,
            } => format!(
                "quotes {size} both sides at ±{half_spread_bps:.1}bp of the calm mid, widening the unwanted side by up to {skew_bps:.1}bp on inventory; withdraws past {max_inventory} or under any condition"
            ),
        }
    }
}

/// A named counterparty with one [`Behaviour`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterpartyAgent {
    name: String,
    behaviour: Behaviour,
}

fn positive_quantity(name: &str, what: &str, quantity: Decimal) -> Result<()> {
    if !quantity.is_positive() {
        return Err(Error::invalid(format!(
            "agent {name} needs a positive {what}; {quantity} would generate no flow and say it had"
        )));
    }
    Ok(())
}

fn finite_threshold(name: &str, threshold: f64) -> Result<()> {
    if !threshold.is_finite() || threshold < 0.0 {
        return Err(Error::invalid(format!(
            "agent {name} needs a finite, non-negative threshold; {threshold} is not one"
        )));
    }
    Ok(())
}

fn named(name: impl Into<String>) -> Result<String> {
    let name = name.into();
    if name.trim().is_empty() {
        return Err(Error::invalid(
            "an agent needs a name; the run record names each agent's flow by it",
        ));
    }
    Ok(name)
}

impl CounterpartyAgent {
    /// See [`Behaviour::Passive`].
    pub fn passive(name: impl Into<String>, size: Decimal, participation: f64) -> Result<Self> {
        let name = named(name)?;
        positive_quantity(&name, "size", size)?;
        if !participation.is_finite() || !(0.0..=1.0).contains(&participation) {
            return Err(Error::invalid(format!(
                "agent {name} needs a participation probability in [0, 1]; {participation} is not one"
            )));
        }
        Ok(Self {
            name,
            behaviour: Behaviour::Passive {
                size,
                participation,
            },
        })
    }

    /// See [`Behaviour::Informed`]. A horizon of zero is refused: an informed
    /// agent that may read nothing ahead is a passive one wearing the name.
    pub fn informed(
        name: impl Into<String>,
        clip: Decimal,
        horizon: usize,
        threshold: f64,
    ) -> Result<Self> {
        let name = named(name)?;
        positive_quantity(&name, "clip", clip)?;
        finite_threshold(&name, threshold)?;
        if horizon == 0 {
            return Err(Error::invalid(format!(
                "agent {name} needs an information horizon of at least one step; with zero it knows nothing and is not informed"
            )));
        }
        Ok(Self {
            name,
            behaviour: Behaviour::Informed {
                clip,
                horizon,
                threshold,
            },
        })
    }

    /// See [`Behaviour::Momentum`].
    pub fn momentum(
        name: impl Into<String>,
        clip: Decimal,
        lookback: usize,
        threshold: f64,
    ) -> Result<Self> {
        let name = named(name)?;
        positive_quantity(&name, "clip", clip)?;
        finite_threshold(&name, threshold)?;
        if lookback == 0 {
            return Err(Error::invalid(format!(
                "agent {name} needs a lookback of at least one step; a zero-step return is always zero"
            )));
        }
        Ok(Self {
            name,
            behaviour: Behaviour::Momentum {
                clip,
                lookback,
                threshold,
            },
        })
    }

    /// See [`Behaviour::Competitor`].
    pub fn competitor(
        name: impl Into<String>,
        clip: Decimal,
        lookback: usize,
        threshold: f64,
        crowd_limit: usize,
    ) -> Result<Self> {
        let name = named(name)?;
        positive_quantity(&name, "clip", clip)?;
        finite_threshold(&name, threshold)?;
        if lookback == 0 || crowd_limit == 0 {
            return Err(Error::invalid(format!(
                "agent {name} needs a lookback and a crowd limit of at least one; got {lookback} and {crowd_limit}"
            )));
        }
        Ok(Self {
            name,
            behaviour: Behaviour::Competitor {
                clip,
                lookback,
                threshold,
                crowd_limit,
            },
        })
    }

    /// See [`Behaviour::Maker`].
    pub fn maker(
        name: impl Into<String>,
        size: Decimal,
        half_spread_bps: f64,
        skew_bps: f64,
        max_inventory: Decimal,
    ) -> Result<Self> {
        let name = named(name)?;
        positive_quantity(&name, "size", size)?;
        positive_quantity(&name, "inventory limit", max_inventory)?;
        if !half_spread_bps.is_finite() || half_spread_bps <= 0.0 {
            return Err(Error::invalid(format!(
                "agent {name} needs a positive half-spread; {half_spread_bps}bp is a maker paying to be hit"
            )));
        }
        if !skew_bps.is_finite() || skew_bps < 0.0 {
            return Err(Error::invalid(format!(
                "agent {name} needs a finite, non-negative skew; {skew_bps}bp is not one"
            )));
        }
        Ok(Self {
            name,
            behaviour: Behaviour::Maker {
                size,
                half_spread_bps,
                skew_bps,
                max_inventory,
            },
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn behaviour(&self) -> &Behaviour {
        &self.behaviour
    }

    pub fn kind(&self) -> AgentKind {
        self.behaviour.kind()
    }

    /// The entry the run record carries for this agent.
    pub fn record(&self) -> AgentRecord {
        AgentRecord {
            name: self.name.clone(),
            kind: self.kind(),
            rule: self.behaviour.describe(),
        }
    }
}

/// An agent as the run record names it: who, which behaviour, and the rule
/// in words, so the record can be read without the type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub name: String,
    pub kind: AgentKind,
    pub rule: String,
}

/// What an agent did at one instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowAction {
    /// Took `quantity` from the opposite side of the book.
    Take { side: Side, quantity: Decimal },
    /// Rested `size` at `bid` and at `ask`.
    Quote {
        bid: Decimal,
        ask: Decimal,
        size: Decimal,
    },
}

/// One agent's action at one instant, at one venue, for one instrument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRecord {
    pub at: Timestamp,
    pub agent: String,
    pub kind: AgentKind,
    pub object_id: String,
    pub venue: String,
    pub action: FlowAction,
}

impl FlowRecord {
    /// The taker flow with its sign: positive for a buy, negative for a sell,
    /// `None` for a quote.
    pub fn signed_quantity(&self) -> Option<Decimal> {
        match self.action {
            FlowAction::Take {
                side: Side::Buy,
                quantity,
            } => Some(quantity),
            FlowAction::Take {
                side: Side::Sell,
                quantity,
            } => Some(-quantity),
            FlowAction::Quote { .. } => None,
        }
    }
}

/// One point of the price path as an agent sees it.
///
/// `price` is `f64` because everything an agent computes from it is a
/// return, a statistic; the crossing from the path's `Decimal` happens where
/// the observation is built, in [`PathObservation::new`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathObservation {
    /// The instant the price was true.
    pub at: Timestamp,
    /// The instant the price became readable. For a synthetic path this is
    /// `at`; for a replayed bar it is the bar's close, which is also the
    /// instant the path keys it on.
    pub known_at: Timestamp,
    pub price: f64,
}

impl PathObservation {
    /// The crossing from money to statistic, in one place.
    pub fn new(at: Timestamp, known_at: Timestamp, price: Decimal) -> Result<Self> {
        if !price.is_positive() {
            return Err(Error::invalid(format!(
                "a path observation at {at} needs a positive price; {price} has no return"
            )));
        }
        Ok(Self {
            at,
            known_at,
            price: price.to_f64(),
        })
    }
}

/// The path as one agent may read it at one instant.
///
/// Reads backward are unrestricted; every read forward is checked against the
/// window's licence, which is the instant `horizon` steps ahead of now. An
/// observation whose `known_at` lies beyond that is refused with a message
/// naming the horizon, rather than returned — a
/// window that answered with `None` would let an agent treat "not yet
/// knowable" as "no move", which is a reading of the future too.
#[derive(Debug)]
pub struct PathWindow<'a> {
    observations: &'a [PathObservation],
    now: usize,
    horizon: usize,
}

impl<'a> PathWindow<'a> {
    pub fn new(observations: &'a [PathObservation], now: usize, horizon: usize) -> Result<Self> {
        if now >= observations.len() {
            return Err(Error::invalid(format!(
                "a path window at index {now} on a path of {} point(s) has no present",
                observations.len()
            )));
        }
        Ok(Self {
            observations,
            now,
            horizon,
        })
    }

    pub fn now(&self) -> Timestamp {
        self.observations[self.now].at
    }

    pub fn horizon(&self) -> usize {
        self.horizon
    }

    /// The latest instant this window may read: the instant `horizon` steps
    /// ahead of now, or the end of the path if that comes first.
    ///
    /// The instant, and not that observation's `known_at`: a licence taken
    /// from `known_at` would move with a bar stamped knowable late, and a bar
    /// inside the horizon by position but not yet knowable is the exact leak
    /// this window refuses.
    pub fn licensed_until(&self) -> Timestamp {
        let last = self.observations.len() - 1;
        self.observations[self.now.saturating_add(self.horizon).min(last)].at
    }

    pub fn price_now(&self) -> f64 {
        self.observations[self.now].price
    }

    /// The price `steps` ahead of now, `None` past the end of the path.
    ///
    /// Refused when the observation was not knowable within the licence.
    pub fn price_ahead(&self, steps: usize) -> Result<Option<f64>> {
        let Some(observation) = self.observations.get(self.now.saturating_add(steps)) else {
            return Ok(None);
        };
        let licence = self.licensed_until();
        if observation.known_at > licence {
            return Err(Error::denied(format!(
                "the observation at {} is knowable at {}, beyond this agent's licence ending {} \
                 ({} step(s) past {}); an agent reads no bar before its known_at — widen the \
                 declared horizon or do not ask",
                observation.at,
                observation.known_at,
                licence,
                self.horizon,
                self.now()
            )));
        }
        Ok(Some(observation.price))
    }

    /// The return from now to `steps` ahead, `None` past the end of the path.
    pub fn planted_return(&self, steps: usize) -> Result<Option<f64>> {
        Ok(self
            .price_ahead(steps)?
            .map(|ahead| ahead / self.price_now() - 1.0))
    }

    /// The return over the last `lookback` steps, `None` before there are
    /// that many. Reads only what has printed, so it cannot be refused.
    pub fn trailing_return(&self, lookback: usize) -> Option<f64> {
        let earlier = self.observations.get(self.now.checked_sub(lookback)?)?;
        Some(self.price_now() / earlier.price - 1.0)
    }
}

/// What the market tells an agent about the instant it is acting in.
#[derive(Clone, Debug)]
pub struct StepContext<'a> {
    pub at: Timestamp,
    pub object_id: &'a str,
    pub venue: &'a str,
    /// The undisturbed calm price, exact.
    pub price: Decimal,
    /// The calm half-spread the book is built with, in basis points.
    pub calm_half_spread_bps: f64,
    /// The regime at this instant and scope, leg zero.
    pub regime: &'a Regime,
}

impl StepContext<'_> {
    /// Whether any condition is in force: every agent that rests liquidity
    /// withdraws on this, and every taker holds off, so injected chaos is
    /// never met by an agent supplying what the chaos removed.
    pub fn stressed(&self) -> bool {
        !self.regime.applied.is_empty()
    }

    fn calm_bid(&self) -> Decimal {
        self.price - self.price.apply_bps(self.calm_half_spread_bps)
    }

    fn calm_ask(&self) -> Decimal {
        self.price + self.price.apply_bps(self.calm_half_spread_bps)
    }
}

/// The state an agent carries from one step to the next.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentState {
    /// Consecutive steps the competitor's signal has held, with its sign.
    run: usize,
    run_sign: i8,
    /// The maker's inventory: the negative of the taker flow it absorbed.
    inventory: Decimal,
}

impl AgentState {
    /// Book the taker flow the other agents sent this step against the
    /// maker's inventory. A buy took from the maker, so its inventory falls.
    pub fn absorb(&mut self, signed_taker_flow: Decimal) {
        self.inventory -= signed_taker_flow;
    }

    pub fn inventory(&self) -> Decimal {
        self.inventory
    }
}

fn direction(value: f64, threshold: f64) -> Option<Side> {
    if value > threshold {
        Some(Side::Buy)
    } else if value < -threshold {
        Some(Side::Sell)
    } else {
        None
    }
}

impl CounterpartyAgent {
    /// The agent's actions at one instant.
    ///
    /// The window's horizon must be the agent's own: a caller handing an
    /// agent a wider window than its behaviour declares is handing it the
    /// future, and is refused before any read happens.
    pub fn act(
        &self,
        window: &PathWindow<'_>,
        step: &StepContext<'_>,
        state: &mut AgentState,
        rng: &mut Xoshiro256,
    ) -> Result<Vec<FlowAction>> {
        if window.horizon() != self.behaviour.information_horizon() {
            return Err(Error::denied(format!(
                "agent {} declares an information horizon of {} step(s) and was handed a window \
                 licensed {} ahead; the window must match the declaration",
                self.name,
                self.behaviour.information_horizon(),
                window.horizon()
            )));
        }
        let mut actions = Vec::with_capacity(2);
        match &self.behaviour {
            Behaviour::Passive {
                size,
                participation,
            } => {
                // The coin is flipped before the stress check so the stream
                // advances identically whether or not a condition is in
                // force: a schedule change must not redraw the calm steps.
                let participates = rng.bernoulli(*participation);
                let buys = rng.bernoulli(0.5);
                if step.stressed() {
                    return Ok(actions);
                }
                actions.push(FlowAction::Quote {
                    bid: step.calm_bid(),
                    ask: step.calm_ask(),
                    size: *size,
                });
                if participates {
                    actions.push(FlowAction::Take {
                        side: if buys { Side::Buy } else { Side::Sell },
                        quantity: *size,
                    });
                }
            }
            Behaviour::Informed {
                clip,
                horizon,
                threshold,
            } => {
                if step.stressed() {
                    return Ok(actions);
                }
                if let Some(side) = window
                    .planted_return(*horizon)?
                    .and_then(|ahead| direction(ahead, *threshold))
                {
                    actions.push(FlowAction::Take {
                        side,
                        quantity: *clip,
                    });
                }
            }
            Behaviour::Momentum {
                clip,
                lookback,
                threshold,
            } => {
                if step.stressed() {
                    return Ok(actions);
                }
                if let Some(side) = window
                    .trailing_return(*lookback)
                    .and_then(|trailing| direction(trailing, *threshold))
                {
                    actions.push(FlowAction::Take {
                        side,
                        quantity: *clip,
                    });
                }
            }
            Behaviour::Competitor {
                clip,
                lookback,
                threshold,
                crowd_limit,
            } => {
                let signal = window
                    .trailing_return(*lookback)
                    .and_then(|trailing| direction(trailing, *threshold));
                // The run length is a fact about the signal, not about the
                // regime, so it advances under stress too; only the order is
                // withheld.
                match signal {
                    Some(side) => {
                        let sign: i8 = match side {
                            Side::Buy => 1,
                            Side::Sell => -1,
                        };
                        if state.run_sign == sign {
                            state.run = state.run.saturating_add(1);
                        } else {
                            state.run = 1;
                            state.run_sign = sign;
                        }
                    }
                    None => {
                        state.run = 0;
                        state.run_sign = 0;
                    }
                }
                if step.stressed() {
                    return Ok(actions);
                }
                if let Some(side) = signal {
                    let crowd = state.run.min(*crowd_limit);
                    // Refused rather than capped: a crowd too large to
                    // represent is a parameter to reconsider, not a quantity
                    // to quietly shrink.
                    let quantity = clip
                        .checked_mul(Decimal::from_int(crowd as i64))
                        .ok_or_else(|| {
                            Error::numeric(format!(
                                "agent {}'s crowd of {crowd} × {clip} is not representable",
                                self.name
                            ))
                        })?;
                    actions.push(FlowAction::Take { side, quantity });
                }
            }
            Behaviour::Maker {
                size,
                half_spread_bps,
                skew_bps,
                max_inventory,
            } => {
                if step.stressed() || state.inventory.abs() > *max_inventory {
                    return Ok(actions);
                }
                // Inventory over its limit, in [-1, 1]. A statistic: it only
                // ever scales a basis-point figure.
                let load = (state.inventory.to_f64() / max_inventory.to_f64()).clamp(-1.0, 1.0);
                let half = step
                    .price
                    .apply_bps(half_spread_bps.max(step.calm_half_spread_bps));
                let skew = step.price.apply_bps(skew_bps * load.abs());
                let (mut bid, mut ask) = (step.price - half, step.price + half);
                if load > 0.0 {
                    // Long: does not want to buy more, so the bid backs off.
                    bid -= skew;
                } else if load < 0.0 {
                    // Short: does not want to sell more, so the ask backs off.
                    ask += skew;
                }
                // Never inside the calm touch, whatever the skew did.
                let bid = bid.min(step.calm_bid());
                let ask = ask.max(step.calm_ask());
                if bid.is_positive() {
                    actions.push(FlowAction::Quote {
                        bid,
                        ask,
                        size: *size,
                    });
                }
            }
        }
        Ok(actions)
    }
}

/// What one instrument's flow generation reads.
pub(crate) struct FlowInputs<'a> {
    pub object_id: &'a str,
    pub observations: &'a [PathObservation],
    /// The exact calm price at each observation, parallel to `observations`.
    pub prices: &'a [Decimal],
    pub calm_half_spread_bps: f64,
    pub venues: &'a [String],
}

/// Generate every agent's flow for one instrument over its whole path.
///
/// Order is fixed: step by step, and within a step the agents in name order.
/// The maker's inventory is settled at the end of each step from the taker
/// flow the other agents sent in it, so a maker never reacts to flow that
/// has not happened yet.
pub(crate) fn generate_flow(
    agents: &BTreeMap<String, CounterpartyAgent>,
    inputs: &FlowInputs<'_>,
    regime_of: impl Fn(Timestamp, &str) -> Regime,
    seed: u64,
) -> Result<Vec<FlowRecord>> {
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    if inputs.venues.is_empty() {
        return Err(Error::invalid(format!(
            "agents on {} need a venue to trade at",
            inputs.object_id
        )));
    }
    let mut streams: BTreeMap<&str, Xoshiro256> = BTreeMap::new();
    let mut states: BTreeMap<&str, AgentState> = BTreeMap::new();
    for name in agents.keys() {
        // Forked per instrument and per agent off the run seed, so adding an
        // agent or an instrument leaves every other stream untouched.
        streams.insert(
            name.as_str(),
            Xoshiro256::seeded(seed)
                .fork("agents")
                .fork(inputs.object_id)
                .fork(name),
        );
        states.insert(name.as_str(), AgentState::default());
    }

    let mut records = Vec::new();
    for (index, observation) in inputs.observations.iter().enumerate() {
        let price = *inputs.prices.get(index).ok_or_else(|| {
            Error::invalid(format!(
                "the exact price series for {} is shorter than its observations",
                inputs.object_id
            ))
        })?;
        let mut taker_flow = Decimal::ZERO;
        for (name, agent) in agents {
            let rng = streams
                .get_mut(name.as_str())
                .ok_or_else(|| Error::invalid(format!("no stream was forked for agent {name}")))?;
            let state = states
                .get_mut(name.as_str())
                .ok_or_else(|| Error::invalid(format!("no state was kept for agent {name}")))?;
            let venue = &inputs.venues[rng.below(inputs.venues.len() as u64) as usize];
            let regime = regime_of(observation.at, venue);
            let window = PathWindow::new(
                inputs.observations,
                index,
                agent.behaviour().information_horizon(),
            )?;
            let step = StepContext {
                at: observation.at,
                object_id: inputs.object_id,
                venue,
                price,
                calm_half_spread_bps: inputs.calm_half_spread_bps,
                regime: &regime,
            };
            for action in agent.act(&window, &step, state, rng)? {
                let record = FlowRecord {
                    at: observation.at,
                    agent: name.clone(),
                    kind: agent.kind(),
                    object_id: inputs.object_id.to_string(),
                    venue: venue.clone(),
                    action,
                };
                if let Some(signed) = record.signed_quantity() {
                    taker_flow += signed;
                }
                records.push(record);
            }
        }
        for (name, agent) in agents {
            if agent.kind() == AgentKind::Maker
                && let Some(state) = states.get_mut(name.as_str())
            {
                state.absorb(taker_flow);
            }
        }
    }
    Ok(records)
}
