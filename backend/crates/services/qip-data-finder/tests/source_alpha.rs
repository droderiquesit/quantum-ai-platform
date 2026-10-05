//! DATA-015 replay: one informative source and one that duplicates it.

// A test returning `Result` so it can use `?` still has to assert; the abort
// is its reporting mechanism, not a defect.
#![allow(clippy::panic_in_result_fn)]

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::source_alpha::{
    HISTORY_BOUND, SourceAlphaHistory, incremental_information_gain, realised_contribution,
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
