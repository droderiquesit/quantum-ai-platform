//! DATA-015 replay: one informative source and one that duplicates it.

// A test returning `Result` so it can use `?` still has to assert; the abort
// is its reporting mechanism, not a defect.
#![allow(clippy::panic_in_result_fn)]

mod common;

use qip_core::error::Result;
use qip_core::{Currency, Decimal, Timestamp};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source_alpha::{
    HISTORY_BOUND, PRICE_OF_INFORMATION_PER_IC, SourceAlpha, SourceAlphaHistory, SourceEconomics, incremental_information_gain,
    incremental_information_gain_over, net_score, realised_contribution,
};

/// A deterministic pseudo-random series in `[-1, 1)`, so the replay is the
/// same on every machine.
fn noise(seed: u64, len: usize) -> Vec<f64> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
        })
        .collect()
}

#[test]
fn a_duplicate_source_adds_no_information_and_an_informative_one_earns_a_positive_contribution()
-> Result<()> {
    let len = 200;
    let incumbent = noise(1, len);
    // The duplicate is the incumbent rescaled and shifted: same information.
    let duplicate: Vec<f64> = incumbent.iter().map(|v| v * 3.0 + 5.0).collect();
    // The informative source is independent of the incumbent, and the outcome
    // follows it.
    let informative = noise(2, len);
    let wobble = noise(3, len);
    let outcome: Vec<f64> = informative
        .iter()
        .zip(&wobble)
        .map(|(signal, w)| signal * 0.8 + w * 0.3)
        .collect();

    let duplicate_gain = incremental_information_gain(&duplicate, &incumbent)?;
    let informative_gain = incremental_information_gain(&informative, &incumbent)?;
    assert!(
        informative_gain > 0.5,
        "premise: an independent series is mostly new information, got {informative_gain}"
    );
    assert!(
        duplicate_gain < 0.01,
        "a rescaled copy must score near zero, got {duplicate_gain}"
    );
    let contribution = realised_contribution(&informative, &outcome)?;
    assert!(
        contribution > 0.5,
        "the informative source's outcome followed it: {contribution}"
    );
    Ok(())
}

#[test]
fn a_series_that_never_moves_or_is_too_short_is_refused_rather_than_scored() {
    let moving = noise(4, 20);
    assert!(incremental_information_gain(&[1.0; 20], &moving).is_err());
    assert!(incremental_information_gain(&moving[..3], &moving[..3]).is_err());
    assert!(incremental_information_gain(&moving, &moving[..19]).is_err());
}

#[test]
fn a_sources_scores_are_recorded_over_time_within_a_bound() -> Result<()> {
    let incumbent = noise(5, 30);
    let signal = noise(6, 30);
    let outcome = noise(7, 30);
    let mut history = SourceAlphaHistory::new();
    assert_eq!(history.history("wire").count(), 0, "premise: nothing yet");
    for tick in 0..(HISTORY_BOUND as i64 + 3) {
        history.score(
            "wire",
            Timestamp::from_secs(tick),
            &signal,
            &incumbent,
            &outcome,
        )?;
    }
    let kept: Vec<_> = history.history("wire").collect();
    assert_eq!(kept.len(), HISTORY_BOUND);
    assert_eq!(
        kept[0].at,
        Timestamp::from_secs(3),
        "oldest are dropped first"
    );
    assert!(
        history
            .score("  ", Timestamp::from_secs(0), &signal, &incumbent, &outcome)
            .is_err()
    );
    Ok(())
}

/// R12-DATA-2022. The gain over several incumbents is the pairwise gain when
/// there is one, near zero for a candidate that is a combination of them,
/// and an incumbent that copies an earlier one exactly is left out rather than
/// making the fit singular. Mutation: delete the `COLLINEAR_R_SQUARED`
/// check in `incremental_information_gain_over`, so every incumbent is kept
/// — the duplicate column makes the normal equations singular and the
/// three-incumbent call is refused.
#[test]
fn the_gain_over_every_held_series_matches_the_pairwise_one_and_survives_a_duplicate_incumbent()
-> Result<()> {
    let own = noise(1, 60);
    let other = noise(2, 60);
    let candidate = noise(3, 60);
    let single = incremental_information_gain_over(&candidate, std::slice::from_ref(&own))?;
    assert!((single - incremental_information_gain(&candidate, &own)?).abs() < 1e-12);

    let combination: Vec<f64> = own.iter().zip(&other).map(|(a, b)| a - 2.0 * b).collect();
    let pairwise = incremental_information_gain(&combination, &own)?;
    assert!(
        pairwise > 0.5,
        "premise: against one series the mix looks new ({pairwise})"
    );
    let duplicate = own.clone();
    let over = incremental_information_gain_over(&combination, &[own, duplicate, other])?;
    assert!(over < 1e-9, "a combination of held series earned {over}");
    assert!(incremental_information_gain_over(&candidate, &[]).is_err());
    Ok(())
}

/// R12-DATA-2023. A source's timeliness was the age of the one sample it was
/// probed with, so a source that arrived after every other source's copy of a
/// fact scored as fresh as the one that arrived first. The lead ledger's
/// ranking now moves the term.
///
/// Mutation: ignore the ranking in the freshness blend (`None => freshness`
/// for every source) - the two sources score identically and the ordering
/// assertion fails. Mutation: score `led_share` as `1.0` - the lagging source
/// matches the leading one and the same assertion fails.
#[test]
fn a_leading_source_outscores_a_lagging_one_on_timeliness() -> Result<()> {
    use crate::common::{AGENT, candidate, licensed_for, now, permissive_robots, probe_for};
    use qip_contracts::governance::Usage;
    use qip_core::Duration;
    use qip_data_finder::finder::{DataFinder, FinderConfig};
    use qip_data_finder::freshness::SourceLeadSummary;

    const URL_A: &str = "https://a.example/data/prices.json";
    const URL_B: &str = "https://b.example/data/prices.json";
    let freshness_of = |leads: &[SourceLeadSummary]| -> Result<(f64, f64)> {
        let mut finder =
            DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)?);
        finder.observe_leads(leads);
        let mut probe = probe_for(URL_A, "a.example", permissive_robots());
        probe = probe.with_robots("b.example", permissive_robots());
        probe = probe.with_head(URL_B, crate::common::ok_head());
        probe = probe.with_sample(URL_B, crate::common::sample(crate::common::QUOTE_PAYLOAD));
        let decisions = finder.assess(
            vec![
                candidate("alpha", URL_A, licensed_for(&[Usage::Derive])?, &["EU0001"])?,
                candidate("beta", URL_B, licensed_for(&[Usage::Derive])?, &["EU0002"])?,
            ],
            &mut probe,
            now(),
        )?;
        let of = |id: &str| -> Result<f64> {
            decisions
                .iter()
                .find(|decision| decision.source_id() == id)
                .and_then(|decision| decision.scores())
                .map(qip_data_finder::scoring::SourceScores::freshness)
                .ok_or_else(|| qip_core::error::Error::not_found(format!("{id} has no scores")))
        };
        Ok((of("alpha")?, of("beta")?))
    };

    let (alpha, beta) = freshness_of(&[])?;
    assert!(
        (alpha - beta).abs() < 1e-12,
        "premise: with no measured lead the two score alike on sample age ({alpha} vs {beta})"
    );

    let led = SourceLeadSummary {
        source: "alpha".into(),
        led: 10,
        trailed: 0,
        mean_lead: Duration::from_secs(60),
        longest_lead: Duration::from_secs(90),
    };
    let trailed = SourceLeadSummary {
        source: "beta".into(),
        // Led once by as much as the leader's usual margin and trailed on
        // the other nine: only the share of comparisons it led separates it.
        led: 1,
        trailed: 9,
        mean_lead: Duration::from_secs(60),
        longest_lead: Duration::from_secs(60),
    };
    let (leading, lagging) = freshness_of(&[led, trailed])?;
    assert!(
        leading > lagging,
        "the source that led scored {leading}, the one that trailed {lagging}"
    );
    // A fresh sample already scores one, which a lead cannot raise: it is
    // the lag that moves the term, so the leader keeps its score.
    assert!(
        (leading - alpha).abs() < 1e-12,
        "a measured lead lowered the leading source's timeliness: {alpha} -> {leading}"
    );
    assert!(
        lagging < beta,
        "a measured lag did not lower the lagging source's timeliness"
    );
    Ok(())
}

fn alpha(gain: f64, contribution: f64) -> SourceAlpha {
    SourceAlpha {
        at: Timestamp::from_secs(1),
        incremental_gain: gain,
        realised_contribution: contribution,
    }
}

fn economics(monthly_fee: i64, delay_ms: u64) -> Result<SourceEconomics> {
    let cost = SourceCost::new(
        Decimal::from_int(monthly_fee),
        Decimal::ZERO,
        0,
        Currency::USD,
    )?;
    SourceEconomics::new(&cost, 0, delay_ms, 1_000)
}

/// R12-DATA-6006, owner decision 36: with the provisional price of zero no
/// source earns a net score, however informative, so none is promoted on a
/// priced gain. Mutation: replace the constant with `Decimal::ONE` and the
/// assertion on the constant fails.
#[test]
fn at_the_provisional_price_of_zero_no_source_earns_a_net_score() -> Result<()> {
    assert!(PRICE_OF_INFORMATION_PER_IC.is_zero());
    let perfect = alpha(1.0, 1.0);
    assert_eq!(
        net_score(&perfect, &economics(0, 0)?, PRICE_OF_INFORMATION_PER_IC)?,
        Decimal::ZERO
    );
    Ok(())
}

/// Mutation: drop the `incremental_gain *` factor in `net_score` and the
/// duplicate scores as much as the informative source.
#[test]
fn a_duplicate_source_scores_near_zero_net_and_an_informative_one_scores_positive() -> Result<()> {
    let price = Decimal::from_int(100);
    let free = economics(0, 0)?;
    let informative = net_score(&alpha(0.9, 0.8), &free, price)?;
    assert!(informative > Decimal::from_int(60), "premise: {informative:?}");
    let duplicate = net_score(&alpha(0.0, 0.8), &free, price)?;
    assert!(duplicate < Decimal::from_int(1), "{duplicate:?}");
    Ok(())
}

/// Mutation: skip the `delay_discount` multiplication and the late source
/// scores the same as the prompt one.
#[test]
fn a_late_source_scores_less_than_the_same_source_on_time() -> Result<()> {
    let price = Decimal::from_int(100);
    let signal = alpha(0.9, 0.8);
    let on_time = net_score(&signal, &economics(0, 0)?, price)?;
    let late = net_score(&signal, &economics(0, 1_000)?, price)?;
    let very_late = net_score(&signal, &economics(0, 99_000)?, price)?;
    assert!(on_time.is_positive(), "premise: {on_time:?}");
    // A delay equal to the horizon halves the value.
    let half = on_time.checked_div(Decimal::from_int(2)).unwrap_or_default();
    assert!((late.checked_sub(half).unwrap_or_default()).abs() < Decimal::from_scaled(1, 3).unwrap_or_default());
    assert!(very_late < late, "{very_late:?} {late:?}");
    Ok(())
}

/// Mutation: skip the `checked_sub(monthly_cost)` and the costly source
/// scores the same as the free one.
#[test]
fn a_costly_source_scores_less_than_a_free_one_and_never_below_zero() -> Result<()> {
    let price = Decimal::from_int(100);
    let signal = alpha(0.9, 0.8);
    let free = net_score(&signal, &economics(0, 0)?, price)?;
    let costly = net_score(&signal, &economics(30, 0)?, price)?;
    assert!(free.is_positive(), "premise: {free:?}");
    assert!(costly < free, "{costly:?} {free:?}");
    let ruinous = net_score(&signal, &economics(1_000_000, 0)?, price)?;
    assert_eq!(ruinous, Decimal::ZERO);
    Ok(())
}

#[test]
fn a_negative_price_and_a_zero_horizon_are_refused() -> Result<()> {
    assert!(net_score(&alpha(0.5, 0.5), &economics(0, 0)?, Decimal::NEG_ONE).is_err());
    let cost = SourceCost::free(Currency::USD);
    assert!(SourceEconomics::new(&cost, 0, 0, 0).is_err());
    Ok(())
}
