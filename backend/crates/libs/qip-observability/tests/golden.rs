//! The four golden signals as one recorder (OBS-018).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_observability::golden::{GoldenSignals, Outcome};
use qip_observability::metrics::{Labels, Metrics, labels, names};
use std::sync::Arc;

#[test]
fn a_mix_of_served_refused_and_failed_work_moves_all_four_signals_and_keeps_the_error_classes_apart()
 {
    let metrics = Arc::new(Metrics::new("golden-test"));
    let golden = GoldenSignals::new(metrics.clone());

    // The premise: describing the four creates none of them, so a service
    // that has done nothing exports nothing and what follows is movement.
    let before = metrics.snapshot();
    assert_eq!(before.counter_total(names::SERVICE_REQUESTS), 0);
    assert_eq!(before.counter_total(names::SERVICE_ERRORS), 0);
    assert!(
        before
            .histogram(names::SERVICE_LATENCY_MS, &Labels::new())
            .is_none()
    );
    assert!(
        before
            .gauge(names::SERVICE_SATURATION, &Labels::new())
            .is_none()
    );

    golden.finished(4.0, Outcome::of_status(200));
    golden.finished(6.0, Outcome::of_status(404));
    golden.finished(8.0, Outcome::of_status(404));
    golden.finished(30.0, Outcome::of_status(503));
    golden.saturation(48.0, 64.0).unwrap();

    let after = metrics.snapshot();
    assert_eq!(after.counter_total(names::SERVICE_REQUESTS), 4, "traffic");
    assert_eq!(
        after.counter(names::SERVICE_ERRORS, &labels([("class", "caller")])),
        2,
        "the caller's errors"
    );
    assert_eq!(
        after.counter(names::SERVICE_ERRORS, &labels([("class", "service")])),
        1,
        "the service's own failures, not mixed with the caller's"
    );
    let latency = after
        .histogram(names::SERVICE_LATENCY_MS, &Labels::new())
        .expect("latency");
    assert_eq!(latency.count, 4);
    assert!((latency.sum - 48.0).abs() < 1e-9, "{}", latency.sum);
    assert_eq!(
        after.gauge(names::SERVICE_SATURATION, &Labels::new()),
        Some(0.75),
        "saturation"
    );
}

#[test]
fn saturation_past_capacity_is_reported_as_more_than_one_and_a_capacity_of_nothing_is_refused() {
    let metrics = Arc::new(Metrics::new("golden-test"));
    let golden = GoldenSignals::new(metrics.clone());

    // A cycle that took 150 ms of a 100 ms interval: the overrun is the
    // reading, so it must not be flattened to "full".
    golden.saturation(150.0, 100.0).unwrap();
    assert_eq!(
        metrics
            .snapshot()
            .gauge(names::SERVICE_SATURATION, &Labels::new()),
        Some(1.5)
    );

    for (used, capacity) in [
        (1.0, 0.0),
        (1.0, -5.0),
        (-1.0, 5.0),
        (f64::NAN, 5.0),
        (1.0, f64::INFINITY),
    ] {
        let refused = golden.saturation(used, capacity).unwrap_err();
        assert!(
            refused.message().contains("positive finite capacity"),
            "{}",
            refused.message()
        );
    }
    // And a refusal writes nothing: the last good reading stands.
    assert_eq!(
        metrics
            .snapshot()
            .gauge(names::SERVICE_SATURATION, &Labels::new()),
        Some(1.5)
    );
}
