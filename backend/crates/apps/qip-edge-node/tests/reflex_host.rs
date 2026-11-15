//! The microsecond row's issuer as the node hosts it (R12-WORLD-3084,
//! R12-WORLD-7197, ADR 0633).
//!
//! `ReflexIssuer` was calibrated in `qip-edge`'s own tests and called by
//! nothing a binary runs. These drive the host with the pass outcomes the
//! loop produces and read what it issued and scored.
//!
//! The failure prevented: an issuer nothing feeds states nothing, so the row
//! "promoted only to the local reflex" described forecasts that never
//! existed on any path.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::message::BookSide;
use qip_contracts::reflex_forecast::ReflexTarget;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::ids::ObjectId;
use qip_core::{Decimal, Duration, Timestamp, dec};
use qip_edge::cell::{ConfirmedFill, PlacedOrder, WorkReport};
use qip_edge::reflex_forecast::MIN_OBSERVATIONS;
use qip_edge_node::feed::FeedTick;
use qip_edge_node::pass::PassOutcome;
use qip_edge_node::reflex_host::{ReflexHost, parse_reflex_horizon};

const HORIZON_US: u64 = 500;

fn t(micros: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_micros(micros))
}

fn venue() -> VenueId {
    VenueId::new("SIM")
}

fn order(id: &str) -> PlacedOrder {
    PlacedOrder {
        order_id: id.to_string(),
        strategy: StrategyId::new("s"),
        contributors: Vec::new(),
        object_id: ObjectId::from_string("ACME"),
        venue: venue(),
        side: BookSide::Ask,
        quantity: dec!("1"),
        price: dec!("100"),
        simulated: true,
    }
}

fn fill(id: &str, at: Timestamp) -> ConfirmedFill {
    ConfirmedFill {
        order_id: id.to_string(),
        venue: venue(),
        object_id: ObjectId::from_string("ACME"),
        side: BookSide::Ask,
        quantity: dec!("1"),
        price: dec!("100"),
        simulated: true,
        at,
        shares: Vec::new(),
    }
}

fn ran(orders: Vec<PlacedOrder>, fills: Vec<ConfirmedFill>) -> PassOutcome {
    let report = WorkReport {
        orders,
        fills,
        ..WorkReport::default()
    };
    PassOutcome::Ran {
        feed: FeedTick::default(),
        report: Box::new(report),
        requotes: Vec::new(),
        breaks: Vec::new(),
    }
}

/// Place `count` orders one per pass, a millisecond apart; the first
/// `filled_every`-th of them fill inside the horizon (reported on the next
/// pass), the rest never do.
fn warm(host: &mut ReflexHost, count: usize, filled_every: usize) -> i64 {
    let mut clock = 0_i64;
    for n in 0..count {
        let id = format!("w{n}");
        host.observe(&ran(vec![order(&id)], Vec::new()), t(clock))
            .unwrap();
        clock += 1_000;
        let fills = if n % filled_every == 0 {
            vec![fill(&id, t(clock - 900))]
        } else {
            Vec::new()
        };
        host.observe(&ran(Vec::new(), fills), t(clock)).unwrap();
    }
    clock
}

fn occurred_bp(host: &ReflexHost) -> u32 {
    host.recent()
        .last()
        .expect("premise: a forecast was issued")
        .fields()
        .occurred_bp()
        .expect("fill probability is an event")
}

/// Nothing is stated before the issuer has an evidence base, and an order is
/// forecast at placement from the outcomes before it. Mutation: skip the
/// `issuer.issue` call in `ReflexHost::observe` and `issued()` stays zero.
#[test]
fn the_host_states_nothing_until_it_has_seen_enough_orders_and_then_forecasts_each_one() {
    let mut host = ReflexHost::new("cell-1", HORIZON_US).unwrap();
    let clock = warm(&mut host, MIN_OBSERVATIONS - 1, 4);
    assert_eq!(host.issued(), 0, "premise: a cold issuer states nothing");
    // The twentieth order resolves only after its own horizon, so the
    // twenty-first has the twenty observations it needs.
    let clock = warm_one(&mut host, MIN_OBSERVATIONS - 1, clock);
    host.observe(&ran(vec![order("next")], Vec::new()), t(clock))
        .unwrap();
    assert_eq!(host.issued(), 1);
    let forecast = host.recent().last().unwrap().fields().clone();
    assert_eq!(forecast.target, ReflexTarget::FillProbability);
    assert_eq!(forecast.horizon_us, HORIZON_US);
}

fn warm_one(host: &mut ReflexHost, n: usize, mut clock: i64) -> i64 {
    let id = format!("w{n}");
    host.observe(&ran(vec![order(&id)], Vec::new()), t(clock))
        .unwrap();
    clock += 1_000;
    host.observe(&ran(Vec::new(), Vec::new()), t(clock))
        .unwrap();
    clock
}

/// The stated probability follows what the venue did: a venue that fills one
/// order in four is forecast far below one that fills all of them. Mutation:
/// make `resolve_due` observe `occurred = true` always; both hosts then state
/// the same high figure.
#[test]
fn the_stated_fill_probability_follows_what_the_venue_did() {
    let mut slow = ReflexHost::new("cell-1", HORIZON_US).unwrap();
    let mut quick = ReflexHost::new("cell-1", HORIZON_US).unwrap();
    let c1 = warm(&mut slow, 40, 4);
    let c2 = warm(&mut quick, 40, 1);
    slow.observe(&ran(vec![order("probe")], Vec::new()), t(c1))
        .unwrap();
    quick
        .observe(&ran(vec![order("probe")], Vec::new()), t(c2))
        .unwrap();
    let (s, q) = (occurred_bp(&slow), occurred_bp(&quick));
    assert!(s < 4_000, "one in four filled, stated {s} bp");
    assert!(q > 9_000, "all filled, stated {q} bp");
}

/// A fill after the horizon is not a fill inside it. Mutation: drop the
/// `at <= deadline` test in `resolve_due`; the late fills count and the
/// stated figure rises.
#[test]
fn a_fill_after_the_horizon_does_not_count_as_a_fill_within_it() {
    let mut host = ReflexHost::new("cell-1", HORIZON_US).unwrap();
    let mut clock = 0_i64;
    for n in 0..30 {
        let id = format!("l{n}");
        host.observe(&ran(vec![order(&id)], Vec::new()), t(clock))
            .unwrap();
        // Reported on the next pass, and stamped after the horizon.
        clock += 1_000;
        host.observe(&ran(Vec::new(), vec![fill(&id, t(clock - 100))]), t(clock))
            .unwrap();
    }
    host.observe(&ran(vec![order("probe")], Vec::new()), t(clock))
        .unwrap();
    let stated = occurred_bp(&host);
    assert!(stated < 1_000, "every fill was late yet stated {stated} bp");
}

/// Each forecast is scored against what happened, and the score is the
/// issuer's own. Mutation: skip `resolve_event` in `resolve_due`; `resolved`
/// stays zero and there is no Brier score.
#[test]
fn forecasts_are_scored_when_their_horizon_has_elapsed() {
    let mut host = ReflexHost::new("cell-1", HORIZON_US).unwrap();
    let clock = warm(&mut host, 40, 2);
    assert!(host.issued() > 0, "premise: forecasts were issued");
    assert!(host.resolved() > 0, "none was scored");
    assert!(host.brier().is_some());
    let _ = clock;
}

/// A halted pass books the fills it carries but places nothing, so it issues
/// nothing. Mutation: read `report.orders` for the halted arm too.
#[test]
fn a_halted_turn_books_fills_and_issues_nothing() {
    let mut host = ReflexHost::new("cell-1", HORIZON_US).unwrap();
    host.observe(
        &PassOutcome::Halted {
            feed: FeedTick::default(),
            fills: Vec::new(),
        },
        t(0),
    )
    .unwrap();
    assert_eq!(host.issued(), 0);
    assert_eq!(host.untracked(), 0);
}

/// The horizon is the operator's and is refused outside the microsecond row,
/// never clamped. Mutation: widen the upper bound to a second.
#[test]
fn the_declared_horizon_is_refused_outside_the_microsecond_row() {
    assert_eq!(parse_reflex_horizon(None).unwrap(), None);
    assert_eq!(parse_reflex_horizon(Some("  ")).unwrap(), None);
    assert_eq!(parse_reflex_horizon(Some("500")).unwrap(), Some(500));
    for bad in ["0", "1000000", "-5", "fast", "5e2"] {
        assert!(
            parse_reflex_horizon(Some(bad)).is_err(),
            "{bad} was accepted"
        );
    }
    let _ = Decimal::ZERO;
}
