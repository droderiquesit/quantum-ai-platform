//! Tests for the routing path.
//!
//! Two of these matter more than the rest. The first is that a split adds back
//! up to the parent, exactly, however the lot sizes fall — a router that leaks
//! a share on a rounding boundary produces a position nobody ordered, rarely
//! enough that it is found by somebody else. The second is that the venue with
//! the best quoted price is not automatically the venue that gets the order,
//! because it usually is not.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use qip_routing::children::{ChildOrder, ChildState, ParentOrder};
use qip_routing::gateway::{
    Gateway, GatewayEvent, GatewaySettings, NativeGateway, NativeGatewayConfig, SimulatedGateway,
};
use qip_routing::health::{HealthPolicy, HealthTracker, HealthVerdict};
use qip_routing::ordertype::{OrderTypeKind, RoutedOrderType, Touch, Urgency, select_order_type};
use qip_routing::router::{
    ExclusionReason, Router, RouterSettings, RoutingRequest, VenueCandidate,
};
use qip_routing::venue::{FeeSchedule, FeeTier, Liquidity, VenueProfile};

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn object() -> ObjectId {
    ObjectId::from_string("TKN")
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn parent_id() -> OrderId {
    OrderId::from_string("ord-parent")
}

fn book(venue_name: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBook {
    OrderBook::from_levels(
        object(),
        venue_name,
        at(),
        bids.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
        asks.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
    )
}

fn profile(name: &str, maker_bps_f64: f64, taker_bps_f64: f64) -> VenueProfile {
    VenueProfile::listed(venue(name), maker_bps_f64, taker_bps_f64)
        .with_sizes(Decimal::from_raw(1_000_000), Decimal::from_raw(1_000_000))
}

fn candidate(
    name: &str,
    maker_bps_f64: f64,
    taker_bps_f64: f64,
    bids: &[(&str, &str)],
    asks: &[(&str, &str)],
) -> VenueCandidate {
    VenueCandidate::new(
        profile(name, maker_bps_f64, taker_bps_f64),
        VenueStatus::Open,
        book(name, bids, asks),
    )
}

fn buy(quantity: &str, urgency: Urgency) -> RoutingRequest {
    RoutingRequest::new(parent_id(), object(), BookSide::Ask, d(quantity), urgency)
}

fn sell(quantity: &str, urgency: Urgency) -> RoutingRequest {
    RoutingRequest::new(parent_id(), object(), BookSide::Bid, d(quantity), urgency)
}

#[test]
fn routing_picks_the_best_net_cost_and_not_the_best_quote() -> Result<()> {
    // One venue is a basis point better on the screen and twenty-eight worse
    // once its taker fee is paid. The screen is what a naive router compares.
    let candidates = [
        candidate(
            "CHEAP-QUOTE",
            -1.0,
            30.0,
            &[("99.90", "100000")],
            &[("100.00", "100000")],
        ),
        candidate(
            "BETTER-NET",
            0.0,
            2.0,
            &[("99.95", "100000")],
            &[("100.10", "100000")],
        ),
    ];
    let decision = Router::default().route(
        &buy("100", Urgency::Normal),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;

    assert_eq!(decision.slices.len(), 1);
    let slice = &decision.slices[0];
    assert_eq!(slice.venue, venue("BETTER-NET"));
    assert_eq!(slice.quoted_price, d("100.10"), "and it quotes worse");
    assert!(
        slice.effective_price < d("100.30"),
        "but nets better than the 100.30 the other one would have cost"
    );
    assert!(
        decision
            .notes
            .iter()
            .any(|note| note.contains("CHEAP-QUOTE")),
        "the venue that was passed over is still priced in the record"
    );
    Ok(())
}

#[test]
fn a_maker_rebate_can_beat_a_better_quote() -> Result<()> {
    // Patient, so both orders rest and both pay maker rates. One venue pays the
    // order to be there.
    let candidates = [
        candidate(
            "NO-REBATE",
            2.0,
            3.0,
            &[("100.00", "100000")],
            &[("100.20", "100000")],
        ),
        candidate(
            "REBATE",
            -1.0,
            3.0,
            &[("99.99", "100000")],
            &[("100.19", "100000")],
        ),
    ];
    let decision = Router::default().route(
        &buy("100", Urgency::Patient),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;

    assert_eq!(decision.slices.len(), 1);
    assert_eq!(decision.slices[0].venue, venue("REBATE"));
    assert!(
        decision.slices[0].fee < Decimal::ZERO,
        "a rebate is a negative fee"
    );
    Ok(())
}

#[test]
fn a_sale_is_routed_to_the_venue_that_pays_most_after_fees() -> Result<()> {
    let candidates = [
        candidate(
            "HIGH-BID",
            -1.0,
            30.0,
            &[("100.00", "100000")],
            &[("100.10", "100000")],
        ),
        candidate(
            "LOW-FEE",
            0.0,
            2.0,
            &[("99.95", "100000")],
            &[("100.05", "100000")],
        ),
    ];
    let decision = Router::default().route(
        &sell("100", Urgency::Normal),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;

    assert_eq!(decision.slices.len(), 1);
    assert_eq!(
        decision.slices[0].venue,
        venue("LOW-FEE"),
        "99.95 less 2bp beats 100.00 less 30bp"
    );
    Ok(())
}

#[test]
fn a_split_adds_back_up_to_the_parent_exactly() -> Result<()> {
    // Shallow tops of book, so the order has to be spread and the arithmetic
    // has somewhere to go wrong.
    let candidates = [
        candidate(
            "VENUE-A",
            0.0,
            2.0,
            &[("99.90", "1000")],
            &[("100.00", "30"), ("100.50", "1000")],
        ),
        candidate(
            "VENUE-B",
            0.0,
            2.0,
            &[("99.85", "1000")],
            &[("100.05", "30"), ("100.40", "1000")],
        ),
    ];
    let decision = Router::default().route(
        &buy("100", Urgency::Immediate),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;

    assert!(
        decision.is_split(),
        "no single venue is cheapest throughout"
    );
    assert_eq!(decision.routed(), d("100"));
    assert_eq!(decision.unrouted, Decimal::ZERO);
    assert!(decision.accounts_for_every_share());
    decision.validate()?;
    Ok(())
}

#[test]
fn lot_rounding_reports_the_remainder_rather_than_losing_it() -> Result<()> {
    // Whole lots of ten against a request of twenty-five: five shares cannot be
    // placed anywhere, and they are said out loud rather than rounded away.
    let mut lumpy = candidate(
        "LUMPY",
        0.0,
        2.0,
        &[("99.90", "1000")],
        &[("100.00", "1000")],
    );
    lumpy.profile = lumpy
        .profile
        .with_sizes(Decimal::from_int(10), Decimal::from_int(10));

    let decision = Router::default().route(
        &buy("25", Urgency::Immediate),
        &[lumpy],
        &HealthTracker::default(),
        at(),
    )?;

    assert_eq!(decision.routed(), d("20"));
    assert_eq!(decision.unrouted, d("5"));
    assert_eq!(
        decision.routed() + decision.unrouted,
        decision.requested,
        "nothing is created and nothing is lost"
    );
    assert!(decision.notes.iter().any(|note| note.contains("unrouted")));
    Ok(())
}

#[test]
fn a_halted_or_unreachable_venue_never_receives_an_order() -> Result<()> {
    let mut halted = candidate(
        "HALTED",
        0.0,
        0.0,
        &[("99.99", "100000")],
        &[("100.00", "100000")],
    );
    halted.status = VenueStatus::Halted;
    let mut unreachable = candidate(
        "UNREACHABLE",
        0.0,
        0.0,
        &[("99.98", "100000")],
        &[("100.01", "100000")],
    );
    unreachable.status = VenueStatus::Unreachable;
    let open = candidate(
        "OPEN",
        0.0,
        20.0,
        &[("99.00", "100000")],
        &[("101.00", "100000")],
    );

    let decision = Router::default().route(
        &buy("100", Urgency::Normal),
        &[halted, unreachable, open],
        &HealthTracker::default(),
        at(),
    )?;

    assert_eq!(decision.venues(), vec![&venue("OPEN")]);
    assert_eq!(
        decision
            .exclusions
            .iter()
            .filter(|e| e.reason == ExclusionReason::NotAccepting)
            .count(),
        2,
        "both are excluded, and both say why"
    );
    Ok(())
}

#[test]
fn a_venue_with_a_high_reject_rate_is_deprioritised() -> Result<()> {
    // Identical venues, and the degrading one is named so that it would win any
    // tie. Only its behaviour can move the order.
    let candidates = [
        candidate(
            "AAA",
            0.0,
            2.0,
            &[("99.90", "100000")],
            &[("100.00", "100000")],
        ),
        candidate(
            "BBB",
            0.0,
            2.0,
            &[("99.90", "100000")],
            &[("100.00", "100000")],
        ),
    ];

    let clean = Router::default().route(
        &buy("100", Urgency::Normal),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;
    assert_eq!(
        clean.slices[0].venue,
        venue("AAA"),
        "the tie-break, unchanged"
    );

    let mut health = HealthTracker::default();
    for _ in 0..20 {
        health.record_sent(&venue("AAA"));
    }
    health.record_reject(&venue("AAA"), at());
    let decision =
        Router::default().route(&buy("100", Urgency::Normal), &candidates, &health, at())?;

    assert_eq!(
        decision.slices[0].venue,
        venue("BBB"),
        "one reject in twenty is enough to lose a tie"
    );
    assert!(decision.slices[0].health_cost <= Decimal::ZERO);
    Ok(())
}

#[test]
fn a_venue_rejecting_most_of_its_orders_is_taken_out_of_rotation() -> Result<()> {
    let candidates = [
        candidate(
            "AAA",
            0.0,
            2.0,
            &[("99.90", "100000")],
            &[("100.00", "100000")],
        ),
        candidate(
            "BBB",
            0.0,
            2.0,
            &[("99.90", "100000")],
            &[("100.00", "100000")],
        ),
    ];
    let mut health = HealthTracker::default();
    for _ in 0..20 {
        health.record_sent(&venue("AAA"));
    }
    for _ in 0..10 {
        health.record_reject(&venue("AAA"), at());
    }

    let assessment = health.assess(&venue("AAA"), Duration::from_millis(5), at());
    assert!(matches!(
        assessment.verdict,
        HealthVerdict::Quarantined { .. }
    ));
    assert!(!assessment.verdict.is_usable());
    assert!(assessment.verdict.reason().contains("stop sending"));

    let decision =
        Router::default().route(&buy("100", Urgency::Normal), &candidates, &health, at())?;
    assert_eq!(decision.venues(), vec![&venue("BBB")]);
    assert!(
        decision
            .exclusions
            .iter()
            .any(|e| e.reason == ExclusionReason::Quarantined)
    );
    Ok(())
}

#[test]
fn the_thresholds_a_deployment_sets_are_the_ones_that_are_used() -> Result<()> {
    let record = |policy: HealthPolicy| {
        let mut health = HealthTracker::new(policy);
        for _ in 0..20 {
            health.record_sent(&venue("AAA"));
        }
        health.record_reject(&venue("AAA"), at());
        health
            .assess(&venue("AAA"), Duration::from_millis(5), at())
            .verdict
    };

    assert!(
        matches!(
            record(HealthPolicy::default()),
            HealthVerdict::Degraded { .. }
        ),
        "one in twenty is worth paying for, not worth stopping for"
    );
    assert!(
        matches!(
            record(HealthPolicy {
                quarantine_reject_rate_f64: 0.03,
                ..HealthPolicy::default()
            }),
            HealthVerdict::Quarantined { .. }
        ),
        "a deployment that cannot tolerate it says so and is obeyed"
    );
    Ok(())
}

#[test]
fn a_quarantine_lapses_so_a_recovered_venue_is_tried_again() -> Result<()> {
    let mut health = HealthTracker::default();
    for _ in 0..20 {
        health.record_sent(&venue("AAA"));
    }
    for _ in 0..10 {
        health.record_reject(&venue("AAA"), at());
    }
    let later = at().saturating_add(Duration::from_secs(3_600));
    let assessment = health.assess(&venue("AAA"), Duration::from_millis(5), later);
    assert!(
        assessment.verdict.is_usable(),
        "an hour later the venue is tried again rather than written off"
    );
    Ok(())
}

#[test]
fn order_type_selection_works_a_large_order_rather_than_taking_the_book() -> Result<()> {
    let venue_profile = profile("XNAS", 0.0, 2.0);
    let touch = Touch {
        bid: d("99.90"),
        ask: d("100.10"),
    };

    let large = select_order_type(
        &venue_profile,
        BookSide::Ask,
        d("500"),
        d("1000"),
        touch,
        Urgency::Normal,
        false,
    )?;
    assert_eq!(large.order_type.kind(), OrderTypeKind::Limit);
    assert_eq!(
        large.order_type,
        RoutedOrderType::Limit { price: d("99.90") },
        "it rests on the bid rather than lifting the offer"
    );
    assert!(large.participation_f64 > 0.15);

    let small = select_order_type(
        &venue_profile,
        BookSide::Ask,
        d("5"),
        d("1000"),
        touch,
        Urgency::Normal,
        false,
    )?;
    assert_eq!(small.order_type.kind(), OrderTypeKind::Market);
    Ok(())
}

#[test]
fn an_empty_book_never_gets_an_unpriced_order() -> Result<()> {
    let venue_profile = profile("XNAS", 0.0, 2.0);
    let touch = Touch {
        bid: d("99.90"),
        ask: d("100.10"),
    };
    let selection = select_order_type(
        &venue_profile,
        BookSide::Ask,
        d("5"),
        Decimal::ZERO,
        touch,
        Urgency::Immediate,
        false,
    )?;
    assert_eq!(selection.order_type.kind(), OrderTypeKind::Limit);
    assert!(!selection.participation_f64.is_finite());
    Ok(())
}

#[test]
fn an_all_or_none_order_is_sent_fill_or_kill() -> Result<()> {
    let venue_profile = profile("XNAS", 0.0, 2.0);
    let touch = Touch {
        bid: d("99.90"),
        ask: d("100.10"),
    };
    let selection = select_order_type(
        &venue_profile,
        BookSide::Ask,
        d("5"),
        d("1000"),
        touch,
        Urgency::Normal,
        true,
    )?;
    assert_eq!(selection.order_type.kind(), OrderTypeKind::FillOrKill);
    assert_eq!(selection.order_type.liquidity(), Liquidity::Taker);
    Ok(())
}

#[test]
fn a_venue_that_cannot_take_the_chosen_type_degrades_visibly() -> Result<()> {
    let venue_profile = profile("LIMITED", 0.0, 2.0).with_supported(vec![OrderTypeKind::Limit]);
    let touch = Touch {
        bid: d("99.90"),
        ask: d("100.10"),
    };
    let selection = select_order_type(
        &venue_profile,
        BookSide::Ask,
        d("5"),
        d("1000"),
        touch,
        Urgency::Normal,
        false,
    )?;
    assert_eq!(selection.preferred, OrderTypeKind::Market);
    assert_eq!(selection.order_type.kind(), OrderTypeKind::Limit);
    assert!(selection.degraded);
    assert!(selection.reason.contains("does not accept"));
    Ok(())
}

#[test]
fn a_price_limit_keeps_the_order_off_a_venue_that_is_too_expensive() -> Result<()> {
    let candidates = [candidate(
        "PRICEY",
        0.0,
        50.0,
        &[("99.90", "100000")],
        &[("100.00", "100000")],
    )];
    let request = buy("100", Urgency::Normal).with_price_limit(d("100.01"));
    let decision =
        Router::default().route(&request, &candidates, &HealthTracker::default(), at())?;

    assert!(decision.slices.is_empty());
    assert_eq!(decision.unrouted, d("100"));
    assert!(decision.accounts_for_every_share());
    assert!(
        decision
            .exclusions
            .iter()
            .any(|e| e.reason == ExclusionReason::PriceLimit)
    );
    Ok(())
}

#[test]
fn a_fully_filled_parent_accounts_for_every_share() -> Result<()> {
    let candidates = [
        candidate(
            "VENUE-A",
            0.0,
            2.0,
            &[("99.90", "1000")],
            &[("100.00", "60")],
        ),
        candidate(
            "VENUE-B",
            0.0,
            2.0,
            &[("99.85", "1000")],
            &[("100.02", "1000")],
        ),
    ];
    let decision = Router::default().route(
        &buy("100", Urgency::Immediate),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;

    let mut parent = ParentOrder::new(parent_id(), object(), BookSide::Ask, d("100"))?;
    let ids = parent.split(&decision)?;
    assert_eq!(ids.len(), decision.slices.len());
    assert_eq!(parent.assigned(), d("100"));

    for id in &ids {
        let quantity = parent
            .child(id)
            .map(|child| child.quantity)
            .unwrap_or_default();
        let child = parent.child_mut(id).expect("the child was just attached");
        child.mark_working()?;
        child.apply_fill(quantity, d("100"))?;
    }

    assert!(parent.is_complete());
    assert_eq!(parent.filled(), d("100"));
    assert_eq!(parent.outstanding(), Decimal::ZERO);
    assert!(parent.accounts_for_every_share());
    parent.reconcile()?;

    let from_children: Decimal = parent.children().map(|child| child.filled()).sum();
    assert_eq!(from_children, parent.quantity);
    Ok(())
}

#[test]
fn a_child_that_fails_hands_its_quantity_back_instead_of_orphaning_the_parent() -> Result<()> {
    let mut parent = ParentOrder::new(parent_id(), object(), BookSide::Ask, d("100"))?;
    for quantity in ["60", "40"] {
        let client_id = parent.next_client_id();
        parent.attach(ChildOrder::new(
            client_id,
            parent_id(),
            venue("VENUE-A"),
            object(),
            BookSide::Ask,
            d(quantity),
            RoutedOrderType::Market,
        )?)?;
    }
    assert_eq!(parent.unassigned(), Decimal::ZERO);

    let child = parent.child_mut("ord-parent-c1").expect("the first child");
    child.mark_working()?;
    child.apply_fill(d("60"), d("100"))?;

    let second = "ord-parent-c2";
    parent
        .child_mut(second)
        .expect("the second child")
        .reject("the venue refused it")?;

    assert_eq!(parent.filled(), d("60"));
    assert_eq!(
        parent.orphaned(),
        d("40"),
        "the reject gave the shares back"
    );
    assert_eq!(parent.outstanding(), d("40"));
    assert!(parent.accounts_for_every_share());
    assert!(!parent.is_complete());
    assert_eq!(parent.failed_venues(), vec![&venue("VENUE-A")]);
    parent.reconcile()?;

    // And the released quantity can be sent somewhere else without inflating
    // the parent.
    let replacement = parent.next_client_id();
    parent.attach(ChildOrder::new(
        replacement,
        parent_id(),
        venue("VENUE-B"),
        object(),
        BookSide::Ask,
        d("40"),
        RoutedOrderType::Market,
    )?)?;
    assert!(parent.accounts_for_every_share());
    assert_eq!(
        parent.available_to_assign(),
        Decimal::ZERO,
        "and the re-routed shares are spoken for again"
    );
    Ok(())
}

#[test]
fn a_child_that_would_oversubscribe_the_parent_is_refused() -> Result<()> {
    let mut parent = ParentOrder::new(parent_id(), object(), BookSide::Ask, d("100"))?;
    let client_id = parent.next_client_id();
    parent.attach(ChildOrder::new(
        client_id,
        parent_id(),
        venue("VENUE-A"),
        object(),
        BookSide::Ask,
        d("100"),
        RoutedOrderType::Market,
    )?)?;

    let extra = parent.next_client_id();
    let refusal = parent
        .attach(ChildOrder::new(
            extra,
            parent_id(),
            venue("VENUE-B"),
            object(),
            BookSide::Ask,
            d("1"),
            RoutedOrderType::Market,
        )?)
        .expect_err("the parent is fully assigned");
    assert!(refusal.message().contains("free to assign"));
    Ok(())
}

#[test]
fn a_venue_reporting_more_than_it_was_asked_for_is_refused() -> Result<()> {
    let mut child = ChildOrder::new(
        "c1",
        parent_id(),
        venue("VENUE-A"),
        object(),
        BookSide::Ask,
        d("10"),
        RoutedOrderType::Market,
    )?;
    child.mark_working()?;
    child.apply_fill(d("6"), d("100"))?;
    let refusal = child
        .apply_fill(d("5"), d("100"))
        .expect_err("eleven of ten is not a fill");
    assert!(refusal.message().contains("over-fill"));
    assert_eq!(child.filled(), d("6"));
    assert_eq!(child.state, ChildState::PartiallyFilled);
    Ok(())
}

#[test]
fn the_simulated_gateway_says_so_and_names_what_it_stands_in_for() -> Result<()> {
    let gateway = SimulatedGateway::new(venue("SIM"), GatewaySettings::default(), 42);
    assert!(gateway.is_simulated());
    assert!(gateway.is_available());
    assert!(!gateway.is_frictionless());

    let missing = gateway.missing_credentials();
    assert!(!missing.is_empty(), "it is standing in for something");
    assert!(
        missing
            .iter()
            .all(|credential| credential.env_var.starts_with("QIP_SIM_")),
        "and names where each one would come from"
    );
    let requirement = gateway.requirement();
    assert!(requirement.contains("QIP_SIM_CREDENTIAL"));
    assert!(requirement.contains("operator enablement"));
    Ok(())
}

#[test]
fn the_native_gateway_refuses_to_send_and_says_exactly_what_is_missing() -> Result<()> {
    let mut gateway = NativeGateway::new(NativeGatewayConfig::new(
        venue("XNAS"),
        "fix.example.invalid:9443",
        "acct-1",
        "FIX 4.4",
    ));
    assert!(!gateway.is_simulated());
    assert!(!gateway.is_available());

    let child = ChildOrder::new(
        "c1",
        parent_id(),
        venue("XNAS"),
        object(),
        BookSide::Ask,
        d("10"),
        RoutedOrderType::Market,
    )?;
    let refusal = gateway
        .send(&child, at())
        .expect_err("there is no transport in this build");
    assert_eq!(refusal.code(), "unavailable");
    assert!(refusal.message().contains("QIP_XNAS_CREDENTIAL"));
    assert!(refusal.message().contains("simulated gateway"));

    // A credential alone does not make it usable, and it says which of the
    // remaining pieces is still absent.
    let configured = NativeGateway::configured(
        NativeGatewayConfig::new(
            venue("XNAS"),
            "fix.example.invalid:9443",
            "acct-1",
            "FIX 4.4",
        ),
        true,
        true,
    );
    assert!(!configured.is_available());
    assert!(configured.requirement().contains("TLS trust store"));
    Ok(())
}

#[test]
fn the_simulated_gateway_marks_every_acknowledgement_as_simulated() -> Result<()> {
    let mut gateway = SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 7)
        .with_book(book("SIM", &[("99.90", "1000")], &[("100.00", "1000")]));
    let child = ChildOrder::new(
        "c1",
        parent_id(),
        venue("SIM"),
        object(),
        BookSide::Ask,
        d("10"),
        RoutedOrderType::Market,
    )?;
    let ack = gateway.send(&child, at())?;
    assert!(ack.simulated);
    assert_eq!(ack.venue, venue("SIM"));

    let events = gateway.drain();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GatewayEvent::Accepted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GatewayEvent::Filled { .. }))
    );
    assert!(gateway.drain().is_empty(), "events are taken, not copied");
    Ok(())
}

#[test]
fn a_fill_or_kill_the_book_cannot_do_in_full_is_killed() -> Result<()> {
    let mut gateway = SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 11)
        .with_book(book("SIM", &[("99.90", "1000")], &[("100.00", "5")]));
    let child = ChildOrder::new(
        "c1",
        parent_id(),
        venue("SIM"),
        object(),
        BookSide::Ask,
        d("50"),
        RoutedOrderType::FillOrKill { limit: d("100.00") },
    )?;
    gateway.send(&child, at())?;

    let events = gateway.drain();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, GatewayEvent::Filled { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GatewayEvent::Cancelled { .. })),
        "all or none means none"
    );
    Ok(())
}

#[test]
fn a_resting_order_stays_working_rather_than_being_assumed_filled() -> Result<()> {
    let mut gateway = SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 13)
        .with_book(book("SIM", &[("99.90", "1000")], &[("100.00", "1000")]));
    let child = ChildOrder::new(
        "c1",
        parent_id(),
        venue("SIM"),
        object(),
        BookSide::Ask,
        d("50"),
        RoutedOrderType::Limit { price: d("99.00") },
    )?;
    gateway.send(&child, at())?;

    assert_eq!(gateway.working_count(), 1);
    let working = gateway.working().get("c1").expect("it is resting");
    assert_eq!(working.remaining(), d("50"));

    gateway.replace("c1", d("30"), Some(d("99.50")), at())?;
    assert_eq!(
        gateway.working().get("c1").map(|order| order.quantity),
        Some(d("30"))
    );
    gateway.cancel("c1", at())?;
    assert_eq!(gateway.working_count(), 0);
    assert!(gateway.cancel("c1", at()).is_err(), "and only once");
    Ok(())
}

#[test]
fn a_tiered_fee_schedule_charges_the_tier_the_volume_earns() -> Result<()> {
    let schedule = FeeSchedule::tiered(vec![
        FeeTier::new(Decimal::ZERO, 1.0, 10.0),
        FeeTier::new(d("1000000"), -0.5, 4.0),
    ])?;
    assert!((schedule.rate_bps_f64(Liquidity::Taker, Decimal::ZERO) - 10.0).abs() < f64::EPSILON);
    assert!((schedule.rate_bps_f64(Liquidity::Taker, d("5000000")) - 4.0).abs() < f64::EPSILON);
    assert!(
        schedule.fee(d("100000"), Liquidity::Maker, d("5000000")) < Decimal::ZERO,
        "the top tier pays a rebate"
    );

    let refusal = FeeSchedule::tiered(vec![FeeTier::new(d("100"), 1.0, 2.0)])
        .expect_err("a schedule with no bottom rung prices nobody");
    assert!(refusal.message().contains("zero volume"));
    Ok(())
}

#[test]
fn routing_the_same_market_twice_produces_the_same_decision() -> Result<()> {
    let candidates = [
        candidate(
            "VENUE-A",
            0.0,
            2.0,
            &[("99.90", "1000")],
            &[("100.00", "30"), ("100.50", "1000")],
        ),
        candidate(
            "VENUE-B",
            0.0,
            2.0,
            &[("99.85", "1000")],
            &[("100.05", "30"), ("100.40", "1000")],
        ),
    ];
    let router = Router::new(RouterSettings::default());
    let first = router.route(
        &buy("100", Urgency::Immediate),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;
    let second = router.route(
        &buy("100", Urgency::Immediate),
        &candidates,
        &HealthTracker::default(),
        at(),
    )?;
    assert_eq!(first, second, "a replay must reproduce the run exactly");
    Ok(())
}
