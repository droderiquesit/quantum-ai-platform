//! Blueprint §23.3, §23.6, §19.2 and §18.4 once they are assembled.
//!
//! Each of the four is a tested unit in its own crate. What no crate's own
//! suite can see is the seam between them, which is where all four of these
//! sections actually meet:
//!
//! * The regime the cost router classifies is the regime the allocator reads
//!   (§23.3) — two services that never name each other.
//! * The desk's mandate, stated in `qip-kernel`'s configuration, is the same
//!   number `qip-capital`'s compounding policy is built from (§18.4). Two
//!   independent claims about one number will disagree, and the louder one
//!   will be wrong.
//! * The strategy population the foundry registers is the population §19.2
//!   tiers, and the cadence that decides whether it is tiered at all is
//!   §23.6's.
//!
//! Every assertion here is about a platform that was actually constructed,
//! rather than about a fixture assembled to suit the assertion.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion aborting a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::compounding::ReinvestmentDecision;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::adaptive_cadence::{
    self, CadenceSignals, REINVESTMENT_CADENCE_CYCLES, TIERING_CADENCE_CYCLES, Work,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_optimization_engine::regime::{self, AllocationRegime};
use qip_optimization_engine::tiers::{EvaluationTier, HOT_TIER_CAP, TierPlan};
use qip_optimization_engine::universe::AlphaFamily;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Exact equality is what these assertions mean: both sides are literals
/// carried unchanged from one crate to another, so a difference of any size
/// is the failure being tested for. The epsilon satisfies `float_cmp` without
/// suppressing it for the whole file.
fn same(left: f64, right: f64) -> bool {
    (left - right).abs() < f64::EPSILON
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["ACME", "BOREAS"] {
        universe
            .insert(
                FinancialObject::builder(
                    ObjectId::from_string(format!("obj-{symbol}")),
                    symbol,
                    InstrumentType::CommonStock,
                    fixture_liquidity(),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("regime-allocation", start()))
                .build(start())
                .expect("valid instrument"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("regime-allocation").with(
        Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        )
        .with_rationale("no single name may dominate the book"),
    )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

#[test]
fn the_compounding_policy_is_built_from_the_desks_own_mandate_and_not_from_a_second_copy_of_it()
-> Result<()> {
    // Principle 6: two independent claims about one fact will disagree, and
    // the louder one will be wrong. The redeployment cost is the mandate's
    // turnover cost, and the minimum lot is the smallest position that
    // mandate will hold at this book's equity — so a desk that changes its
    // mandate changes the compounding policy, and nothing here restates
    // either number.
    let platform = platform()?;
    let mandate = platform.config().mandate;
    // The premise: the two mandate figures are non-trivial, or the equality
    // below would hold against a policy that read nothing at all.
    assert!(
        mandate.turnover_cost_bps > 0.0,
        "the premise is a real cost"
    );
    assert!(
        mandate.minimum_position > 0.0,
        "the premise is a real minimum position"
    );

    let policy = adaptive_cadence::policy_for(&platform)?;
    assert!(
        same(policy.redeployment_cost_bps(), mandate.turnover_cost_bps),
        "the compounding policy invented its own transaction cost"
    );
    let expected_lot = platform
        .equity()
        .checked_apply_bps(mandate.minimum_position * 10_000.0)
        .expect("a representable lot");
    assert_eq!(
        policy.minimum_lot(),
        expected_lot,
        "the minimum reinvestment lot is not the smallest position the mandate holds"
    );
    assert_eq!(policy.cadence_cycles(), REINVESTMENT_CADENCE_CYCLES);

    // And the policy it produced actually refuses: a lot one unit under the
    // smallest position the mandate would hold is not planned, and one at it
    // is. A policy whose threshold no amount could cross would be the
    // `MaxExpectedShortfall` shape.
    let under = expected_lot
        .checked_sub(Decimal::from_raw(1))
        .expect("representable");
    assert!(
        matches!(
            policy.plan(under, 0)?,
            ReinvestmentDecision::BelowMinimumLot { .. }
        ),
        "a lot under the mandate's smallest position was planned"
    );
    assert!(
        policy.plan(expected_lot, 0)?.plan().is_some(),
        "a lot at the mandate's smallest position was refused, so no amount can cross"
    );
    Ok(())
}

#[test]
fn a_freshly_opened_book_runs_no_cadence_work_and_says_nothing_about_it() -> Result<()> {
    // §23.6's last row against a real platform: no strategy is registered
    // and no profit has been realised, so neither item has a subject and the
    // cycle entry gains nothing. The premise is asserted first — this is the
    // absence of subjects rather than a review that refused.
    let platform = platform()?;
    assert_eq!(platform.cycle_count(), 0);
    assert!(
        adaptive_cadence::population_of(&platform).is_empty(),
        "the premise fails: this book already has a registered family"
    );
    let signals = adaptive_cadence::signals_of(&platform);
    assert_eq!(signals.classified_strategies, 0);
    assert_eq!(signals.undeployed_profit, Decimal::ZERO);

    let (summary, problems) = adaptive_cadence::review(&platform);
    assert_eq!(summary, None, "work ran on a book with nothing to work on");
    assert!(
        problems.is_empty(),
        "the saving reported problems: {problems:?}"
    );

    // And the same platform, handed signals with a subject on a due cycle,
    // does run both items — so the silence above is the saving and not a
    // review that can never fire.
    let policy = adaptive_cadence::policy_for(&platform)?;
    let plan = adaptive_cadence::plan(
        &CadenceSignals {
            cycle: TIERING_CADENCE_CYCLES,
            classified_strategies: 4,
            undeployed_profit: policy.minimum_lot(),
        },
        &policy,
    );
    assert!(plan.wants(Work::EvaluationTiering));
    assert!(plan.wants(Work::CompoundingPlan));
    Ok(())
}

#[test]
fn a_family_name_that_is_not_an_alpha_family_is_tiered_cold_rather_than_guessed_at() -> Result<()> {
    // The seam between `qip-lifecycle`'s family names — free text a desk
    // chooses — and §19's ten alpha families. A prefix match here would file
    // `momentum-v3` under continuation on the strength of a string, and the
    // hot tier's cap is a latency budget that may not be spent on a guess.
    let population: BTreeMap<String, (Option<AlphaFamily>, usize)> = ["momentum-v3", "mm-sweep-2"]
        .into_iter()
        .map(|name| (name.to_string(), (AlphaFamily::parse(name).ok(), 400usize)))
        .collect();
    assert_eq!(population.len(), 2, "the premise is two named families");
    assert!(
        population.values().all(|(family, _)| family.is_none()),
        "a desk's sweep name was read as an alpha family"
    );
    let census = TierPlan::assign_counted(&population, HOT_TIER_CAP)?;
    assert_eq!(census.population(), 800);
    assert_eq!(
        census.count(EvaluationTier::Hot),
        0,
        "an unclassified sweep reached the hot tier"
    );
    assert_eq!(census.count(EvaluationTier::Batch), 800);

    // The same population named after the alpha source it harvests *does*
    // reach the hot tier — the admitting half, without which "cold" would be
    // the only answer the census can give.
    let mut named = BTreeMap::new();
    named.insert(
        "market_making".to_string(),
        (AlphaFamily::parse("market_making").ok(), 400usize),
    );
    let census = TierPlan::assign_counted(&named, HOT_TIER_CAP)?;
    assert_eq!(census.count(EvaluationTier::Hot), 400);
    Ok(())
}

#[test]
fn the_regime_reader_narrows_a_bound_in_every_regime_and_widens_one_in_none() -> Result<()> {
    // The composed guarantee: whatever regime the platform is in, the
    // allocator's answer is at most the mandate's own cap, and in no regime
    // is it the cap itself — a reader whose output never moved would be
    // invisible in production however carefully it was written.
    //
    // The enumeration is the allocator's, because `qip-cost-router` is not a
    // dependency of this suite. That the classifier's five arms map onto
    // these five exhaustively is `qip-kernel`'s own
    // `every_regime_the_classifier_can_return_maps_to_one_the_allocator_reads`,
    // which is where both crates are in scope.
    assert_eq!(
        AllocationRegime::ALL.len(),
        5,
        "the premise is the five regimes the classifier can return"
    );
    let mut narrowing = 0usize;
    for regime in AllocationRegime::ALL {
        let multiplier = regime::unattributed_multiplier(regime);
        assert!(
            multiplier > 0.0 && multiplier <= 1.0,
            "{} produced a cap of {multiplier}, which the constructor would refuse",
            regime.as_str()
        );
        if multiplier < 1.0 {
            narrowing += 1;
        }
    }
    assert_eq!(
        narrowing,
        AllocationRegime::ALL.len(),
        "a regime leaves the bound untouched, so the reader is invisible in that regime"
    );
    // And the two values production can take differ, so the regime is
    // genuinely an input to the size rather than a constant wearing a
    // control's clothes.
    assert!(
        regime::unattributed_multiplier(AllocationRegime::Crisis)
            < regime::unattributed_multiplier(AllocationRegime::Trending),
        "every regime narrows by the same amount"
    );

    // And the favouring direction carries no number anywhere: the one regime
    // whose table names a family hands that family exactly its mandate cap.
    let favoured = regime::favoured(AllocationRegime::Trending);
    assert!(!favoured.is_empty(), "the premise is a regime with a table");
    for family in favoured {
        assert!(
            same(regime::multiplier(AllocationRegime::Trending, family), 1.0),
            "a favoured family was handed more than the mandate's cap"
        );
    }
    Ok(())
}
