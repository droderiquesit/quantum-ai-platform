//! Stdout volume of the decision loop must not grow with the cycle count
//! (OBS-022): on Cloud Run stdout is Cloud Logging ingestion.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::{Duration, Timestamp};
use qip_fastbrain::cycle_log::CycleLog;

#[test]
fn the_log_volume_for_a_span_of_time_is_the_same_whether_it_held_a_hundred_cycles_or_ten_thousand()
{
    let lines = |cycles: i64| {
        let mut log = CycleLog::new(Duration::from_secs(10));
        let span_ms = 60_000;
        (0..cycles)
            .filter_map(|i| {
                let now = Timestamp::from_millis(i * span_ms / cycles);
                log.observe(now, i as u64, 1, false, Duration::from_millis(1))
            })
            .count()
    };
    let (few, many) = (lines(100), lines(10_000));
    // premise: the fixture differs 100-fold in cycles and emits something
    assert!(few > 0 && many > 0);
    assert_eq!(few, many);
}

#[test]
fn a_summary_line_counts_what_it_swallowed_so_nothing_is_silently_lost() {
    let mut log = CycleLog::new(Duration::from_secs(10));
    let ms = Duration::from_millis(1);
    assert!(
        log.observe(Timestamp::from_secs(0), 0, 0, false, ms)
            .is_some()
    );
    for i in 1..5 {
        assert!(
            log.observe(Timestamp::from_secs(i), i as u64, 2, true, ms)
                .is_none()
        );
    }
    let line = log
        .observe(Timestamp::from_secs(10), 5, 0, false, ms)
        .expect("due");
    assert!(
        line.contains("5 cycle(s)")
            && line.contains("8 record(s) rejected")
            && line.contains("4 budget breach(es)"),
        "{line}"
    );
}
