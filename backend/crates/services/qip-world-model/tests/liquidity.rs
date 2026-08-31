//! Liquidity topology: the map, its honesty about staleness and basis, and
//! the drift the opportunity engine would consume.

use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::testing::approx_eq;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use qip_world_model::liquidity::{DepthObservation, LiquidityTopology};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn mins_ago(minutes: i64) -> Timestamp {
    now().saturating_sub(Duration::from_mins(minutes))
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-northwind")
}

fn observation(
    venue: &str,
    status: VenueStatus,
    bid: i64,
    ask: i64,
    at: Timestamp,
) -> DepthObservation {
    DepthObservation::new(
        object(),
        VenueId::new(venue),
        status,
        Decimal::from_int(bid),
        Decimal::from_int(ask),
        at,
    )
}

/// A topology with the observation absorbed at its own observation time.
fn absorbed(observations: Vec<DepthObservation>) -> LiquidityTopology {
    let mut topology = LiquidityTopology::default();
    for obs in observations {
        let at = obs.observed_at;
        topology.absorb(obs, at).expect("valid observation");
    }
    topology
}

// --- the map ----------------------------------------------------------------

#[test]
fn the_map_names_each_venues_share_of_visible_depth_on_both_sides() {
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Open, 60, 20, mins_ago(1))
            .with_spread(Decimal::from_scaled(2, 2).expect("in range")),
        observation("XNAS", VenueStatus::Open, 30, 30, mins_ago(1)),
        observation("BATS", VenueStatus::Open, 10, 50, mins_ago(1)),
    ]);

    let map = topology.map(&object(), now(), now()).expect("fresh map");
    assert_eq!(map.venue_count(), 3);
    assert_eq!(map.venues_aged_out, 0);
    assert_eq!(map.total_bid_depth, Decimal::from_int(100));
    assert_eq!(map.total_ask_depth, Decimal::from_int(100));

    let bid_shares: f64 = map.venues.iter().map(|v| v.bid_share).sum();
    let ask_shares: f64 = map.venues.iter().map(|v| v.ask_share).sum();
    let depth_shares: f64 = map.venues.iter().map(|v| v.depth_share).sum();
    assert!(approx_eq(bid_shares, 1.0, 1e-9), "bid shares sum to one");
    assert!(approx_eq(ask_shares, 1.0, 1e-9), "ask shares sum to one");
    assert!(approx_eq(depth_shares, 1.0, 1e-9));

    let xnys = map
        .venues
        .iter()
        .find(|v| v.venue.as_str() == "XNYS")
        .expect("on the map");
    assert!(approx_eq(xnys.bid_share, 0.6, 1e-9));
    assert!(approx_eq(xnys.ask_share, 0.2, 1e-9));
    assert_eq!(xnys.spread, Decimal::from_scaled(2, 2));
    assert_eq!(xnys.observed_at, mins_ago(1), "per-venue as-of is carried");

    // Deterministic ordering: largest combined share first, ties by venue id.
    let order: Vec<&str> = map.venues.iter().map(|v| v.venue.as_str()).collect();
    assert_eq!(order, vec!["XNYS", "BATS", "XNAS"]);
}

#[test]
fn a_venue_observed_twice_counts_once_at_its_latest_observation() {
    // The failure the module doc names: two observations of the same venue
    // summed as two venues.
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Open, 100, 100, mins_ago(3)),
        observation("XNYS", VenueStatus::Open, 40, 40, mins_ago(1)),
        observation("XNAS", VenueStatus::Open, 60, 60, mins_ago(1)),
    ]);

    let map = topology.map(&object(), now(), now()).expect("fresh map");
    assert_eq!(map.venue_count(), 2, "two venues, not three observations");
    assert_eq!(
        map.total_bid_depth,
        Decimal::from_int(100),
        "XNYS counts once, at its latest depth of 40"
    );
    let xnys = map
        .venues
        .iter()
        .find(|v| v.venue.as_str() == "XNYS")
        .expect("on the map");
    assert_eq!(xnys.bid_depth, Decimal::from_int(40));
    assert!(approx_eq(xnys.depth_share, 0.4, 1e-9));
}

#[test]
fn a_point_in_time_read_sees_the_observation_that_was_latest_then() {
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Open, 100, 100, mins_ago(4)),
        observation("XNYS", VenueStatus::Open, 40, 40, mins_ago(1)),
    ]);

    // Asking about the instant between the two observations gets the first.
    let then = mins_ago(2);
    let map = topology.map(&object(), then, now()).expect("fresh then");
    assert_eq!(map.venues[0].bid_depth, Decimal::from_int(100));
}

// --- honest staleness -------------------------------------------------------

#[test]
fn a_stale_map_degrades_to_unknown_not_to_zero() {
    let topology = absorbed(vec![observation(
        "XNYS",
        VenueStatus::Open,
        50,
        50,
        mins_ago(30),
    )]);

    // Within the bound (default five minutes) the map exists.
    let fresh_at = mins_ago(26);
    assert!(topology.map(&object(), fresh_at, now()).is_some());
    // At exactly the bound the observation still counts.
    let at_bound = mins_ago(25);
    assert!(topology.map(&object(), at_bound, now()).is_some());
    // Beyond it the whole map is unknown — None, not a map of zero depth.
    assert!(
        topology.map(&object(), now(), now()).is_none(),
        "a stale map is unknown, never presented as current"
    );
}

#[test]
fn a_partially_stale_map_reports_its_shrunken_basis() {
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Open, 60, 60, mins_ago(1)),
        observation("XNAS", VenueStatus::Open, 40, 40, mins_ago(1)),
        observation("BATS", VenueStatus::Open, 500, 500, mins_ago(45)),
    ]);

    let map = topology
        .map(&object(), now(), now())
        .expect("two fresh venues");
    assert_eq!(map.venue_count(), 2);
    assert_eq!(map.venues_aged_out, 1, "the shrunken basis is stated");
    assert_eq!(map.known_venue_count(), 3);
    assert_eq!(
        map.total_bid_depth,
        Decimal::from_int(100),
        "aged-out depth is in no total"
    );
    // Shares are over the counted basis and still sum to one.
    let shares: f64 = map.venues.iter().map(|v| v.depth_share).sum();
    assert!(approx_eq(shares, 1.0, 1e-9));
    // The concentration carries the basis it was computed over.
    let concentration = map.concentration().expect("depth exists");
    assert_eq!(concentration.venue_count, 2);
}

#[test]
fn zero_observed_depth_is_knowledge_not_ignorance() {
    // An empty book seen a minute ago is a fact about the market; it must not
    // be conflated with "we have not looked".
    let topology = absorbed(vec![observation(
        "XNYS",
        VenueStatus::Open,
        0,
        0,
        mins_ago(1),
    )]);

    let map = topology
        .map(&object(), now(), now())
        .expect("observed, so known");
    assert_eq!(map.total_bid_depth, Decimal::ZERO);
    assert!(
        map.concentration().is_none(),
        "no depth means no concentration statistic, not a fabricated one"
    );
}

// --- bitemporality ----------------------------------------------------------

#[test]
fn an_observation_is_invisible_before_the_platform_learned_it() {
    let observed = mins_ago(10);
    let arrived = mins_ago(2);
    let mut topology = LiquidityTopology::default();
    topology
        .absorb(
            observation("XNYS", VenueStatus::Open, 50, 50, observed),
            arrived,
        )
        .expect("valid observation");

    // It was true at `observed`, but a decision made before `arrived` could
    // not have seen it.
    assert!(
        topology.map(&object(), observed, mins_ago(5)).is_none(),
        "not knowable before arrival"
    );
    assert!(
        topology.map(&object(), observed, arrived).is_some(),
        "knowable once arrived, and it was true at the observation instant"
    );
}

#[test]
fn a_known_at_before_the_observation_is_clamped_forward() {
    let observed = mins_ago(2);
    let mut topology = LiquidityTopology::default();
    // A clock or a parser claims we knew the depth before it was observed.
    topology
        .absorb(
            observation("XNYS", VenueStatus::Open, 50, 50, observed),
            mins_ago(8),
        )
        .expect("valid observation");

    assert!(
        topology.map(&object(), observed, mins_ago(4)).is_none(),
        "not knowable before the depth existed"
    );
    assert!(topology.map(&object(), observed, observed).is_some());
}

// --- concentration ----------------------------------------------------------

#[test]
fn concentration_responds_when_depth_moves_from_many_venues_to_one() {
    let mut topology = LiquidityTopology::default();
    for venue in ["XNYS", "XNAS", "BATS", "ARCX"] {
        let obs = observation(venue, VenueStatus::Open, 25, 25, mins_ago(10));
        topology.absorb(obs, mins_ago(10)).expect("valid");
    }
    let spread_out = topology
        .map(&object(), mins_ago(10), now())
        .expect("fresh then")
        .concentration()
        .expect("depth exists");
    assert!(
        approx_eq(spread_out.herfindahl, 0.25, 1e-9),
        "four equal venues"
    );
    assert!(approx_eq(spread_out.effective_venues(), 4.0, 1e-9));

    // The same depth collapses onto one venue.
    for (venue, size) in [("XNYS", 97), ("XNAS", 1), ("BATS", 1), ("ARCX", 1)] {
        let obs = observation(venue, VenueStatus::Open, size, size, mins_ago(1));
        topology.absorb(obs, mins_ago(1)).expect("valid");
    }
    let concentrated = topology
        .map(&object(), now(), now())
        .expect("fresh now")
        .concentration()
        .expect("depth exists");
    assert!(
        concentrated.herfindahl > spread_out.herfindahl,
        "the index rose as depth concentrated"
    );
    assert!(concentrated.herfindahl > 0.9);
    assert!(
        concentrated.effective_venues() < 1.1,
        "one venue, in effect"
    );
    assert_eq!(concentrated.venue_count, 4);
}

// --- total versus usable ----------------------------------------------------

#[test]
fn unreachable_depth_is_visible_in_the_total_but_never_in_the_usable_figure() {
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Open, 60, 60, mins_ago(1)),
        observation("DARK", VenueStatus::Unreachable, 40, 40, mins_ago(1)),
        observation("XLON", VenueStatus::Closed, 20, 20, mins_ago(1)),
    ]);

    let map = topology.map(&object(), now(), now()).expect("fresh map");
    // The map still shows where the liquidity lives...
    assert_eq!(map.venue_count(), 3);
    assert_eq!(map.total_bid_depth, Decimal::from_int(120));
    assert_eq!(map.total_ask_depth, Decimal::from_int(120));
    // ...but only the venue accepting orders is tradeable.
    assert_eq!(map.usable_bid_depth, Decimal::from_int(60));
    assert_eq!(map.usable_ask_depth, Decimal::from_int(60));
    assert_eq!(map.usable_venue_count(), 1);

    // The visible map looks diversified; the usable one is a single point of
    // failure, and the two statistics say so separately.
    let visible = map.concentration().expect("depth exists");
    let usable = map.usable_concentration().expect("usable depth exists");
    assert!(visible.herfindahl < 0.5);
    assert!(approx_eq(usable.herfindahl, 1.0, 1e-9));
    assert_eq!(usable.venue_count, 1);
}

#[test]
fn a_halted_venue_counts_toward_nothing_usable_either() {
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Halted, 60, 60, mins_ago(1)),
        observation("XNAS", VenueStatus::Auction, 40, 40, mins_ago(1)),
    ]);

    let map = topology.map(&object(), now(), now()).expect("fresh map");
    // An auction accepts orders; a halt does not.
    assert_eq!(map.usable_bid_depth, Decimal::from_int(40));
    assert_eq!(map.total_bid_depth, Decimal::from_int(100));
}

// --- drift ------------------------------------------------------------------

#[test]
fn drift_reports_which_venue_gained_share_and_the_concentration_change() {
    let mut topology = LiquidityTopology::default();
    for (venue, size) in [("XNYS", 50), ("XNAS", 50)] {
        let obs = observation(venue, VenueStatus::Open, size, size, mins_ago(10));
        topology.absorb(obs, mins_ago(10)).expect("valid");
    }
    for (venue, size) in [("XNYS", 80), ("XNAS", 20)] {
        let obs = observation(venue, VenueStatus::Open, size, size, mins_ago(1));
        topology.absorb(obs, mins_ago(1)).expect("valid");
    }

    let drift = topology
        .drift(&object(), Duration::from_mins(10), now(), now())
        .expect("both ends known");
    assert_eq!(drift.from, mins_ago(10));
    assert_eq!(drift.to, now());
    assert_eq!(drift.from_venue_count, 2);
    assert_eq!(drift.to_venue_count, 2);

    let xnys = drift
        .venues
        .iter()
        .find(|s| s.venue.as_str() == "XNYS")
        .expect("shifted");
    assert!(approx_eq(xnys.previous_share, 0.5, 1e-9));
    assert!(approx_eq(xnys.current_share, 0.8, 1e-9));
    assert!(
        approx_eq(xnys.change, 0.3, 1e-9),
        "XNYS gained thirty points"
    );
    let xnas = drift
        .venues
        .iter()
        .find(|s| s.venue.as_str() == "XNAS")
        .expect("shifted");
    assert!(
        approx_eq(xnas.change, -0.3, 1e-9),
        "XNAS lost what XNYS gained"
    );

    // Depth moved onto one venue, so concentration rose — the alert-worthy fact.
    let change = drift.concentration_change.expect("depth at both ends");
    assert!(change > 0.0);
    assert!(approx_eq(change, 0.68 - 0.5, 1e-9));
    assert_eq!(drift.material_shifts(0.25).len(), 2);
    assert!(drift.material_shifts(0.35).is_empty());
}

#[test]
fn a_venue_entering_the_map_drifts_up_from_a_zero_share() {
    let mut topology = LiquidityTopology::default();
    let only = observation("XNYS", VenueStatus::Open, 50, 50, mins_ago(10));
    topology.absorb(only, mins_ago(10)).expect("valid");
    for (venue, size) in [("XNYS", 50), ("XNAS", 50)] {
        let obs = observation(venue, VenueStatus::Open, size, size, mins_ago(1));
        topology.absorb(obs, mins_ago(1)).expect("valid");
    }

    let drift = topology
        .drift(&object(), Duration::from_mins(10), now(), now())
        .expect("both ends known");
    assert_eq!(drift.from_venue_count, 1);
    assert_eq!(drift.to_venue_count, 2, "the changed basis is reported");
    let entrant = drift
        .venues
        .iter()
        .find(|s| s.venue.as_str() == "XNAS")
        .expect("entered");
    assert!(approx_eq(entrant.previous_share, 0.0, 1e-12));
    assert!(approx_eq(entrant.change, 0.5, 1e-9));
}

#[test]
fn drift_is_unknown_when_the_windows_far_end_was_never_seen() {
    let topology = absorbed(vec![observation(
        "XNYS",
        VenueStatus::Open,
        50,
        50,
        mins_ago(1),
    )]);

    assert!(
        topology
            .drift(&object(), Duration::from_mins(60), now(), now())
            .is_none(),
        "a drift from an unseen map is a first observation, not a drift"
    );
    assert!(
        topology
            .drift(&object(), Duration::ZERO, now(), now())
            .is_none(),
        "no interval, no drift"
    );
}

// --- refusals and construction ----------------------------------------------

#[test]
fn corrupt_observations_are_refused() {
    let mut topology = LiquidityTopology::default();
    let negative_depth = DepthObservation::new(
        object(),
        VenueId::new("XNYS"),
        VenueStatus::Open,
        Decimal::from_int(-1),
        Decimal::from_int(10),
        now(),
    );
    assert!(topology.absorb(negative_depth, now()).is_err());

    let crossed =
        observation("XNYS", VenueStatus::Open, 10, 10, now()).with_spread(Decimal::from_int(-1));
    assert!(topology.absorb(crossed, now()).is_err());

    assert_eq!(topology.observation_count(), 0, "nothing corrupt was kept");
}

#[test]
fn an_observation_summarises_a_book_with_the_books_own_arithmetic() {
    let book = OrderBook::from_levels(
        object(),
        "XNYS",
        mins_ago(1),
        vec![
            BookLevel::new(Decimal::from_int(99), Decimal::from_int(10)),
            BookLevel::new(Decimal::from_int(98), Decimal::from_int(20)),
            BookLevel::new(Decimal::from_int(97), Decimal::from_int(40)),
        ],
        vec![
            BookLevel::new(Decimal::from_int(101), Decimal::from_int(5)),
            BookLevel::new(Decimal::from_int(102), Decimal::from_int(15)),
        ],
    );

    let obs = DepthObservation::from_book(&book, 2, VenueStatus::Open);
    assert_eq!(obs.venue.as_str(), "XNYS");
    assert_eq!(obs.observed_at, mins_ago(1));
    assert_eq!(obs.bid_depth, Decimal::from_int(30), "top two bid levels");
    assert_eq!(obs.ask_depth, Decimal::from_int(20), "top two ask levels");
    assert_eq!(obs.spread, Some(Decimal::from_int(2)));
}

#[test]
fn the_topology_lists_what_it_has_seen() {
    let topology = absorbed(vec![
        observation("XNYS", VenueStatus::Open, 1, 1, mins_ago(1)),
        observation("XNAS", VenueStatus::Open, 1, 1, mins_ago(1)),
    ]);

    assert_eq!(topology.instruments(), vec![object()]);
    assert_eq!(
        topology.venues_observed(&object()),
        vec![VenueId::new("XNAS"), VenueId::new("XNYS")]
    );
    assert_eq!(topology.observation_count(), 2);
    assert_eq!(topology.max_staleness(), Duration::from_mins(5));

    let map = topology.current_map(&object(), now()).expect("fresh");
    assert!(map.describe().contains("2 venues counted"));
    assert!(map.share_of(&VenueId::new("XNYS")).is_some());
    assert!(map.share_of(&VenueId::new("XLON")).is_none());
}

// --- bounded retention -------------------------------------------------------

#[test]
fn absorbing_past_the_per_venue_bound_evicts_the_oldest_observations_and_the_count_stops_growing() {
    // The failure this prevents has happened: fed from a live quote stream,
    // the topology held every observation since assembly — 120,805 after
    // seven hours on the deployed fastbrain — and the working set grew
    // linearly with uptime on a platform whose product rules cap working
    // sets. The bound must evict the *oldest*: a bound that evicted the
    // newest would pass a count check while the map silently froze at the
    // depth the venue showed at assembly.
    use qip_world_model::liquidity::HISTORY_PER_VENUE;

    let absorbed_count = HISTORY_PER_VENUE + 44;
    assert!(
        absorbed_count > HISTORY_PER_VENUE,
        "the premise: more observations are absorbed than one venue may keep"
    );

    let mut topology = LiquidityTopology::default();
    let first_at = now().saturating_sub(Duration::from_secs(absorbed_count as i64));
    for index in 0..absorbed_count {
        let at = first_at.saturating_add(Duration::from_secs(index as i64));
        // Depth counts upward with the observation's index, so which
        // observation a read serves is visible in the depth it reports.
        topology
            .absorb(
                observation("XNYS", VenueStatus::Open, 1 + index as i64, 10, at),
                at,
            )
            .expect("valid observation");
    }

    assert_eq!(
        topology.observation_count(),
        HISTORY_PER_VENUE,
        "the per-venue series grew past its bound"
    );

    // The newest observation is the one a current map serves.
    let map = topology
        .map(&object(), now(), now())
        .expect("a fresh map exists");
    assert_eq!(
        map.total_bid_depth,
        Decimal::from_int(absorbed_count as i64),
        "the newest observation did not survive eviction; the map is serving older depth"
    );

    // A query reaching back past the bound finds nothing: the oldest
    // observation was evicted, and unknown — not a stale answer — is what the
    // module's own honesty rules require a missing basis to read as.
    assert!(
        topology.map(&object(), first_at, now()).is_none(),
        "an observation older than the bound is still being served"
    );
}
