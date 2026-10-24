//! R12-MODEL-3009: the deep brain trains models from its learning data, and
//! what a learning round trains is a file a deployment can carry.
//!
//! The test composes the three calls `main.rs` and the node loop compose —
//! `LearningDesk::maybe_learn`, `LearningDesk::promote_candidate` against a
//! serving platform, and `write_model_artifact` into the blob store under
//! `MODEL_ARTIFACTS_NAMESPACE` — so it fails if any of the three stops
//! producing what the next one reads. The register said only that the deep
//! brain "owns training"; a desk whose fit stopped producing a model would
//! have left that sentence true.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Context, Decimal, ObjectId, Timestamp};
use qip_deepbrain::artifacts::{read_model_artifact, write_model_artifact};
use qip_deepbrain::learning::{LearningConfig, LearningDesk, PromotionOutcome};
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_training::serve::InTreeProvider;

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn subject() -> ObjectId {
    ObjectId::from_string("OBJ0000000000000000000AAA")
}

/// Minute bars whose next return is `0.6` times the last plus noise: a
/// series a linear fit can genuinely predict out of sample, so the skill
/// bar is cleared honestly rather than on a ramp with no return variance.
fn learnable(count: usize) -> Vec<Bar> {
    let mut rng = Xoshiro256::seeded(0x1EA2);
    let mut level = 100.0_f64;
    let mut previous = 0.0_f64;
    let price = |v: f64| Decimal::from_f64(v).unwrap();
    (0..count)
        .map(|i| {
            let ret = 0.6 * previous + rng.uniform(-0.004, 0.004);
            previous = ret;
            level *= 1.0 + ret;
            Bar {
                object_id: subject(),
                venue: "XNYS".to_string(),
                interval: Interval::Minute,
                open_time: Timestamp::from_secs(1_760_000_000 + i as i64 * 60),
                open: price(level * (1.0 - ret / 2.0)),
                high: price(level * 1.001),
                low: price(level * 0.999),
                close: price(level),
                volume: price(1_000.0 + i as f64),
                trade_count: 100,
                vwap: Decimal::from_f64(level),
                quality: DataQuality::default(),
            }
        })
        .collect()
}

/// The platform as the deep brain's root assembles it: serving through the
/// in-tree provider (ADR 0083 §4).
fn serving_platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(at(), config.seed);
    Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(InTreeProvider),
    )
    .unwrap()
}

/// One learning round fits a model on the subject's bars, the promote stage
/// publishes it, and the composition root's write leaves exactly one
/// artifact in the store, named by the digest the platform's promotion
/// record carries and readable back under that digest with the promoted
/// reference as its manifest. Mutation: in `LearningDesk::learn`, make the
/// baseline's `trainer.fit(..)` a refusal — the round records no fit that
/// cleared the skill bar, nothing reaches the promote stage, and the test
/// fails on the registration assertion before any artifact could exist.
#[test]
fn a_learning_round_writes_one_trained_artifact_named_by_the_digest_its_promotion_records() {
    let mut desk = LearningDesk::new(
        LearningConfig {
            every_cycles: 1,
            minimum_bars: 64,
            ..LearningConfig::default()
        },
        7,
    );
    let mut platform = serving_platform();
    let store = MemoryBlobStore::new();
    assert!(
        platform.model_promotions().is_empty(),
        "premise: nothing is promoted before the round"
    );

    let round = desk
        .maybe_learn(&subject(), &learnable(400), "trending/calm", 1, at())
        .expect("the learning round fits")
        .expect("a due round with enough bars runs");
    assert!(
        round.registration.as_ref().is_some_and(|r| r.passed),
        "premise: the fit cleared the skill bar"
    );

    // R12-EXPAND-6070: a self-fitted model enters shadow first and is
    // promoted only after it has won its shadow rounds, each one a later fit.
    let mut outcome = desk
        .promote_candidate(&mut platform, at())
        .expect("the promote stage answers")
        .expect("the round left a candidate");
    for round in 1..=u64::from(qip_kernel::model_serving::MIN_SHADOW_ROUNDS) {
        if !matches!(outcome, PromotionOutcome::Shadowed { .. }) {
            break;
        }
        let later = at().saturating_add(qip_core::Duration::from_mins(round as i64));
        desk.maybe_learn(
            &subject(),
            &learnable(400),
            "trending/calm",
            1 + round,
            later,
        )
        .expect("the shadow round's fit runs");
        outcome = desk
            .promote_candidate(&mut platform, later)
            .expect("the promote stage answers")
            .expect("the shadow round left a candidate");
    }
    let PromotionOutcome::Promoted {
        reference,
        published,
        ..
    } = &outcome
    else {
        panic!(
            "the trained candidate was not promoted: {}",
            outcome.describe()
        );
    };
    write_model_artifact(&store, published).expect("the artifact is written");

    let records: Vec<_> = platform.model_promotions().values().collect();
    assert_eq!(records.len(), 1, "one round promoted other than one model");
    let digest = records[0].artifact_digest.clone();
    assert_eq!(
        published.file_name,
        format!("{digest}.json"),
        "the file is not named by the digest the promotion record carries"
    );
    let fetched = read_model_artifact(&store, &digest).expect("the artifact reads back");
    assert_eq!(
        &fetched.reference, reference,
        "the manifest names another model"
    );
    assert_eq!(fetched.reference, records[0].reference);
    assert!(
        store.get(&published.file_name).unwrap().is_some(),
        "the store holds no file under the published name"
    );
}
