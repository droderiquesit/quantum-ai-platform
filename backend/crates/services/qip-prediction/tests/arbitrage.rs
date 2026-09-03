//! Arbitrage within a market, across venues, and the adapters that feed both.
//!
//! Every size asserted here is a size the supplied books can actually fill.
//! The failure mode these tests exist to prevent is the profitable-looking
//! opportunity that is profitable only for the two contracts resting at the
//! touch.

use qip_contracts::{VenueClass, VenueId};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use qip_prediction::adapter::{
    PredictionAdapter, PredictionUpdate, SyntheticPredictionVenue, SyntheticVenueConfig,
    VenueApiAdapter, VenueApiConfig,
};
use qip_prediction::arbitrage::{SetArbitrageKind, implied_sum, set_arbitrage};
use qip_prediction::cross::CrossMarketPair;
use qip_prediction::market::{EventMarket, FeeSchedule, MarketKind, Outcome, OutcomeId};
use qip_prediction::resolution::{
    Comparison, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule, SourceKind,
    UndeterminedRule,
};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn resolves_at() -> Timestamp {
    now().saturating_add(Duration::from_days(30))
}

fn price(value: &str) -> Decimal {
    Decimal::parse(value).expect("a valid price literal")
}

fn levels(entries: &[(&str, i64)]) -> Vec<BookLevel> {
    entries
        .iter()
        .map(|(at, size)| BookLevel::new(price(at), Decimal::from_int(*size)))
        .collect()
}

fn book(object_id: &str, venue: &str, bids: &[(&str, i64)], asks: &[(&str, i64)]) -> OrderBook {
    OrderBook::from_levels(
        ObjectId::from_string(object_id),
        venue,
        now(),
        levels(bids),
        levels(asks),
    )
}

/// A binary market on a rate threshold, parameterised by everything two venues
/// can plausibly disagree about.
fn market(
    venue: &str,
    source_name: &str,
    threshold: i64,
    at: Timestamp,
    fees: FeeSchedule,
) -> EventMarket {
    let source = ResolutionSource::new(
        source_name,
        SourceKind::Official,
        vec!["policy_rate_change_bp".to_string()],
    );
    let criteria = ResolutionCriteria::Threshold {
        metric: "policy_rate_change_bp".to_string(),
        comparison: Comparison::AtMost,
        value: Decimal::from_int(threshold),
    };
    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "a cut",
        ObjectId::from_string(format!("{venue}-YES")),
        criteria.clone(),
    );
    EventMarket::new(
        ObjectId::from_string(format!("{venue}-MARKET")),
        VenueId::new(venue),
        VenueClass::PredictionMarket,
        Proposition::new(
            "the policy rate decision",
            criteria,
            source,
            at,
            SettlementRule::unit(UndeterminedRule::VoidAndRefund),
            Duration::from_hours(24),
        )
        .expect("the source publishes the metric"),
        MarketKind::binary(
            yes,
            OutcomeId::new("no"),
            ObjectId::from_string(format!("{venue}-NO")),
        )
        .expect("distinct yes/no ids"),
        fees,
    )
    .expect("a well-formed market")
}

/// A three-way categorical market whose outcomes partition a rate decision.
fn categorical(fees: FeeSchedule) -> EventMarket {
    let source = ResolutionSource::new(
        "central-bank-statistical-release",
        SourceKind::Official,
        vec!["policy_rate_change_bp".to_string()],
    );
    let outcomes = vec![
        Outcome::new(
            OutcomeId::new("cut"),
            "a cut",
            ObjectId::from_string("CUT"),
            ResolutionCriteria::Within {
                metric: "policy_rate_change_bp".to_string(),
                lower: None,
                upper: Some(Decimal::from_int(-24)),
            },
        ),
        Outcome::new(
            OutcomeId::new("hold"),
            "no change",
            ObjectId::from_string("HOLD"),
            ResolutionCriteria::Within {
                metric: "policy_rate_change_bp".to_string(),
                lower: Some(Decimal::from_int(-24)),
                upper: Some(Decimal::from_int(25)),
            },
        ),
        Outcome::new(
            OutcomeId::new("hike"),
            "a hike",
            ObjectId::from_string("HIKE"),
            ResolutionCriteria::Within {
                metric: "policy_rate_change_bp".to_string(),
                lower: Some(Decimal::from_int(25)),
                upper: None,
            },
        ),
    ];
    EventMarket::new(
        ObjectId::from_string("CATEGORICAL"),
        VenueId::new("PREDICT-A"),
        VenueClass::PredictionMarket,
        Proposition::new(
            "the policy rate decision",
            ResolutionCriteria::Any(outcomes.iter().map(|o| o.criteria.clone()).collect()),
            source,
            resolves_at(),
            SettlementRule::unit(UndeterminedRule::VoidAndRefund),
            Duration::from_hours(24),
        )
        .expect("the source publishes the metric"),
        MarketKind::categorical(outcomes).expect("three distinct outcomes"),
        fees,
    )
    .expect("a well-formed market")
}

// --- within a market --------------------------------------------------------

#[test]
fn a_complete_set_priced_below_its_payoff_is_detected_as_arbitrage() {
    let market = categorical(FeeSchedule::FREE);
    let cut = book(
        "CUT",
        "PREDICT-A",
        &[("0.28", 50)],
        &[("0.30", 10), ("0.32", 20)],
    );
    let hold = book("HOLD", "PREDICT-A", &[("0.28", 50)], &[("0.30", 50)]);
    let hike = book("HIKE", "PREDICT-A", &[("0.28", 50)], &[("0.30", 50)]);
    let books = vec![
        (OutcomeId::new("cut"), &cut),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];

    let found = set_arbitrage(&market, &books)
        .expect("the walk succeeds")
        .expect("a set costing 0.90 against a payoff of 1 is an arbitrage");
    assert_eq!(found.kind, SetArbitrageKind::UnderpricedSet);
    assert_eq!(
        found.quantity,
        Decimal::from_int(30),
        "the walk takes both of the cheap leg's levels and stops when it runs out"
    );
    // 10 contracts at a combined 0.90 and 20 at a combined 0.92.
    assert_eq!(found.profit(), price("2.6"));
    assert!(
        found.depth_limited,
        "the size was set by depth, not by price"
    );
    assert_eq!(found.plan.steps().len(), 3, "one leg per outcome");
    assert!(
        found.plan.mandatory().len() == 3,
        "a partial set is a directional position nobody chose"
    );
    found
        .edge
        .require_complete()
        .expect("every deduction kind must be considered");
}

#[test]
fn arbitrage_sized_against_book_depth_never_claims_more_than_is_available() {
    let market = categorical(FeeSchedule::FREE);
    // The thin leg holds five contracts in total; the others are deep.
    let cut = book("CUT", "PREDICT-A", &[("0.20", 100)], &[("0.25", 5)]);
    let hold = book("HOLD", "PREDICT-A", &[("0.20", 100)], &[("0.30", 400)]);
    let hike = book("HIKE", "PREDICT-A", &[("0.20", 100)], &[("0.30", 400)]);
    let books = vec![
        (OutcomeId::new("cut"), &cut),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];

    let found = set_arbitrage(&market, &books)
        .expect("the walk succeeds")
        .expect("an arbitrage exists");
    assert_eq!(
        found.quantity,
        Decimal::from_int(5),
        "the thinnest leg caps the size of the whole set"
    );
    for (outcome, average) in &found.leg_prices {
        let depth: Decimal = books
            .iter()
            .find(|(id, _)| id == outcome)
            .map(|(_, book)| book.asks.iter().map(|level| level.size).sum())
            .expect("every leg has a book");
        assert!(
            found.quantity <= depth,
            "leg {outcome} claims {} against {depth} of depth at {average}",
            found.quantity
        );
    }
}

#[test]
fn a_binary_complement_priced_below_one_is_the_same_arbitrage_with_two_legs() {
    let market = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let yes = book(
        "PREDICT-A-YES",
        "PREDICT-A",
        &[("0.40", 20)],
        &[("0.42", 20)],
    );
    let no = book(
        "PREDICT-A-NO",
        "PREDICT-A",
        &[("0.50", 20)],
        &[("0.55", 20)],
    );
    let books = vec![(OutcomeId::new("yes"), &yes), (OutcomeId::new("no"), &no)];

    let found = set_arbitrage(&market, &books)
        .expect("the walk succeeds")
        .expect("yes at 0.42 and no at 0.55 costs 0.97 for a payoff of 1");
    assert_eq!(found.quantity, Decimal::from_int(20));
    assert_eq!(found.profit(), price("0.6"));
    assert_eq!(found.cost, price("19.4"));
    assert_eq!(found.proceeds, Decimal::from_int(20));
}

#[test]
fn bids_summing_above_the_payoff_are_detected_as_the_opposite_arbitrage() {
    let market = categorical(FeeSchedule::FREE);
    let cut = book("CUT", "PREDICT-A", &[("0.40", 30)], &[("0.45", 30)]);
    let hold = book("HOLD", "PREDICT-A", &[("0.40", 30)], &[("0.45", 30)]);
    let hike = book("HIKE", "PREDICT-A", &[("0.40", 30)], &[("0.45", 30)]);
    let books = vec![
        (OutcomeId::new("cut"), &cut),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];

    let found = set_arbitrage(&market, &books)
        .expect("the walk succeeds")
        .expect("selling a set for 1.20 that costs 1.00 to mint is an arbitrage");
    assert_eq!(found.kind, SetArbitrageKind::OverpricedSet);
    assert_eq!(found.quantity, Decimal::from_int(30));
    assert_eq!(found.profit(), Decimal::from_int(6));
}

#[test]
fn a_fairly_priced_market_offers_no_arbitrage_however_wide_its_spread() {
    let market = categorical(FeeSchedule::new(50, 0, 100).expect("a valid schedule"));
    let cut = book("CUT", "PREDICT-A", &[("0.30", 100)], &[("0.36", 100)]);
    let hold = book("HOLD", "PREDICT-A", &[("0.30", 100)], &[("0.36", 100)]);
    let hike = book("HIKE", "PREDICT-A", &[("0.30", 100)], &[("0.36", 100)]);
    let books = vec![
        (OutcomeId::new("cut"), &cut),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];
    assert!(
        set_arbitrage(&market, &books)
            .expect("the walk succeeds")
            .is_none(),
        "asks summing to 1.08 and bids to 0.90 is a spread, not an opportunity"
    );
}

#[test]
fn the_deviation_of_the_outcome_prices_from_certainty_is_reported_rather_than_normalised() {
    let market = categorical(FeeSchedule::new(50, 0, 100).expect("a valid schedule"));
    let cut = book("CUT", "PREDICT-A", &[("0.30", 100)], &[("0.36", 100)]);
    let hold = book("HOLD", "PREDICT-A", &[("0.30", 100)], &[("0.36", 100)]);
    let hike = book("HIKE", "PREDICT-A", &[("0.30", 100)], &[("0.36", 100)]);
    let books = vec![
        (OutcomeId::new("cut"), &cut),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];

    let deviation = implied_sum(&market, &books).expect("every outcome is priced");
    assert!(
        deviation.overround().is_positive(),
        "a fee-charging venue's offers should sum above certainty, got {}",
        deviation.ask_sum
    );
    assert!(deviation.underround().is_positive());
    assert!(!deviation.offers_are_arbitrageable());
    assert!(!deviation.bids_are_arbitrageable());

    let cheap = book("CUT", "PREDICT-A", &[("0.30", 100)], &[("0.20", 100)]);
    let cheap_books = vec![
        (OutcomeId::new("cut"), &cheap),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];
    let cheap_deviation = implied_sum(&market, &cheap_books).expect("priced");
    assert!(
        cheap_deviation.offers_are_arbitrageable(),
        "offers summing below certainty are an arbitrage, not a rounding artefact"
    );
}

#[test]
fn fees_reduce_an_arbitrage_and_can_remove_it_entirely() {
    let free = categorical(FeeSchedule::FREE);
    let charged = categorical(FeeSchedule::new(200, 0, 250).expect("a valid schedule"));
    let cut = book("CUT", "PREDICT-A", &[("0.28", 50)], &[("0.32", 50)]);
    let hold = book("HOLD", "PREDICT-A", &[("0.28", 50)], &[("0.32", 50)]);
    let hike = book("HIKE", "PREDICT-A", &[("0.28", 50)], &[("0.32", 50)]);
    let books = vec![
        (OutcomeId::new("cut"), &cut),
        (OutcomeId::new("hold"), &hold),
        (OutcomeId::new("hike"), &hike),
    ];

    let gross = set_arbitrage(&free, &books)
        .expect("the walk succeeds")
        .expect("a set at 0.96 is an arbitrage when nothing is charged");
    assert_eq!(gross.profit(), Decimal::from_int(2));
    assert!(
        set_arbitrage(&charged, &books)
            .expect("the walk succeeds")
            .is_none(),
        "4 points of edge does not survive 2% on the legs and 2.5% on the payoff"
    );
}

// --- across venues ----------------------------------------------------------

#[test]
fn two_markets_with_different_resolution_criteria_are_refused_as_an_arbitrage_pair() {
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let right = market(
        "PREDICT-B",
        "release",
        -50,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let error = CrossMarketPair::new(&left, &right)
        .expect_err("a 25bp question and a 50bp question are different instruments");
    assert_eq!(error.code(), "invalid");
    assert!(
        error.message().contains("resolution criteria"),
        "the refusal should name what differs, got {error}"
    );
}

#[test]
fn two_markets_resolving_on_different_dates_are_refused_as_an_arbitrage_pair() {
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let right = market(
        "PREDICT-B",
        "release",
        -25,
        resolves_at().saturating_add(Duration::from_days(1)),
        FeeSchedule::FREE,
    );
    let error = CrossMarketPair::new(&left, &right)
        .expect_err("the same question answered a day apart is a different contract");
    assert!(error.message().contains("resolution time"));
}

#[test]
fn a_pair_resolving_from_different_authorities_needs_a_stated_haircut_before_it_trades() {
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let right = market(
        "PREDICT-B",
        "committee-vote",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let pair = CrossMarketPair::new(&left, &right)
        .expect("the same question from two authorities is still the same question");
    assert_eq!(
        pair.source_divergence(),
        Some(&("release".to_string(), "committee-vote".to_string()))
    );

    let cheap = book(
        "PREDICT-A-YES",
        "PREDICT-A",
        &[("0.38", 20)],
        &[("0.40", 20)],
    );
    let rich = book(
        "PREDICT-B-YES",
        "PREDICT-B",
        &[("0.45", 30)],
        &[("0.47", 30)],
    );
    let error = pair
        .arbitrage(&OutcomeId::new("yes"), &cheap, &rich)
        .expect_err("trading two authorities against each other is not free");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("haircut"));

    let priced = pair
        .with_source_haircut(price("0.01"))
        .expect("a haircut may be stated")
        .arbitrage(&OutcomeId::new("yes"), &cheap, &rich)
        .expect("the walk succeeds")
        .expect("five points of spread survives a one point haircut");
    assert_eq!(priced.quantity, Decimal::from_int(20));
    assert_eq!(
        priced.profit(),
        price("0.8"),
        "twenty contracts at five points less a one point haircut"
    );
    let uncertainty = priced
        .edge
        .deductions()
        .iter()
        .find(|deduction| deduction.kind == qip_contracts::DeductionKind::Uncertainty)
        .expect("the divergence is deducted, not ignored");
    assert!(
        uncertainty.basis.contains("committee-vote"),
        "the deduction should name the authorities that can disagree, got {}",
        uncertainty.basis
    );
}

#[test]
fn a_haircut_stated_on_a_pair_with_no_source_divergence_is_refused() {
    // The failure this prevents: `with_source_haircut` used to accept a
    // positive value regardless of whether the pair actually diverged on
    // source. Applied to a same-source pair, the walk still recorded an
    // `Uncertainty` deduction with the stated (non-zero) amount, but under
    // the basis text reserved for "no divergence" — "none: both venues
    // resolve from the same source on identical criteria" — so the one
    // record meant to explain where the edge went contradicted its own
    // amount. There is nothing a same-source haircut is the price of, so
    // stating one is refused rather than silently recorded.
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let right = market(
        "PREDICT-B",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let pair = CrossMarketPair::new(&left, &right).expect("the same proposition");
    assert!(
        pair.source_divergence().is_none(),
        "premise: this pair must not diverge on source, or the refusal below proves nothing"
    );

    let error = pair
        .with_source_haircut(price("0.01"))
        .expect_err("a same-source pair has nothing for a haircut to price");
    assert_eq!(error.code(), "invalid");
    assert!(
        error.message().contains("same source"),
        "the refusal should say why, got {error}"
    );

    // A zero haircut is not a claim about divergence and remains legal.
    CrossMarketPair::new(&left, &right)
        .expect("the same proposition")
        .with_source_haircut(Decimal::ZERO)
        .expect("stating no haircut is always legal");
}

#[test]
fn a_cross_venue_spread_is_sized_against_the_thinner_of_the_two_books() {
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let right = market(
        "PREDICT-B",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let pair = CrossMarketPair::new(&left, &right).expect("the same proposition");
    assert!(pair.source_divergence().is_none());

    let cheap = book(
        "PREDICT-A-YES",
        "PREDICT-A",
        &[("0.38", 100)],
        &[("0.40", 12)],
    );
    let rich = book(
        "PREDICT-B-YES",
        "PREDICT-B",
        &[("0.45", 500)],
        &[("0.47", 500)],
    );
    let found = pair
        .arbitrage(&OutcomeId::new("yes"), &cheap, &rich)
        .expect("the walk succeeds")
        .expect("buying at 0.40 and selling at 0.45 is an arbitrage");
    assert_eq!(
        found.quantity,
        Decimal::from_int(12),
        "the offer that can be lifted caps the trade"
    );
    assert_eq!(found.buy_venue, VenueId::new("PREDICT-A"));
    assert_eq!(found.sell_venue, VenueId::new("PREDICT-B"));
    assert_eq!(found.buy_price, price("0.40"));
    assert_eq!(found.sell_price, price("0.45"));
    assert_eq!(found.profit(), price("0.6"));
    found
        .edge
        .require_complete()
        .expect("every deduction kind must be considered");
    assert_eq!(found.plan.steps().len(), 2);
}

#[test]
fn a_cross_venue_pair_priced_consistently_offers_nothing() {
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let right = market(
        "PREDICT-B",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );
    let pair = CrossMarketPair::new(&left, &right).expect("the same proposition");
    let one = book(
        "PREDICT-A-YES",
        "PREDICT-A",
        &[("0.40", 100)],
        &[("0.42", 100)],
    );
    let two = book(
        "PREDICT-B-YES",
        "PREDICT-B",
        &[("0.40", 100)],
        &[("0.42", 100)],
    );
    assert!(
        pair.arbitrage(&OutcomeId::new("yes"), &one, &two)
            .expect("the walk succeeds")
            .is_none(),
        "two venues quoting the same market offer no spread"
    );
}

// --- adapters ---------------------------------------------------------------

#[test]
fn the_synthetic_venue_replays_identically_from_the_same_seed() {
    let render = |seed: u64| {
        let mut venue =
            SyntheticPredictionVenue::new(SyntheticVenueConfig::demo(seed).expect("config"), now())
                .expect("venue");
        venue
            .poll(now().saturating_add(Duration::from_hours(2)))
            .expect("poll")
            .iter()
            .map(|update| match update {
                PredictionUpdate::MarketListed(market) => format!("listed:{}", market.market_id),
                PredictionUpdate::Book { outcome, book, .. } => format!(
                    "book:{outcome}:{}:{}",
                    book.best_bid().map_or(Decimal::ZERO, |level| level.price),
                    book.best_ask().map_or(Decimal::ZERO, |level| level.price)
                ),
                PredictionUpdate::Report { report, .. } => format!("report:{}", report.outcome),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        render(9),
        render(9),
        "the same seed must replay identically"
    );
    assert_ne!(render(9), render(10));
}

#[test]
fn the_synthetic_venue_eventually_lists_a_set_that_can_be_bought_below_its_payoff() {
    let mut venue =
        SyntheticPredictionVenue::new(SyntheticVenueConfig::demo(4).expect("config"), now())
            .expect("venue");
    let updates = venue
        .poll(now().saturating_add(Duration::from_hours(6)))
        .expect("poll");
    let market = venue.market().clone();

    let mut latest: Vec<(OutcomeId, OrderBook)> = Vec::new();
    let mut arbitrages = 0;
    for update in updates {
        let PredictionUpdate::Book { outcome, book, .. } = update else {
            continue;
        };
        latest.retain(|(id, _)| id != &outcome);
        latest.push((outcome, *book));
        if latest.len() < market.outcomes().len() {
            continue;
        }
        let books: Vec<(OutcomeId, &OrderBook)> =
            latest.iter().map(|(id, book)| (id.clone(), book)).collect();
        if let Some(found) = set_arbitrage(&market, &books).expect("the walk succeeds") {
            assert!(found.quantity.is_positive());
            assert!(found.profit().is_positive());
            arbitrages += 1;
        }
    }
    assert!(
        arbitrages > 0,
        "a venue that sometimes prices a set below its payoff must produce a detectable arbitrage"
    );
}

/// Every book the demo venue publishes over `hours`, in order.
fn demo_books(config: SyntheticVenueConfig, hours: i64) -> Vec<OrderBook> {
    let mut venue = SyntheticPredictionVenue::new(config, now()).expect("venue");
    venue
        .poll(now().saturating_add(Duration::from_hours(hours)))
        .expect("poll")
        .into_iter()
        .filter_map(|update| match update {
            PredictionUpdate::Book { book, .. } => Some(*book),
            _ => None,
        })
        .collect()
}

#[test]
fn the_demo_venue_quotes_a_real_spread_around_a_real_fair_price() {
    // The failure this prevents: the half-spread, the price floors and the
    // level tick were `Decimal::parse(..).unwrap_or(Decimal::ZERO)` on literal
    // constants. Every one of those fallbacks is silent and every one produces
    // a book that reads as free liquidity — a zero half-spread quotes bid and
    // ask at the same price, a zero tick stacks every level on the touch, and a
    // fair price that fell back to zero pins the quote to the floor. Asserting
    // the constants parse would prove nothing, because they always parse; these
    // are the observable consequences of each one being wrong.
    //
    // Every expected value below is a literal. Deriving one from
    // `config.half_spread` would compare the quote against the same constant
    // that produced it, and a mutation setting that constant to zero passed the
    // first draft of this test unharmed.
    let config = SyntheticVenueConfig::demo(11).expect("config");
    assert_eq!(
        config.half_spread,
        price("0.005"),
        "the demo venue quotes half a cent either side of fair"
    );
    let books = demo_books(config, 6);
    assert!(
        books.len() >= 6,
        "premise: the venue must publish books before any of them can be checked, got {}",
        books.len()
    );

    for book in &books {
        let bid = book.best_bid().expect("a two-sided book has a bid").price;
        let ask = book.best_ask().expect("a two-sided book has an ask").price;

        assert_eq!(
            ask - bid,
            price("0.01"),
            "the touch must be exactly the half-spread either side of fair; a zero half-spread \
             quotes a crossed book at the fair price"
        );
        assert!(
            bid > price("0.005") && ask > price("0.01"),
            "a fair price defaulted to zero pins the quote to its floor: bid {bid}, ask {ask}"
        );

        let asks: Vec<Decimal> = book.asks.iter().map(|level| level.price).collect();
        assert!(
            asks.len() >= 2,
            "premise: the tick needs two levels to show"
        );
        assert_eq!(
            asks[1] - asks[0],
            price("0.002"),
            "a zero tick stacks every level on the touch and overstates depth at the best price"
        );
    }
}

#[test]
fn the_arbitrage_discount_is_spread_across_the_legs_rather_than_dropped() {
    // The failure this prevents: the per-leg discount was
    // `discount.checked_div(..).unwrap_or(Decimal::ZERO)`, so a division that
    // failed priced the complete set at its payoff and the arbitrage the step
    // was drawn to contain would simply not be there. Holding the seed fixed
    // and setting the depth to zero isolates the discount: the same steps draw
    // the same fair prices, so any difference between the two runs is the
    // discount and nothing else.
    let with_depth = SyntheticVenueConfig::demo(4).expect("config");
    let without_depth = SyntheticVenueConfig {
        arbitrage_depth: Decimal::ZERO,
        ..with_depth.clone()
    };
    assert_eq!(
        with_depth.arbitrage_depth,
        price("0.03"),
        "premise: the depth is a literal here rather than read back from the config, so a depth \
         mutated to zero cannot agree with itself"
    );
    // Three outcomes share 0.03.
    let per_leg = price("0.01");

    let discounted = demo_books(with_depth, 6);
    let undiscounted = demo_books(without_depth, 6);
    assert_eq!(
        discounted.len(),
        undiscounted.len(),
        "the same seed must draw the same number of steps"
    );

    let mut discounted_steps = 0;
    for (a, b) in discounted.iter().zip(undiscounted.iter()) {
        let moved = b.best_ask().expect("ask").price - a.best_ask().expect("ask").price;
        if moved.is_zero() {
            continue;
        }
        assert_eq!(
            moved, per_leg,
            "a discounted leg is cheaper by exactly the depth divided across the legs"
        );
        discounted_steps += 1;
    }
    assert!(
        discounted_steps > 0,
        "premise: this seed must draw at least one arbitrage step, or the comparison above never \
         runs and the test guards nothing"
    );
}

#[test]
fn the_venue_api_adapter_names_the_endpoints_and_credential_it_is_missing() {
    let mut adapter = VenueApiAdapter::new(
        VenueApiConfig::standard(VenueId::new("REAL-VENUE")),
        false,
        false,
    );
    assert!(!adapter.is_available());
    let error = adapter
        .poll(now())
        .expect_err("an unavailable venue must not return markets");
    assert_eq!(error.code(), "unavailable");
    for required in [
        "QIP_PREDICTION_API_ENDPOINT",
        "QIP_PREDICTION_API_CREDENTIAL",
        "GET /markets/{id}/resolution-criteria",
        "GET /markets/{id}/disputes",
    ] {
        assert!(
            error.message().contains(required),
            "the requirement should name {required}, got: {}",
            error.message()
        );
    }
}

#[test]
fn the_same_outcome_is_matched_across_venues_by_its_criteria_not_by_its_label() {
    let left = market(
        "PREDICT-A",
        "release",
        -25,
        resolves_at(),
        FeeSchedule::FREE,
    );

    // The same proposition, listed with a different outcome identifier.
    let criteria = ResolutionCriteria::Threshold {
        metric: "policy_rate_change_bp".to_string(),
        comparison: Comparison::AtMost,
        value: Decimal::from_int(-25),
    };
    let renamed = Outcome::new(
        OutcomeId::new("EASING"),
        "easing",
        ObjectId::from_string("PREDICT-B-EASING"),
        criteria.clone(),
    );
    let right = EventMarket::new(
        ObjectId::from_string("PREDICT-B-MARKET"),
        VenueId::new("PREDICT-B"),
        VenueClass::PredictionMarket,
        Proposition::new(
            "the policy rate decision",
            criteria,
            ResolutionSource::new(
                "release",
                SourceKind::Official,
                vec!["policy_rate_change_bp".to_string()],
            ),
            resolves_at(),
            SettlementRule::unit(UndeterminedRule::VoidAndRefund),
            Duration::from_hours(24),
        )
        .expect("the source publishes the metric"),
        MarketKind::binary(
            renamed,
            OutcomeId::new("NO-EASING"),
            ObjectId::from_string("PREDICT-B-NO-EASING"),
        )
        .expect("distinct yes/no ids"),
        FeeSchedule::FREE,
    )
    .expect("a well-formed market");

    let pair = CrossMarketPair::new(&left, &right).expect("the same proposition");
    let cheap = book(
        "PREDICT-A-YES",
        "PREDICT-A",
        &[("0.38", 100)],
        &[("0.40", 25)],
    );
    let rich = book(
        "PREDICT-B-EASING",
        "PREDICT-B",
        &[("0.46", 40)],
        &[("0.48", 40)],
    );
    let found = pair
        .arbitrage(&OutcomeId::new("yes"), &cheap, &rich)
        .expect("the walk succeeds")
        .expect("the outcomes match on criteria even though their labels differ");
    assert_eq!(found.quantity, Decimal::from_int(25));
    assert_eq!(
        found.plan.steps()[1].object_id,
        ObjectId::from_string("PREDICT-B-EASING"),
        "the leg must name the instrument the other venue actually lists"
    );
}
