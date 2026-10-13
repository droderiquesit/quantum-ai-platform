//! RM-P5-03: a multi-leg saga that cannot complete is compensated in reverse.
//!
//! `LegGroup` is the saga coordinator for a multi-leg trade (its module doc
//! names the compensation paths). A cycle whose later leg fails is unwound by
//! reversing every leg that filled, and the order those reversals go out in is
//! not cosmetic. In a cycle each leg spends what the leg before it delivered:
//! leg one buys BTC with dollars, leg two sells that BTC for euros, leg three
//! would have sold the euros for dollars. When leg three fails the group
//! holds euros and no BTC. Reversing leg two first buys the BTC back with
//! the euros it holds; only then does reversing leg one sell BTC the group
//! actually owns. Reversed the other way round, the first compensation sells
//! BTC the group no longer holds — a naked short opened by the recovery
//! path, held until the second compensation lands.
//!
//! `settle` matches reversals one by one and does not care about their
//! order, so nothing else in the crate pins it: a recovery that issued the
//! compensations forwards would settle, close and report itself flat.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::Decimal;
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_execution_engine::multileg::{Leg, LegGroup, Verdict};
use qip_execution_engine::order::{Fill, Order, OrderType, Side};

fn at(offset: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + offset)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("the fixture holds a well-formed decimal")
}

fn leg(order_id: &str, object: &str, side: Side, quantity: &str, price: &str) -> Order {
    Order::new(
        OrderId::from_string(order_id),
        ObjectId::from_string(object),
        side,
        d(quantity),
        OrderType::Market,
        d(price),
        "prop-cycle",
        vec!["hyp-cycle".to_string()],
        "platform",
        at(0),
    )
}

fn fill(order_id: &str, quantity: &str, price: &str) -> Fill {
    Fill {
        fill_id: FillId::from_string(format!("fill-{order_id}")),
        order_id: OrderId::from_string(order_id),
        at: at(1),
        quantity: d(quantity),
        price: d(price),
        costs: Decimal::ZERO,
        venue: "SIM".to_string(),
        simulated: true,
    }
}

/// Whether `order` is the reversal of `leg`: the same instrument, the other
/// side, and exactly what the leg filled.
fn reverses(order: &Order, leg: &Leg) -> bool {
    order.object_id == leg.order.object_id
        && order.side != leg.order.side
        && order.quantity == leg.filled
}

/// The failure this prevents is the naked short described in the module
/// doc: compensations issued in the order the legs were placed sell, first,
/// what an earlier compensation has not yet bought back.
#[test]
fn a_failed_third_leg_runs_the_compensations_of_the_first_two_in_reverse() {
    let mut group = LegGroup::new(
        "cycle-btc-eur-usd",
        vec![
            leg("leg-1", "BTC-USD", Side::Buy, "2", "60000"),
            leg("leg-2", "BTC-EUR", Side::Sell, "2", "55000"),
            leg("leg-3", "EUR-USD", Side::Sell, "110000", "1.09"),
        ],
        at(60),
        // Generous, so the deadline and not the risk bound is what ends it.
        d("1000000"),
    )
    .expect("a three-leg cycle assembles");
    group
        .record_fill(&fill("leg-1", "2", "60000"))
        .expect("leg one fills");
    group
        .record_fill(&fill("leg-2", "2", "55000"))
        .expect("leg two fills");
    // Leg three never fills: its venue refused it, and the deadline passes.

    // Premise: legs one and two filled in full, leg three filled nothing,
    // and the group's own verdict is to unwind rather than complete or
    // abandon. Without it a group with one filled leg — or none — would
    // produce a reversal list too short to have an order at all.
    assert_eq!(group.legs.len(), 3, "premise: a three-leg cycle");
    assert!(
        group.legs[0].is_complete() && group.legs[1].is_complete(),
        "premise: legs one and two filled in full"
    );
    assert_eq!(
        group.legs[2].filled,
        Decimal::ZERO,
        "premise: leg three filled nothing"
    );
    let verdict = group.assess(at(61));
    assert!(
        matches!(verdict, Verdict::Unwind { .. }),
        "premise: a cycle stranded after two legs is unwound, got {verdict:?}"
    );

    // The property. Each compensation is mapped to the leg it reverses
    // (1-based), and the sequence is compared whole: leg two, then leg one,
    // and nothing for leg three.
    let compensations = group.unwind_orders(at(61));
    let reversed: Vec<Option<usize>> = compensations
        .iter()
        .map(|order| {
            group
                .legs
                .iter()
                .position(|leg| reverses(order, leg))
                .map(|index| index + 1)
        })
        .collect();
    assert_eq!(
        reversed,
        vec![Some(2), Some(1)],
        "the compensations of a cycle stranded at leg three must run leg two, then leg one; \
         issued as {:?}",
        compensations
            .iter()
            .map(|order| order.order_id.as_str())
            .collect::<Vec<_>>()
    );
}
