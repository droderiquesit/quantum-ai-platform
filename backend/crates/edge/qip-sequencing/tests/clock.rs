//! Clock discipline: what the estimate is worth, and what it is never allowed
//! to do.

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::{Property, approx_eq};
use qip_core::{Duration, Timestamp};
use qip_sequencing::{ClockDiscipline, ClockObservation};

const NANOS_PER_MICRO: i64 = 1_000;

fn discipline() -> ClockDiscipline {
    ClockDiscipline::new(64, 8, Duration::from_micros(500)).expect("a valid window")
}

/// A feed whose clock is `offset` behind ours, reaching us after a constant
/// `path` delay plus one-sided queuing jitter.
fn observations(
    rng: &mut Xoshiro256,
    count: usize,
    offset: Duration,
    path: Duration,
    jitter_nanos: i64,
) -> Vec<ClockObservation> {
    (0..count)
        .map(|index| {
            let venue = Timestamp::from_nanos(1_704_207_845_000_000_000 + index as i64 * 1_000_000);
            let queuing = Duration::from_nanos(rng.below(jitter_nanos.max(1) as u64) as i64);
            ClockObservation::new(venue, venue.saturating_add(offset + path + queuing))
        })
        .collect()
}

#[test]
fn the_estimate_recovers_a_known_offset_once_a_packet_arrives_without_queuing() {
    // The one-way estimator: the least delayed observation in the window is the
    // one closest to the true constant term.
    let mut rng = Xoshiro256::seeded(7);
    let offset = Duration::from_micros(120);
    let path = Duration::from_micros(30);
    let mut discipline = discipline().with_path_delay(path);
    for observation in observations(&mut rng, 64, offset, path, 20 * NANOS_PER_MICRO) {
        discipline.observe(observation);
    }

    let estimate = discipline.estimate().expect("observations were made");
    assert!(
        estimate.trustworthy,
        "64 clean-ish samples should be usable"
    );
    let error_nanos = (estimate.offset.as_nanos() - offset.as_nanos()).abs();
    assert!(
        error_nanos < 2 * NANOS_PER_MICRO,
        "estimate {:?} is {error_nanos}ns from the true {offset:?}",
        estimate.offset
    );
}

#[test]
fn a_feed_with_no_known_path_delay_yields_an_estimate_that_still_contains_it() {
    // Stated so nobody mistakes the estimate for an absolute clock offset: with
    // a one-way feed the propagation delay and the offset are not separable, and
    // the code does not pretend otherwise.
    let mut rng = Xoshiro256::seeded(11);
    let offset = Duration::from_micros(120);
    let path = Duration::from_micros(30);
    let mut discipline = discipline();
    for observation in observations(&mut rng, 64, offset, path, 5 * NANOS_PER_MICRO) {
        discipline.observe(observation);
    }

    let estimate = discipline.estimate().expect("observations were made");
    assert!(
        estimate.offset > offset,
        "without a path-delay hint the estimate carries the propagation delay too"
    );
    assert!(estimate.offset < offset + path + Duration::from_micros(5));
}

#[test]
fn a_noisy_feed_reports_a_wide_uncertainty_and_is_refused() {
    let mut rng = Xoshiro256::seeded(3);
    let mut noisy = discipline();
    for observation in observations(
        &mut rng,
        64,
        Duration::from_micros(120),
        Duration::from_micros(30),
        5_000 * NANOS_PER_MICRO,
    ) {
        noisy.observe(observation);
    }
    let noisy_estimate = noisy.estimate().expect("observations were made");

    let mut clean = discipline();
    for observation in observations(
        &mut rng,
        64,
        Duration::from_micros(120),
        Duration::from_micros(30),
        NANOS_PER_MICRO,
    ) {
        clean.observe(observation);
    }
    let clean_estimate = clean.estimate().expect("observations were made");

    assert!(noisy_estimate.uncertainty_nanos_f64 > clean_estimate.uncertainty_nanos_f64);
    assert!(
        !noisy_estimate.trustworthy,
        "an estimate this uncertain must not be applied silently"
    );
    assert!(clean_estimate.trustworthy);
}

#[test]
fn too_few_observations_are_never_trusted_however_clean_they_look() {
    let mut rng = Xoshiro256::seeded(5);
    let mut discipline = discipline();
    for observation in observations(
        &mut rng,
        3,
        Duration::from_micros(120),
        Duration::from_micros(30),
        1,
    ) {
        discipline.observe(observation);
    }
    let estimate = discipline.estimate().expect("observations were made");
    assert!(approx_eq(estimate.uncertainty_nanos_f64, 0.0, 1e-9));
    assert!(
        !estimate.trustworthy,
        "three samples cannot distinguish a clean feed from a lucky one"
    );
}

#[test]
fn an_untrusted_estimate_leaves_the_timestamp_exactly_as_it_arrived() {
    let mut rng = Xoshiro256::seeded(13);
    let mut discipline = discipline();
    for observation in observations(
        &mut rng,
        4,
        Duration::from_micros(900),
        Duration::ZERO,
        NANOS_PER_MICRO,
    ) {
        discipline.observe(observation);
    }

    let venue_time = Timestamp::from_nanos(1_704_207_900_000_000_000);
    assert_eq!(
        discipline.discipline(venue_time),
        venue_time,
        "an unreliable correction is worse than none, because it is invisible"
    );
}

#[test]
fn a_drifting_clock_is_reported_with_the_right_sign_and_rough_magnitude() {
    // One microsecond of drift per second of wall time, which is a plausible
    // free-running crystal and enough to matter over a session.
    let drift_nanos_per_sec = 1_000.0;
    let mut discipline =
        ClockDiscipline::new(256, 8, Duration::from_millis(10)).expect("a valid window");
    for index in 0..256i64 {
        let venue = Timestamp::from_nanos(1_704_207_845_000_000_000 + index * 100_000_000);
        let elapsed_secs = index as f64 * 0.1;
        let raw = Duration::from_nanos(50_000 + (drift_nanos_per_sec * elapsed_secs) as i64);
        discipline.observe(ClockObservation::new(venue, venue.saturating_add(raw)));
    }

    let estimate = discipline.estimate().expect("observations were made");
    assert!(
        estimate.drift_nanos_per_sec_f64 > 0.0,
        "a clock falling behind must not be reported as gaining"
    );
    assert!(
        approx_eq(estimate.drift_nanos_per_sec_f64, drift_nanos_per_sec, 0.1),
        "drift estimated at {} against a true {drift_nanos_per_sec}",
        estimate.drift_nanos_per_sec_f64
    );
}

#[test]
fn a_disciplined_timestamp_never_moves_backwards_whatever_the_estimate_does() {
    // The invariant that matters most: time going backwards breaks event-log
    // ordering, bitemporal reads and watermarks, and it does it silently.
    Property::new("disciplined timestamps are non-decreasing")
        .cases(200)
        .for_all(
            |rng: &mut Xoshiro256| {
                let offset = Duration::from_micros(rng.below(500) as i64);
                let jitter = (1 + rng.below(50_000)) as i64;
                let observations = observations(rng, 40, offset, Duration::from_micros(10), jitter);
                let inputs: Vec<i64> = (0..40)
                    .map(|_| 1_704_207_845_000_000_000 + rng.below(1_000_000_000) as i64)
                    .collect();
                (observations, inputs)
            },
            |(observations, inputs)| {
                let mut discipline = discipline();
                let mut previous = Timestamp::EPOCH;
                for (index, input) in inputs.iter().enumerate() {
                    // Interleave observation and use, so the estimate moves under
                    // the caller's feet exactly as it does live.
                    if let Some(observation) = observations.get(index) {
                        discipline.observe(*observation);
                    }
                    let emitted = discipline.discipline(Timestamp::from_nanos(*input));
                    if emitted < previous {
                        return Err(format!("{emitted:?} follows {previous:?}"));
                    }
                    previous = emitted;
                }
                Ok(())
            },
        );
}

#[test]
fn clearing_the_history_after_a_clock_step_does_not_release_the_monotonic_floor() {
    let mut rng = Xoshiro256::seeded(17);
    let mut discipline = discipline();
    for observation in observations(
        &mut rng,
        32,
        Duration::from_micros(200),
        Duration::ZERO,
        NANOS_PER_MICRO,
    ) {
        discipline.observe(observation);
    }
    let venue_time = Timestamp::from_nanos(1_704_207_900_000_000_000);
    let high = discipline.discipline(venue_time);

    discipline.reset_history();
    let after = discipline.discipline(venue_time.saturating_sub(Duration::from_secs(1)));
    assert_eq!(
        after, high,
        "a venue stepping its clock does not make it acceptable for ours to run backwards"
    );
    assert_eq!(
        discipline.clamped(),
        1,
        "and the clamp is counted, not hidden"
    );
}
