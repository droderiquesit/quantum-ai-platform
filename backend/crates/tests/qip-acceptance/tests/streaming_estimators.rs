//! Cross-cutting properties of the streaming estimators (§21.1, §22.2) and of
//! the capacity estimate (§20.1) that no single crate's tests can see.
//!
//! Three seams meet here and none of the three crates can assert across them:
//!
//! * **One sketch per question.** `qip-numerics` holds a count-min sketch in
//!   `sketch.rs` and three further estimators in `streaming.rs`, and nothing
//!   inside either file can notice that a fourth module elsewhere has grown a
//!   second implementation of the same algorithm. A second answer to one
//!   question is worse than no answer, because the two will disagree and
//!   whichever is louder will be believed.
//! * **The bound must be exceedable.** `qip-training`'s drift index refuses to
//!   call a shift a finding unless it exceeds what the estimators' own error
//!   can manufacture. That is a property of `qip-numerics`' declared bounds and
//!   `qip-training`'s consumer together, and the two crates cannot see each
//!   other's tests.
//! * **Every estimator refuses a ceiling.** Four independent memory ceilings,
//!   in two crates, each of which stops being a bound the moment its refusal is
//!   softened. Checking them one file at a time is how three of them stay
//!   correct while the fourth quietly does not.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the estimators it is exercising still
// has to assert, and the abort is the reporting mechanism rather than a defect.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_numerics::sketch::{CountMinSketch, ErrorBound, MAX_COUNTERS};
use qip_numerics::streaming::{
    Compression, HLL_MAX_PRECISION, HyperLogLog, Precision, RESERVOIR_MAX_ROWS, Reservoir,
    TDIGEST_MAX_COMPRESSION, TDigest,
};
use qip_training::estimators::{DRIFT_BUCKETS, FeatureEstimators, StreamingDrift, degraded_models};
use std::collections::BTreeSet;

/// Every estimator in the workspace refuses to allocate past a named ceiling,
/// and admits a configuration below it.
///
/// Both halves, because a gate that refuses everything is not a working gate —
/// the rule `.claude/rules/domains/infrastructure.md` states for a Terraform
/// validation applies to a memory ceiling for exactly the same reason.
///
/// Four ceilings, and the reason they are checked together rather than one per
/// crate is that they are one decision: a sketch exists to bound memory, and
/// one that allocates in proportion to its stream defeats its own purpose. A
/// fifth estimator arriving without a ceiling is what this test is here to
/// catch, and it will catch it only if somebody adds a case — so the list is
/// deliberately exhaustive rather than sampled.
#[test]
fn every_streaming_estimator_refuses_a_configuration_past_its_named_memory_ceiling() -> Result<()> {
    // Count-min: a bound whose counters exceed MAX_COUNTERS.
    let usable_bound = ErrorBound::new(0.001, 0.01)?;
    assert!(
        usable_bound.counters() < MAX_COUNTERS,
        "premise: the campaign's own bound is admitted, so the refusal below is about the edge"
    );
    let refused = ErrorBound::new(1e-9, 0.5).expect_err("a billion-counter row was allocated");
    assert!(
        refused.message().contains(&MAX_COUNTERS.to_string()),
        "the count-min refusal does not name its ceiling: {refused}"
    );
    assert!(CountMinSketch::new(usable_bound).cells() < MAX_COUNTERS);

    // t-digest: a compression past TDIGEST_MAX_COMPRESSION.
    let usable_compression = Compression::new(100.0)?;
    assert!(usable_compression.value() < TDIGEST_MAX_COMPRESSION);
    let refused = Compression::new(TDIGEST_MAX_COMPRESSION + 1.0)
        .expect_err("a compression past the ceiling was allocated");
    assert!(
        refused
            .message()
            .contains(&TDIGEST_MAX_COMPRESSION.to_string()),
        "the t-digest refusal does not name its ceiling: {refused}"
    );
    assert!(TDigest::new(usable_compression).bytes() <= usable_compression.bytes());

    // HyperLogLog: a precision past HLL_MAX_PRECISION.
    let usable_precision = Precision::new(12)?;
    assert!(usable_precision.bits() < HLL_MAX_PRECISION);
    let refused = Precision::new(HLL_MAX_PRECISION + 1)
        .expect_err("a precision past the ceiling was allocated");
    assert!(
        refused.message().contains(&HLL_MAX_PRECISION.to_string()),
        "the HyperLogLog refusal does not name its ceiling: {refused}"
    );
    assert_eq!(
        HyperLogLog::new(usable_precision).bytes(),
        usable_precision.registers()
    );

    // Reservoir: a capacity past RESERVOIR_MAX_ROWS.
    let usable: Reservoir<f64> = Reservoir::new(1_024, 1)?;
    assert!(usable.capacity() < RESERVOIR_MAX_ROWS);
    let refused = Reservoir::<f64>::new(RESERVOIR_MAX_ROWS + 1, 1)
        .expect_err("a capacity past the ceiling was allocated");
    assert!(
        refused.message().contains(&RESERVOIR_MAX_ROWS.to_string()),
        "the reservoir refusal does not name its ceiling: {refused}"
    );
    Ok(())
}

/// The drift index has a floor the estimators' own declared errors set, and
/// that floor is neither zero nor larger than the index it gates.
///
/// This is the acceptance-level statement of the property each crate can only
/// half-see. A floor of zero would make every comparison a finding, which is a
/// control that fires always. A floor above every achievable index would make
/// no comparison a finding, which is a control that cannot fire. Both read in
/// the code like a working check, and the first draft of
/// `FeatureEstimators::standard` was the second of the two: at compression 20
/// the floor came out at 4.5 against indices around 3, so nothing could ever
/// be reported.
#[test]
fn the_drift_floor_sits_between_an_identical_redraw_and_a_genuine_shift() -> Result<()> {
    let compression = Compression::new(100.0)?;
    let draw = |mean: f64, seed: u64| -> Result<TDigest> {
        let mut digest = TDigest::new(compression);
        let mut state = seed;
        for _ in 0..5_000 {
            let mut sum = 0.0;
            for _ in 0..12 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                sum += ((state >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0;
            }
            digest.add(sum - 6.0 + mean)?;
        }
        Ok(digest)
    };
    let mut reference = draw(0.0, 0xA11CE)?;
    let mut redrawn = draw(0.0, 0xB0B)?;
    let mut shifted = draw(1.0, 0xB0B)?;

    let stable = StreamingDrift::compare(&mut reference, &mut redrawn, DRIFT_BUCKETS)?;
    let moved = StreamingDrift::compare(&mut reference, &mut shifted, DRIFT_BUCKETS)?;
    // Premise: the floor is a real number and not zero, so the assertions
    // below are about where it sits and not about whether it exists.
    assert!(
        stable.floor > 0.0 && (stable.floor - moved.floor).abs() < 1e-12,
        "the floor is {} against {}",
        stable.floor,
        moved.floor
    );

    assert!(
        stable.population_stability_index < stable.floor,
        "an independent redraw of one distribution scored {} against a floor of {}",
        stable.population_stability_index,
        stable.floor
    );
    assert!(
        moved.population_stability_index > moved.floor,
        "a one-standard-deviation shift scored {} against a floor of {}",
        moved.population_stability_index,
        moved.floor
    );
    // And the gap is wide on both sides, so the floor is not sitting on a
    // knife edge where a change of seed flips the verdict. Measured on this
    // data: the calm index is 0.015 against a floor of 0.306, twenty times
    // under; the shifted index is 1.076, three and a half times over. The
    // asserted margins are five and three, which is inside both with room and
    // outside anything a reseeding would produce.
    assert!(
        stable.population_stability_index * 5.0 < stable.floor,
        "the calm index {} is not comfortably under the floor {}",
        stable.population_stability_index,
        stable.floor
    );
    assert!(
        moved.population_stability_index > moved.floor * 3.0,
        "the shifted index {} is not comfortably over the floor {}",
        moved.population_stability_index,
        moved.floor
    );
    Ok(())
}

/// §21.1's sentence, end to end: an estimator that drifts past its bound marks
/// every model depending on it as degraded, and marks no other.
///
/// The whole chain in one test — the estimators summarise, the comparison
/// finds the shift, the floor admits it, and the join against a model's
/// declared feature list names exactly the models that read it. No single
/// crate holds both ends.
#[test]
fn a_feature_that_drifts_past_its_bound_degrades_the_models_that_read_it_and_no_others()
-> Result<()> {
    let mut reference = FeatureEstimators::standard(1)?;
    let mut current = FeatureEstimators::standard(2)?;
    let mut state_a = 0xA11CE_u64;
    let mut state_b = 0xB0B_u64;
    let next = |state: &mut u64| -> f64 {
        let mut sum = 0.0;
        for _ in 0..12 {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            sum += ((*state >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0;
        }
        sum - 6.0
    };
    for _ in 0..5_000 {
        let stable = next(&mut state_a);
        let moving = next(&mut state_b);
        reference.observe("volatility_10", stable)?;
        reference.observe("return_1", moving)?;
        current.observe("volatility_10", stable + 1.0)?;
        current.observe("return_1", moving)?;
    }

    let drifts = current.drift_against(&mut reference, DRIFT_BUCKETS)?;
    // Premise: both features were compared, so the result below is a verdict
    // on each and not a silent omission of one.
    assert_eq!(drifts.len(), 2, "{drifts:?}");
    let drifted: BTreeSet<String> = drifts
        .iter()
        .filter(|(_, drift)| drift.is_material())
        .map(|(name, _)| name.clone())
        .collect();
    assert_eq!(
        drifted.iter().collect::<Vec<_>>(),
        vec!["volatility_10"],
        "the shift was found in {drifted:?}"
    );

    let regime = vec!["return_1".to_string(), "volatility_10".to_string()];
    let momentum = vec!["return_1".to_string(), "momentum_5".to_string()];
    let degraded = degraded_models(
        &drifted,
        [
            ("regime@1.0.0", regime.as_slice()),
            ("momentum@2.0.0", momentum.as_slice()),
        ],
    );
    assert!(
        degraded.contains_key("regime@1.0.0"),
        "the model reading the drifted feature was not degraded: {degraded:?}"
    );
    assert!(
        !degraded.contains_key("momentum@2.0.0"),
        "a model reading only the stable feature was degraded: {degraded:?}"
    );
    Ok(())
}

/// Exactly one implementation of each sketch, in one crate.
///
/// A second count-min, a second t-digest or a second HyperLogLog anywhere in
/// the workspace would be two answers to one question, and the two would
/// diverge the first time either was tuned. This lane's brief named the
/// hazard directly and it is not hypothetical: `CountMinSketch` and
/// `ErrorBound` already existed when the three estimators beside them were
/// written, and writing a fourth sketch rather than finding the third would
/// have been the easier path.
///
/// Matches on the `struct` declaration rather than on the name alone, because
/// a name appears in every doc comment that discusses it — including this one
/// once it is read as a file — and a count of mentions is not a count of
/// implementations.
///
/// **And it excludes its own source file**, which is not fastidiousness: the
/// first run of this test failed, reporting `pub struct CountMinSketch` in two
/// places, one of which was this file — the list of declarations it searches
/// for. A measurement instrument that reads itself is a real failure mode in
/// this repository and `.claude/rules/domains/observability.md` records
/// another instance of it, a recount command that matched the prose quoting
/// the command. The exclusion is by path equality against `file!()` rather
/// than by skipping `crates/tests`, because a second implementation hiding in
/// an acceptance suite would be exactly as bad as one in a library.
#[test]
fn each_sketch_is_declared_exactly_once_in_the_workspace() -> Result<()> {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .expect("the acceptance crate sits two levels below crates/");
    let mut sources = Vec::new();
    collect_rust_sources(&crates, &mut sources);
    // Premise: the walk found the workspace and not an empty directory.
    assert!(
        sources.len() > 100,
        "only {} Rust sources found under {}; the walk is not reaching the workspace",
        sources.len(),
        crates.display()
    );

    // This file names every declaration it searches for, so it would find
    // itself. See the doc comment: the first run did exactly that.
    let own_source = std::path::Path::new(file!())
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .expect("this test's own source has a file name");
    for declaration in [
        "pub struct CountMinSketch",
        "pub struct TDigest",
        "pub struct HyperLogLog",
        "pub struct Reservoir",
    ] {
        let holders: Vec<String> = sources
            .iter()
            .filter(|(path, _)| {
                std::path::Path::new(path).file_name() != Some(own_source.as_os_str())
            })
            .filter(|(_, body)| body.contains(declaration))
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(
            holders.len(),
            1,
            "`{declaration}` is declared in {holders:?}; two implementations of one sketch are \
             two answers to one question, and the louder will be believed"
        );
    }
    Ok(())
}

/// Every `.rs` file under `path`, with its contents.
fn collect_rust_sources(path: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            collect_rust_sources(&child, out);
        } else if child.extension().is_some_and(|ext| ext == "rs")
            && let Ok(body) = std::fs::read_to_string(&child)
        {
            out.push((child.display().to_string(), body));
        }
    }
}
