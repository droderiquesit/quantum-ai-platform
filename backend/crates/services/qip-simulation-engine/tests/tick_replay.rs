//! Tick-level replay of recorded history with execution realism
//! (R12-TICK-2059, 2061, 2067, 2076, 2078).

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_simulation_engine::tick_replay::{
    ExecutionModel, ExecutionStatus, LegPlan, MultiLegPlan, OrderSide, ReplayOrder, ReplaySession,
    ReplayTick, replay,
};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn d(v: i64) -> Decimal {
    Decimal::from_int(v)
}

fn tick(instrument: &str, sequence: u64, ms: i64, body: MessageBody) -> ReplayTick {
    ReplayTick {
        message: MarketMessage::new(
            ObjectId::from_string(instrument),
            Origin::new(
                VenueId::new("XLON"),
                format!("feed-{instrument}"),
                0,
                sequence,
            ),
            body,
            at(ms),
            at(ms),
        ),
        knowable_at: at(ms),
    }
}

fn level(side: BookSide, price: i64, quantity: i64) -> MessageBody {
    MessageBody::LevelSet {
        side,
        price: d(price),
        quantity: d(quantity),
        order_count: None,
    }
}

fn trade(price: i64, quantity: i64) -> MessageBody {
    MessageBody::Trade {
        price: d(price),
        quantity: d(quantity),
        condition: TradeCondition::Regular,
        aggressor: None,
    }
}

fn model() -> ExecutionModel {
    ExecutionModel {
        latency: Duration::from_millis(10),
        fee_bps: 2.0,
        impact_bps_at_full_depth: 20.0,
        limit_ttl: Duration::from_secs(60),
    }
}

/// Bid 99 x10, asks 101 x5 and 102 x5; at 10ms the 101 ask shrinks to 3; a
/// trade prints at 20ms.
fn acme() -> Vec<ReplayTick> {
    vec![
        tick("obj-ACME", 1, 0, level(BookSide::Bid, 99, 10)),
        tick("obj-ACME", 2, 1, level(BookSide::Ask, 101, 5)),
        tick("obj-ACME", 3, 2, level(BookSide::Ask, 102, 5)),
        tick("obj-ACME", 4, 10, level(BookSide::Ask, 101, 3)),
        tick("obj-ACME", 5, 20, trade(100, 1)),
    ]
}

fn market_buy(id: &str, quantity: i64, decided_ms: i64) -> ReplayOrder {
    ReplayOrder {
        id: id.into(),
        instrument: "obj-ACME".into(),
        side: OrderSide::Buy,
        quantity: d(quantity),
        limit: None,
        decided_at: at(decided_ms),
    }
}

/// Mutation: arrival = `decided_at` (drop the latency) — the order meets
/// the 5-lot ask and fills all ten. Mutation: zero the fee or the impact in
/// `meet_book` — the respective assertion fails. Mutation: fill `requested`
/// rather than the sweep — the partial-fill assertion fails.
#[test]
fn a_decision_replayed_against_history_pays_latency_fees_impact_and_fills_only_what_was_offered()
-> Result<()> {
    let run = replay(model(), acme(), vec![market_buy("o-1", 10, 5)], Vec::new())?;
    assert_eq!(run.executions.len(), 1, "premise: the one order resolved");
    let execution = &run.executions[0];
    assert_eq!(
        execution.arrived_at,
        Some(at(15)),
        "the order meets the book its latency after the decision"
    );
    assert_eq!(
        execution.filled,
        d(8),
        "at arrival the 101 ask had shrunk to 3, so 3 + 5 is all the book offered"
    );
    assert_eq!(execution.status, ExecutionStatus::PartiallyFilled);
    assert!(
        execution.fees.is_positive(),
        "fees are charged: {}",
        execution.fees
    );
    assert!(execution.impact_cost.is_positive(), "impact is charged");
    // 3 @ 101 + 5 @ 102 = 813 before impact.
    assert!(
        execution.notional > d(813),
        "impact is added to a buy's cost: {}",
        execution.notional
    );
    assert_eq!(execution.arrival_mid, Some(d(100)));
    assert!(execution.slippage_bps().is_some_and(|s| s > 0.0));
    Ok(())
}

/// Mutation: in `work_resting`, fill on any print at the limit without
/// consuming `queue_ahead` first — the first trade fills the order.
#[test]
fn a_resting_limit_order_waits_behind_the_size_already_queued_at_its_price() -> Result<()> {
    let mut ticks = acme();
    ticks.push(tick("obj-ACME", 6, 30, trade(99, 6)));
    ticks.push(tick("obj-ACME", 7, 40, trade(99, 5)));
    let order = ReplayOrder {
        id: "bid-1".into(),
        instrument: "obj-ACME".into(),
        side: OrderSide::Buy,
        quantity: d(2),
        limit: Some(d(99)),
        decided_at: at(5),
    };
    let run = replay(model(), ticks.clone(), vec![order.clone()], Vec::new())?;
    let execution = &run.executions[0];
    assert_eq!(
        execution.queue_ahead_on_arrival,
        Some(d(10)),
        "ten lots were resting at 99 when the bid joined"
    );
    assert_eq!(
        execution.filled,
        d(1),
        "6 then 5 printed at 99: the first is all queue, one lot of the second reaches us"
    );
    assert_eq!(execution.status, ExecutionStatus::PartiallyFilled);

    ticks.push(tick("obj-ACME", 8, 50, trade(98, 1)));
    let run = replay(model(), ticks, vec![order], Vec::new())?;
    assert_eq!(
        run.executions[0].filled,
        d(2),
        "a print through the limit fills the rest"
    );
    assert_eq!(run.executions[0].status, ExecutionStatus::Filled);
    Ok(())
}

/// Mutation: open a span only when `sequence > previous + 2` — a hole one
/// message wide replays as wholly reliable, which is the defect
/// `qip_orderbook::replay` records as open.
#[test]
fn a_gap_one_message_wide_is_flagged_on_the_output_until_the_next_reset() -> Result<()> {
    let ticks = vec![
        tick("obj-ACME", 1, 0, level(BookSide::Bid, 99, 10)),
        tick("obj-ACME", 2, 1, level(BookSide::Ask, 101, 5)),
        tick("obj-ACME", 4, 2, level(BookSide::Ask, 101, 7)),
        tick("obj-ACME", 5, 3, level(BookSide::Bid, 98, 1)),
        tick(
            "obj-ACME",
            6,
            4,
            MessageBody::Reset {
                reason: "snapshot follows".into(),
            },
        ),
        tick("obj-ACME", 7, 5, level(BookSide::Bid, 99, 10)),
    ];
    let run = replay(model(), ticks, Vec::new(), Vec::new())?;
    assert_eq!(run.steps, 6, "premise: every message replayed");
    assert_eq!(run.spans.len(), 1, "one unreliable span: {:?}", run.spans);
    let span = &run.spans[0];
    assert_eq!(
        (span.first_sequence, span.last_sequence),
        (3, 3),
        "sequence 3 never arrived"
    );
    assert_eq!(span.until, Some(at(4)), "closed by the reset");
    assert_eq!(
        run.unreliable_steps,
        vec![2, 3],
        "the two messages inside the span are flagged"
    );
    Ok(())
}

/// Mutation: drop the `is_unreliable` refusal at the top of `meet_book` —
/// the order fills against a book the replay knows is wrong.
#[test]
fn no_fill_is_credited_against_a_book_inside_an_unreliable_span() -> Result<()> {
    let ticks = vec![
        tick("obj-ACME", 1, 0, level(BookSide::Bid, 99, 10)),
        tick("obj-ACME", 2, 1, level(BookSide::Ask, 101, 5)),
        tick("obj-ACME", 9, 2, level(BookSide::Ask, 101, 7)),
        tick("obj-ACME", 10, 30, trade(100, 1)),
    ];
    let run = replay(model(), ticks, vec![market_buy("o-1", 1, 5)], Vec::new())?;
    assert!(
        !run.unreliable_steps.is_empty(),
        "premise: the gap opened a span"
    );
    match &run.executions[0].status {
        ExecutionStatus::Rejected { reason } => assert!(reason.contains("unreliable"), "{reason}"),
        other => panic!("an order inside an unreliable span was {other:?}"),
    }
    assert!(run.executions[0].filled.is_zero());
    Ok(())
}

fn three_legs() -> MultiLegPlan {
    MultiLegPlan {
        id: "cycle-1".into(),
        decided_at: at(5),
        legs: vec![
            LegPlan {
                instrument: "obj-ACME".into(),
                side: OrderSide::Buy,
                quantity: d(2),
                limit: None,
                region: "europe-west2".into(),
            },
            LegPlan {
                instrument: "obj-BETA".into(),
                side: OrderSide::Sell,
                quantity: d(2),
                limit: None,
                region: "us-east4".into(),
            },
            LegPlan {
                instrument: "obj-GAMA".into(),
                side: OrderSide::Buy,
                quantity: d(2),
                limit: None,
                region: "asia-northeast1".into(),
            },
        ],
        leg_interval: Duration::from_millis(5),
    }
}

/// BETA has asks and no bids, so a sell of BETA finds nothing.
fn three_books() -> Vec<ReplayTick> {
    let mut ticks = acme();
    ticks.push(tick("obj-BETA", 1, 0, level(BookSide::Ask, 50, 10)));
    ticks.push(tick("obj-GAMA", 1, 0, level(BookSide::Ask, 20, 10)));
    ticks.push(tick("obj-GAMA", 2, 0, level(BookSide::Bid, 19, 10)));
    for (i, ms) in [60, 70, 80, 90].into_iter().enumerate() {
        ticks.push(tick("obj-GAMA", 3 + i as u64, ms, trade(19, 1)));
    }
    ticks
}

/// Mutation: in `resolve`, treat a leg that did not fill as complete and
/// send the next — the plan completes and nothing is unwound. Mutation:
/// skip scheduling unwinds — `unhedged` holds the bought ACME.
#[test]
fn a_three_leg_plan_whose_second_venue_fails_unwinds_the_first_leg_and_charges_the_cost()
-> Result<()> {
    let run = replay(model(), three_books(), Vec::new(), vec![three_legs()])?;
    let plan = &run.plans[0];
    assert_eq!(
        plan.legs.len(),
        2,
        "the third leg is never sent after the second fails"
    );
    assert_eq!(
        plan.legs[0].status,
        ExecutionStatus::Filled,
        "premise: the first leg filled"
    );
    assert!(matches!(
        plan.legs[1].status,
        ExecutionStatus::Rejected { .. }
    ));
    assert!(!plan.completed, "completion is not assumed");
    assert_eq!(plan.failed_leg, Some(1));
    assert_eq!(plan.unwinds.len(), 1, "the open ACME leg is flattened");
    assert_eq!(plan.unwinds[0].side, OrderSide::Sell);
    assert_eq!(plan.unwinds[0].filled, d(2));
    assert!(
        plan.unwind_pnl.is_negative(),
        "bought at the ask, sold at the bid, fees both ways: {}",
        plan.unwind_pnl
    );
    assert!(
        plan.unhedged.is_empty(),
        "nothing is left open: {:?}",
        plan.unhedged
    );
    assert!(
        plan.capital_by_region
            .get("europe-west2")
            .is_some_and(|c| c.is_positive())
    );
    Ok(())
}

/// Mutation: drop the `rolling digest` update or order ticks by event time
/// rather than knowable time in `sequence_ticks` with an input whose two
/// differ — the runs diverge. Mutation: omit `working` (or `plans`) from
/// `ReplayCheckpoint` — the resumed run differs from the whole one.
#[test]
fn one_history_replayed_twice_and_from_a_checkpoint_gives_identical_order_books_fills_and_flags()
-> Result<()> {
    let mut ticks = three_books();
    ticks.push(tick("obj-ACME", 12, 25, level(BookSide::Ask, 101, 9)));
    ticks.push(tick(
        "obj-ACME",
        13,
        26,
        MessageBody::Reset {
            reason: "rebuild".into(),
        },
    ));
    ticks.reverse();
    let orders = vec![
        market_buy("o-1", 4, 5),
        ReplayOrder {
            id: "bid-1".into(),
            instrument: "obj-GAMA".into(),
            side: OrderSide::Buy,
            quantity: d(3),
            limit: Some(d(19)),
            decided_at: at(1),
        },
    ];
    let first = replay(model(), ticks.clone(), orders.clone(), vec![three_legs()])?;
    let second = replay(model(), ticks.clone(), orders.clone(), vec![three_legs()])?;
    assert!(
        !first.spans.is_empty(),
        "premise: the history carries an integrity flag"
    );
    assert!(
        !first.executions.is_empty() && !first.plans.is_empty(),
        "premise: fills to compare"
    );
    assert_eq!(first, second, "the same history replays identically");

    let mut session =
        ReplaySession::new(model(), ticks.clone(), orders.clone(), vec![three_legs()])?;
    // Ten steps reach 26 ms: the resting bid decided at 1 ms has arrived
    // (11 ms) and waits behind the queue for prints that come at 60 ms, so
    // the checkpoint holds a working order. Taken at six steps (2 ms) it
    // held none, and omitting `working` from the checkpoint survived.
    for _ in 0..10 {
        assert!(session.step()?);
    }
    let checkpoint = session.checkpoint();
    assert_eq!(checkpoint.position(), 10);
    assert!(
        serde_json::to_value(&checkpoint)?["working"]
            .as_array()
            .is_some_and(|working| !working.is_empty()),
        "premise: the checkpoint is taken while an order is working"
    );
    let checkpoint: qip_simulation_engine::tick_replay::ReplayCheckpoint =
        serde_json::from_str(&serde_json::to_string(&checkpoint)?)?;
    let resumed = ReplaySession::resume(model(), ticks, orders, vec![three_legs()], &checkpoint)?
        .run_to_end()?;
    assert_eq!(
        resumed, first,
        "a replay resumed from a checkpoint reaches the same run"
    );
    Ok(())
}

/// Mutation: drop the input-digest comparison in `resume` — a checkpoint
/// splices into another history.
#[test]
fn a_checkpoint_is_refused_against_a_different_history() -> Result<()> {
    let mut session = ReplaySession::new(model(), acme(), Vec::new(), Vec::new())?;
    session.step()?;
    let checkpoint = session.checkpoint();
    let mut other = acme();
    other.pop();
    assert!(ReplaySession::resume(model(), other, Vec::new(), Vec::new(), &checkpoint).is_err());
    Ok(())
}

/// Mutation: remove the latency check from `ExecutionModel::validate`.
#[test]
fn a_zero_latency_or_a_single_leg_plan_is_refused() -> Result<()> {
    let instant = ExecutionModel {
        latency: Duration::ZERO,
        ..model()
    };
    assert!(replay(instant, acme(), Vec::new(), Vec::new()).is_err());
    let mut plan = three_legs();
    plan.legs.truncate(1);
    assert!(replay(model(), acme(), Vec::new(), vec![plan]).is_err());
    Ok(())
}
