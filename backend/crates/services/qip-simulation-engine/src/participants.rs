//! Participant behaviour on the tick-replay path (R12-TICK-9036).
//!
//! A replay reproduces what the *recorded* participants did: their messages
//! are the tape. It cannot say what a participant who was not there, or who
//! would have acted differently, would have done to a resting order's queue.
//! The typed counterparty agents of [`crate::agents`] can say it, but until
//! this module they ran only inside the synthetic market and a shipped
//! replay printed their names without simulating them.
//!
//! Here the agents act on the replayed book. The tape is walked once to
//! read each instrument's mid at each instant it became knowable; each
//! agent then acts over that path (the same generator the synthetic market
//! uses, so an agent behaves one way in both worlds); and every action
//! becomes an order the replay session executes against the rebuilt book,
//! where queue position, latency, fees and impact apply to it as to any
//! other order:
//!
//! * a **take** is a market order;
//! * a **quote** is a buy limit at its bid and a sell limit at its ask,
//!   which rest behind whatever was already queued;
//! * a **cancelled** layer was withdrawn within its own step, before any
//!   order could reach it, so it is not an order here either.
//!
//! The tape itself is never altered: recorded history is evidence, and an
//! agent's order is a counterfactual on it, labelled by its id prefix
//! ([`PARTICIPANT_PREFIX`]) so no report can mistake it for a recorded fill.
//!
//! # Why a shard sees the same participants
//!
//! The generator forks its random stream per instrument and per agent off
//! the run seed, and an agent reads only its own instrument's path. So an
//! instrument's participant orders are the same whichever instruments share
//! the process, and the union of shards equals the unsharded run.

use crate::agents::{CounterpartyAgent, FlowAction, FlowInputs, PathObservation, generate_flow};
use crate::conditions::Regime;
use crate::tick_replay::{OrderSide, ReplayOrder, ReplaySession, ReplayTick};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_market::book::Side;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Every participant order's id starts with this, so a report can tell a
/// simulated participant's execution from the platform's own.
pub const PARTICIPANT_PREFIX: &str = "participant:";

/// The most participant orders one replay may add. Bounded so a long tape
/// with a chatty agent is refused rather than exhausting memory.
pub const MAX_PARTICIPANT_ORDERS: usize = 1_000_000;

/// Who acts on the tape, and under what stated terms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParticipantSpec {
    /// The run seed the agents' streams are forked from.
    pub seed: u64,
    /// The calm half-spread agents quote around, in basis points. Stated by
    /// the operator, not defaulted: it is a claim about the venue.
    pub calm_half_spread_bps: f64,
    /// The agents, by name.
    pub agents: Vec<CounterpartyAgent>,
}

impl ParticipantSpec {
    fn validate(&self) -> Result<()> {
        if self.agents.is_empty() {
            return Err(Error::invalid(
                "a participant spec names at least one agent; none would simulate nothing and \
                 report that participants were simulated",
            ));
        }
        if !self.calm_half_spread_bps.is_finite() || self.calm_half_spread_bps <= 0.0 {
            return Err(Error::invalid(format!(
                "calm_half_spread_bps must be finite and positive, not {}",
                self.calm_half_spread_bps
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for agent in &self.agents {
            if !seen.insert(agent.name().to_string()) {
                return Err(Error::invalid(format!(
                    "two agents are named {}; the record names each agent's flow by its name",
                    agent.name()
                )));
            }
        }
        Ok(())
    }
}

/// The orders `spec`'s agents would send against `ticks`, in a stable order.
pub fn participant_orders(
    spec: &ParticipantSpec,
    model: crate::tick_replay::ExecutionModel,
    ticks: &[ReplayTick],
) -> Result<Vec<ReplayOrder>> {
    spec.validate()?;
    // Pass one: the tape alone, to read each instrument's mid as it became
    // knowable. No orders, so nothing here depends on the agents.
    let mut session = ReplaySession::new(model, ticks.to_vec(), Vec::new(), Vec::new())?;
    let mut paths: BTreeMap<String, Vec<(Timestamp, Decimal)>> = BTreeMap::new();
    let mut venues: BTreeMap<String, String> = BTreeMap::new();
    let ordered = crate::tick_replay::sequence_ticks(ticks.to_vec());
    for tick in &ordered {
        if !session.step()? {
            break;
        }
        let instrument = tick.message.object_id.as_str();
        venues
            .entry(instrument.to_string())
            .or_insert_with(|| tick.message.origin.venue.to_string());
        let Some(mid) = session.book(instrument).and_then(|book| book.mid()) else {
            continue;
        };
        let path = paths.entry(instrument.to_string()).or_default();
        match path.last_mut() {
            // Several messages at one instant are one observation: the
            // book as it stood when the instant was complete.
            Some(last) if last.0 == tick.knowable_at => last.1 = mid,
            _ => path.push((tick.knowable_at, mid)),
        }
    }
    let agents: BTreeMap<String, CounterpartyAgent> = spec
        .agents
        .iter()
        .map(|agent| (agent.name().to_string(), agent.clone()))
        .collect();
    let mut orders = Vec::new();
    for (instrument, path) in &paths {
        let observations = path
            .iter()
            .map(|(at, mid)| PathObservation::new(*at, *at, *mid))
            .collect::<Result<Vec<_>>>()?;
        let prices: Vec<Decimal> = path.iter().map(|(_, mid)| *mid).collect();
        let venue = venues.get(instrument).cloned().unwrap_or_default();
        let venues_list = [venue];
        let records = generate_flow(
            &agents,
            &FlowInputs {
                object_id: instrument,
                observations: &observations,
                prices: &prices,
                calm_half_spread_bps: spec.calm_half_spread_bps,
                venues: &venues_list,
                own_flow: None,
            },
            |_, _| Regime::calm(),
            spec.seed,
        )?;
        for record in records {
            let at = record.at;
            let mut push =
                |tag: &str, side: OrderSide, quantity: Decimal, limit: Option<Decimal>| {
                    let id = format!(
                        "{PARTICIPANT_PREFIX}{}:{instrument}:{}:{tag}",
                        record.agent,
                        at.as_nanos()
                    );
                    orders.push(ReplayOrder {
                        id,
                        instrument: instrument.clone(),
                        side,
                        quantity,
                        limit,
                        decided_at: at,
                    });
                };
            match record.action {
                FlowAction::Take { side, quantity } => {
                    push("take", order_side(side), quantity, None);
                }
                FlowAction::Quote { bid, ask, size } => {
                    push("bid", OrderSide::Buy, size, Some(bid));
                    push("ask", OrderSide::Sell, size, Some(ask));
                }
                FlowAction::Cancelled { .. } => {}
            }
            if orders.len() > MAX_PARTICIPANT_ORDERS {
                return Err(Error::invalid(format!(
                    "the participants would send more than {MAX_PARTICIPANT_ORDERS} orders; \
                     shorten the tape or the agent list"
                )));
            }
        }
    }
    // Ids are unique by construction only if one agent acts once per
    // instant per instrument, which the generator guarantees for every
    // behaviour but a quote-and-take agent: its two actions carry distinct
    // tags. Refuse a collision rather than let the session refuse it later
    // without naming the cause.
    orders.sort_by(|a, b| (a.decided_at, &a.id).cmp(&(b.decided_at, &b.id)));
    for pair in orders.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(Error::invalid(format!(
                "two participant orders share id {}; an agent acted twice with one tag at one \
                 instant",
                pair[0].id
            )));
        }
    }
    Ok(orders)
}

const fn order_side(side: Side) -> OrderSide {
    match side {
        Side::Buy => OrderSide::Buy,
        Side::Sell => OrderSide::Sell,
    }
}
