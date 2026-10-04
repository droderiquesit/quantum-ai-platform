//! The platform's own belief about an event: a distribution, a tree of
//! developments that implies one, a dated update, and the fair values a cited
//! belief gives a contract.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::{VenueClass, VenueId};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_prediction::belief::{Arrival, Branch, Development, OutcomeDistribution, fair_values};
use qip_prediction::market::{EventMarket, FeeSchedule, MarketKind, Outcome, OutcomeId};
use qip_prediction::pricing::Probability;
use qip_prediction::resolution::{
    Comparison, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule, SourceKind,
    UndeterminedRule,
};
use std::collections::BTreeMap;

fn t0() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn dec(text: &str) -> Decimal {
    Decimal::parse(text).expect("a decimal literal")
}

fn p(text: &str) -> Probability {
    Probability::new(dec(text)).expect("a probability")
}

fn yes() -> OutcomeId {
    OutcomeId::new("yes")
}

fn no() -> OutcomeId {
    OutcomeId::new("no")
}

fn market() -> EventMarket {
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
        t0().saturating_add(Duration::from_days(30)),
        SettlementRule::new(Decimal::from_int(10), UndeterminedRule::VoidAndRefund)
            .expect("a positive payoff"),
        Duration::from_hours(24),
    )
    .expect("the source publishes the metric");
    let yes_outcome = Outcome::new(yes(), "a cut", ObjectId::from_string("M-YES"), criteria);
    EventMarket::new(
        ObjectId::from_string("M"),
        VenueId::new("PREDICT-A"),
        VenueClass::PredictionMarket,
        proposition,
        MarketKind::binary(yes_outcome, no(), ObjectId::from_string("M-NO")).expect("distinct ids"),
        FeeSchedule::FREE,
    )
    .expect("a well-formed binary market")
}

fn distribution(yes_p: &str, no_p: &str) -> BTreeMap<OutcomeId, Probability> {
    BTreeMap::from([(yes(), p(yes_p)), (no(), p(no_p))])
}

fn arrival(name: &str, knowable_at: Timestamp, yes_l: &str, no_l: &str) -> Arrival {
    Arrival {
        name: name.to_string(),
        knowable_at,
        likelihoods: BTreeMap::from([(yes(), dec(yes_l)), (no(), dec(no_l))]),
    }
}

#[test]
fn a_distribution_that_does_not_sum_to_one_is_refused_rather_than_rescaled() {
    let refused = OutcomeDistribution::new(distribution("0.6", "0.3"), t0(), "stated");
    assert!(refused.is_err(), "0.9 total must be refused");
    let admitted = OutcomeDistribution::new(distribution("0.6", "0.4"), t0(), "stated")
        .expect("a total of exactly one is a distribution");
    assert_eq!(admitted.probability(&yes()), Some(p("0.6")));
    let single = BTreeMap::from([(yes(), Probability::ONE)]);
    assert!(OutcomeDistribution::new(single, t0(), "stated").is_err());
}

#[test]
fn an_arrival_moves_belief_toward_the_outcome_it_favours_and_names_itself() {
    let prior = OutcomeDistribution::new(distribution("0.5", "0.5"), t0(), "stated")
        .expect("a distribution");
    let later = t0().saturating_add(Duration::from_hours(1));
    let news = arrival("hot-cpi-print", t0(), "0.2", "0.8");
    let posterior = prior.update(&news, later).expect("a knowable arrival");
    // Premise: the prior was not already tilted, so the move is the arrival's.
    assert_eq!(prior.probability(&yes()), Some(p("0.5")));
    assert_eq!(posterior.probability(&yes()), Some(p("0.2")));
    assert_eq!(posterior.probability(&no()), Some(p("0.8")));
    assert!(posterior.basis().contains("hot-cpi-print"));
    assert_eq!(posterior.as_of(), later);
}

#[test]
fn an_arrival_not_yet_knowable_is_refused_as_leakage() {
    let prior = OutcomeDistribution::new(distribution("0.5", "0.5"), t0(), "stated")
        .expect("a distribution");
    let knowable = t0().saturating_add(Duration::from_hours(2));
    let news = arrival("leaked-minutes", knowable, "0.9", "0.1");
    let early = t0().saturating_add(Duration::from_hours(1));
    assert!(prior.update(&news, early).is_err());
    assert!(
        prior.update(&news, knowable).is_ok(),
        "the same arrival is admitted at the instant it became knowable"
    );
    assert!(
        prior
            .update(&arrival("old", t0(), "1", "1"), Timestamp::from_secs(1))
            .is_err(),
        "an update cannot be dated before the belief it revises"
    );
}

#[test]
fn an_arrival_ruling_out_every_outcome_forms_no_belief() {
    let prior = OutcomeDistribution::new(distribution("0.5", "0.5"), t0(), "stated")
        .expect("a distribution");
    let impossible = arrival("contradiction", t0(), "0", "0");
    assert!(prior.update(&impossible, t0()).is_err());
    let partial = Arrival {
        name: "partial".to_string(),
        knowable_at: t0(),
        likelihoods: BTreeMap::from([(yes(), dec("1"))]),
    };
    assert!(prior.update(&partial, t0()).is_err());
}

fn tree() -> Development {
    // Two ways to a cut: the committee splits then cuts, or moves early.
    Development::Branches(vec![
        Branch {
            label: "committee-splits".to_string(),
            conditional: p("0.5"),
            then: Development::Branches(vec![
                Branch {
                    label: "chair-sides-with-cut".to_string(),
                    conditional: p("0.4"),
                    then: Development::Ends(yes()),
                },
                Branch {
                    label: "chair-holds".to_string(),
                    conditional: p("0.6"),
                    then: Development::Ends(no()),
                },
            ]),
        },
        Branch {
            label: "committee-unanimous".to_string(),
            conditional: p("0.5"),
            then: Development::Branches(vec![
                Branch {
                    label: "cut".to_string(),
                    conditional: p("0.8"),
                    then: Development::Ends(yes()),
                },
                Branch {
                    label: "hold".to_string(),
                    conditional: p("0.2"),
                    then: Development::Ends(no()),
                },
            ]),
        },
    ])
}

#[test]
fn a_tree_of_conditional_developments_implies_the_outcome_distribution() {
    let paths = tree().paths().expect("a well-formed tree");
    assert_eq!(paths.len(), 4, "premise: four root-to-outcome paths");
    let belief = tree()
        .outcome_distribution(&market(), t0())
        .expect("the tree's outcomes are the market's");
    // 0.5*0.4 + 0.5*0.8 = 0.6
    assert_eq!(belief.probability(&yes()), Some(p("0.6")));
    assert_eq!(belief.probability(&no()), Some(p("0.4")));
}

#[test]
fn a_tree_whose_conditionals_do_not_sum_to_one_or_that_names_a_foreign_outcome_is_refused() {
    let lopsided = Development::Branches(vec![
        Branch {
            label: "a".to_string(),
            conditional: p("0.5"),
            then: Development::Ends(yes()),
        },
        Branch {
            label: "b".to_string(),
            conditional: p("0.4"),
            then: Development::Ends(no()),
        },
    ]);
    assert!(lopsided.paths().is_err());
    let foreign = Development::Branches(vec![
        Branch {
            label: "a".to_string(),
            conditional: p("0.5"),
            then: Development::Ends(OutcomeId::new("maybe")),
        },
        Branch {
            label: "b".to_string(),
            conditional: p("0.5"),
            then: Development::Ends(no()),
        },
    ]);
    assert!(foreign.outcome_distribution(&market(), t0()).is_err());
}

#[test]
fn fair_values_scale_with_probability_sum_to_the_payoff_and_cite_their_belief() {
    let market = market();
    let low = OutcomeDistribution::new(distribution("0.3", "0.7"), t0(), "stated")
        .expect("a distribution");
    let high = OutcomeDistribution::new(distribution("0.6", "0.4"), t0(), "stated")
        .expect("a distribution");
    let low_v = fair_values(&market, &low).expect("priced");
    let high_v = fair_values(&market, &high).expect("priced");
    let payoff = Decimal::from_int(10);
    // Premise: the payoff is the fixture's 10, not a default of 1.
    assert_eq!(market.proposition.settlement.payoff, payoff);
    assert_eq!(low_v.fair_values[&yes()], dec("3"));
    assert_eq!(high_v.fair_values[&yes()], dec("6"));
    assert!(high_v.fair_values[&yes()] > low_v.fair_values[&yes()]);
    assert!(high_v.fair_values[&no()] < low_v.fair_values[&no()]);
    let total: Decimal = high_v.fair_values.values().copied().sum();
    assert_eq!(total, payoff);
    assert_eq!(high_v.distribution, high.digest());
    assert_ne!(high_v.distribution, low_v.distribution);
}

#[test]
fn a_belief_about_a_different_event_cannot_price_this_market() {
    let foreign = BTreeMap::from([
        (OutcomeId::new("up"), p("0.5")),
        (OutcomeId::new("down"), p("0.5")),
    ]);
    let belief = OutcomeDistribution::new(foreign, t0(), "stated").expect("a distribution");
    assert!(fair_values(&market(), &belief).is_err());
    let three = BTreeMap::from([
        (yes(), p("0.3")),
        (no(), p("0.3")),
        (OutcomeId::new("void"), p("0.4")),
    ]);
    let wider = OutcomeDistribution::new(three, t0(), "stated").expect("a distribution");
    assert!(fair_values(&market(), &wider).is_err());
}

#[test]
fn updating_twice_from_the_same_inputs_gives_the_identical_belief() {
    let prior = OutcomeDistribution::new(distribution("0.3", "0.7"), t0(), "stated")
        .expect("a distribution");
    let news = arrival("survey", t0(), "0.7", "0.3");
    let first = prior.update(&news, t0()).expect("update");
    let second = prior.update(&news, t0()).expect("update");
    assert_eq!(first, second);
    assert_eq!(first.digest(), second.digest());
    let total: Decimal = first.outcomes().map(|(_, q)| q.value()).sum();
    assert_eq!(total, Decimal::ONE);
}
