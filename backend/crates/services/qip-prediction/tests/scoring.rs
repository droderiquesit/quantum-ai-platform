//! Brier scoring of the platform against the market, on the same contracts at
//! the same instants.
//!
//! The properties here are the ones that make the Phase 6 gate's answer worth
//! believing: that the comparison can find for the market as readily as for
//! the platform, that a quote the platform could not have seen is refused
//! rather than scored, that nothing is scored over nothing, and that the error
//! bar on "better" narrows as evidence accumulates rather than being a fixed
//! decoration.

use qip_contracts::{Stamped, VenueClass, VenueId};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_prediction::market::{EventMarket, FeeSchedule, MarketKind, Outcome, OutcomeId};
use qip_prediction::oracle::{
    MarketResolution, OracleIdentity, OracleKind, OracleReport, ResolutionState,
};
use qip_prediction::pricing::Probability;
use qip_prediction::resolution::{
    Comparison, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule, SourceKind,
    UndeterminedRule,
};
use qip_prediction::scoring::{ScoredForecast, brier_score, compare, market_brier_score};

fn formed_at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn binary_market(id: &str) -> EventMarket {
    let criteria = ResolutionCriteria::Threshold {
        metric: "policy_rate_change_bp".to_string(),
        comparison: Comparison::AtMost,
        value: Decimal::from_int(-25),
    };
    let source = ResolutionSource::new(
        "central-bank-statistical-release",
        SourceKind::Official,
        vec!["policy_rate_change_bp".to_string()],
    );
    let proposition = Proposition::new(
        "the policy rate decision",
        criteria.clone(),
        source,
        formed_at().saturating_add(Duration::from_days(30)),
        SettlementRule::unit(UndeterminedRule::VoidAndRefund),
        Duration::from_hours(24),
    )
    .expect("the source publishes the metric");
    let yes = Outcome::new(
        OutcomeId::new("yes"),
        "a cut of at least 25bp",
        ObjectId::from_string(format!("{id}-YES")),
        criteria,
    );
    EventMarket::new(
        ObjectId::from_string(id),
        VenueId::new("PREDICT-A"),
        VenueClass::PredictionMarket,
        proposition,
        MarketKind::binary(
            yes,
            OutcomeId::new("no"),
            ObjectId::from_string(format!("{id}-NO")),
        )
        .expect("distinct yes/no ids"),
        FeeSchedule::FREE,
    )
    .expect("a well-formed binary market")
}

/// A resolution driven all the way to `Final` on `winner`, through the same
/// state machine a real one takes: proposed, then finalised after the window.
fn final_resolution(market: &EventMarket, winner: &str) -> MarketResolution {
    let identity = OracleIdentity::new(
        "attestor",
        OracleKind::Attestor,
        Duration::from_hours(24),
        0.5,
    )
    .expect("a valid identity");
    let mut resolution = MarketResolution::new(market, identity);
    let reported_at = market.proposition.resolves_at;
    resolution
        .observe(
            Some(OracleReport {
                outcome: OutcomeId::new(winner),
                confidence: 1.0,
                reported_at,
                evidence: "the release".to_string(),
            }),
            reported_at,
        )
        .expect("a pending resolution accepts a confident report");
    resolution
        .finalise(reported_at.saturating_add(Duration::from_hours(25)))
        .expect("an unchallenged window closes");
    assert!(
        matches!(resolution.state(), ResolutionState::Final { .. }),
        "the fixture must reach a final resolution or nothing below scores anything"
    );
    resolution
}

fn probability(text: &str) -> Probability {
    Probability::new(Decimal::parse(text).expect("a decimal literal")).expect("a probability")
}

/// One scored contract on the `yes` outcome, both probabilities stamped as
/// known at the instant the platform formed its own.
fn scored(id: &str, platform: &str, market: &str, yes_won: bool) -> ScoredForecast {
    let event = binary_market(id);
    let resolution = final_resolution(&event, if yes_won { "yes" } else { "no" });
    ScoredForecast::new(
        ObjectId::from_string(id),
        OutcomeId::new("yes"),
        Stamped::immediate(probability(platform), formed_at()),
        Stamped::immediate(probability(market), formed_at()),
        &resolution,
    )
    .expect("a same-instant pair on a final resolution scores")
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

#[test]
fn a_platform_closer_to_every_outcome_than_the_market_scores_better_with_a_negative_difference() {
    // Platform 0.8 / market 0.6 on three yes-wins, platform 0.2 / market 0.4
    // on two no-wins: platform Brier 0.04 everywhere, market 0.16 everywhere.
    let set = vec![
        scored("M1", "0.8", "0.6", true),
        scored("M2", "0.8", "0.6", true),
        scored("M3", "0.8", "0.6", true),
        scored("M4", "0.2", "0.4", false),
        scored("M5", "0.2", "0.4", false),
    ];
    assert_eq!(set.len(), 5, "the premise is a five-contract set");
    assert!(
        set.iter()
            .filter(|forecast| forecast.resolved_yes())
            .count()
            == 3,
        "the fixture must carry both outcomes, or the score is one-sided"
    );

    let platform = brier_score(&set).expect("a non-empty set scores");
    let market = market_brier_score(&set).expect("a non-empty set scores");
    assert!(close(platform, 0.04), "platform Brier was {platform}");
    assert!(close(market, 0.16), "market Brier was {market}");

    let comparison = compare(&set).expect("five contracts compare");
    assert!(
        close(comparison.difference, -0.12),
        "the difference must be platform minus market, got {}",
        comparison.difference
    );
    assert!(
        comparison.difference < 0.0,
        "a negative difference is the platform being closer to what happened"
    );
    assert_eq!(comparison.count, 5);
    // Every contract has the same difference, so there is no variance: the
    // platform beat the market on every single contract and no error bar
    // should say otherwise.
    assert!(
        close(comparison.standard_error, 0.0),
        "identical per-contract differences have no spread, got {}",
        comparison.standard_error
    );
    assert!(
        comparison.platform_beats_market_by(2.0),
        "an unanimous set beats the market at any number of sigmas"
    );
}

#[test]
fn a_market_closer_to_every_outcome_than_the_platform_wins_the_comparison_so_it_is_not_rigged() {
    // The mirror of the set above: the market's 0.8 against the platform's
    // 0.6. If the comparison could only ever find for the platform, this
    // would be the test that noticed.
    let set = vec![
        scored("M1", "0.6", "0.8", true),
        scored("M2", "0.6", "0.8", true),
        scored("M3", "0.4", "0.2", false),
        scored("M4", "0.4", "0.2", false),
    ];
    assert_eq!(set.len(), 4, "the premise is a four-contract set");

    let comparison = compare(&set).expect("four contracts compare");
    assert!(
        close(comparison.platform, 0.16),
        "platform Brier was {}",
        comparison.platform
    );
    assert!(
        close(comparison.market, 0.04),
        "market Brier was {}",
        comparison.market
    );
    assert!(
        close(comparison.difference, 0.12),
        "a positive difference is the market being closer, got {}",
        comparison.difference
    );
    assert!(
        !comparison.platform_beats_market_by(0.0),
        "a platform that lost on every contract must not be reported as better"
    );
    assert!(
        comparison.z_score() >= 0.0,
        "the z-score must carry the sign of the difference, got {}",
        comparison.z_score()
    );
}

#[test]
fn a_market_quote_known_after_the_platform_formed_its_probability_is_refused_as_leakage() {
    let event = binary_market("M1");
    let resolution = final_resolution(&event, "yes");
    let later = formed_at().saturating_add(Duration::from_secs(1));

    // The same quote one second earlier is accepted: the refusal is about the
    // ordering of known-times, not about the quote.
    let same_instant = ScoredForecast::new(
        ObjectId::from_string("M1"),
        OutcomeId::new("yes"),
        Stamped::immediate(probability("0.7"), formed_at()),
        Stamped::immediate(probability("0.6"), formed_at()),
        &resolution,
    );
    assert!(
        same_instant.is_ok(),
        "a quote known at the same instant is exactly the one to score against"
    );

    let leaked = ScoredForecast::new(
        ObjectId::from_string("M1"),
        OutcomeId::new("yes"),
        Stamped::immediate(probability("0.7"), formed_at()),
        Stamped::new(probability("0.6"), formed_at(), later),
        &resolution,
    );
    let error = match leaked {
        Ok(forecast) => panic!("a later quote was scored: {forecast:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("became known at") && error.contains("after the platform's probability"),
        "the refusal must name the leakage, got: {error}"
    );
}

#[test]
fn a_forecast_on_an_unresolved_or_foreign_resolution_is_refused_rather_than_scored_as_zero() {
    let event = binary_market("M1");
    let pending = MarketResolution::new(
        &event,
        OracleIdentity::new(
            "attestor",
            OracleKind::Attestor,
            Duration::from_hours(24),
            0.5,
        )
        .expect("a valid identity"),
    );
    assert!(
        matches!(pending.state(), ResolutionState::Pending),
        "the premise is an unresolved market"
    );
    let unresolved = ScoredForecast::new(
        ObjectId::from_string("M1"),
        OutcomeId::new("yes"),
        Stamped::immediate(probability("0.7"), formed_at()),
        Stamped::immediate(probability("0.6"), formed_at()),
        &pending,
    );
    let error = match unresolved {
        Ok(forecast) => panic!("an unresolved market was scored: {forecast:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("is pending and has no outcome"),
        "the refusal must name the state, got: {error}"
    );

    let other = final_resolution(&binary_market("M2"), "yes");
    let foreign = ScoredForecast::new(
        ObjectId::from_string("M1"),
        OutcomeId::new("yes"),
        Stamped::immediate(probability("0.7"), formed_at()),
        Stamped::immediate(probability("0.6"), formed_at()),
        &other,
    );
    let error = match foreign {
        Ok(forecast) => panic!("another market's resolution was scored: {forecast:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("is for market M2, not M1"),
        "the refusal must name both markets, got: {error}"
    );
}

#[test]
fn an_empty_set_is_refused_rather_than_scored_as_a_perfect_zero() {
    let empty: Vec<ScoredForecast> = Vec::new();
    assert!(empty.is_empty(), "the premise is an empty set");

    let error = match brier_score(&empty) {
        Ok(score) => panic!("an empty set was scored {score}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("no resolved contracts to score"),
        "the refusal must say there was nothing to score, got: {error}"
    );

    let error = match market_brier_score(&empty) {
        Ok(score) => panic!("an empty set was scored {score} for the market"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("no resolved contracts to score"),
        "got: {error}"
    );

    let error = match compare(&empty) {
        Ok(comparison) => panic!("an empty set was compared: {comparison:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("at least two resolved contracts and was given 0"),
        "the refusal must state the count, got: {error}"
    );

    // One contract has a difference but no variance; an error bar of zero on
    // it would present one contract's luck as certainty.
    let one = vec![scored("M1", "0.8", "0.6", true)];
    let error = match compare(&one) {
        Ok(comparison) => panic!("a single contract was compared: {comparison:?}"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("was given 1"),
        "the refusal must state the count, got: {error}"
    );
}

#[test]
fn the_standard_error_of_the_difference_shrinks_as_more_contracts_resolve() {
    // A mixed set: on half the contracts the platform is better by 0.12, on
    // the other half the market is better by 0.12, so the per-contract
    // differences have a fixed spread and the error bar's only job is to
    // divide it by the square root of the count.
    fn mixed(pairs: usize) -> Vec<ScoredForecast> {
        (0..pairs)
            .flat_map(|index| {
                [
                    scored(&format!("P{index}"), "0.8", "0.6", true),
                    scored(&format!("Q{index}"), "0.6", "0.8", true),
                ]
            })
            .collect()
    }
    let small = compare(&mixed(2)).expect("four contracts compare");
    let large = compare(&mixed(8)).expect("sixteen contracts compare");
    assert_eq!(small.count, 4);
    assert_eq!(large.count, 16);
    assert!(
        close(small.difference, 0.0) && close(large.difference, 0.0),
        "the premise is a set on which the two forecasters tie on average"
    );
    assert!(
        small.standard_error > 0.0,
        "a mixed set has a spread, got {}",
        small.standard_error
    );
    // Sample standard deviation of ±0.12 is 0.12 * sqrt(n / (n - 1)); the
    // standard error divides by sqrt(n), so it is 0.12 / sqrt(n - 1).
    assert!(
        close(small.standard_error, 0.12 / 3.0_f64.sqrt()),
        "four contracts: {}",
        small.standard_error
    );
    assert!(
        close(large.standard_error, 0.12 / 15.0_f64.sqrt()),
        "sixteen contracts: {}",
        large.standard_error
    );
    assert!(
        large.standard_error < small.standard_error,
        "more evidence must narrow the error bar: {} against {}",
        large.standard_error,
        small.standard_error
    );
    assert!(
        !small.platform_beats_market_by(1.0) && !large.platform_beats_market_by(1.0),
        "a tie is not a win at any error bar"
    );
}
