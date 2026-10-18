//! R12-COVERAGE-7177: one tick replay that both queues a resting order behind
//! the size already at its price and unwinds a multi-leg plan whose third leg
//! finds no liquidity.
//!
//! Each half already has its own test inside `qip-simulation-engine`. This
//! suite holds the composite from outside the crate, through the public
//! `replay` a backtest or the deep brain's tick desk calls: a queue model that
//! only held with no plan in flight, or an unwind that only ran with no
//! resting order working, would pass both crate tests and fail here.

#![allow(clippy::panic_in_result_fn, clippy::expect_used)]

use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_simulation_engine::tick_replay::{
    ExecutionModel, ExecutionStatus, LegPlan, MultiLegPlan, OrderSide, ReplayOrder, ReplayTick,
    replay,
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

fn leg(instrument: &str, region: &str) -> LegPlan {
    LegPlan {
        instrument: instrument.into(),
        side: OrderSide::Buy,
        quantity: d(2),
        limit: None,
        region: region.into(),
    }
}

/// Mutation: rest the order with `queue_ahead: Decimal::ZERO` instead of the
/// size already at its price (a late order jumping the queue) — it fills on
/// the first print and the queue assertion fails. Mutation: in `resolve`,
/// treat every leg as complete (`let complete = true || ..`) — the third leg
/// is not treated as failed and the leg-status assertion fails.
#[test]
fn a_replayed_order_waits_its_turn_in_the_queue_while_a_plan_whose_third_leg_fails_is_unwound()
-> Result<()> {
    let ticks = vec![
        // ACME: ten lots bid at 99 before anyone joins; an ask to buy from.
        tick("obj-ACME", 1, 0, level(BookSide::Bid, 99, 10)),
        tick("obj-ACME", 2, 0, level(BookSide::Ask, 101, 10)),
        // BETA: both sides, so its leg fills and can be unwound.
        tick("obj-BETA", 1, 0, level(BookSide::Ask, 50, 10)),
        tick("obj-BETA", 2, 0, level(BookSide::Bid, 49, 10)),
        // GAMA: bids only. A buy leg arriving here finds nothing to take.
        tick("obj-GAMA", 1, 0, level(BookSide::Bid, 19, 10)),
        // Prints at 99 on ACME: six lots, then five.
        tick("obj-ACME", 3, 60, trade(99, 6)),
        tick("obj-ACME", 4, 70, trade(99, 5)),
        tick("obj-BETA", 3, 200, trade(49, 1)),
    ];
    let resting = ReplayOrder {
        id: "bid-1".into(),
        instrument: "obj-ACME".into(),
        side: OrderSide::Buy,
        quantity: d(2),
        limit: Some(d(99)),
        decided_at: at(1),
    };
    let plan = MultiLegPlan {
        id: "plan-1".into(),
        decided_at: at(2),
        legs: vec![
            leg("obj-ACME", "europe-west2"),
            leg("obj-BETA", "europe-west2"),
            leg("obj-GAMA", "us-east4"),
        ],
        leg_interval: Duration::from_millis(5),
        max_path_decay_bps: None,
        region_reservations: Default::default(),
    };
    let model = ExecutionModel {
        latency: Duration::from_millis(1),
        fee_bps: 1.0,
        impact_bps_at_full_depth: 10.0,
        limit_ttl: Duration::from_secs(60),
        message_budget: None,
    };
    let run = replay(model, ticks, vec![resting], vec![plan])?;

    // The queue: ten lots were ahead when the bid joined. The six-lot print
    // is all queue, and one lot of the five-lot print reaches the order.
    let execution = run
        .executions
        .iter()
        .find(|e| e.order_id == "bid-1")
        .expect("the resting order resolved");
    assert_eq!(execution.queue_ahead_on_arrival, Some(d(10)));
    assert_eq!(
        execution.filled,
        d(1),
        "a resting order filled ahead of the ten lots already queued at its price"
    );

    // The plan: two legs filled, the third found no ask, both open legs
    // are flattened and nothing is left unhedged.
    let outcome = &run.plans[0];
    assert_eq!(outcome.legs.len(), 3, "premise: all three legs were sent");
    assert_eq!(outcome.legs[0].status, ExecutionStatus::Filled);
    assert_eq!(outcome.legs[1].status, ExecutionStatus::Filled);
    assert!(
        matches!(outcome.legs[2].status, ExecutionStatus::Rejected { .. }),
        "the third leg was not refused: {:?}",
        outcome.legs[2].status
    );
    assert!(
        !outcome.completed,
        "a plan with a failed leg is not complete"
    );
    assert_eq!(outcome.failed_leg, Some(2));
    assert_eq!(outcome.unwinds.len(), 2, "both open legs are unwound");
    assert!(
        outcome.unwinds.iter().all(|u| u.side == OrderSide::Sell),
        "an unwind of a bought leg sells"
    );
    assert!(
        outcome.unhedged.is_empty(),
        "exposure left open: {:?}",
        outcome.unhedged
    );
    assert!(outcome.unwind_pnl.is_negative(), "{}", outcome.unwind_pnl);
    Ok(())
}
