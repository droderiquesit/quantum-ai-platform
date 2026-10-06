//! Property-based and fuzz-style testing of core invariants.
//!
//! This test module covers the "fuzz" category in the seven-category testing
//! hierarchy (unit / integration / property / fuzz / replay / performance / model).
//! Property-based tests exercise a large space of generated inputs to check that
//! key invariants hold for all cases, not just a few hand-written examples.
//!
//! These tests use qip_core::Xoshiro256 for deterministic pseudo-random input generation,
//! avoiding the need for an external fuzzing dependency (which would require a new ADR
//! per ADR 0002's two-dependency policy). The same generator ensures test reproducibility.

#![allow(clippy::panic_in_result_fn)]

use qip_core::testing::{Property, any_decimal};
use qip_core::{Decimal, Duration, Rng, Xoshiro256};

/// Arbitrary Duration generation for property tests.
fn any_duration(rng: &mut Xoshiro256) -> Duration {
    Duration::from_millis(rng.below(100_000) as i64)
}

#[test]
fn property_decimal_addition_is_commutative() {
    Property::new("Decimal addition is commutative")
        .cases(256)
        .for_all(
            |rng| (any_decimal(rng), any_decimal(rng)),
            |(a, b)| {
                if *a + *b != *b + *a {
                    return Err("addition is not commutative".into());
                }
                Ok(())
            },
        );
}

#[test]
fn property_decimal_addition_is_associative() {
    Property::new("Decimal addition is associative")
        .cases(256)
        .for_all(
            |rng| (any_decimal(rng), any_decimal(rng), any_decimal(rng)),
            |(a, b, c)| {
                if (*a + *b) + *c != *a + (*b + *c) {
                    return Err(format!(
                        "({} + {}) + {} != {} + ({} + {})",
                        a, b, c, a, b, c
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn property_decimal_subtraction_inverts_addition() {
    Property::new("Decimal subtraction inverts addition")
        .cases(256)
        .for_all(
            |rng| (any_decimal(rng), any_decimal(rng)),
            |(a, b)| {
                if *a + *b - *b != *a {
                    return Err(format!("({} + {}) - {} != {}", a, b, b, a));
                }
                Ok(())
            },
        );
}

#[test]
fn property_decimal_zero_is_additive_identity() {
    Property::new("Decimal zero is additive identity")
        .cases(256)
        .for_all(any_decimal, |a| {
            let zero = Decimal::ZERO;
            if *a + zero != *a {
                return Err(format!("{} + 0 is not identity", a));
            }
            Ok(())
        });
}

#[test]
fn property_decimal_multiplication_is_commutative() {
    Property::new("Decimal multiplication is commutative")
        .cases(256)
        .for_all(
            |rng| (any_decimal(rng), any_decimal(rng)),
            |(a, b)| {
                if *a * *b != *b * *a {
                    return Err("multiplication is not commutative".into());
                }
                Ok(())
            },
        );
}

// Multiplication associativity does not hold for fixed-point Decimal due to rounding:
// (a * b) * c may differ from a * (b * c) when intermediate results require rounding.
// This is expected behavior and not a bug in the implementation.

#[test]
fn property_decimal_one_is_multiplicative_identity() {
    Property::new("Decimal one is multiplicative identity")
        .cases(256)
        .for_all(any_decimal, |a| {
            let one = Decimal::ONE;
            if *a * one != *a {
                return Err(format!("{} * 1 is not identity", a));
            }
            Ok(())
        });
}

#[test]
fn property_decimal_zero_absorbs_multiplication() {
    Property::new("Decimal zero absorbs multiplication")
        .cases(256)
        .for_all(any_decimal, |a| {
            let zero = Decimal::ZERO;
            if *a * zero != zero || zero * *a != zero {
                return Err(format!("{} * 0 is not zero", a));
            }
            Ok(())
        });
}

#[test]
fn property_duration_addition_is_associative() {
    Property::new("Duration addition is associative")
        .cases(256)
        .for_all(
            |rng| (any_duration(rng), any_duration(rng), any_duration(rng)),
            |(a, b, c)| {
                if (*a + *b) + *c != *a + (*b + *c) {
                    return Err(format!(
                        "({:?} + {:?}) + {:?} != {:?} + ({:?} + {:?})",
                        a, b, c, a, b, c
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn property_duration_zero_is_additive_identity() {
    Property::new("Duration zero is additive identity")
        .cases(256)
        .for_all(any_duration, |a| {
            let zero = Duration::ZERO;
            if *a + zero != *a {
                return Err(format!("{:?} + 0 is not identity", a));
            }
            Ok(())
        });
}

#[test]
fn property_rng_below_respects_bounds() {
    Property::new("Rng::below returns values in [0, n)")
        .cases(256)
        .for_all(
            |rng| rng.below(1_000_000),
            |n| {
                if *n == 0 {
                    return Ok(());
                }
                let mut test_rng = Xoshiro256::seeded(*n);
                let result = test_rng.below(*n);
                if result >= *n {
                    return Err(format!(
                        "below({}) returned {}, which is >= {}",
                        n, result, n
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn property_rng_uniform_stays_in_bounds() {
    Property::new("Rng::uniform returns values in [lo, hi)")
        .cases(256)
        .for_all(
            |rng| {
                let lo = rng.uniform(0.0, 1000.0);
                let hi = rng.uniform(lo, lo + 1000.0);
                (lo, hi)
            },
            |(lo, hi)| {
                let mut test_rng = Xoshiro256::seeded((*lo as u64).wrapping_mul((*hi as u64) | 1));
                let result = test_rng.uniform(*lo, *hi);
                if result < *lo || result >= *hi {
                    return Err(format!(
                        "uniform({}, {}) returned {}, outside [{}, {})",
                        lo, hi, result, lo, hi
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn property_rng_bernoulli_produces_booleans() {
    Property::new("Rng::bernoulli always produces valid probabilities")
        .cases(256)
        .for_all(
            |rng| rng.next_f64(),
            |p| {
                let mut test_rng = Xoshiro256::seeded(*p as u64);
                // Just check that it doesn't panic and returns a bool
                let _ = test_rng.bernoulli(*p);
                Ok(())
            },
        );
}

#[test]
fn property_decimal_division_by_nonzero_is_defined() {
    Property::new("Decimal division by nonzero is defined")
        .cases(256)
        .for_all(
            |rng| {
                let a = any_decimal(rng);
                let b = any_decimal(rng);
                // Just use a and b directly; the property will skip if b is zero
                (a, b)
            },
            |(a, b)| {
                if *b == Decimal::ZERO {
                    return Ok(()); // Skip this case
                }
                // Just check that division doesn't panic
                let _ = *a / *b;
                Ok(())
            },
        );
}

// Distributive law does not hold for fixed-point Decimal due to rounding:
// a * (b + c) may differ from a * b + a * c when either side involves rounding.
// This is expected behavior and not a bug in the implementation.
