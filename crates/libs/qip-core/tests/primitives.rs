//! Determinism of the RNG and id generator, currency safety, config layering.

use qip_core::config::{Config, Environment};
use qip_core::ids::{IdGenerator, OrderId, timestamp_of};
use qip_core::money::{Currency, Money};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal};

// --- rng --------------------------------------------------------------------

#[test]
fn same_seed_produces_the_same_stream() {
    let a: Vec<u64> = (0..64).map(|_| Xoshiro256::seeded(7).next_u64()).collect();
    let mut r = Xoshiro256::seeded(7);
    let b: Vec<u64> = (0..64).map(|_| r.next_u64()).collect();
    assert_eq!(a[0], b[0]);

    let mut r1 = Xoshiro256::seeded(42);
    let mut r2 = Xoshiro256::seeded(42);
    for _ in 0..1000 {
        assert_eq!(r1.next_u64(), r2.next_u64());
    }
}

#[test]
fn different_seeds_diverge_immediately() {
    // Sequential seeds must not produce correlated streams; SplitMix64 expansion
    // is what guarantees this.
    let first: Vec<u64> = (0..8).map(|s| Xoshiro256::seeded(s).next_u64()).collect();
    let unique: std::collections::HashSet<_> = first.iter().collect();
    assert_eq!(unique.len(), first.len(), "seeds 0..8 collided: {first:?}");
}

#[test]
fn uniform_draws_fall_in_range_and_cover_it() {
    let mut r = Xoshiro256::seeded(1);
    let mut buckets = [0usize; 10];
    for _ in 0..100_000 {
        let v = r.next_f64();
        assert!((0.0..1.0).contains(&v), "out of range: {v}");
        buckets[(v * 10.0) as usize] += 1;
    }
    for (i, count) in buckets.iter().enumerate() {
        assert!((8_000..12_000).contains(count), "bucket {i} had {count}");
    }
}

#[test]
fn below_is_unbiased_across_a_non_power_of_two_range() {
    let mut r = Xoshiro256::seeded(3);
    let mut counts = [0usize; 7];
    for _ in 0..70_000 {
        counts[r.below(7) as usize] += 1;
    }
    for (i, c) in counts.iter().enumerate() {
        assert!((9_000..11_000).contains(c), "value {i} appeared {c} times");
    }
    assert_eq!(r.below(0), 0, "degenerate range must not loop");
}

#[test]
fn normal_draws_have_the_requested_moments() {
    let mut r = Xoshiro256::seeded(11);
    let n = 200_000;
    let samples: Vec<f64> = (0..n).map(|_| r.normal()).collect();
    let mean = samples.iter().sum::<f64>() / n as f64;
    let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    assert!(mean.abs() < 0.02, "mean {mean}");
    assert!((var - 1.0).abs() < 0.03, "variance {var}");
}

#[test]
fn shuffle_is_a_permutation() {
    let mut r = Xoshiro256::seeded(5);
    let mut items: Vec<u32> = (0..100).collect();
    r.shuffle(&mut items);
    let mut sorted = items.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..100).collect::<Vec<_>>());
    assert_ne!(items, sorted, "shuffle should reorder");
}

#[test]
fn weighted_index_respects_weights_and_rejects_degenerate_input() {
    let mut r = Xoshiro256::seeded(9);
    let weights = [0.0, 10.0, 0.0, 90.0];
    let mut counts = [0usize; 4];
    for _ in 0..10_000 {
        counts[r.weighted_index(&weights).unwrap()] += 1;
    }
    assert_eq!(counts[0], 0);
    assert_eq!(counts[2], 0);
    assert!((800..1200).contains(&counts[1]), "counts {counts:?}");
    assert!(r.weighted_index(&[]).is_none());
    assert!(r.weighted_index(&[0.0, -1.0]).is_none());
}

#[test]
fn forked_streams_are_independent_and_reproducible() {
    let mut a = Xoshiro256::seeded(100);
    let mut b = Xoshiro256::seeded(100);
    let mut fork_a = a.fork("simulation");
    let mut fork_b = b.fork("simulation");
    assert_eq!(fork_a.next_u64(), fork_b.next_u64());

    let mut c = Xoshiro256::seeded(100);
    let mut other = c.fork("execution");
    assert_ne!(fork_a.next_u64(), other.next_u64());
}

#[test]
fn context_streams_are_stable_per_domain() {
    let (ctx, _clock) = Context::deterministic(Timestamp::EPOCH, 2026);
    let first = ctx.rng_for("discovery").next_u64();
    let again = ctx.rng_for("discovery").next_u64();
    let other = ctx.rng_for("execution").next_u64();
    assert_eq!(first, again, "same domain must be stable");
    assert_ne!(first, other, "different domains must differ");
}

// --- ids --------------------------------------------------------------------

#[test]
fn ids_sort_by_creation_time() {
    let idgen = IdGenerator::new(1);
    let early: OrderId = idgen.generate(Timestamp::from_civil(2020, 1, 1));
    let late: OrderId = idgen.generate(Timestamp::from_civil(2026, 1, 1));
    assert!(early < late, "{early} should sort before {late}");
    assert_eq!(early.as_str().len(), 26);
}

#[test]
fn ids_encode_their_timestamp() {
    let idgen = IdGenerator::new(1);
    let at = Timestamp::from_civil(2026, 8, 22);
    let id: OrderId = idgen.generate(at);
    assert_eq!(timestamp_of(&id).unwrap().as_millis(), at.as_millis());
}

#[test]
fn id_generation_is_deterministic_and_collision_free() {
    let make = || {
        let idgen = IdGenerator::new(99);
        (0..1000)
            .map(|i| {
                idgen.generate::<qip_core::ids::OrderKind>(Timestamp::from_millis(i))
                    .into_string()
            })
            .collect::<Vec<_>>()
    };
    let a = make();
    let b = make();
    assert_eq!(a, b, "same seed must replay the same ids");
    let unique: std::collections::HashSet<_> = a.iter().collect();
    assert_eq!(unique.len(), a.len(), "ids collided");
}

// --- money ------------------------------------------------------------------

#[test]
fn money_refuses_to_add_across_currencies() {
    let usd = Money::usd(Decimal::from_int(100));
    let eur = Money::new(Decimal::from_int(100), Currency::EUR);
    assert!(usd.checked_add(eur).is_err());
    assert!(usd.checked_add(Money::usd(Decimal::from_int(5))).is_ok());
}

#[test]
fn money_rounds_to_currency_minor_units() {
    let jpy = Money::new(Decimal::parse("1234.56").unwrap(), Currency::parse("JPY").unwrap());
    assert_eq!(jpy.round_to_minor().amount, Decimal::from_int(1235));
    let usd = Money::usd(Decimal::parse("1234.567").unwrap());
    assert_eq!(usd.round_to_minor().amount, Decimal::parse("1234.57").unwrap());
}

#[test]
fn currency_parsing_is_strict() {
    assert!(Currency::parse("usd").is_ok());
    assert_eq!(Currency::parse("usd").unwrap(), Currency::USD);
    assert!(Currency::parse("US").is_ok());
    assert!(Currency::parse("U").is_err());
    assert!(Currency::parse("TOOLONG").is_err());
    assert!(Currency::parse("US$").is_err());
}

#[test]
fn money_conversion_records_an_explicit_rate() {
    let usd = Money::usd(Decimal::from_int(100));
    let eur = usd.convert(Currency::EUR, Decimal::parse("0.92").unwrap());
    assert_eq!(eur.currency, Currency::EUR);
    assert_eq!(eur.amount, Decimal::from_int(92));
}

// --- config -----------------------------------------------------------------

#[test]
fn config_layers_file_then_environment() {
    let base = Config::from_json(r#"{"risk":{"max_leverage":1.5,"max_position":100},"env":"local"}"#)
        .unwrap();
    let merged = base.with_env_from(vec![
        ("QIP_RISK__MAX_LEVERAGE".to_string(), "2.5".to_string()),
        ("QIP_RISK__KILL_SWITCH".to_string(), "true".to_string()),
        ("UNRELATED".to_string(), "ignored".to_string()),
    ]);
    assert_eq!(merged.get_f64("risk.max_leverage"), Some(2.5));
    assert_eq!(merged.get_i64("risk.max_position"), Some(100), "untouched keys survive");
    assert_eq!(merged.get_bool("risk.kill_switch"), Some(true));
    assert_eq!(merged.get_str("env"), Some("local"));
    assert!(merged.get("nope.missing").is_none());
}

#[test]
fn config_merge_is_recursive() {
    let a = Config::from_json(r#"{"a":{"x":1,"y":2},"b":1}"#).unwrap();
    let b = Config::from_json(r#"{"a":{"y":9,"z":3}}"#).unwrap();
    let m = a.merge(b);
    assert_eq!(m.get_i64("a.x"), Some(1));
    assert_eq!(m.get_i64("a.y"), Some(9));
    assert_eq!(m.get_i64("a.z"), Some(3));
    assert_eq!(m.get_i64("b"), Some(1));
}

#[test]
fn config_redacts_secret_looking_keys() {
    let c = Config::from_json(
        r#"{"broker":{"api_key":"live-abc","endpoint":"https://x"},"quantum":{"ibm_token":"t"}}"#,
    )
    .unwrap();
    let text = serde_json::to_string(&c.redacted()).unwrap();
    assert!(!text.contains("live-abc"), "secret leaked: {text}");
    assert!(!text.contains("\"t\""), "secret leaked: {text}");
    assert!(text.contains("https://x"), "non-secrets must survive");
}

#[test]
fn config_flatten_produces_dotted_keys() {
    let c = Config::from_json(r#"{"a":{"b":{"c":1}}}"#).unwrap();
    let flat = c.flatten();
    assert_eq!(flat.get("a.b.c").and_then(|v| v.as_i64()), Some(1));
}

#[test]
fn missing_config_file_is_not_an_error() {
    let c = Config::from_file("/nonexistent/qip.json").unwrap();
    assert!(c.get("anything").is_none());
}

#[test]
fn production_forbids_synthetic_data() {
    assert!(!Environment::Production.allows_synthetic_data());
    assert!(Environment::Local.allows_synthetic_data());
    assert!(Environment::Staging.allows_synthetic_data());
    assert_eq!(Environment::parse("prod"), Environment::Production);
    assert_eq!(Environment::parse("whatever"), Environment::Local);
}
