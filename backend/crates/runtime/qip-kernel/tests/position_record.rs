//! What the Asset Brain holds for each position, driven through
//! `Platform::run_cycle` (blueprint §12, R12-ASSET-4066..4069).
//!
//! Every test here opens a position the way a deployment does — a tape, a
//! thesis the review approves, a construction, a fill at the simulated venue
//! — and asserts what the platform can then say about the holding. A test
//! that called the private helpers would prove the arithmetic and not that a
//! cycle reaches it, which is the gap `valuation_seam.rs`'s module note
//! describes in detail.
//!
//! **The fixture lowers the review floor**, as `valuation_seam.rs` does and
//! for the same reason: no synthetic tape clears the shipped 0.50, so at the
//! shipped floor no thesis is approved and no position is ever opened. The
//! floor governs whether the red team's verdict admits a hypothesis; every
//! assertion here is strictly downstream of that verdict. No risk limit, no
//! autonomy ceiling and no paper-trading layer is touched.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::{CycleReport, Stage};
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_portfolio::PositionLifecycle;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// See the module note: low enough that the panel approves a thesis on the
/// synthetic tape. Not the shipped value.
const REVIEW_FLOOR: f64 = 0.10;

const TAPE: usize = 120;
const DROP: f64 = -0.09;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// A leverage cap and nothing else, so a position opened here is not also
/// being judged by a concentration or notional control.
fn leverage_only() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn config() -> PlatformConfig {
    PlatformConfig {
        initial_equity: dec!("1000000"),
        review: ReviewPolicy {
            minimum_surviving_confidence: REVIEW_FLOOR,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    }
}

fn listed(symbol: &str) -> Result<FinancialObject> {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .venue("XNYS")
    .geography("US")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(listed("AAA")?)?;
    Ok(universe)
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

/// A quiet series that drops nine percent on its final bar: a `PriceMove`
/// read as `Claim::Undervalued`, a long the mandate permits.
fn tape(symbol: &str) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let drop = if i == TAPE - 1 { DROP } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// A file-backed log in a directory of this test's own, so a second process
/// can be assembled over the first's.
fn log_path(tag: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-position-record-{tag}-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    directory.join("events.jsonl")
}

fn platform_over(path: &std::path::Path, at: Timestamp) -> Result<Platform> {
    let config = config().with_event_log_file(path);
    let (context, _clock) = Context::deterministic(at, config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe()?,
        leverage_only(),
    )
}

fn learn_problems(report: &CycleReport) -> Vec<String> {
    report
        .problems()
        .into_iter()
        .filter(|(stage, _)| *stage == Stage::Learn)
        .map(|(_, problem)| problem.to_string())
        .collect()
}

/// R12-ASSET-4066: every held position resolves to the mandate and the
/// target weight it was sized under, or it is flagged.
///
/// Two processes over one log. The first opens a position through a whole
/// cycle, so the construction recorded its mandate and target and the
/// position is merely held. The second resumes the same book from the log —
/// proposals are not journalled, so it holds the position and has no record
/// of why — and its first cycle must flag it through
/// `Portfolio::flag_position` and say so on the LEARN stage.
///
/// The failure prevented is a holding with no stated purpose reading exactly
/// like one with a purpose: before this, `construct_from` forgot every
/// target the moment the proposal left and `flag_position` had no kernel
/// caller, so neither process could tell its positions apart.
///
/// Mutation: delete the `flag_position` call in
/// `Platform::review_position_mandates` — the resumed position stays `Held`
/// and the flag assertion fails.
#[test]
fn every_held_position_resolves_to_a_mandate_and_target_or_is_flagged() -> Result<()> {
    let path = log_path("mandate");
    let aaa = object("AAA");
    {
        let mut first = platform_over(&path, start())?;
        first.observe(tape("AAA"));
        let report = first.run_cycle(start());
        assert!(report.traversed_every_stage(), "{}", report.summarise());

        // Premise: the cycle opened a position, so there is a holding to
        // resolve.
        let held = first.positions_at_cost();
        let (quantity, _) = held
            .get(aaa.as_str())
            .copied()
            .expect("premise: the cycle opened a position in obj-AAA");
        assert!(quantity.is_positive(), "premise: a long was opened");

        let record = first
            .position_mandate(aaa.as_str())
            .expect("the construction recorded the mandate it sized obj-AAA under");
        assert_eq!(record.mandate(), first.user_ledger().desk().as_str());
        assert!(
            record.target_weight() > 0.0,
            "a long was opened toward a target of {}",
            record.target_weight()
        );
        let sized = first
            .proposals()
            .iter()
            .find(|proposal| proposal.proposal_id.as_str() == record.proposal())
            .expect("the record names a proposal the platform made");
        assert!(
            sized.legs.iter().any(|leg| leg.object_id == aaa
                && (leg.target_weight - record.target_weight()).abs() < f64::EPSILON),
            "the recorded target is not the sized leg's"
        );
        assert_eq!(
            first.position_lifecycle(aaa.as_str()),
            Some(PositionLifecycle::Held),
            "a position with a mandate on record is held, not flagged"
        );
        assert!(
            !learn_problems(&report)
                .iter()
                .any(|problem| problem.contains("no mandate and target exposure")),
            "a mandated position was reported unmandated: {:?}",
            learn_problems(&report)
        );
    }

    let later = start().saturating_add(Duration::from_hours(1));
    let mut second = platform_over(&path, later)?;
    // Premise: the second process holds the same position, resumed from the
    // log, and has no construction behind it.
    assert!(
        second.positions_at_cost().contains_key(aaa.as_str()),
        "premise: the resumed book holds obj-AAA"
    );
    assert!(second.position_mandate(aaa.as_str()).is_none());
    assert_eq!(
        second.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Held),
        "premise: the register resumed the position as held"
    );

    let report = second.run_cycle(later);
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    assert!(
        second.position_mandate(aaa.as_str()).is_none(),
        "premise: the second process sized obj-AAA again, so the resumed position is not the \
         unmandated one this test needs"
    );
    assert_eq!(
        second.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Flagged),
        "a held position with no mandate on record was not flagged"
    );
    let problems = learn_problems(&report);
    assert!(
        problems.iter().any(|problem| problem
            .starts_with("obj-AAA is held with no mandate and target exposure on record")),
        "the flag was not said on the LEARN stage: {problems:?}"
    );
    Ok(())
}
