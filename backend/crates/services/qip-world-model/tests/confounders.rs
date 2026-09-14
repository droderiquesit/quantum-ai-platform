//! Blueprint §9.1's confounders layer and §9.4's handling of the ones
//! nobody can observe, as the establishment method actually applies them.
//!
//! The contract these tests hold: an edge established under controls says
//! what it was adjusted for; an edge established with an unobserved
//! confounder recorded against it says so, is marked suggestive, and can
//! never rank above the same edge without that confounder.

use std::collections::BTreeSet;

use qip_core::{Duration, Timestamp};
use qip_world_model::causal::{EdgeStanding, Mechanism};
use qip_world_model::confounder::{Confounder, ConfounderSet};
use qip_world_model::granger::{
    establish_temporal_precedence, establish_temporal_precedence_controlling_for,
};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn stream(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) - 0.5
    }
}

/// A genuine one-bar transmission: the effect depends on the cause's
/// previous value and on nothing else that matters.
fn real_link(bars: usize) -> (Vec<f64>, Vec<f64>) {
    let mut noise = stream(0x51ED_270B_2E32_9B7F);
    let cause: Vec<f64> = (0..bars).map(|_| noise()).collect();
    let effect: Vec<f64> = (0..bars)
        .map(|t| {
            if t == 0 {
                noise()
            } else {
                0.7 * cause[t - 1] + 0.3 * noise()
            }
        })
        .collect();
    (cause, effect)
}

#[test]
fn an_edge_established_under_a_control_records_what_it_was_adjusted_for() {
    let (cause, effect) = real_link(300);
    let mut unrelated = stream(11);
    let control: Vec<f64> = (0..300).map(|_| unrelated()).collect();

    let confounders = ConfounderSet::new()
        .with(
            Confounder::observed("market_factor", "moves both books at once", control)
                .expect("a well-formed confounder"),
        )
        .expect("the set admits one");

    let edge = establish_temporal_precedence_controlling_for(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        &confounders,
        Duration::from_days(1),
        at(1_000),
    )
    .expect("well-formed series")
    .expect("the premise: this link clears the bar under the control");

    assert!(
        edge.adjusted_for.contains("market_factor"),
        "an edge must record the common cause it was adjusted for; without it a reader cannot \
         tell a controlled edge from an uncontrolled one"
    );
    assert!(
        edge.suspected_confounders.is_empty(),
        "nothing was recorded as unobserved here"
    );
    assert_eq!(
        edge.standing(),
        EdgeStanding::Established,
        "an edge with no unobserved confounder against it is established"
    );
    // The evidence string is the one place two edges are usually compared in
    // a log, so the controls must be visible there too.
    assert!(
        edge.evidence
            .iter()
            .any(|e| e.contains("controls=market_factor")),
        "the evidence id must name the controls; got {:?}",
        edge.evidence
    );
}

#[test]
fn an_unobserved_confounder_makes_the_edge_suggestive_and_never_raises_its_confidence() {
    let (cause, effect) = real_link(300);

    // The premise: with nothing recorded against it, this pair produces an
    // established edge. Without asserting this, the comparison below would
    // be comparing an edge to nothing.
    let plain = establish_temporal_precedence(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        Duration::from_days(1),
        at(1_000),
    )
    .expect("well-formed")
    .expect("the premise: the pair clears the bar uncontrolled");
    assert_eq!(plain.standing(), EdgeStanding::Established);

    let confounders = ConfounderSet::new()
        .with(
            Confounder::unobserved(
                "unpriced_liquidity",
                "a plausible common cause the platform holds no series for",
            )
            .expect("a well-formed confounder"),
        )
        .expect("the set admits one");

    let suggestive = establish_temporal_precedence_controlling_for(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        &confounders,
        Duration::from_days(1),
        at(1_000),
    )
    .expect("well-formed")
    .expect("the same pair still clears the bar — nothing was adjusted for");

    assert_eq!(
        suggestive.standing(),
        EdgeStanding::Suggestive,
        "§9.4: where a plausible unobserved confounder exists the edge is treated as suggestive \
         rather than established"
    );
    assert!(
        suggestive
            .suspected_confounders
            .contains("unpriced_liquidity"),
        "and it is recorded as such, by name"
    );
    assert!(
        suggestive.adjusted_for.is_empty(),
        "recording an unobserved confounder must never read as having adjusted for it"
    );

    // The ordering is the claim, not either number — see
    // TEMPORAL_PRECEDENCE_SUGGESTIVE_CEILING's own comment on why no test
    // here asserts a value.
    assert!(
        suggestive.confidence < plain.confidence,
        "an edge carrying a confounder nobody could adjust for must never outrank the same \
         statistics without it: {} vs {}",
        suggestive.confidence,
        plain.confidence
    );
    // The ordering between the two ceilings themselves is not asserted here.
    // It is a `const` assertion in `qip_world_model::granger`, so a value
    // that broke it fails the build rather than a test run — a guarantee the
    // compiler holds beating one a test holds.
}

#[test]
fn a_statistically_identical_test_is_not_made_established_by_naming_a_confounder() {
    let (cause, effect) = real_link(300);
    let mut unrelated = stream(13);
    let control: Vec<f64> = (0..300).map(|_| unrelated()).collect();

    // Both observed and unobserved recorded at once. The observed one is
    // genuinely adjusted for; the unobserved one still stands.
    let confounders = ConfounderSet::new()
        .with(Confounder::observed("rates", "discount rate moves both", control).unwrap())
        .unwrap()
        .with(Confounder::unobserved("crowding", "positioning nobody can see").unwrap())
        .unwrap();

    let edge = establish_temporal_precedence_controlling_for(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        &confounders,
        Duration::from_days(1),
        at(1_000),
    )
    .expect("well-formed")
    .expect("the pair clears the bar");

    // The failure this prevents: an edge that names one control reading as
    // fully adjusted while another confounder stands unaddressed. Adjusting
    // for something is not adjusting for everything.
    assert!(edge.adjusted_for.contains("rates"));
    assert_eq!(
        edge.standing(),
        EdgeStanding::Suggestive,
        "one adjusted confounder does not clear an unadjusted one"
    );
}

#[test]
fn a_control_not_sampled_on_the_pairs_bars_is_refused_by_name() {
    let (cause, effect) = real_link(300);
    let confounders = ConfounderSet::new()
        .with(Confounder::observed("rates", "a common cause", vec![0.1; 299]).unwrap())
        .unwrap();

    let refused = establish_temporal_precedence_controlling_for(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        &confounders,
        Duration::from_days(1),
        at(1_000),
    );
    let error = refused.expect_err("a mis-sampled control is refused");
    // Delimited rather than a substring: the message must name the driver,
    // and a test matching "rates" alone would pass on "interest_rates_usd"
    // or on the word appearing in prose.
    assert!(
        error.to_string().contains("'rates'"),
        "the refusal must name which driver is mis-sampled, so an operator does not have to \
         reconstruct the set's ordering; got: {error}"
    );
}

#[test]
fn an_edge_built_without_confounders_at_all_carries_empty_sets_rather_than_a_guess() {
    let (cause, effect) = real_link(300);
    let edge = establish_temporal_precedence(
        "CAUSE",
        &cause,
        "EFFECT",
        &effect,
        Duration::from_days(1),
        at(1_000),
    )
    .expect("well-formed")
    .expect("the pair clears the bar");

    // An unasked question is not a negative answer — the same convention
    // `is_decayed` uses. The remedy for a method that never considers
    // confounders is to make it consider them, not to have the type guess.
    assert_eq!(edge.adjusted_for, BTreeSet::new());
    assert_eq!(edge.suspected_confounders, BTreeSet::new());
    assert!(
        edge.evidence.iter().any(|e| e.contains("controls=none")),
        "and the evidence says plainly that nothing was controlled for; got {:?}",
        edge.evidence
    );
    assert_eq!(edge.mechanism.as_str(), "temporal_precedence");
    assert!(matches!(
        edge.mechanism,
        Mechanism::TemporalPrecedence | Mechanism::InverseTemporalPrecedence
    ));
}
