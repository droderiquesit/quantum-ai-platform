//! The learned representation stage (R12-TICK-1009).

#![allow(clippy::panic_in_result_fn, clippy::float_cmp)]

use qip_core::{Duration, Timestamp};
use qip_training::microstructure::MicroExample;
use qip_training::representation::MicroEmbedding;

fn example(i: usize, features: Vec<f64>) -> MicroExample {
    MicroExample {
        venue: "venue-a".to_string(),
        sequence: i as u64,
        entitlement: "entitled".to_string(),
        instrument: "inst".to_string(),
        at: Timestamp::from_millis(1_791_676_800_000 + i as i64 * 1_000),
        horizon: Duration::from_secs(1),
        regime: "calm".to_string(),
        features,
        fill: 0.0,
        future_return: 0.0,
        realized_volatility: 0.0,
        adverse_selection: 0.0,
        market_impact: 0.0,
    }
}

/// Columns 0 and 1 move together; column 2 alternates independently.
fn examples() -> Vec<MicroExample> {
    (0..40)
        .map(|i| {
            let t = i as f64;
            let wobble = if i % 2 == 0 { 0.01 } else { -0.01 };
            let other = if i % 3 == 0 { 1.0 } else { -1.0 };
            example(i, vec![t, 2.0 * t + wobble, other])
        })
        .collect()
}

#[test]
fn the_same_replayed_examples_always_give_the_same_embedding() {
    let set = examples();
    assert!(set.len() >= 2);
    let a = MicroEmbedding::fit(&set, 2).unwrap();
    let b = MicroEmbedding::fit(&set, 2).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.dims(), 2);
    assert_eq!(a.width(), 3);
}

#[test]
fn the_leading_component_captures_the_two_columns_that_move_together() {
    let set = examples();
    let e = MicroEmbedding::fit(&set, 2).unwrap();
    let v = e.explained_variance();
    assert_eq!(v.len(), 2);
    assert!(v[0] > 1.9 && v[0] > v[1], "variance {v:?}");
    let first = e.embed(&set[0].features).unwrap();
    let last = e.embed(&set[39].features).unwrap();
    assert!(
        (first[0] - last[0]).abs() > 2.0,
        "first component must separate the ends of the trend"
    );
}

#[test]
fn a_constant_column_a_wrong_width_and_a_bad_dims_are_refused() {
    let flat: Vec<MicroExample> = (0..5).map(|i| example(i, vec![i as f64, 1.0])).collect();
    assert!(MicroEmbedding::fit(&flat, 1).is_err());
    let set = examples();
    assert!(MicroEmbedding::fit(&set, 0).is_err());
    assert!(MicroEmbedding::fit(&set, 4).is_err());
    assert!(MicroEmbedding::fit(&set[..1], 1).is_err());
    let e = MicroEmbedding::fit(&set, 1).unwrap();
    assert!(e.embed(&[1.0, 2.0]).is_err());
    assert!(e.embed(&[1.0, f64::NAN, 3.0]).is_err());
}
