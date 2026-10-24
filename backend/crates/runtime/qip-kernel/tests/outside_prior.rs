//! An outside prior may only narrow REASON (ADR 0151 decision 3, R12-TICK-1002).
//!
//! The failure the suite guards: a fused posterior is a belief about an
//! instrument reacting, not evidence the panel weighed. If it could raise a
//! hypothesis's confidence, an unreviewed statistic would be one governed
//! approval away from a position size. So these tests drive the production
//! cycle (`Platform::run_cycle` through REASON) and read the confidence the
//! claim was written at, not a number the test computed.

#![allow(clippy::panic_in_result_fn)]
// Exact comparison is the claim: a prior that leans for the hypothesis must
// leave the confidence the same number, bit for bit.
#![allow(clippy::float_cmp)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::fusion::OutsidePrior;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- fixtures, the same shape `learning.rs` feeds -----------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    fixture_liquidity(),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

fn fresh() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// A price series with a jump partway through, so the detectors have
/// something real to find.
fn bars(symbol: &str, count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let jump = if i == count * 2 / 3 { 0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// One REASON over the jump tape with `priors` set; the confidence the claim
/// was written at, which is the number calibration grades and DECIDE sizes on,
/// and the sign of the move it expects.
fn reasoned_with(priors: Vec<OutsidePrior>) -> Result<(f64, f64)> {
    let mut platform = fresh()?;
    platform.set_outside_priors(priors)?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());
    let claim = platform
        .predictions()
        .first()
        .and_then(|prediction| prediction.claim.as_ref())
        .unwrap_or_else(|| panic!("premise: the cycle formed a claim:\n{}", report.summarise()));
    Ok((claim.confidence, claim.expected_move_bps))
}

fn confidence_with(priors: Vec<OutsidePrior>) -> Result<f64> {
    Ok(reasoned_with(priors)?.0)
}

#[test]
fn a_posterior_against_the_hypothesis_narrows_its_confidence_and_one_for_it_changes_nothing()
-> Result<()> {
    let (baseline, expected_move) = reasoned_with(Vec::new())?;
    assert!(
        baseline > 0.0,
        "premise: an unnarrowed hypothesis has a confidence to narrow"
    );
    assert!(
        expected_move != 0.0,
        "premise: the claim expects a signed move, so a posterior can lean for or against it"
    );
    let sign = expected_move.signum();

    // 0.9 in the claim's own direction is for the hypothesis; 0.9 the other
    // way is against it.
    let for_it = confidence_with(vec![OutsidePrior::new("obj-AAA", sign, 0.9)?])?;
    let against_it = confidence_with(vec![OutsidePrior::new("obj-AAA", -sign, 0.9)?])?;

    assert_eq!(
        for_it, baseline,
        "a posterior leaning for the hypothesis moved its confidence from {baseline} to {for_it}"
    );
    // m = 1 - (2 * 0.9 - 1) = 0.2.
    assert!(
        (against_it - baseline * 0.2).abs() < 1e-12,
        "a 0.9 posterior against must scale the confidence by 0.2: {baseline} became {against_it}"
    );
    Ok(())
}

#[test]
fn a_posterior_about_another_instrument_or_no_better_than_a_coin_changes_nothing() -> Result<()> {
    let baseline = confidence_with(Vec::new())?;
    for prior in [
        OutsidePrior::new("obj-BBB", 1.0, 0.99)?,
        OutsidePrior::new("obj-BBB", -1.0, 0.99)?,
        OutsidePrior::new("obj-AAA", 1.0, 0.5)?,
        OutsidePrior::new("obj-AAA", -1.0, 0.5)?,
    ] {
        let narrowed = confidence_with(vec![prior.clone()])?;
        assert_eq!(
            narrowed, baseline,
            "{prior:?} must not act: another subject, or a posterior of one half"
        );
    }
    Ok(())
}

#[test]
fn no_posterior_ever_raises_a_confidence_and_malformed_ones_are_refused() -> Result<()> {
    for tenth in 0..=10 {
        let p = f64::from(tenth) / 10.0;
        for direction in [1.0, -1.0] {
            let prior = OutsidePrior::new("obj-AAA", direction, p)?;
            for sign in [1.0, -1.0] {
                let m = prior.narrowing_for(sign);
                assert!(
                    (0.0..=1.0).contains(&m),
                    "p {p} direction {direction} sign {sign} gave multiplier {m}"
                );
            }
        }
    }
    // The premise of the sweep: a certain posterior does narrow to nothing.
    assert_eq!(
        OutsidePrior::new("obj-AAA", 1.0, 1.0)?.narrowing_for(-1.0),
        0.0
    );
    assert!(OutsidePrior::new("obj-AAA", 1.0, 1.5).is_err());
    assert!(OutsidePrior::new("obj-AAA", 1.0, f64::NAN).is_err());
    assert!(OutsidePrior::new("obj-AAA", 0.0, 0.9).is_err());
    assert!(OutsidePrior::new("", 1.0, 0.9).is_err());
    let too_many: Vec<OutsidePrior> = (0..1_025)
        .map(|i| OutsidePrior::new(format!("obj-{i}"), 1.0, 0.9))
        .collect::<Result<_>>()?;
    assert!(fresh()?.set_outside_priors(too_many).is_err());
    Ok(())
}
