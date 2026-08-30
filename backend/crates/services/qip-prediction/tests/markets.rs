//! Propositions, market structure, implied probability and resolution.
//!
//! The properties here are the ones that decide whether a position in an event
//! market is what its holder thinks it is: that the criteria can be evaluated
//! by a program, that the outcomes cover the world exactly once, that the
//! probability read off a price accounts for what it costs to act on, and that
//! a resolution nobody has agreed to cannot be settled.

use qip_contracts::{VenueClass, VenueId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use qip_prediction::market::{
    EventMarket, FeeSchedule, MarketKind, MarketVerdict, Outcome, OutcomeId,
};
use qip_prediction::oracle::{
    Dispute, MarketResolution, Oracle, OracleIdentity, OracleKind, OracleReport, ResolutionState,
    ScriptedOracle,
};
use qip_prediction::pricing::{
    Probability, implied_band, implied_from_ask, implied_from_bid, naive_probability,
};
use qip_prediction::resolution::{
    Comparison, Observation, Observations, Proposition, ResolutionCriteria, ResolutionSource,
    SettlementRule, SourceKind, UndeterminedRule, Verdict,
};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn source() -> ResolutionSource {
    ResolutionSource::new(
        "central-bank-statistical-release",
        SourceKind::Official,
        vec!["policy_rate_change_bp".to_string()],
    )
}

fn rate_criteria(comparison: Comparison, value: i64) -> ResolutionCriteria {
    ResolutionCriteria::Threshold {
        metric: "policy_rate_change_bp".to_string(),
        comparison,
        value: Decimal::from_int(value),
    }
}

fn proposition(criteria: ResolutionCriteria) -> Proposition {
    Proposition::new(
        "the policy rate decision",
        criteria,
        source(),
        now().saturating_add(Duration::from_days(30)),
        SettlementRule::unit(UndeterminedRule::VoidAndRefund),
        Duration::from_hours(24),
    )
    .expect("the source publishes the metric")
}

fn binary_market(fees: FeeSchedule) -> EventMarket {
    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "a cut of at least 25bp",
        ObjectId::from_string("YES"),
        rate_criteria(Comparison::AtMost, -25),
    );
    EventMarket::new(
        ObjectId::from_string("MARKET"),
        VenueId::new("PREDICT-A"),
        VenueClass::PredictionMarket,
        proposition(rate_criteria(Comparison::AtMost, -25)),
        MarketKind::binary(yes, OutcomeId::new("no"), ObjectId::from_string("NO")),
        fees,
    )
    .expect("a well-formed binary market")
}

fn observed(value: i64) -> Observations {
    Observations::at(now()).with(
        "policy_rate_change_bp",
        Observation::Numeric(Decimal::from_int(value)),
    )
}

// --- resolution criteria ----------------------------------------------------

#[test]
fn resolution_criteria_are_evaluated_from_published_observations_rather_than_from_prose() {
    let market = binary_market(FeeSchedule::FREE);
    assert_eq!(
        market.evaluate(&observed(-50)),
        MarketVerdict::Resolved(OutcomeId::new("yes")),
        "a 50bp cut resolves the yes outcome"
    );
    assert_eq!(
        market.evaluate(&observed(0)),
        MarketVerdict::Resolved(OutcomeId::new("no")),
        "no change resolves the no outcome"
    );
    assert_eq!(
        market.evaluate(&observed(-25)),
        MarketVerdict::Resolved(OutcomeId::new("yes")),
        "the boundary belongs to exactly one side and the criteria say which"
    );
}

#[test]
fn a_market_whose_source_has_published_nothing_is_undetermined_rather_than_resolved_against() {
    let market = binary_market(FeeSchedule::FREE);
    let verdict = market.evaluate(&Observations::at(now()));
    let MarketVerdict::Undetermined { missing } = verdict else {
        panic!("an unpublished metric cannot resolve a market, got {verdict:?}");
    };
    assert_eq!(missing, vec!["policy_rate_change_bp".to_string()]);
}

#[test]
fn a_proposition_whose_source_does_not_publish_its_metrics_is_refused() {
    let error = Proposition::new(
        "something nobody measures",
        ResolutionCriteria::Threshold {
            metric: "unpublished_metric".to_string(),
            comparison: Comparison::AtLeast,
            value: Decimal::ONE,
        },
        source(),
        now(),
        SettlementRule::unit(UndeterminedRule::VoidAndRefund),
        Duration::from_hours(1),
    )
    .expect_err("a market that cannot be settled must not be listed");
    assert_eq!(error.code(), "invalid");
    assert!(error.message().contains("unpublished_metric"));
}

#[test]
fn the_same_conjunction_written_in_either_order_is_the_same_proposition() {
    let left = ResolutionCriteria::All(vec![
        rate_criteria(Comparison::AtMost, -25),
        rate_criteria(Comparison::AtLeast, -100),
    ]);
    let right = ResolutionCriteria::All(vec![
        rate_criteria(Comparison::AtLeast, -100),
        rate_criteria(Comparison::AtMost, -25),
    ]);
    assert_eq!(
        left.digest(),
        right.digest(),
        "argument order does not change the question being asked"
    );

    let different = ResolutionCriteria::All(vec![
        rate_criteria(Comparison::LessThan, -25),
        rate_criteria(Comparison::AtLeast, -100),
    ]);
    assert_ne!(
        left.digest(),
        different.digest(),
        "a different comparison is a different question"
    );
}

#[test]
fn a_conjunction_fails_as_soon_as_one_part_fails_even_with_another_unobserved() {
    let criteria = ResolutionCriteria::All(vec![
        rate_criteria(Comparison::AtMost, -25),
        ResolutionCriteria::Flag {
            metric: "statement_mentions_inflation".to_string(),
            expected: true,
        },
    ]);
    assert_eq!(
        criteria.evaluate(&observed(0)),
        Verdict::Fails,
        "the rate part already decides it; waiting for the flag learns nothing"
    );
    let partial = criteria.evaluate(&observed(-50));
    assert!(
        !partial.is_determined(),
        "with the deciding part satisfied, the unobserved one still matters"
    );
}

// --- market structure -------------------------------------------------------

#[test]
fn a_binary_markets_no_outcome_is_the_exact_complement_of_its_yes() {
    let market = binary_market(FeeSchedule::FREE);
    for change in [-200, -26, -25, -24, 0, 25, 300] {
        let verdict = market.evaluate(&observed(change));
        assert!(
            matches!(verdict, MarketVerdict::Resolved(_)),
            "a change of {change}bp must resolve to exactly one side, got {verdict:?}"
        );
    }
}

#[test]
fn a_categorical_market_whose_outcomes_share_criteria_is_refused() {
    let duplicate = || {
        Outcome::new(
            OutcomeId::new("a"),
            "a cut",
            ObjectId::from_string("A"),
            rate_criteria(Comparison::AtMost, -25),
        )
    };
    let mut second = duplicate();
    second.id = OutcomeId::new("b");
    second.object_id = ObjectId::from_string("B");
    let error = MarketKind::categorical(vec![duplicate(), second])
        .expect_err("outcomes that resolve identically are not mutually exclusive");
    assert_eq!(error.code(), "invalid");
}

#[test]
fn a_market_on_a_venue_that_is_not_a_prediction_market_is_refused() {
    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "a cut",
        ObjectId::from_string("YES"),
        rate_criteria(Comparison::AtMost, -25),
    );
    let error = EventMarket::new(
        ObjectId::from_string("MARKET"),
        VenueId::new("XNYS"),
        VenueClass::Exchange,
        proposition(rate_criteria(Comparison::AtMost, -25)),
        MarketKind::binary(yes, OutcomeId::new("no"), ObjectId::from_string("NO")),
        FeeSchedule::FREE,
    )
    .expect_err("an exchange does not list event contracts on these terms");
    assert_eq!(error.code(), "invalid");
}

#[test]
fn a_scalar_markets_buckets_partition_the_range_with_no_gap_and_no_overlap() {
    let edges = vec![
        Decimal::from_int(-50),
        Decimal::from_int(-25),
        Decimal::ZERO,
        Decimal::from_int(25),
        Decimal::from_int(50),
    ];
    let kind = MarketKind::scalar("policy_rate_change_bp", edges, |index| {
        ObjectId::from_string(format!("BUCKET-{index}"))
    })
    .expect("edges that increase produce a partition");
    assert_eq!(kind.outcomes().len(), 6, "five edges make six buckets");

    Property::new("every value falls in exactly one bucket")
        .cases(1_000)
        .for_all(
            |rng: &mut Xoshiro256| {
                let units = rng.below(200_001) as i128 - 100_000;
                let fraction = rng.below(1_000_000_000) as i128;
                Decimal::from_raw(units * 1_000_000_000 + fraction)
            },
            |value| {
                let containing: Vec<_> = kind
                    .outcomes()
                    .into_iter()
                    .filter(|outcome| {
                        outcome
                            .criteria
                            .evaluate(
                                &Observations::at(now())
                                    .with("policy_rate_change_bp", Observation::Numeric(*value)),
                            )
                            .holds()
                    })
                    .collect();
                if containing.len() != 1 {
                    return Err(format!(
                        "{value} is claimed by {} buckets",
                        containing.len()
                    ));
                }
                let bucket = kind.bucket_for(*value).map_err(|e| e.to_string())?;
                if bucket.outcome.id != containing[0].id {
                    return Err(format!(
                        "bucket_for said {} but the criteria said {}",
                        bucket.outcome.id, containing[0].id
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn scalar_edges_that_do_not_increase_are_refused() {
    let error = MarketKind::scalar(
        "policy_rate_change_bp",
        vec![Decimal::from_int(25), Decimal::from_int(25)],
        |index| ObjectId::from_string(format!("BUCKET-{index}")),
    )
    .expect_err("a repeated edge would make an empty bucket and an ambiguous boundary");
    assert_eq!(error.code(), "invalid");
}

// --- implied probability ----------------------------------------------------

#[test]
fn implied_probability_accounts_for_fees_and_never_leaves_the_unit_interval() {
    Property::new("implied probability stays a probability")
        .cases(1_000)
        .for_all(
            |rng: &mut Xoshiro256| {
                let price = Decimal::from_raw(rng.below(1_000_000_001) as i128);
                let taker = rng.below(500) as u32;
                let settlement = rng.below(500) as u32;
                (price, taker, settlement)
            },
            |(price, taker, settlement)| {
                let fees = FeeSchedule::new(*taker, 0, *settlement).map_err(|e| e.to_string())?;
                let payoff = Decimal::ONE;
                let naive = naive_probability(*price, payoff).map_err(|e| e.to_string())?;

                if let Ok(ask) = implied_from_ask(*price, &fees, payoff) {
                    if ask.value() > Decimal::ONE || ask.value().is_negative() {
                        return Err(format!("{} is not a probability", ask.value()));
                    }
                    if ask.value() < naive.value() {
                        return Err(format!(
                            "fees must raise a buyer's break-even, {} < {}",
                            ask.value(),
                            naive.value()
                        ));
                    }
                }
                let bid = implied_from_bid(*price, &fees, payoff).map_err(|e| e.to_string())?;
                if bid.value() > naive.value() {
                    return Err(format!(
                        "fees must lower a seller's break-even, {} > {}",
                        bid.value(),
                        naive.value()
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn the_band_a_book_implies_is_wider_than_its_spread_once_fees_are_paid() {
    let fees = FeeSchedule::new(200, 0, 100).expect("a valid schedule");
    let book = OrderBook::from_levels(
        ObjectId::from_string("YES"),
        "PREDICT-A",
        now(),
        vec![BookLevel::new(
            Decimal::parse("0.60").expect("parses"),
            Decimal::from_int(100),
        )],
        vec![BookLevel::new(
            Decimal::parse("0.62").expect("parses"),
            Decimal::from_int(100),
        )],
    );
    let band = implied_band(&book, &fees, Decimal::ONE).expect("a two-sided book implies a band");
    let spread = book.spread().expect("a two-sided book has a spread");
    assert!(
        band.width() > spread,
        "the band {} should exceed the raw spread {spread}",
        band.width()
    );
    assert!(
        band.lower().value() < band.upper().value(),
        "the seller's break-even sits below the buyer's"
    );
    assert!(
        !band.admits(band.midpoint().expect("a midpoint")),
        "a forecast inside the band is not tradable"
    );
    assert!(
        band.admits(Probability::new(Decimal::parse("0.95").expect("parses")).expect("valid")),
        "a forecast far above the band is tradable"
    );
}

#[test]
fn the_naive_reading_of_a_price_understates_what_a_buyer_must_be_right_about() {
    let fees = FeeSchedule::new(200, 0, 100).expect("a valid schedule");
    let price = Decimal::parse("0.50").expect("parses");
    let naive = naive_probability(price, Decimal::ONE).expect("a probability");
    let adjusted = implied_from_ask(price, &fees, Decimal::ONE).expect("a probability");
    assert!(
        adjusted.value() > naive.value(),
        "a buyer at {price} needs more than {} to break even, not exactly it",
        naive.value()
    );
}

#[test]
fn a_price_implying_more_than_certainty_is_reported_as_an_arbitrage_not_clamped() {
    let fees = FeeSchedule::new(100, 0, 100).expect("a valid schedule");
    let error = implied_from_ask(Decimal::parse("1.05").expect("parses"), &fees, Decimal::ONE)
        .expect_err("a price above the payoff is not a belief");
    assert_eq!(error.code(), "invalid");
    assert!(
        error.message().contains("arbitrage"),
        "the error should name what it actually is, got {error}"
    );
}

// --- oracles and settlement -------------------------------------------------

fn oracle_identity() -> OracleIdentity {
    OracleIdentity::new(
        "optimistic-resolver",
        OracleKind::Optimistic,
        Duration::from_hours(24),
        0.8,
    )
    .expect("a valid identity")
}

fn report(outcome: &str, at: Timestamp, confidence: f64) -> OracleReport {
    OracleReport {
        outcome: OutcomeId::new(outcome),
        confidence,
        reported_at: at,
        evidence: "the published release".to_string(),
    }
}

#[test]
fn a_disputed_resolution_blocks_settlement() {
    let market = binary_market(FeeSchedule::FREE);
    let mut resolution = MarketResolution::new(&market, oracle_identity());
    let resolves_at = market.proposition.resolves_at;

    resolution
        .observe(Some(report("yes", resolves_at, 0.95)), resolves_at)
        .expect("a confident report is accepted");
    assert!(
        matches!(resolution.state(), ResolutionState::Proposed { .. }),
        "a report starts a dispute window rather than ending the market"
    );
    let proposed_error = resolution
        .settle(&OutcomeId::new("yes"), Decimal::from_int(100), &market.fees)
        .expect_err("a proposed resolution is not money");
    assert_eq!(proposed_error.code(), "denied");

    resolution
        .dispute(Dispute {
            raised_by: "counterparty".to_string(),
            reason: "the release was revised".to_string(),
            raised_at: resolves_at.saturating_add(Duration::from_hours(2)),
            competing: Some(OutcomeId::new("no")),
        })
        .expect("a challenge inside the window is accepted");

    let error = resolution
        .settle(&OutcomeId::new("yes"), Decimal::from_int(100), &market.fees)
        .expect_err("a disputed resolution must not settle");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("disputed"),
        "the refusal should say why, got {error}"
    );
    assert!(
        resolution
            .finalise(resolves_at.saturating_add(Duration::from_days(7)))
            .is_err(),
        "waiting does not resolve a dispute"
    );
}

#[test]
fn an_unchallenged_resolution_settles_once_its_window_closes() {
    let market = binary_market(FeeSchedule::new(0, 0, 100).expect("a valid schedule"));
    let mut resolution = MarketResolution::new(&market, oracle_identity());
    let resolves_at = market.proposition.resolves_at;
    resolution
        .observe(Some(report("yes", resolves_at, 0.99)), resolves_at)
        .expect("accepted");

    assert!(
        resolution
            .finalise(resolves_at.saturating_add(Duration::from_hours(1)))
            .is_err(),
        "the window has not closed"
    );
    let outcome = resolution
        .finalise(resolves_at.saturating_add(Duration::from_hours(25)))
        .expect("an unchallenged window closes");
    assert_eq!(outcome, OutcomeId::new("yes"));

    let settlement = resolution
        .settle(&OutcomeId::new("yes"), Decimal::from_int(100), &market.fees)
        .expect("a final resolution settles");
    assert_eq!(settlement.gross, Decimal::from_int(100));
    assert_eq!(settlement.fee, Decimal::ONE, "1% of the payoff");
    assert_eq!(settlement.net, Decimal::from_int(99));

    let loser = resolution
        .settle(&OutcomeId::new("no"), Decimal::from_int(100), &market.fees)
        .expect("the losing side settles too");
    assert!(loser.net.is_zero(), "the losing outcome pays nothing");
}

#[test]
fn an_overturned_dispute_settles_the_outcome_the_challenger_named() {
    let market = binary_market(FeeSchedule::FREE);
    let mut resolution = MarketResolution::new(&market, oracle_identity());
    let resolves_at = market.proposition.resolves_at;
    resolution
        .observe(Some(report("yes", resolves_at, 0.9)), resolves_at)
        .expect("accepted");
    resolution
        .dispute(Dispute {
            raised_by: "counterparty".to_string(),
            reason: "the release was revised".to_string(),
            raised_at: resolves_at.saturating_add(Duration::from_hours(1)),
            competing: Some(OutcomeId::new("no")),
        })
        .expect("challenged");
    resolution
        .overturn(
            Some(OutcomeId::new("no")),
            resolves_at.saturating_add(Duration::from_days(2)),
        )
        .expect("the challenge succeeds");

    let winner = resolution
        .settle(&OutcomeId::new("no"), Decimal::from_int(10), &market.fees)
        .expect("settles");
    assert_eq!(winner.net, Decimal::from_int(10));
    let loser = resolution
        .settle(&OutcomeId::new("yes"), Decimal::from_int(10), &market.fees)
        .expect("settles");
    assert!(
        loser.net.is_zero(),
        "the position that was 'risk-free' at 0.97 is worth nothing"
    );
}

#[test]
fn a_delayed_oracle_is_visible_as_overdue_rather_than_assumed_to_have_resolved() {
    let market = binary_market(FeeSchedule::FREE);
    let mut resolution = MarketResolution::new(&market, oracle_identity());
    let late = market
        .proposition
        .resolves_at
        .saturating_add(Duration::from_days(3));

    resolution
        .observe(None, late)
        .expect("no report is not an error");
    assert!(matches!(
        resolution.state(),
        ResolutionState::Overdue { .. }
    ));
    assert!(resolution.is_delayed(late));
    assert!(
        resolution
            .settle(&OutcomeId::new("yes"), Decimal::ONE, &market.fees)
            .is_err(),
        "an overdue market has nothing to settle"
    );
}

#[test]
fn a_report_the_oracle_is_not_confident_enough_about_is_refused() {
    let market = binary_market(FeeSchedule::FREE);
    let mut resolution = MarketResolution::new(&market, oracle_identity());
    let resolves_at = market.proposition.resolves_at;
    let error = resolution
        .observe(Some(report("yes", resolves_at, 0.5)), resolves_at)
        .expect_err("an oracle below its own threshold has not reported");
    assert_eq!(error.code(), "guard");
    assert!(matches!(resolution.state(), ResolutionState::Pending));
}

#[test]
fn a_scripted_oracle_reports_only_once_its_moment_has_arrived() {
    let market = binary_market(FeeSchedule::FREE);
    let resolves_at = market.proposition.resolves_at;
    let oracle = ScriptedOracle::new(oracle_identity()).schedule(
        &market.market_id,
        resolves_at,
        report("yes", resolves_at, 0.95),
    );
    assert!(
        oracle
            .report(&market, resolves_at.saturating_sub(Duration::from_hours(1)))
            .expect("polling early is legal")
            .is_none(),
        "an oracle cannot report before it reports"
    );
    assert!(
        oracle
            .report(&market, resolves_at)
            .expect("polling")
            .is_some()
    );
}
