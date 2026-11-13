//! R12-WORLD-3084 / ADR 0506: the node issues the lattice's microsecond row
//! itself, promotes it to the local reflex only, and its forecasts are
//! calibrated against what the cell then observed.

use qip_contracts::forecast::{Calibration, OutcomeDistribution};
use qip_contracts::reflex_forecast::{ReflexPromotion, ReflexTarget};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Rng, Timestamp, Xoshiro256};
use qip_edge::reflex_forecast::{MIN_OBSERVATIONS, REFLEX_WINDOW, RELIABILITY_BINS, ReflexIssuer};

const HORIZON_US: u64 = 500;

fn at(step: i64) -> Timestamp {
    Timestamp::from_secs(1_800_000_000).saturating_add(Duration::from_micros(step * 1_000))
}

/// A venue whose orders fill with probability `per_ten` in ten, drawn from a
/// seeded generator so the run is reproducible. Not a periodic pattern: a
/// pattern makes the stated frequency track the pattern's phase, and a
/// reliability table then measures the phase rather than the model.
fn venue_outcomes(seed: u64, per_ten: u64, n: usize) -> Vec<bool> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n).map(|_| rng.below(10) < per_ten).collect()
}

#[test]
fn the_nodes_fill_forecasts_are_calibrated_against_what_the_venue_then_did() -> Result<()> {
    let thin = VenueId::new("sim-thin");
    let deep = VenueId::new("sim-deep");
    let mut issuer = ReflexIssuer::new("newyork-1", HORIZON_US)?;
    let target = ReflexTarget::FillProbability;
    let thin_fills = venue_outcomes(11, 3, 2_001);
    let deep_fills = venue_outcomes(29, 8, 2_001);

    // Warm-up: nothing is stated before the minimum, and that is a None, not
    // a guess.
    for i in 0..MIN_OBSERVATIONS - 1 {
        issuer.observe_event(&thin, "ABC", target, thin_fills[i])?;
        issuer.observe_event(&deep, "ABC", target, deep_fills[i])?;
    }
    assert!(issuer.issue(&thin, "ABC", target, at(0))?.is_none());

    // Constant 50 % forecaster, scored on the same outcomes, as the bar.
    let mut naive_sq: u128 = 0;
    let mut resolved = 0u128;
    for i in MIN_OBSERVATIONS - 1..2_000 {
        for (venue, outcomes) in [(&thin, &thin_fills), (&deep, &deep_fills)] {
            issuer.observe_event(venue, "ABC", target, outcomes[i])?;
            let Some(f) = issuer.issue(venue, "ABC", target, at(i as i64))? else {
                return Err(Error::invalid("a warmed series issued nothing"));
            };
            let outcome = outcomes[i + 1];
            issuer.resolve_event(&f, outcome)?;
            let e: i64 = 5_000 - if outcome { 10_000 } else { 0 };
            naive_sq += (e * e) as u128;
            resolved += 1;
        }
    }

    // The latest forecast per venue sits at that venue's true rate, within
    // about three standard errors of a 256-outcome window (±290 bp at 30 %,
    // ±250 bp at 80 %), and the two venues are told apart.
    let t = issuer
        .issue(&thin, "ABC", target, at(3_000))?
        .ok_or_else(|| Error::invalid("no thin forecast"))?;
    let d = issuer
        .issue(&deep, "ABC", target, at(3_000))?
        .ok_or_else(|| Error::invalid("no deep forecast"))?;
    let (tp, dp) = (
        t.fields().occurred_bp().unwrap_or(0),
        d.fields().occurred_bp().unwrap_or(0),
    );
    assert!((2_100..=3_900).contains(&tp), "thin venue stated {tp} bp");
    assert!((7_100..=8_900).contains(&dp), "deep venue stated {dp} bp");

    // Sharper than the naive bar on the same outcomes.
    let brier = issuer
        .brier(target)
        .ok_or_else(|| Error::invalid("no Brier score"))?;
    let naive = Decimal::from(i64::try_from(naive_sq).unwrap_or(i64::MAX))
        .checked_div(Decimal::from(
            i64::try_from(resolved * 100_000_000).unwrap_or(i64::MAX),
        ))
        .ok_or_else(|| Error::invalid("naive score"))?;
    assert!(brier < naive, "issuer {brier} not below naive {naive}");

    // Reliable: in every well-populated bin, what was stated is what
    // happened, within a tenth. Bins under 200 forecasts are the warm-up's
    // tail and too noisy to judge (±1,100 bp at 20 forecasts); the premise
    // below is that the judged bins hold nearly every forecast, so the check
    // cannot pass by judging nothing.
    let table = issuer.reliability(target);
    let total: u64 = table.iter().map(|b| b.forecasts).sum();
    let judged: Vec<_> = table.iter().filter(|b| b.forecasts >= 200).collect();
    let judged_total: u64 = judged.iter().map(|b| b.forecasts).sum();
    assert!(judged.len() >= 2, "premise: two venues fill two bins");
    assert!(
        judged_total * 10 >= total * 9,
        "premise: judged bins hold {judged_total} of {total} forecasts"
    );
    for b in judged {
        if let (Some(stated), Some(seen)) = (b.mean_stated_bp(), b.observed_bp()) {
            assert!(
                stated.abs_diff(seen) <= 1_000,
                "stated {stated} bp, observed {seen} bp over {} forecasts",
                b.forecasts
            );
        }
    }
    assert_eq!(table.len(), RELIABILITY_BINS);

    // The forecast carries the measured score, and is the reflex's alone.
    match &t.fields().calibration {
        Calibration::Measured {
            method, resolved, ..
        } => {
            assert_eq!(method, "brier");
            assert!(*resolved > 1_000);
        }
        Calibration::Uncalibrated => return Err(Error::invalid("calibration not carried")),
    }
    assert_eq!(t.fields().promotion, ReflexPromotion::LocalReflexOnly);
    assert_eq!(t.fields().horizon_us, HORIZON_US);
    assert_eq!(
        t.fields().expires_at,
        at(3_000).saturating_add(Duration::from_micros(HORIZON_US as i64))
    );
    Ok(())
}

#[test]
fn a_magnitude_forecasts_quantiles_cover_what_they_say_they_cover() -> Result<()> {
    let venue = VenueId::new("sim-a");
    let target = ReflexTarget::QueueMovement;
    let mut issuer = ReflexIssuer::new("newyork-1", HORIZON_US)?;
    // A deterministic cycle over 0..100 queue units, scrambled by a stride
    // coprime to 100 so the window always holds an even spread.
    let value = |i: usize| Decimal::from(((i * 37) % 100) as i64);
    for i in 0..REFLEX_WINDOW {
        issuer.observe_magnitude(&venue, "ABC", target, value(i))?;
    }
    for i in REFLEX_WINDOW..REFLEX_WINDOW + 2_000 {
        let f = issuer
            .issue(&venue, "ABC", target, at(i as i64))?
            .ok_or_else(|| Error::invalid("no forecast"))?;
        assert!(matches!(
            f.fields().distribution,
            OutcomeDistribution::Quantiles { .. }
        ));
        issuer.resolve_magnitude(&f, value(i))?;
        issuer.observe_magnitude(&venue, "ABC", target, value(i))?;
    }
    let cov = issuer
        .coverage_bp(target)
        .ok_or_else(|| Error::invalid("no coverage"))?;
    for (seen, stated) in cov.iter().zip([1_000u64, 5_000, 9_000]) {
        assert!(
            seen.abs_diff(stated) <= 500,
            "stated quantile {stated} bp covered {seen} bp"
        );
    }
    Ok(())
}

#[test]
fn the_issuer_keeps_a_bounded_window_and_refuses_a_horizon_outside_its_row() -> Result<()> {
    let venue = VenueId::new("sim-a");
    let mut issuer = ReflexIssuer::new("newyork-1", HORIZON_US)?;
    for i in 0..REFLEX_WINDOW * 4 {
        issuer.observe_event(&venue, "ABC", ReflexTarget::AdverseSelection, i % 2 == 0)?;
    }
    assert_eq!(
        issuer.observations(&venue, "ABC", ReflexTarget::AdverseSelection),
        REFLEX_WINDOW
    );
    assert!(ReflexIssuer::new("newyork-1", 0).is_err());
    assert!(ReflexIssuer::new("newyork-1", 1_000_000).is_err());
    assert!(ReflexIssuer::new("newyork-1", 999_999).is_ok());
    // A magnitude observed as an event is refused rather than coerced.
    assert!(
        issuer
            .observe_event(&venue, "ABC", ReflexTarget::ExecutableEdge, true)
            .is_err()
    );
    Ok(())
}

#[test]
fn a_forecast_is_scored_only_by_the_issuer_that_made_it() -> Result<()> {
    let venue = VenueId::new("sim-a");
    let target = ReflexTarget::FillProbability;
    let mut ours = ReflexIssuer::new("newyork-1", HORIZON_US)?;
    let mut theirs = ReflexIssuer::new("london-1", HORIZON_US)?;
    for i in 0..MIN_OBSERVATIONS {
        theirs.observe_event(&venue, "ABC", target, i % 2 == 0)?;
    }
    let f = theirs
        .issue(&venue, "ABC", target, at(0))?
        .ok_or_else(|| Error::invalid("no forecast"))?;
    assert!(ours.resolve_event(&f, true).is_err());
    assert!(ours.brier(target).is_none());
    Ok(())
}
