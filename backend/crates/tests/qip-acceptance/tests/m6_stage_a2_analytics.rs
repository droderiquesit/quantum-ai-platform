//! M6 Stage A2: Analytics Engine — bitemporal feature store with model disagreement.
//!
//! This suite validates the 16-packet bitemporal feature store implementation,
//! enforcing the following invariants:
//!
//! 1. **Bitemporal schema**: Every feature carries both `instant_true` (when the
//!    fact was true in the market) and `knowable_at` (when the platform could
//!    first act on it).
//!
//! 2. **No point-in-time leakage**: Features are unreadable before their
//!    knowable instant. The type system enforces this through `KnowableAt`.
//!
//! 3. **Distributions, not means**: All model outputs carry full distributions
//!    via `Distribution` type (ADR 0005 enforcement).
//!
//! 4. **ForecastLattice tracks disagreement**: Model disagreement is quantified
//!    as an epistemic asset. Wide disagreement signals uncertainty; narrow
//!    disagreement signals consensus.
//!
//! 5. **Immutable snapshots**: Feature snapshots in the event log are the sole
//!    source of truth and cannot be mutated after creation.
//!
//! Tests use the full name as a sentence describing the property.
//! Each is mutation-verified by altering the implementation, confirming
//! the test fails, then restoring byte-for-byte.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap

use qip_contracts::{
    Distribution, FeatureKey, FeatureSnapshot, FeatureValue, ForecastLattice, KnowableAt,
};
use qip_core::dec;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;

// ============================================================================
// PACKET 1: Basic Distribution Construction
// ============================================================================

#[test]
fn a_distribution_with_valid_percentiles_constructs() {
    let result = Distribution::new(vec![(25, 100.0), (50, 110.0), (75, 120.0)]);
    assert!(result.is_ok());
    let dist = result.unwrap();
    assert_eq!(dist.median(), Some(110.0));
    assert_eq!(dist.percentiles().len(), 3);
}

#[test]
fn a_distribution_rejects_invalid_percentiles() {
    let result = Distribution::new(vec![(101, 100.0)]);
    assert!(result.is_err());
}

#[test]
fn a_distribution_requires_percentiles() {
    let result = Distribution::new(vec![]);
    assert!(result.is_err());
}

#[test]
fn a_distribution_computes_interquartile_range() {
    let dist = Distribution::new(vec![(25, 50.0), (50, 100.0), (75, 150.0)]).unwrap();
    let iqr = dist.iqr().expect("has quartiles");
    assert!((iqr - 100.0).abs() < f64::EPSILON);
}

#[test]
fn a_distribution_with_moments_stores_them() {
    let dist = Distribution::new(vec![(50, 100.0)])
        .unwrap()
        .with_moments(102.0, 5.0);
    assert_eq!(dist.mean(), Some(102.0));
    assert_eq!(dist.std_dev(), Some(5.0));
}

// ============================================================================
// PACKET 2: KnowableAt Type Barrier
// ============================================================================

#[test]
fn knowable_at_seals_a_timestamp_barrier() {
    let ts = Timestamp::from_secs(1000);
    let knowable = KnowableAt::at(ts);
    assert_eq!(knowable.instant(), ts);
}

#[test]
fn knowable_at_is_knowable_at_itself_or_later() {
    let ts = Timestamp::from_secs(1000);
    let knowable = KnowableAt::at(ts);

    assert!(knowable.is_knowable_at(Timestamp::from_secs(1000)));
    assert!(!knowable.is_knowable_at(Timestamp::from_secs(999)));
    assert!(knowable.is_knowable_at(Timestamp::from_secs(1001)));
}

#[test]
fn knowable_at_refuses_future_queries() {
    let knowable = KnowableAt::at(Timestamp::from_secs(1000));
    for i in 0..10 {
        assert!(!knowable.is_knowable_at(Timestamp::from_secs(999 - i)));
    }
}

// ============================================================================
// PACKET 3: FeatureSnapshot Creation
// ============================================================================

#[test]
fn a_feature_snapshot_constructs_with_valid_times() {
    let key = FeatureKey::new("volatility", ObjectId::from_string("instr-001".to_string()));
    let value = FeatureValue::Statistic(0.25);

    let snapshot = FeatureSnapshot::new(
        key,
        value,
        Timestamp::from_secs(1000),
        Timestamp::from_secs(1001),
    );

    assert_eq!(snapshot.value(), value);
    assert_eq!(snapshot.instant_true(), Timestamp::from_secs(1000));
    assert_eq!(snapshot.knowable_at().instant(), Timestamp::from_secs(1001));
}

#[test]
fn a_feature_snapshot_clamps_late_knowledge() {
    let key = FeatureKey::new("vol", ObjectId::from_string("instr-002".to_string()));
    let value = FeatureValue::Statistic(0.25);

    // knowable_at before instant_true
    let snapshot = FeatureSnapshot::new(
        key,
        value,
        Timestamp::from_secs(1000),
        Timestamp::from_secs(999),
    );

    // Should be clamped to instant_true
    assert_eq!(snapshot.knowable_at().instant(), Timestamp::from_secs(1000));
}

#[test]
fn a_feature_snapshot_immediate_stamps_both_times() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-003".to_string()));
    let value = FeatureValue::Exact(dec!("1.50"));
    let at = Timestamp::from_secs(5000);

    let snapshot = FeatureSnapshot::immediate(key, value, at);

    assert_eq!(snapshot.instant_true(), at);
    assert_eq!(snapshot.knowable_at().instant(), at);
}

#[test]
fn a_feature_snapshot_is_readable_after_knowable() {
    let snapshot = FeatureSnapshot::new(
        FeatureKey::new("vol", ObjectId::from_string("instr-004".to_string())),
        FeatureValue::Statistic(0.3),
        Timestamp::from_secs(1000),
        Timestamp::from_secs(1005),
    );

    assert!(!snapshot.is_knowable_at(Timestamp::from_secs(1004)));
    assert!(snapshot.is_knowable_at(Timestamp::from_secs(1005)));
    assert!(snapshot.is_knowable_at(Timestamp::from_secs(1006)));
}

#[test]
fn a_feature_snapshot_is_not_readable_before_knowable() {
    let snapshot = FeatureSnapshot::new(
        FeatureKey::new("vol", ObjectId::from_string("instr-005".to_string())),
        FeatureValue::Statistic(0.25),
        Timestamp::from_secs(1000),
        Timestamp::from_secs(2000),
    );

    for i in 1..200 {
        let ts = Timestamp::from_secs(1000 + i * 5);
        if ts.secs() < 2000 {
            assert!(!snapshot.is_knowable_at(ts));
        }
    }
}

// ============================================================================
// PACKET 4: ForecastLattice Creation
// ============================================================================

#[test]
fn a_forecast_lattice_with_one_forecast_constructs() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-006".to_string()));
    let dist = Distribution::new(vec![(50, 100.0)]).unwrap();

    let result = ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist]);

    assert!(result.is_ok());
}

#[test]
fn a_forecast_lattice_rejects_empty_forecasts() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-007".to_string()));

    let result = ForecastLattice::new(key, Timestamp::from_secs(5000), vec![]);

    assert!(result.is_err());
}

#[test]
fn a_forecast_lattice_computes_zero_disagreement_for_identical() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-008".to_string()));
    let dist1 = Distribution::new(vec![(50, 100.0)]).unwrap();
    let dist2 = Distribution::new(vec![(50, 100.0)]).unwrap();

    let lattice =
        ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist1, dist2]).unwrap();

    assert_eq!(lattice.disagreement_width(), 0.0);
    assert!(lattice.is_consensus());
}

#[test]
fn a_forecast_lattice_computes_disagreement_as_median_spread() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-009".to_string()));
    let dist1 = Distribution::new(vec![(50, 100.0)]).unwrap();
    let dist2 = Distribution::new(vec![(50, 120.0)]).unwrap();

    let lattice =
        ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist1, dist2]).unwrap();

    assert!((lattice.disagreement_width() - 20.0).abs() < f64::EPSILON);
}

#[test]
fn a_forecast_lattice_consensus_false_when_wide() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-010".to_string()));
    let dist1 = Distribution::new(vec![(50, 50.0)]).unwrap();
    let dist2 = Distribution::new(vec![(50, 150.0)]).unwrap();

    let lattice =
        ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist1, dist2]).unwrap();

    assert!(lattice.disagreement_width() > 0.01);
    assert!(!lattice.is_consensus());
}

// ============================================================================
// PACKET 5: ForecastLattice Consensus
// ============================================================================

#[test]
fn a_forecast_lattice_consensus_averages_medians() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-011".to_string()));
    let dist1 = Distribution::new(vec![(50, 100.0)]).unwrap();
    let dist2 = Distribution::new(vec![(50, 110.0)]).unwrap();
    let dist3 = Distribution::new(vec![(50, 120.0)]).unwrap();

    let lattice =
        ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist1, dist2, dist3]).unwrap();

    let consensus = lattice.consensus_distribution().expect("has consensus");
    let median = consensus.median().expect("median");
    assert!((median - 110.0).abs() < f64::EPSILON);
}

#[test]
fn a_forecast_lattice_consensus_includes_means() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-012".to_string()));
    let dist1 = Distribution::new(vec![(50, 100.0)])
        .unwrap()
        .with_moments(102.0, 3.0);
    let dist2 = Distribution::new(vec![(50, 110.0)])
        .unwrap()
        .with_moments(108.0, 4.0);

    let lattice =
        ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist1, dist2]).unwrap();

    let consensus = lattice.consensus_distribution().expect("has consensus");
    let mean = consensus.mean().expect("mean");
    assert!((mean - 105.0).abs() < f64::EPSILON);
    let std_dev = consensus.std_dev().expect("std_dev");
    assert!((std_dev - 3.5).abs() < f64::EPSILON);
}

// ============================================================================
// PACKET 6: Feature Value Variants
// ============================================================================

#[test]
fn a_feature_snapshot_holds_exact_decimal() {
    let value = FeatureValue::Exact(dec!("10000.00"));
    let snapshot = FeatureSnapshot::immediate(
        FeatureKey::new("notional", ObjectId::from_string("instr-013".to_string())),
        value,
        Timestamp::from_secs(1000),
    );

    assert_eq!(snapshot.value(), value);
    assert_eq!(snapshot.value().as_exact(), Some(dec!("10000.00")));
}

#[test]
fn a_feature_snapshot_holds_statistic() {
    let value = FeatureValue::Statistic(0.35);
    let snapshot = FeatureSnapshot::immediate(
        FeatureKey::new("volatility", ObjectId::from_string("instr-014".to_string())),
        value,
        Timestamp::from_secs(1000),
    );

    assert_eq!(snapshot.value(), value);
    assert_eq!(snapshot.value().as_f64(), Some(0.35));
}

#[test]
fn a_feature_snapshot_holds_count() {
    let value = FeatureValue::Count(42);
    let snapshot = FeatureSnapshot::immediate(
        FeatureKey::new("trades", ObjectId::from_string("instr-015".to_string())),
        value,
        Timestamp::from_secs(1000),
    );

    assert_eq!(snapshot.value(), value);
}

#[test]
fn a_feature_snapshot_holds_flag() {
    let value = FeatureValue::Flag(true);
    let snapshot = FeatureSnapshot::immediate(
        FeatureKey::new("halted", ObjectId::from_string("instr-016".to_string())),
        value,
        Timestamp::from_secs(1000),
    );

    assert_eq!(snapshot.value(), value);
}

#[test]
fn a_feature_snapshot_holds_undefined() {
    let value = FeatureValue::Undefined;
    let snapshot = FeatureSnapshot::immediate(
        FeatureKey::new("unknown", ObjectId::from_string("instr-017".to_string())),
        value,
        Timestamp::from_secs(1000),
    );

    assert_eq!(snapshot.value(), value);
    assert!(!value.is_defined());
}

// ============================================================================
// PACKET 7: Point-in-Time Leakage Prevention
// ============================================================================

#[test]
fn point_in_time_query_cannot_read_before_knowable() {
    let snapshot = FeatureSnapshot::new(
        FeatureKey::new("vol", ObjectId::from_string("instr-018".to_string())),
        FeatureValue::Statistic(0.25),
        Timestamp::from_secs(1000),
        Timestamp::from_secs(1050),
    );

    assert!(!snapshot.is_knowable_at(Timestamp::from_secs(1049)));
    assert!(snapshot.is_knowable_at(Timestamp::from_secs(1050)));
}

#[test]
fn backtesting_filters_by_knowable_not_instant_true() {
    let snapshot_early = FeatureSnapshot::new(
        FeatureKey::new("early", ObjectId::from_string("instr-019".to_string())),
        FeatureValue::Statistic(0.20),
        Timestamp::from_secs(1000),
        Timestamp::from_secs(1010),
    );

    let snapshot_late = FeatureSnapshot::new(
        FeatureKey::new("late", ObjectId::from_string("instr-020".to_string())),
        FeatureValue::Statistic(0.30),
        Timestamp::from_secs(1000),
        Timestamp::from_secs(1020),
    );

    let as_of = Timestamp::from_secs(1015);

    assert!(snapshot_early.is_knowable_at(as_of));
    assert!(!snapshot_late.is_knowable_at(as_of));
}

// ============================================================================
// PACKET 8: Serialization
// ============================================================================

#[test]
fn a_feature_snapshot_serializes_to_json() {
    let snapshot = FeatureSnapshot::immediate(
        FeatureKey::new("price", ObjectId::from_string("instr-021".to_string())),
        FeatureValue::Exact(dec!("1.50")),
        Timestamp::from_secs(1000),
    );

    let json = serde_json::to_string(&snapshot).expect("serializable");
    assert!(json.contains("price"));
    assert!(json.contains("instr-021"));
}

#[test]
fn a_distribution_serializes_to_json() {
    let dist = Distribution::new(vec![(50, 100.0)])
        .unwrap()
        .with_moments(102.0, 3.0);

    let json = serde_json::to_string(&dist).expect("serializable");
    assert!(json.contains("100.0"));
}

// ============================================================================
// PACKET 9: Lattice Properties
// ============================================================================

#[test]
fn a_forecast_lattice_preserves_all_forecasts() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-022".to_string()));
    let dist1 = Distribution::new(vec![(50, 100.0)]).unwrap();
    let dist2 = Distribution::new(vec![(50, 110.0)]).unwrap();

    let lattice =
        ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist1, dist2]).unwrap();

    assert_eq!(lattice.forecasts().len(), 2);
}

#[test]
fn a_forecast_lattice_reports_key_and_instant() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-023".to_string()));
    let dist = Distribution::new(vec![(50, 100.0)]).unwrap();
    let instant = Timestamp::from_secs(5000);

    let lattice = ForecastLattice::new(key.clone(), instant, vec![dist]).unwrap();

    assert_eq!(lattice.key().name, "price");
    assert_eq!(lattice.instant_forecast(), instant);
}

// ============================================================================
// PACKET 10: Edge Cases
// ============================================================================

#[test]
fn a_distribution_deduplicates_percentiles() {
    let result = Distribution::new(vec![(50, 100.0), (50, 110.0)]);
    assert!(result.is_ok());

    let dist = result.unwrap();
    assert_eq!(dist.percentiles().len(), 1);
    assert_eq!(dist.median(), Some(110.0));
}

#[test]
fn a_feature_snapshot_handles_large_deltas() {
    let snapshot = FeatureSnapshot::new(
        FeatureKey::new("old", ObjectId::from_string("instr-024".to_string())),
        FeatureValue::Count(0),
        Timestamp::from_secs(0),
        Timestamp::from_secs(u32::MAX as i64 * 2),
    );

    assert_eq!(snapshot.instant_true(), Timestamp::from_secs(0));
}

#[test]
fn a_single_forecast_lattice_is_consensus() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-025".to_string()));
    let dist = Distribution::new(vec![(50, 100.0)]).unwrap();

    let lattice = ForecastLattice::new(key, Timestamp::from_secs(5000), vec![dist]).unwrap();

    assert_eq!(lattice.disagreement_width(), 0.0);
    assert!(lattice.is_consensus());
}

// ============================================================================
// PACKET 11: Knowledge Boundary
// ============================================================================

#[test]
fn knowing_at_boundary_is_inclusive() {
    let ts = Timestamp::from_secs(1000);
    let knowable = KnowableAt::at(ts);

    assert!(knowable.is_knowable_at(ts));
    assert!(!knowable.is_knowable_at(Timestamp::from_secs(999)));
}

#[test]
fn a_clamped_snapshot_knowable_equals_instant_true() {
    let snapshot = FeatureSnapshot::new(
        FeatureKey::new("vol", ObjectId::from_string("instr-026".to_string())),
        FeatureValue::Statistic(0.25),
        Timestamp::from_secs(1000),
        Timestamp::from_secs(900),
    );

    assert_eq!(snapshot.knowable_at().instant(), snapshot.instant_true());
}

// ============================================================================
// PACKET 12: Distribution Ordering
// ============================================================================

#[test]
fn percentiles_are_sorted() {
    let result = Distribution::new(vec![(75, 150.0), (25, 50.0), (50, 100.0)]);
    assert!(result.is_ok());

    let dist = result.unwrap();
    let percentiles = dist.percentiles();
    let keys: Vec<u8> = percentiles.keys().copied().collect();
    assert_eq!(keys, vec![25, 50, 75]);
}

#[test]
fn distribution_returns_none_for_absent_percentile() {
    let dist = Distribution::new(vec![(10, 50.0), (90, 150.0)]).unwrap();

    assert!(dist.percentiles().get(&25).is_none());
    assert!(dist.percentiles().get(&50).is_none());
}

// ============================================================================
// PACKET 13: Feature Key Canonicalization
// ============================================================================

#[test]
fn a_feature_key_with_parameters_is_canonical() {
    let key = FeatureKey::new("avg", ObjectId::from_string("instr-027".to_string()))
        .with("period", 20)
        .with("type", "EMA");

    let canonical = key.canonical();

    assert!(canonical.contains("avg"));
    assert!(canonical.contains("instr-027"));
    assert!(canonical.contains("period=20"));
    assert!(canonical.contains("type=EMA"));
}

#[test]
fn two_feature_keys_with_same_params_have_same_canonical() {
    let key1 = FeatureKey::new("avg", ObjectId::from_string("instr-028".to_string()))
        .with("period", 20)
        .with("type", "EMA");

    let key2 = FeatureKey::new("avg", ObjectId::from_string("instr-028".to_string()))
        .with("type", "EMA")
        .with("period", 20);

    assert_eq!(key1.canonical(), key2.canonical());
}

// ============================================================================
// PACKET 14: Disagreement Properties
// ============================================================================

#[test]
fn wide_disagreement_lattice_lacks_consensus() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-029".to_string()));

    let forecasts = vec![
        Distribution::new(vec![(50, 10.0)]).unwrap(),
        Distribution::new(vec![(50, 100.0)]).unwrap(),
        Distribution::new(vec![(50, 190.0)]).unwrap(),
    ];

    let lattice = ForecastLattice::new(key, Timestamp::from_secs(5000), forecasts).unwrap();

    assert!(lattice.disagreement_width() > 0.01);
    assert!(!lattice.is_consensus());
}

#[test]
fn nearly_identical_forecasts_have_consensus() {
    let key = FeatureKey::new("price", ObjectId::from_string("instr-030".to_string()));

    let forecasts = vec![
        Distribution::new(vec![(50, 100.0)]).unwrap(),
        Distribution::new(vec![(50, 100.00001)]).unwrap(),
    ];

    let lattice = ForecastLattice::new(key, Timestamp::from_secs(5000), forecasts).unwrap();

    assert!(lattice.disagreement_width() < 1e-4);
    assert!(lattice.is_consensus());
}

// ============================================================================
// PACKET 15: Time Ordering Invariant
// ============================================================================

#[test]
fn a_snapshot_never_has_knowable_before_instant_true() {
    for true_time_secs in &[1000, 5000, 10000] {
        let key = FeatureKey::new("test", ObjectId::from_string("instr-031".to_string()));
        let value = FeatureValue::Statistic(0.5);

        for offset in &[-100, -500, -1000] {
            let true_time = Timestamp::from_secs(*true_time_secs);
            let know_time = Timestamp::from_secs(*true_time_secs + offset);

            let snapshot = FeatureSnapshot::new(key.clone(), value, true_time, know_time);

            assert!(
                snapshot.knowable_at().instant() >= snapshot.instant_true(),
                "knowable_at < instant_true"
            );
        }
    }
}

// ============================================================================
// PACKET 16: Integration — Complete Bitemporal Workflow
// ============================================================================

#[test]
fn complete_bitemporal_workflow_maintains_invariants() {
    // Three models forecast with varying certainty
    let feature_key = FeatureKey::new("price", ObjectId::from_string("AAPL".to_string()));

    let model1 = Distribution::new(vec![(25, 149.0), (50, 150.0), (75, 151.0)])
        .unwrap()
        .with_moments(150.0, 0.5);

    let model2 = Distribution::new(vec![(25, 148.0), (50, 152.0), (75, 156.0)])
        .unwrap()
        .with_moments(152.0, 2.0);

    let model3 = Distribution::new(vec![(25, 145.0), (50, 155.0), (75, 165.0)])
        .unwrap()
        .with_moments(155.0, 5.0);

    let lattice = ForecastLattice::new(
        feature_key.clone(),
        Timestamp::from_secs(10000),
        vec![model1, model2, model3],
    )
    .expect("lattice created");

    assert_eq!(lattice.forecasts().len(), 3);
    assert!(lattice.disagreement_width() > 0.0);
    assert!(!lattice.is_consensus());

    let consensus = lattice.consensus_distribution().expect("has consensus");
    let consensus_median = consensus.median().expect("has median");

    // Consensus should be roughly between the medians (150, 152, 155)
    assert!(consensus_median > 151.0 && consensus_median < 153.0);

    let time_market_true = Timestamp::from_secs(10005);
    let time_known_early = Timestamp::from_secs(10010);

    let snapshot = FeatureSnapshot::new(
        feature_key,
        FeatureValue::Exact(dec!("1.51")),
        time_market_true,
        time_known_early,
    );

    // Point-in-time integrity
    assert!(!snapshot.is_knowable_at(Timestamp::from_secs(10009)));
    assert!(snapshot.is_knowable_at(Timestamp::from_secs(10010)));

    // Round-trip serialization
    let serialized = serde_json::to_string(&snapshot).expect("serializable");
    let deserialized: FeatureSnapshot = serde_json::from_str(&serialized).expect("deserializable");

    assert_eq!(deserialized.value(), snapshot.value());
    assert_eq!(deserialized.instant_true(), snapshot.instant_true());
}
