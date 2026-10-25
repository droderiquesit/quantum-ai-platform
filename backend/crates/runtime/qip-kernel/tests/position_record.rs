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
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::position_record::Declared;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::{CycleReport, Stage};
use qip_kernel::hedge_review::HedgePolicyDeclaration;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_portfolio::PositionLifecycle;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::hedge::HedgeAxis;
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
/// A private fund with an undrawn commitment the commitment book will hold:
/// 400,000 promised against 150,000 drawn, and a residual the valuation
/// plane marks from.
fn private_fund(symbol: &str) -> Result<FinancialObject> {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::PrivateEquityFund,
        LiquidityProfile::illiquid(90.0, 250.0),
    )
    .venue("OTC")
    .geography("US")
    .price(dec!("100"))
    .extension(Extension::PrivateAsset(PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: dec!("400000"),
        called_capital: dec!("150000"),
        distributed_capital: Decimal::ZERO,
        residual_value: dec!("160000"),
        stage: "buyout".to_string(),
        lockup_years: 7.0,
        capital_call_notice_days: 10,
        call_schedule: None,
    }))
    .provenance(Provenance::synthetic("administrator", start()))
    .build(start())
}

fn in_memory(universe: Universe, config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        leverage_only(),
    )
}

fn present<T: Clone>(declared: &Declared<T>, what: &str) -> T {
    match declared {
        Declared::Present(value) => value.clone(),
        Declared::NoneApplicable { reason } => {
            panic!("{what} was declared not applicable ({reason}) where it applies")
        }
    }
}

fn none_reason<T: std::fmt::Debug>(declared: &Declared<T>, what: &str) -> String {
    match declared {
        Declared::NoneApplicable { reason } => reason.clone(),
        Declared::Present(value) => {
            panic!("{what} was stated as {value:?} where none applies; 'none' must be declared")
        }
    }
}

/// R12-ASSET-4068: each held position resolves its hedge relationship, its
/// pending corporate actions and its commitments, with "none" *stated* —
/// `Declared::NoneApplicable` with a reason — rather than inferred from an
/// empty collection nobody filled in.
///
/// Two books, so every one of the three facts is seen both present and
/// absent. The listed name is held under a sector hedge policy and has a
/// dividend announced and not yet ex, and owes nothing; the private fund
/// owes an undrawn commitment, is under no hedge policy and has nothing
/// announced.
///
/// The failure prevented: before this the facts lived in three records keyed
/// by object id and nothing joined them to a holding, so "this position is
/// unhedged" was an absence anyone could read into an empty list.
///
/// Mutation: return `Declared::Present(Vec::new())` instead of
/// `NoneApplicable` for an unhedged position in
/// `Platform::position_obligations` — the private fund's hedges read as an
/// empty list and the stated-none assertion fails.
#[test]
fn each_held_position_states_its_hedge_corporate_actions_and_commitments_or_none() -> Result<()> {
    let policies = vec![
        HedgePolicyDeclaration::new(
            "tech-beta",
            HedgeAxis::Sector,
            Sector::InformationTechnology.as_str(),
            object("HEDGE"),
            dec!("1"),
        ),
        HedgePolicyDeclaration::new(
            "energy-beta",
            HedgeAxis::Sector,
            Sector::Energy.as_str(),
            object("HEDGE"),
            dec!("1"),
        ),
    ];

    // --- the listed name -------------------------------------------------
    let mut listed_book = in_memory(universe()?, config().with_hedge_policies(policies))?;
    listed_book.observe(tape("AAA"));
    let report = listed_book.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    assert!(
        listed_book.positions_at_cost().contains_key("obj-AAA"),
        "premise: the cycle opened a position in obj-AAA"
    );
    let ex_date = start().saturating_add(Duration::from_days(30));
    listed_book.observe(vec![SensedRecord::CorporateAction(Box::new(
        CorporateAction {
            object_id: object("AAA"),
            ex_date,
            record_date: None,
            payment_date: None,
            kind: CorporateActionKind::CashDividend {
                amount: dec!("0.5"),
            },
            announced_at: start(),
        },
    ))]);

    let obligations = listed_book.position_obligations()?;
    let aaa = obligations
        .get("obj-AAA")
        .expect("the held position states its obligations");
    let hedges = present(aaa.hedges(), "obj-AAA's hedge relationship");
    assert_eq!(hedges.len(), 1, "{hedges:?}");
    assert!(
        hedges[0].starts_with("tech-beta (hedged with obj-HEDGE on sector "),
        "{hedges:?}"
    );
    let actions = present(
        aaa.corporate_actions(),
        "obj-AAA's pending corporate actions",
    );
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert!(
        actions[0].starts_with(&format!("obj-AAA:{}:", ex_date.as_nanos())),
        "{actions:?}"
    );
    let owed = none_reason(aaa.commitments(), "obj-AAA's commitments");
    assert!(!owed.trim().is_empty());

    // --- the private fund ------------------------------------------------
    let mut fund_universe = Universe::new();
    fund_universe.insert(private_fund("PRIV")?)?;
    let mut fund_book = in_memory(fund_universe, config())?;
    fund_book.observe(tape("PRIV"));
    let report = fund_book.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    assert!(
        fund_book.positions_at_cost().contains_key("obj-PRIV"),
        "premise: the cycle opened a position in obj-PRIV: {}",
        report.summarise()
    );
    let obligations = fund_book.position_obligations()?;
    let fund = obligations
        .get("obj-PRIV")
        .expect("the held position states its obligations");
    let commitment = present(fund.commitments(), "obj-PRIV's commitment");
    assert_eq!(commitment.subject(), "obj-PRIV");
    assert_eq!(
        none_reason(fund.hedges(), "obj-PRIV's hedge relationship"),
        "no hedge policy is declared for this deployment"
    );
    let announced = none_reason(fund.corporate_actions(), "obj-PRIV's corporate actions");
    assert!(!announced.trim().is_empty());
    Ok(())
}
