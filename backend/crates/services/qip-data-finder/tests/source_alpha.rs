//! DATA-015 replay: one informative source and one that duplicates it.

// A test returning `Result` so it can use `?` still has to assert; the abort
// is its reporting mechanism, not a defect.
#![allow(clippy::panic_in_result_fn)]

mod common;

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::source_alpha::{
    HISTORY_BOUND, SourceAlphaHistory, incremental_information_gain,
    incremental_information_gain_over, realised_contribution,
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
