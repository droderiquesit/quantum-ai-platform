//! Phase 16's exit (R12-AMBIENT-10175): the deep brain's ambient research
//! loop, driven through `ResearchLoop::sweep_over` (the entry
//! `node::run_with` calls between cycles), grants compute through the
//! market, and holds a compiled cognitive package unsigned when no signer is
//! installed.
//!
//! Value-of-information sensing is driven here too since 2026-11-13: a
//! residual gap priced from the notional the platform paper-filled launches
//! an information request on §9.10's product rank
//! (`a_gap_carrying_economic_value_launches_an_information_request_on_the_product_rank`).
//!
//! One exit item is not driven here, and the register row says so. A
//! specialist proposal needs a platform calibration of at least
//! `MIN_EVALUATED` evaluated theses with a class hit rate under
//! `HIT_RATE_FLOOR`, which no fixture in the tree produces through a cycle
//! (`qip-expansion/tests/meta.rs` proves `propose_specialists` itself), and
//! even an accepted proposal is an `InstantiateSpecialist` plan step with no
//! executor: the deep brain's roster is a reviewed static manifest.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::{Context, Decimal, Duration, Timestamp};
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

// --- value-of-information sensing: a priced gap launches on the product rank

/// A platform that knows `AAA` and has paper-filled `quantity` of it, so the
/// outcome record carries the notional a residual gap is priced from.
fn platform_with_a_fill_of(quantity: i64) -> Result<(Platform, String)> {
    use qip_core::ObjectId;
    use qip_execution_engine::order::Side;
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::costs::LiquidityProfile;
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    let aaa = || ObjectId::from_string("obj-AAA".to_string());
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            aaa(),
            "AAA",
            InstrumentType::CommonStock,
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(Decimal::from_int(100))
        .provenance(Provenance::synthetic("phase-16-exit", t0()))
        .build(t0())?,
    )?;
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t0(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    platform.observe(quiet_tape(&aaa()));
    let order = platform.order_from(
        aaa(),
        Side::Buy,
        Decimal::from_int(quantity),
        Decimal::from_int(100),
        "prop-phase-16",
        vec!["hyp-phase-16".to_string()],
        t0(),
    );
    platform.submit_order(order, t0())?;
    let venue = qip_deepbrain::research::fill_notionals(&platform)?
        .into_iter()
        .find(|(_, notional)| notional.is_positive())
        .map(|(venue, _)| venue)
        .ok_or_else(|| Error::invalid("premise: the fill is on the outcome record"))?;
    Ok((platform, venue))
}

/// Ninety quiet daily bars ending at `t0`, so the paper venue has a price.
fn quiet_tape(object: &qip_core::ObjectId) -> Vec<qip_market_ingestion::adapter::SensedRecord> {
    use qip_financial::quality::DataQuality;
    use qip_market::bar::{Bar, Interval};
    (0..90i64)
        .filter_map(|i| {
            let close = Decimal::from_f64(if i % 2 == 0 { 100.3 } else { 99.7 })?;
            Some(qip_market_ingestion::adapter::SensedRecord::Bar(Box::new(
                Bar {
                    object_id: object.clone(),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: t0().saturating_sub(Duration::from_days(90 - i)),
                    open: close,
                    high: close,
                    low: close,
                    close,
                    volume: Decimal::from_int(1_000_000),
                    trade_count: 5_000,
                    vwap: None,
                    quality: DataQuality::default(),
                },
            )))
        })
        .collect()
}

/// Phase 16's value-of-information deliverable, driven through the sweep
/// rather than through `cheapest_viable` alone: fills that miss the twin by
/// ~200 bp at a venue that traded real paper notional raise a residual gap
/// whose economic value is that miss applied to the notional, and the sweep
/// launches an information request for it on §9.10's product rank. A ~40 bp
/// miss on the same notional ranks below `MIN_VIABLE_BP` (severity 0.16 at
/// half-scale urgency), which is the rank working, not failing; the larger
/// miss is what makes the evidence worth its cost.
///
/// The premise half is the existing fixture: the same residuals on a
/// platform that filled nothing state no economic value and launch nothing,
/// so this test is not passing on a rank that launches everything.
///
/// Mutation (2026-11-13): make `cheapest_viable` return `None` for every
/// gap — `info_launched` is zero and the first assertion fails.
#[test]
fn a_gap_carrying_economic_value_launches_an_information_request_on_the_product_rank() -> Result<()>
{
    let (unpriced, _, _) = two_sweeps()?;
    assert_eq!(
        unpriced.info_launched, 0,
        "premise: an unpriced residual gap launches nothing: {unpriced:?}"
    );

    let (platform, venue) = platform_with_a_fill_of(150)?;
    let observed: Vec<FillObservation> = (0..10)
        .map(|i| FillObservation {
            venue: venue.clone(),
            object_id: "obj-AAA".to_string(),
            filled_at: t0().saturating_add(Duration::from_mins(i)),
            trade_error_bps: Some(if i % 2 == 0 { 190.0 } else { 210.0 }),
        })
        .collect();
    let mut research = ResearchLoop::new(ResearchConfig::default(), BTreeMap::new())?;
    let round = research.sweep_over(
        &platform,
        &observed,
        &[],
        t0().saturating_add(Duration::from_days(1)),
        &set(&[]),
        &set(&["US"]),
    )?;
    assert!(
        round.info_launched > 0,
        "a priced gap launched no information request: {round:?}"
    );
    let launched: Vec<_> = research
        .info_requests()
        .filter(|r| r.status() == qip_contracts::ambient::AcquisitionStatus::InProgress)
        .collect();
    assert!(!launched.is_empty(), "no request is in progress: {round:?}");
    for request in launched {
        assert!(
            request.criteria().net_value_bp() >= qip_deepbrain::research::MIN_VIABLE_BP,
            "a request below the viability floor launched: {request:?}"
        );
    }
    Ok(())
}
