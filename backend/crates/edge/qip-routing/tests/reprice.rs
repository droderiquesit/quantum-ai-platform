//! Tests for the repricer.
//!
//! The one that matters most is mutation-grade on the replace/cancel ordering:
//! it first exhibits the race — a replacement sent before the cancel is
//! acknowledged leaves two live orders for one intention, and both fill —
//! and then shows both mechanisms that make it impossible here: the repricer
//! refuses to mint a replacement for a child the venue has not finished with,
//! and the parent's conservation arithmetic refuses to attach one whose
//! quantity is still working at a venue.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_routing::children::{ChildOrder, ParentOrder};
use qip_routing::ordertype::{PegReference, RoutedOrderType, Touch};
use qip_routing::reprice::{HoldReason, RepriceDecision, RepricePolicy, Repricer, ThrottleScope};

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("TKN")
}

fn venue() -> VenueId {
    VenueId::new("NYX")
}

fn touch(bid: &str, ask: &str) -> Touch {
    Touch {
        bid: d(bid),
        ask: d(ask),
    }
}

/// Tick 0.01; stale at 5 ticks or 50 bps, whichever binds first.
fn policy() -> RepricePolicy {
    RepricePolicy::new(d("0.01"), 5, 50.0)
}

fn repricer() -> Repricer {
    Repricer::new(policy()).expect("the fixture policy is valid")
}

/// A parent with one working limit child resting at `price`.
fn parent_with_resting_buy(quantity: &str, price: &str) -> Result<(ParentOrder, String)> {
    let mut parent = ParentOrder::new(
        OrderId::from_string("ord-parent"),
        object(),
        BookSide::Ask,
        d(quantity),
    )?;
    let id = attach_resting(&mut parent, quantity, price)?;
    Ok((parent, id))
}

/// Attach a further working limit child for `quantity` at `price`.
fn attach_resting(parent: &mut ParentOrder, quantity: &str, price: &str) -> Result<String> {
    let id = parent.next_client_id();
    let child = ChildOrder::new(
        id.clone(),
        parent.order_id.clone(),
        venue(),
        parent.object_id.clone(),
        parent.side,
        d(quantity),
        RoutedOrderType::Limit { price: d(price) },
    )?;
    parent.attach(child)?;
    parent
        .child_mut(&id)
        .ok_or_else(|| Error::not_found("the child just attached"))?
        .mark_working()?;
    Ok(id)
}

fn expect_cancel(decision: RepriceDecision) -> String {
    match decision {
        RepriceDecision::CancelAndReplace { client_id, .. } => client_id,
        other => panic!("expected a cancel-and-replace, got {other:?}"),
    }
}

// --- the race, and the guard ------------------------------------------------

#[test]
fn without_the_ack_gate_a_replace_racing_its_cancel_fills_on_both() -> Result<()> {
    // The race, exhibited before the guard is shown: one intention to buy
    // 100, one resting child, and a "replacement" sent while the cancel is
    // still in flight. At the venue both are simply live orders; a market
    // that trades through both prices fills both.
    let parent_id = OrderId::from_string("ord-race");
    let mut original = ChildOrder::new(
        "ord-race-c1",
        parent_id.clone(),
        venue(),
        object(),
        BookSide::Ask,
        d("100"),
        RoutedOrderType::Limit { price: d("99.90") },
    )?;
    original.mark_working()?;
    let mut premature = ChildOrder::new(
        "ord-race-c2",
        parent_id.clone(),
        venue(),
        object(),
        BookSide::Ask,
        d("100"),
        RoutedOrderType::Limit { price: d("100.00") },
    )?;
    premature.mark_working()?;

    original.apply_fill(d("100"), d("99.90"))?;
    premature.apply_fill(d("100"), d("100.00"))?;
    assert_eq!(
        original.filled() + premature.filled(),
        d("200"),
        "the premise of this test: one 100-share intention became a 200-share position, \
         which is exactly the doubled fill the acknowledgement gate exists to prevent"
    );

    // First mechanism: the parent's conservation arithmetic. With the
    // original still working, its 100 shares are spoken for, and a premature
    // replacement cannot be attached at all.
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let early = ChildOrder::new(
        parent.next_client_id(),
        parent.order_id.clone(),
        venue(),
        object(),
        BookSide::Ask,
        d("100"),
        RoutedOrderType::Limit { price: d("100.00") },
    )?;
    let refused = parent.attach(early);
    assert!(
        refused.is_err(),
        "attaching a replacement while the original works must be refused"
    );
    assert!(parent.child(&id).is_some_and(|c| !c.state.is_terminal()));
    Ok(())
}

#[test]
fn the_repricer_refuses_to_mint_a_replacement_until_the_cancel_is_acknowledged() -> Result<()> {
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let market = touch("100.00", "100.02");

    let child = parent.child(&id).expect("the resting child");
    let cancel_id = expect_cancel(repricer.consider(child, market, at()));
    assert_eq!(cancel_id, id);

    // Second mechanism: the venue has not answered the cancel, and the
    // repricer will not construct the replacement on hope.
    let refused = repricer.on_cancel_acknowledged(&mut parent, &id, market);
    match refused {
        Err(Error::Guard(message)) => {
            assert!(message.contains("both fill"), "message was: {message}");
        }
        other => panic!("expected a guard refusal before the acknowledgement, got {other:?}"),
    }

    // The venue acknowledges; now — and only now — the replacement exists.
    parent
        .child_mut(&id)
        .expect("the resting child")
        .cancel("stale: repriced")?;
    let replacement = repricer
        .on_cancel_acknowledged(&mut parent, &id, market)?
        .expect("a full remainder must yield a replacement");
    assert_eq!(replacement.quantity, d("100"));
    assert_eq!(
        replacement.order_type,
        RoutedOrderType::Limit { price: d("100.00") },
        "the replacement rejoins the touch passively — it rests on the bid, never crosses"
    );
    parent.reconcile()?;
    Ok(())
}

#[test]
fn the_replacement_carries_a_fresh_identity_not_the_cancelled_orders() -> Result<()> {
    // Downstream, `qip-brokers` derives its idempotency key from the order id
    // and terms; a replacement reusing the old id would be deduped away by an
    // honouring venue as a retry of the order that was just cancelled. So the
    // fresh client id is not cosmetic — it is what makes the replacement a
    // new order on the wire.
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let market = touch("100.00", "100.02");
    expect_cancel(repricer.consider(parent.child(&id).expect("child"), market, at()));
    parent.child_mut(&id).expect("child").cancel("stale")?;
    let replacement = repricer
        .on_cancel_acknowledged(&mut parent, &id, market)?
        .expect("replacement");
    assert_ne!(
        replacement.client_id, id,
        "a replacement must be a new order"
    );
    assert!(
        parent.child(&replacement.client_id).is_some(),
        "the replacement is attached to the parent under its own identity"
    );
    Ok(())
}

// --- partial fills ----------------------------------------------------------

#[test]
fn only_the_remainder_is_repriced_and_the_booked_fill_is_never_resent() -> Result<()> {
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let market = touch("100.00", "100.02");

    // 40 filled before the market ran away.
    parent
        .child_mut(&id)
        .expect("child")
        .apply_fill(d("40"), d("99.90"))?;
    expect_cancel(repricer.consider(parent.child(&id).expect("child"), market, at()));
    parent.child_mut(&id).expect("child").cancel("stale")?;
    let replacement = repricer
        .on_cancel_acknowledged(&mut parent, &id, market)?
        .expect("replacement");

    assert_eq!(
        replacement.quantity,
        d("60"),
        "the replacement is the remainder; re-sending the booked 40 would invent a position"
    );
    assert_eq!(
        parent.filled(),
        d("40"),
        "the fill stays booked exactly once"
    );
    parent.reconcile()?;
    assert!(
        parent.accounts_for_every_share(),
        "40 filled + 60 working must add back to the 100 intended"
    );
    Ok(())
}

#[test]
fn an_order_that_filled_while_its_cancel_was_in_flight_yields_no_replacement() -> Result<()> {
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let market = touch("100.00", "100.02");
    expect_cancel(repricer.consider(parent.child(&id).expect("child"), market, at()));

    // The venue's answer to the cancel: it had already filled in full.
    parent
        .child_mut(&id)
        .expect("child")
        .apply_fill(d("100"), d("99.90"))?;
    let replacement = repricer.on_cancel_acknowledged(&mut parent, &id, market)?;
    assert!(
        replacement.is_none(),
        "a completed intention has no remainder, so nothing may be re-sent"
    );
    assert_eq!(parent.filled(), d("100"));
    Ok(())
}

// --- staleness: distance from the touch -------------------------------------

#[test]
fn a_resting_buy_is_stale_when_the_bid_rises_past_the_tick_threshold() -> Result<()> {
    // Tick 0.01, threshold 5 ticks. A buy resting at 99.96 with the bid at
    // 100.00 is 4 ticks behind: fresh. At 99.95 it is exactly 5: stale.
    let (parent, _) = parent_with_resting_buy("100", "99.96")?;
    let fresh_child = parent.children().next().expect("child");
    let mut repricer = repricer();
    match repricer.consider(fresh_child, touch("100.00", "100.02"), at()) {
        RepriceDecision::Hold { reason, .. } => assert_eq!(reason, HoldReason::Fresh),
        other => panic!("4 ticks behind must hold, got {other:?}"),
    }

    let (parent, id) = parent_with_resting_buy("100", "99.95")?;
    let decision = repricer.consider(
        parent.child(&id).expect("child"),
        touch("100.00", "100.02"),
        at(),
    );
    match decision {
        RepriceDecision::CancelAndReplace { drift, .. } => {
            assert!(
                (drift.ticks_f64 - 5.0).abs() < 1e-9,
                "drift was {}",
                drift.ticks_f64
            );
        }
        other => panic!("5 ticks behind must reprice, got {other:?}"),
    }
    Ok(())
}

#[test]
fn an_order_at_or_ahead_of_the_touch_is_never_repriced() -> Result<()> {
    // At the touch: drift zero. Ahead of it (the resting buy IS the best
    // bid, above the rest of the book): drift negative. Pulling either
    // spends queue priority to buy nothing.
    let mut repricer = repricer();
    for price in ["100.00", "100.05"] {
        let (parent, id) = parent_with_resting_buy("100", price)?;
        match repricer.consider(
            parent.child(&id).expect("child"),
            touch("100.00", "100.06"),
            at(),
        ) {
            RepriceDecision::Hold { reason, .. } => assert_eq!(reason, HoldReason::Fresh),
            other => panic!("an order at {price} against a 100.00 bid must hold, got {other:?}"),
        }
    }
    Ok(())
}

#[test]
fn a_resting_sell_falls_behind_when_the_ask_drops_and_rejoins_the_ask() -> Result<()> {
    // The sell side of the same arithmetic: a sell rests on the ask and is
    // behind when its price sits above it.
    let mut parent = ParentOrder::new(
        OrderId::from_string("ord-sell"),
        object(),
        BookSide::Bid,
        d("100"),
    )?;
    let id = attach_resting(&mut parent, "100", "100.20")?;
    let mut repricer = repricer();
    let market = touch("99.98", "100.02");

    let decision = repricer.consider(parent.child(&id).expect("child"), market, at());
    let cancel_id = expect_cancel(decision);
    parent
        .child_mut(&cancel_id)
        .expect("child")
        .cancel("stale")?;
    let replacement = repricer
        .on_cancel_acknowledged(&mut parent, &cancel_id, market)?
        .expect("replacement");
    assert_eq!(
        replacement.order_type,
        RoutedOrderType::Limit { price: d("100.02") },
        "a sell's replacement rests on the ask"
    );
    Ok(())
}

#[test]
fn the_basis_point_threshold_binds_even_when_the_tick_threshold_does_not() -> Result<()> {
    // A coarse tick: 1.00, threshold 100 ticks — unreachable here. The 50 bps
    // threshold still catches a resting buy 0.60 behind a 100.00 bid (60 bps).
    let coarse = RepricePolicy::new(d("1"), 100, 50.0);
    let mut repricer = Repricer::new(coarse)?;
    let (parent, id) = parent_with_resting_buy("100", "99.40")?;
    let decision = repricer.consider(
        parent.child(&id).expect("child"),
        touch("100.00", "100.02"),
        at(),
    );
    match decision {
        RepriceDecision::CancelAndReplace { drift, .. } => {
            assert!(
                (drift.bps_f64 - 60.0).abs() < 1e-6,
                "drift was {} bps",
                drift.bps_f64
            );
            assert!(drift.ticks_f64 < 1.0, "the tick threshold did not bind");
        }
        other => panic!("60 bps behind must reprice under a 50 bps threshold, got {other:?}"),
    }
    Ok(())
}

#[test]
fn pegs_and_unpriced_orders_are_not_the_repricers_business() -> Result<()> {
    // A peg follows the book at the venue; repricing it would cancel an order
    // that was never stale.
    let mut parent = ParentOrder::new(
        OrderId::from_string("ord-peg"),
        object(),
        BookSide::Ask,
        d("100"),
    )?;
    let id = parent.next_client_id();
    let child = ChildOrder::new(
        id.clone(),
        parent.order_id.clone(),
        venue(),
        object(),
        BookSide::Ask,
        d("100"),
        RoutedOrderType::Peg {
            reference: PegReference::Mid,
            offset: Decimal::ZERO,
        },
    )?;
    parent.attach(child)?;
    parent.child_mut(&id).expect("child").mark_working()?;

    let mut repricer = repricer();
    match repricer.consider(
        parent.child(&id).expect("child"),
        touch("100.00", "100.02"),
        at(),
    ) {
        RepriceDecision::Hold { reason, .. } => assert_eq!(reason, HoldReason::NotRepriceable),
        other => panic!("a peg must not be repriced, got {other:?}"),
    }
    Ok(())
}

// --- budgets: the repricer must not chase ------------------------------------

#[test]
fn the_per_order_budget_stops_a_chase_and_names_the_failure() -> Result<()> {
    // A market that runs away from a resting buy, requote after requote. The
    // default budget is 3 per order; the fourth attempt must be refused as
    // the self-inflicted throttling it is, not silently skipped.
    let (mut parent, first) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let mut resting = first;
    let mut bid = d("100.00");
    let mut now = at();

    for round in 0..3 {
        let market = Touch {
            bid,
            ask: bid + d("0.02"),
        };
        let decision = repricer.consider(parent.child(&resting).expect("child"), market, now);
        let cancel_id = expect_cancel(decision);
        assert_eq!(cancel_id, resting, "round {round}");
        parent
            .child_mut(&cancel_id)
            .expect("child")
            .cancel("stale")?;
        let replacement = repricer
            .on_cancel_acknowledged(&mut parent, &cancel_id, market)?
            .expect("replacement");
        resting = replacement.client_id.clone();
        // The market keeps running.
        bid += d("0.10");
        now = now.saturating_add(Duration::from_secs(1));
    }

    let market = Touch {
        bid,
        ask: bid + d("0.02"),
    };
    let fourth = repricer.consider(parent.child(&resting).expect("child"), market, now);
    match fourth {
        RepriceDecision::Throttled {
            scope,
            used,
            budget,
            detail,
        } => {
            assert_eq!(scope, ThrottleScope::Order);
            assert_eq!((used, budget), (3, 3));
            assert!(
                detail.contains("self-inflicted throttling"),
                "the failure must be named for what it is; detail was: {detail}"
            );
            assert!(detail.contains("chasing"), "detail was: {detail}");
        }
        other => panic!("the fourth requote must be throttled, got {other:?}"),
    }
    assert_eq!(repricer.spent_by(&parent.order_id), 3);
    Ok(())
}

#[test]
fn the_per_instrument_budget_spans_orders_and_refills_on_the_window() -> Result<()> {
    // Two requotes per instrument per 60s window, across as many orders as it
    // takes. Two distinct parents spend the window; a third order in the same
    // instrument is throttled; the next window refills.
    let policy = policy()
        .with_order_budget(10)
        .with_instrument_budget(2, Duration::from_secs(60));
    let mut repricer = Repricer::new(policy)?;
    let market = touch("100.00", "100.02");
    let base = at().floor_to(Duration::from_secs(60));

    let spend_one =
        |name: &str, when: Timestamp, repricer: &mut Repricer| -> Result<RepriceDecision> {
            let mut parent = ParentOrder::new(
                OrderId::from_string(name),
                object(),
                BookSide::Ask,
                d("100"),
            )?;
            let id = attach_resting(&mut parent, "100", "99.90")?;
            Ok(repricer.consider(parent.child(&id).expect("child"), market, when))
        };

    expect_cancel(spend_one("ord-a", base, &mut repricer)?);
    expect_cancel(spend_one(
        "ord-b",
        base.saturating_add(Duration::from_secs(10)),
        &mut repricer,
    )?);
    let third = spend_one(
        "ord-c",
        base.saturating_add(Duration::from_secs(20)),
        &mut repricer,
    )?;
    match third {
        RepriceDecision::Throttled { scope, .. } => assert_eq!(scope, ThrottleScope::Instrument),
        other => panic!("the third requote in the window must be throttled, got {other:?}"),
    }

    // The next window: the budget refills and the same instrument may requote.
    let next_window = base.saturating_add(Duration::from_secs(60));
    expect_cancel(spend_one("ord-d", next_window, &mut repricer)?);
    Ok(())
}

#[test]
fn an_abandoned_cancel_clears_the_flight_but_keeps_the_budget_spent() -> Result<()> {
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let market = touch("100.00", "100.02");
    expect_cancel(repricer.consider(parent.child(&id).expect("child"), market, at()));
    assert_eq!(repricer.pending().len(), 1);

    // The cancel was refused by the venue; the instruction still happened.
    let abandoned = repricer.abandon(&id);
    assert!(abandoned.is_some());
    assert!(repricer.pending().is_empty());
    assert_eq!(
        repricer.spent_by(&parent.order_id),
        1,
        "chasing is measured in instructions sent, not in instructions that worked"
    );

    // And a replacement can no longer be minted from the abandoned flight.
    parent
        .child_mut(&id)
        .expect("child")
        .cancel("late cancel")?;
    let refused = repricer.on_cancel_acknowledged(&mut parent, &id, market);
    assert!(matches!(refused, Err(Error::Invalid(_))));
    Ok(())
}

// --- discipline: one flight per child, no invented flights -------------------

#[test]
fn a_second_consideration_while_the_cancel_is_in_flight_holds() -> Result<()> {
    let (parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    let market = touch("100.00", "100.02");
    expect_cancel(repricer.consider(parent.child(&id).expect("child"), market, at()));
    match repricer.consider(parent.child(&id).expect("child"), market, at()) {
        RepriceDecision::Hold { reason, .. } => assert_eq!(reason, HoldReason::CancelInFlight),
        other => panic!("a child with a cancel in flight must hold, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_replacement_without_a_requested_cancel_is_refused() -> Result<()> {
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    parent
        .child_mut(&id)
        .expect("child")
        .cancel("cancelled by someone else")?;
    let mut repricer = repricer();
    let refused = repricer.on_cancel_acknowledged(&mut parent, &id, touch("100.00", "100.02"));
    match refused {
        Err(Error::Invalid(message)) => {
            assert!(
                message.contains("no cancel was requested"),
                "message was: {message}"
            );
        }
        other => panic!("expected an invalid refusal, got {other:?}"),
    }
    Ok(())
}

#[test]
fn the_replacement_is_priced_against_the_market_at_acknowledgement_time() -> Result<()> {
    // The market keeps moving while the cancel is in flight. The replacement
    // rests at the touch that exists when it is minted — the market it will
    // live in, not the one that made its predecessor stale.
    let (mut parent, id) = parent_with_resting_buy("100", "99.90")?;
    let mut repricer = repricer();
    expect_cancel(repricer.consider(
        parent.child(&id).expect("child"),
        touch("100.00", "100.02"),
        at(),
    ));
    parent.child_mut(&id).expect("child").cancel("stale")?;
    let replacement = repricer
        .on_cancel_acknowledged(&mut parent, &id, touch("100.30", "100.32"))?
        .expect("replacement");
    assert_eq!(
        replacement.order_type,
        RoutedOrderType::Limit { price: d("100.30") }
    );
    Ok(())
}

// --- determinism -------------------------------------------------------------

#[test]
fn the_same_book_sequence_produces_the_same_decisions() -> Result<()> {
    // Two repricers under the same policy fed the identical sequence of
    // children, touches and timestamps must agree on every decision, to the
    // digit — repricing is a function of the inputs, not of anything either
    // instance did on the side.
    let sequence: Vec<(&str, &str, &str, i64)> = vec![
        ("99.96", "100.00", "100.02", 0),
        ("99.90", "100.00", "100.02", 1),
        ("99.90", "100.10", "100.12", 2),
        ("100.00", "100.00", "100.02", 3),
        ("99.50", "100.00", "100.02", 4),
    ];
    let mut first = repricer();
    let mut second = repricer();
    for (round, (price, bid, ask, offset)) in sequence.into_iter().enumerate() {
        let name = format!("ord-seq-{round}");
        let build = |suffix: &str| -> Result<(ParentOrder, String)> {
            let mut parent = ParentOrder::new(
                OrderId::from_string(format!("{name}{suffix}")),
                object(),
                BookSide::Ask,
                d("100"),
            )?;
            let id = attach_resting(&mut parent, "100", price)?;
            Ok((parent, id))
        };
        // Identical inputs (the parent id differs only by nothing — both use
        // the same name so the order-budget keys match too).
        let (parent_a, id_a) = build("")?;
        let (parent_b, id_b) = build("")?;
        let when = at().saturating_add(Duration::from_secs(offset));
        let market = touch(bid, ask);
        let decision_a = first.consider(parent_a.child(&id_a).expect("child"), market, when);
        let decision_b = second.consider(parent_b.child(&id_b).expect("child"), market, when);
        assert_eq!(decision_a, decision_b, "round {round} diverged");
    }
    Ok(())
}

// --- policy validation --------------------------------------------------------

#[test]
fn a_policy_that_would_chase_or_do_nothing_is_refused_at_construction() {
    // Zero ticks of tolerated drift is chasing by construction.
    assert!(Repricer::new(RepricePolicy::new(d("0.01"), 0, 50.0)).is_err());
    // A zero tick makes "ticks" meaningless.
    assert!(Repricer::new(RepricePolicy::new(d("0"), 5, 50.0)).is_err());
    // A zero budget is repricing disabled by stealth.
    assert!(Repricer::new(policy().with_order_budget(0)).is_err());
    // A negative bps threshold is not a threshold.
    assert!(Repricer::new(RepricePolicy::new(d("0.01"), 5, -1.0)).is_err());
    // The fixture itself is valid, so the refusals above mean something.
    assert!(Repricer::new(policy()).is_ok());
}
