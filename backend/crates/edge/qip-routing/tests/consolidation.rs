//! Blueprint §27.2's third row: the router consolidating intents that named no
//! venue onto the one it prices best.
//!
//! What is being defended is not tidiness. Netting groups on instrument, venue
//! and representation, which is correct whenever a strategy *chose* a venue and
//! wrong when nobody did: two strategies that both wanted to buy the same thing
//! and neither of which cared where would otherwise produce two orders at two
//! placeholder venues, pay the spread twice, and be able to cross each other.
//! The last of those is the self-trade §27 exists to make impossible, arriving
//! through the door the netting key leaves open.
//!
//! Every test here asserts its own premise before its conclusion, because most
//! of them are about a vector being shorter, and a vector that was empty to
//! begin with is shorter than nothing.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::intent::{CycleLeg, Intent, Representation, net};
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::ids::OrderId;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use qip_routing::consolidate::{Consolidator, UNSPECIFIED_VENUE, is_unspecified};
use qip_routing::health::HealthTracker;
use qip_routing::ordertype::Urgency;
use qip_routing::ratelimit::RateLedger;
use qip_routing::router::{Router, RoutingRequest, VenueCandidate};
use qip_routing::venue::VenueProfile;

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn object() -> ObjectId {
    ObjectId::from_string("TKN")
}

fn book(name: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBook {
    OrderBook::from_levels(
        object(),
        name,
        at(),
        bids.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
        asks.iter()
            .map(|(p, s)| BookLevel::new(d(p), d(s)))
            .collect(),
    )
}

fn candidate(name: &str, taker_bps_f64: f64) -> VenueCandidate {
    VenueCandidate::new(
        VenueProfile::listed(VenueId::new(name), 0.0, taker_bps_f64)
            .with_sizes(Decimal::from_raw(1_000_000), Decimal::from_raw(1_000_000)),
        VenueStatus::Open,
        book(name, &[("99.90", "100000")], &[("100.00", "100000")]),
    )
}

/// Two venues quoting identically, one charging forty basis points to take and
/// the other one. The all-in difference is the fee and nothing else, which is
/// what makes "best venue" a claim about cost rather than about the screen.
fn two_venues() -> [VenueCandidate; 2] {
    [candidate("DEAR", 40.0), candidate("CHEAP", 1.0)]
}

fn unspecified(strategy: &str, signed_size: &str) -> Intent {
    Intent::new(
        StrategyId::new(strategy),
        object(),
        VenueId::new(UNSPECIFIED_VENUE),
        d(signed_size),
        d("100"),
        at(),
    )
    .expect("a non-zero intent")
}

fn at_venue(strategy: &str, venue: &str, signed_size: &str) -> Intent {
    Intent::new(
        StrategyId::new(strategy),
        object(),
        VenueId::new(venue),
        d(signed_size),
        d("100"),
        at(),
    )
    .expect("a non-zero intent")
}

fn consolidator() -> Consolidator {
    Consolidator::new(Router::default(), Urgency::Normal)
}

#[test]
fn two_strategies_that_named_no_venue_become_one_order_rather_than_two() -> Result<()> {
    let intents = vec![unspecified("alpha", "40"), unspecified("beta", "60")];

    // The premise, asserted before the conclusion: without consolidation these
    // net into a single group only because they share the placeholder — and
    // that group names a venue nothing can send to.
    let unconsolidated = net(intents.clone());
    assert_eq!(unconsolidated.len(), 1);
    assert!(
        is_unspecified(&unconsolidated[0].venue),
        "the netting seam happily produces an order for a venue that does not exist"
    );

    let consolidated = consolidator().consolidate(
        intents,
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert!(consolidated.is_complete());
    assert_eq!(consolidated.decisions.len(), 1);
    let decision = &consolidated.decisions[0];
    assert_eq!(
        decision.venue,
        VenueId::new("CHEAP"),
        "the venue is chosen on the all-in cost, and the two quote identically"
    );
    assert_eq!(decision.intents_moved, 2);
    assert_eq!(decision.side, BookSide::Ask);
    assert_eq!(
        decision.quantity_priced,
        d("100"),
        "priced on the net a venue would actually see"
    );

    let nets = net(consolidated.into_intents());
    assert_eq!(nets.len(), 1, "one instrument, one venue, one order");
    assert_eq!(nets[0].venue, VenueId::new("CHEAP"));
    assert_eq!(nets[0].net_size, d("100"));
    assert_eq!(nets[0].contributors.len(), 2);
    Ok(())
}

#[test]
fn an_intent_that_named_its_venue_is_left_exactly_where_it_was() -> Result<()> {
    // §27.2's default is that two venues are two executions at two prices. A
    // consolidator that "helpfully" moved a chosen venue would overrule the
    // strategy that chose it, which is the opposite of what the row permits.
    let chosen = at_venue("gamma", "DEAR", "25");
    let consolidated = consolidator().consolidate(
        vec![chosen.clone(), unspecified("alpha", "40")],
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;

    let survivor = consolidated
        .intents()
        .iter()
        .find(|intent| intent.strategy.as_str() == "gamma")
        .expect("the intent that chose a venue is still there");
    assert_eq!(
        survivor, &chosen,
        "unchanged in every field, not merely in venue"
    );
    assert_eq!(
        consolidated.decisions.len(),
        1,
        "and exactly one group was decided — the one that chose nothing"
    );

    let nets = net(consolidated.into_intents());
    assert_eq!(
        nets.len(),
        2,
        "two venues stay two orders, which is the row's default"
    );
    Ok(())
}

#[test]
fn a_buy_and_a_sell_that_named_no_venue_cancel_internally_instead_of_crossing_each_other()
-> Result<()> {
    // The self-trade. Before consolidation these are one group only by accident
    // of sharing a placeholder; the point is that after it they are one group
    // at a real venue, so the netting seam cancels them rather than sending
    // both.
    //
    // Three contributors rather than two, and deliberately of different sizes:
    // with two equal-and-opposite intents the largest contributor and the first
    // one in strategy order are the same intent, so a mutation that priced on
    // whichever happened to be first went unnoticed. Here `alpha` is first and
    // smallest and sells, `beta` is largest and buys, so the two candidate
    // answers differ in both the quantity and the side.
    let intents = vec![
        unspecified("alpha", "-25"),
        unspecified("beta", "75"),
        unspecified("gamma", "-50"),
    ];
    let consolidated = consolidator().consolidate(
        intents,
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;

    let decision = consolidated
        .decisions
        .first()
        .expect("a venue is chosen even when the net is zero");
    assert_eq!(
        decision.quantity_priced,
        d("75"),
        "with no net to price, the largest single contributor is what any venue \
         could have been asked for"
    );
    assert_eq!(
        decision.side,
        BookSide::Ask,
        "and the side is that contributor's, not the first one's"
    );

    let nets = net(consolidated.into_intents());
    assert_eq!(nets.len(), 1, "one group, not three");
    assert!(
        nets[0].is_cancelled(),
        "and it cancels internally, so nothing reaches a venue at all"
    );
    assert_eq!(nets[0].gross_size, d("150"));
    assert_eq!(nets[0].contributors.len(), 3);
    Ok(())
}

#[test]
fn spot_and_perpetual_are_consolidated_separately_because_they_are_different_instruments()
-> Result<()> {
    // §27.2's third row: the same underlying in two representations is two
    // instruments with different risk, and consolidation must not be the thing
    // that finally merges them.
    let intents = vec![
        unspecified("alpha", "40"),
        unspecified("beta", "60").with_representation(Representation::Perpetual),
    ];
    let consolidated = consolidator().consolidate(
        intents,
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert_eq!(
        consolidated.decisions.len(),
        2,
        "two representations are two groups even with one venue list"
    );
    let nets = net(consolidated.into_intents());
    assert_eq!(nets.len(), 2);
    assert_eq!(nets[0].net_size, d("40"));
    assert_eq!(nets[1].net_size, d("60"));
    Ok(())
}

#[test]
fn a_group_no_venue_can_take_is_withheld_and_never_leaves_naming_the_placeholder() -> Result<()> {
    // The safety property that makes a reserved identifier acceptable in place
    // of an absent field: an intent carrying it never comes out the other side.
    // A caller that ignores `refused` still cannot send to a venue called
    // UNSPECIFIED, because the vector it reads no longer holds one.
    let halted = VenueCandidate::new(
        VenueProfile::listed(VenueId::new("CHEAP"), 0.0, 1.0),
        VenueStatus::Halted,
        book("CHEAP", &[("99.90", "100000")], &[("100.00", "100000")]),
    );

    // The premise: the very same intents do consolidate when a venue is open.
    let admitted = consolidator().consolidate(
        vec![unspecified("alpha", "40")],
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert!(admitted.is_complete());

    let consolidated = consolidator().consolidate(
        vec![unspecified("alpha", "40"), unspecified("beta", "60")],
        std::slice::from_ref(&halted),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert!(!consolidated.is_complete());
    assert!(
        consolidated.intents().is_empty(),
        "nothing survives that could be sent to a placeholder"
    );
    let refusal = &consolidated.refused[0];
    assert_eq!(refusal.net_size, d("100"));
    assert_eq!(refusal.strategies.len(), 2, "both callers are named");
    assert!(
        !refusal.rejected.is_empty(),
        "and the venues that could not take it say why"
    );
    assert!(net(consolidated.into_intents()).is_empty());
    Ok(())
}

#[test]
fn a_cycle_leg_that_arrived_without_a_venue_is_refused_rather_than_moved() -> Result<()> {
    // §27.2's fourth row. Moving a leg to the cheapest venue after the cycle was
    // priced breaks the cycle's economics exactly as netting it would, and more
    // quietly, because the sizes still add up.
    let leg: Intent = CycleLeg::new(
        "cycle-7",
        StrategyId::new("arb"),
        object(),
        VenueId::new(UNSPECIFIED_VENUE),
        d("50"),
        d("100"),
        at(),
    )?
    .into();
    // The premise: a leg naming a real venue passes through untouched, so the
    // refusal below is about the missing venue and not about legs in general.
    let placed: Intent = CycleLeg::new(
        "cycle-7",
        StrategyId::new("arb"),
        object(),
        VenueId::new("CHEAP"),
        d("50"),
        d("100"),
        at(),
    )?
    .into();
    let fine = consolidator().consolidate(
        vec![placed],
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert_eq!(fine.intents().len(), 1);

    let refusal = consolidator()
        .consolidate(
            vec![leg],
            &two_venues(),
            &HealthTracker::default(),
            &RateLedger::new(),
            at(),
        )
        .expect_err("a leg without a venue is a producer bug, not something to price");
    assert!(
        refusal.message().contains("cycle-7"),
        "the refusal names the cycle so the producer can be found: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn the_venue_a_group_lands_on_is_the_one_the_rate_limit_left_open() -> Result<()> {
    // Consolidation reads the router's whole comparison, not just its prices,
    // so a venue whose §34.1 allowance is spent is not the venue an entire
    // strategy set is consolidated onto — which is how one busy window would
    // otherwise have become every strategy's problem at once.
    let limits = qip_routing::ratelimit::RateLimits::per_second(1, 10)?;
    let candidates = [
        VenueCandidate::new(
            VenueProfile::listed(VenueId::new("CHEAP"), 0.0, 1.0)
                .with_sizes(Decimal::from_raw(1_000_000), Decimal::from_raw(1_000_000))
                .with_rate_limits(limits),
            VenueStatus::Open,
            book("CHEAP", &[("99.90", "100000")], &[("100.00", "100000")]),
        ),
        candidate("DEAR", 40.0),
    ];

    // The premise: with the window untouched the cheap venue wins.
    let open = consolidator().consolidate(
        vec![unspecified("alpha", "40")],
        &candidates,
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert_eq!(open.decisions[0].venue, VenueId::new("CHEAP"));

    let mut rates = RateLedger::new();
    rates.spend_order("CHEAP", &limits, at())?;
    let spent = consolidator().consolidate(
        vec![unspecified("alpha", "40")],
        &candidates,
        &HealthTracker::default(),
        &rates,
        at(),
    )?;
    assert_eq!(
        spent.decisions[0].venue,
        VenueId::new("DEAR"),
        "the group goes where there is budget, and the exclusion says why"
    );
    assert!(
        spent.decisions[0]
            .rejected
            .iter()
            .any(|exclusion| exclusion.venue == VenueId::new("CHEAP"))
    );
    Ok(())
}

#[test]
fn consolidating_the_same_intents_against_the_same_market_twice_produces_the_same_answer()
-> Result<()> {
    // A replay that reorders is not a replay, and this vector is what the
    // netting seam groups.
    let intents = vec![
        unspecified("zeta", "10"),
        unspecified("alpha", "40"),
        at_venue("gamma", "DEAR", "25"),
        unspecified("beta", "-15"),
    ];
    let first = consolidator().consolidate(
        intents.clone(),
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    let second = consolidator().consolidate(
        intents,
        &two_venues(),
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert_eq!(first.intents().len(), 4, "the premise: nothing was dropped");
    assert_eq!(first, second);
    Ok(())
}
#[test]
fn a_group_the_router_would_split_is_consolidated_onto_the_cheaper_rung_not_the_larger_one()
-> Result<()> {
    // The one place `best_slice` earns its existence. The router's job is to
    // split across venues when that is cheapest; consolidation's job is to pick
    // *one*, and the obvious wrong answer — the venue that took the most — is
    // wrong precisely when the cheap venue is thin, which is the ordinary shape
    // of a thin book. A mutation that inverted the comparison here went
    // unnoticed until this test existed, because every other fixture in this
    // file routes to a single venue and never runs the comparator at all.
    let thin_and_cheap = VenueCandidate::new(
        VenueProfile::listed(VenueId::new("THIN"), 0.0, 1.0)
            .with_sizes(Decimal::from_raw(1_000_000), Decimal::from_raw(1_000_000)),
        VenueStatus::Open,
        book("THIN", &[("99.90", "20")], &[("100.00", "20")]),
    );
    let deep_and_dear = VenueCandidate::new(
        VenueProfile::listed(VenueId::new("DEEP"), 0.0, 25.0)
            .with_sizes(Decimal::from_raw(1_000_000), Decimal::from_raw(1_000_000)),
        VenueStatus::Open,
        book("DEEP", &[("99.90", "100000")], &[("100.00", "100000")]),
    );
    let candidates = [thin_and_cheap, deep_and_dear];

    // The premise, and the whole reason this test is different from the others:
    // the router really does split this request across both venues, so
    // `best_slice` is choosing between two rungs rather than confirming one.
    let request = RoutingRequest::new(
        OrderId::from_string("premise"),
        object(),
        BookSide::Ask,
        d("100"),
        Urgency::Immediate,
    );
    let split = Router::default().route(
        &request,
        &candidates,
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert!(
        split.slices.len() > 1,
        "the fixture must produce a split or this test proves nothing; it produced {:?}",
        split
            .slices
            .iter()
            .map(|s| s.venue.as_str())
            .collect::<Vec<_>>()
    );
    let largest = split
        .slices
        .iter()
        .max_by(|left, right| left.quantity.cmp(&right.quantity))
        .expect("a non-empty split");
    assert_eq!(
        largest.venue,
        VenueId::new("DEEP"),
        "and the venue that took the most must not be the cheapest, or the two \
         answers coincide and the assertion below is vacuous"
    );

    let consolidated = Consolidator::new(Router::default(), Urgency::Immediate).consolidate(
        vec![unspecified("alpha", "100")],
        &candidates,
        &HealthTracker::default(),
        &RateLedger::new(),
        at(),
    )?;
    assert_eq!(
        consolidated.decisions[0].venue,
        VenueId::new("THIN"),
        "consolidation picks the cheapest all-in rung, not the one that took the most"
    );
    Ok(())
}
