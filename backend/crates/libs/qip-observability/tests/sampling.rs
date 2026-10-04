//! Log volume on a high-rate path is bounded by the sample rate (OBS-021).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::{Duration, Timestamp};
use qip_observability::sampling::LineSampler;

fn drive(messages: i64) -> Vec<String> {
    let mut s = LineSampler::new(5, Duration::from_secs(1)).unwrap();
    // a fixed ten-second window, whatever the message count
    (0..messages)
        .filter_map(|i| s.offer(Timestamp::from_millis(i * 10_000 / messages), "requote"))
        .collect()
}

#[test]
fn log_volume_over_a_fixed_window_is_set_by_the_sample_rate_not_the_message_count() {
    let (slow, fast) = (drive(1_000), drive(100_000));
    // premise: the fast run really offered 100x the messages and both logged
    assert!(!slow.is_empty() && !fast.is_empty());
    assert_eq!(slow.len(), 50, "5 per second over 10 seconds");
    assert_eq!(fast.len(), 50);
}

#[test]
fn the_first_line_after_a_dropped_burst_says_how_many_were_dropped() {
    let mut s = LineSampler::new(1, Duration::from_secs(1)).unwrap();
    assert!(s.offer(Timestamp::from_millis(0), "x").is_some());
    for i in 1..4 {
        assert!(s.offer(Timestamp::from_millis(i), "x").is_none());
    }
    let line = s.offer(Timestamp::from_millis(1_000), "x").unwrap();
    assert!(line.contains("3 similar line(s) sampled out"), "{line}");
}

#[test]
fn a_sampler_that_would_drop_everything_or_bound_nothing_is_refused() {
    assert!(LineSampler::new(0, Duration::from_secs(1)).is_err());
    assert!(LineSampler::new(1, Duration::ZERO).is_err());
}
