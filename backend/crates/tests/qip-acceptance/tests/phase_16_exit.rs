//! Phase 16's exit (R12-AMBIENT-10175): the deep brain's ambient research
//! loop, driven through `ResearchLoop::sweep_over` (the entry
//! `node::run_with` calls between cycles), grants compute through the
//! market, and holds a compiled cognitive package unsigned when no signer is
//! installed.
//!
//! Two exit items are not driven here, and the register row says so. A
//! specialist proposal needs a platform calibration of at least
//! `MIN_EVALUATED` evaluated theses with a class hit rate under
//! `HIT_RATE_FLOOR`, which no fixture in the tree produces through a cycle
//! (`qip-expansion/tests/meta.rs` proves `propose_specialists` itself). An
//! information request launches only for a gap carrying economic value; the
//! fill-residual gaps this fixture raises carry none, so `info_launched`
//! stays zero by design of the rank (`research.rs`'s in-module test proves
//! the launching case).

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::{Context, Duration, Timestamp};
use qip_deepbrain::research::{FillObservation, ResearchConfig, ResearchLoop, ResearchRound};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const VENUE: &str = "XSIM";

fn t0() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t0(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

/// Ten scored fills before `t0`, then `post` after it, all ~40 bp off.
fn fills(post: usize) -> Vec<FillObservation> {
    let at = |offset_mins: i64| t0().saturating_add(Duration::from_mins(offset_mins));
    let mut out: Vec<FillObservation> = (0..10)
        .map(|i| FillObservation {
            venue: VENUE.to_string(),
            object_id: "BTC-USD".to_string(),
            filled_at: at(i - 60),
            trade_error_bps: Some(if i % 2 == 0 { 35.0 } else { 45.0 }),
        })
        .collect();
    out.extend((0..post as i64).map(|i| FillObservation {
        venue: VENUE.to_string(),
        object_id: "BTC-USD".to_string(),
        filled_at: at(i + 1),
        trade_error_bps: Some(if i % 2 == 0 { 38.0 } else { 42.0 }),
    }));
    out
}

fn two_sweeps() -> Result<(ResearchRound, ResearchRound, Arc<MemoryBlobStore>)> {
    let platform = platform()?;
    let store = Arc::new(MemoryBlobStore::new());
    // No signer is installed: the package the compiler emits can only be held.
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())?
        .with_package_store(store.clone());
    let first = research.sweep_over(&platform, &fills(0), &[], t0(), &set(&[]), &set(&["US"]))?;
    let second = research.sweep_over(
        &platform,
        &fills(8),
        &[],
        t0().saturating_add(Duration::from_hours(1)),
        &set(&[]),
        &set(&["US"]),
    )?;
    Ok((first, second, store))
}

#[test]
fn a_sweep_grants_compute_through_the_market_to_the_research_the_residuals_raised() -> Result<()> {
    let (first, _, _) = two_sweeps()?;
    assert!(
        first.raised > 0,
        "premise: the fill residuals raised a gap: {first:?}"
    );
    assert!(
        first.units_granted > 0,
        "the market granted nothing: {first:?}"
    );
    assert!(
        first.desk.started.iter().any(|q| q.contains(VENUE)),
        "the granted research did not start: {first:?}"
    );
    Ok(())
}

#[test]
fn a_compiled_cognitive_package_with_no_signer_is_held_unsigned_and_never_published() -> Result<()>
{
    let (_, second, store) = two_sweeps()?;
    assert_eq!(
        second.packages.get("held_unsigned"),
        Some(&1),
        "premise: a package was compiled: {second:?}"
    );
    assert!(
        store
            .list("")
            .map_err(|e| Error::invalid(e.message()))?
            .is_empty(),
        "an unsigned package reached the store"
    );
    Ok(())
}
