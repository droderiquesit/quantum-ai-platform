//! Participant behaviour simulated on the tick-replay path (R12-TICK-9036).
//!
//! A replay that only prints the names of participants has not simulated
//! them. These tests prove the agents' orders reach the rebuilt book and are
//! executed there under queue position, latency and fees; that sharding by
//! instrument leaves each instrument's participants unchanged; and that a
//! spec naming no agents is refused rather than reported as simulated.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_simulation_engine::agents::CounterpartyAgent;
use qip_simulation_engine::participants::{
    PARTICIPANT_PREFIX, ParticipantSpec, participant_orders,
};
use qip_simulation_engine::tick_replay::{
    ExecutionModel, ExecutionStatus, ReplaySession, ReplayTick,
};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn d(v: i64) -> Decimal {
    Decimal::from_int(v)
}

fn model() -> ExecutionModel {
    ExecutionModel {
        latency: Duration::from_millis(10),
        fee_bps: 2.0,
        impact_bps_at_full_depth: 20.0,
        limit_ttl: Duration::from_secs(60),
        message_budget: None,
    }
}

fn level(
    instrument: &str,
    sequence: u64,
    ms: i64,
    side: BookSide,
    price: i64,
    qty: i64,
) -> ReplayTick {
    ReplayTick {
        message: MarketMessage::new(
            ObjectId::from_string(instrument),
            Origin::new(
                VenueId::new("XLON"),
                format!("feed-{instrument}"),
                0,
                sequence,
            ),
            MessageBody::LevelSet {
                side,
                price: d(price),
                quantity: d(qty),
                order_count: None,
            },
            at(ms),
            at(ms),
        ),
        knowable_at: at(ms),
    }
}

/// A market that walks up one point per step, a two-point spread, deep on
/// both sides. Each step removes the old touch and sets the new one, so the
/// mid is exactly `100 + step + 1`.
fn rising(instrument: &str, steps: i64) -> Vec<ReplayTick> {
    let mut ticks = Vec::new();
    let mut sequence = 0;
    let mut push = |ms: i64, side: BookSide, price: i64, qty: i64| {
        sequence += 1;
        ticks.push(level(instrument, sequence, ms, side, price, qty));
    };
    for step in 0..steps {
        let ms = step * 100;
        if step > 0 {
            push(ms, BookSide::Bid, 99 + step, 0);
            push(ms, BookSide::Ask, 101 + step, 0);
        }
        push(ms, BookSide::Bid, 100 + step, 1_000);
        push(ms, BookSide::Ask, 102 + step, 1_000);
    }
    ticks
}

fn momentum_spec() -> ParticipantSpec {
    ParticipantSpec {
        seed: 7,
        calm_half_spread_bps: 5.0,
        agents: vec![CounterpartyAgent::momentum("trend", d(10), 2, 0.001).unwrap()],
    }
}

/// R12-TICK-9036: an agent's order is executed on the replayed book, with the
/// latency and fees the model states, and changes what the replay produced.
///
/// The failure it prevents: participants were printed and never simulated,
/// so a replay's fills reflected recorded participants only.
///
/// Mutation: in `participant_orders` (src/participants.rs) drop the
/// `FlowAction::Take` arm's `push`: no participant order exists and the
/// first premise assertion fails.
#[test]
fn a_momentum_participant_buys_into_a_rising_replay_and_pays_latency_and_fees() -> Result<()> {
    let ticks = rising("obj-ACME", 12);
    let orders = participant_orders(&momentum_spec(), model(), &ticks)?;
    assert!(
        orders.iter().any(|o| o.id.starts_with(PARTICIPANT_PREFIX)),
        "premise: the agent sent orders"
    );
    assert!(
        orders.iter().all(|o| o.id.starts_with(PARTICIPANT_PREFIX)),
        "every order is labelled as a participant's, never as a recorded fill"
    );
    let bare = ReplaySession::new(model(), ticks.clone(), Vec::new(), Vec::new())?.run_to_end()?;
    assert!(
        bare.executions.is_empty(),
        "premise: the bare tape executes nothing"
    );
    let run = ReplaySession::new(model(), ticks, orders.clone(), Vec::new())?.run_to_end()?;
    assert_eq!(
        run.executions.len(),
        orders.len(),
        "every participant order resolved"
    );
    let filled: Vec<_> = run
        .executions
        .iter()
        .filter(|e| e.status == ExecutionStatus::Filled)
        .collect();
    assert!(!filled.is_empty(), "a momentum buy into a deep book fills");
    assert!(
        filled.iter().all(|e| e.fees.is_positive()),
        "fees are charged"
    );
    assert!(
        filled
            .iter()
            .all(|e| e.arrived_at.is_some_and(|arrived| arrived > e.decided_at)),
        "latency separates decision from arrival"
    );
    assert_ne!(
        run.input_digest, bare.input_digest,
        "the participants are part of the input"
    );
    assert_eq!(run.steps, bare.steps, "the recorded tape is untouched");
    Ok(())
}

/// R12-TICK-9036: an instrument's participants are the same whichever
/// instruments share the process, so the union of shards is the whole run.
///
/// Mutation: in `participant_orders` seed `generate_flow` with
/// `spec.seed.wrapping_add(paths.len() as u64)`, so the stream depends on how
/// many instruments share the replay: the run fails at
/// `assert_eq!(from_whole.len(), only_b.len())` (the draws differ).
#[test]
fn an_instruments_participants_do_not_depend_on_which_instruments_share_the_replay() -> Result<()> {
    let a = rising("obj-AAA", 12);
    let b = rising("obj-BBB", 12);
    let both: Vec<ReplayTick> = a.iter().chain(&b).cloned().collect();
    let spec = ParticipantSpec {
        seed: 11,
        calm_half_spread_bps: 5.0,
        agents: vec![
            CounterpartyAgent::momentum("trend", d(10), 2, 0.001).unwrap(),
            CounterpartyAgent::passive("flow", d(5), 0.5).unwrap(),
        ],
    };
    let whole = participant_orders(&spec, model(), &both)?;
    let only_b = participant_orders(&spec, model(), &b)?;
    assert!(
        whole.iter().any(|o| o.instrument == "obj-AAA") && !only_b.is_empty(),
        "premise: both instruments drew participant orders"
    );
    let from_whole: Vec<_> = whole.iter().filter(|o| o.instrument == "obj-BBB").collect();
    assert_eq!(from_whole.len(), only_b.len());
    for (x, y) in from_whole.iter().zip(&only_b) {
        assert_eq!(
            *x, y,
            "obj-BBB's participants changed when obj-AAA joined the replay"
        );
    }
    Ok(())
}

/// R12-TICK-9036: a spec with no agents, or no stated spread, is refused
/// instead of reporting that participants were simulated.
///
/// Mutation: delete the `agents.is_empty()` refusal in
/// `ParticipantSpec::validate`: the first assertion fails (an empty spec
/// returns no orders silently).
#[test]
fn a_participant_spec_with_no_agents_or_no_spread_is_refused() {
    let ticks = rising("obj-ACME", 4);
    let none = ParticipantSpec {
        seed: 1,
        calm_half_spread_bps: 5.0,
        agents: Vec::new(),
    };
    assert!(participant_orders(&none, model(), &ticks).is_err());
    let mut flat = momentum_spec();
    flat.calm_half_spread_bps = 0.0;
    assert!(participant_orders(&flat, model(), &ticks).is_err());
}
