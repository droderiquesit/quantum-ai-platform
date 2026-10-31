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
/// A private fund with an undrawn commitment the commitment book will hold,
/// whose one-year lockup expired before `start()` — a fund inside its lockup
/// cannot be sized on a twenty-day thesis (ADR 0183):
/// 400,000 promised against 150,000 drawn, and a residual the valuation
/// plane marks from.
fn private_fund(symbol: &str) -> Result<FinancialObject> {
    private_fund_locked_for(symbol, 1.0)
}

/// [`private_fund`] with a stated lockup term, in years from its vintage.
fn private_fund_locked_for(symbol: &str, lockup_years: f64) -> Result<FinancialObject> {
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
        lockup_years,
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
/// R12-ASSET-4067: a held position's record carries its valuation confidence,
/// its investment horizon and its expected liquidity, each read from the
/// figure the platform decides with — and the record refuses where one of
/// them cannot be stated.
///
/// The private fund is the case that carries meaning: its confidence is the
/// decayed mark's, below one, and its expected exit is read from its terms.
/// Since ADR 0183 a fund still inside its lockup cannot be sized on a thesis
/// shorter than the lockup (the exit plan would be dated inside it), so the
/// fixture's lockup has expired and the expected exit is the catalogue's
/// ninety days; the lockup's dated return for a fund still locked up is
/// proven in `valuation_plane.rs`
/// (`a_private_fund_inside_its_lockup_is_not_counted_as_exitable_on_the_catalogue_s_word`).
/// The
/// horizon is the approved hypothesis's own, the one the thesis's prediction
/// is dated from. A second process resumed from the same log holds the same
/// position with no hypothesis on record, and its record refuses naming the
/// horizon rather than stating one nobody approved.
///
/// The failure prevented: the three facts existed in three places — the
/// valuation plane, the reasoning engine's hypothesis, the liquidity
/// ladder — and nothing joined them to a holding, so a position held past
/// its horizon, or on a mark nobody could defend, read like any other.
///
/// Mutation: drop the horizon lookup in `Platform::construct_from` (record
/// no horizon for the sized leg) — the record refuses for want of a horizon
/// and the first assertion on it fails.
#[test]
fn a_held_position_record_carries_valuation_confidence_horizon_and_expected_liquidity() -> Result<()>
{
    let path = log_path("holding");
    let fund = "obj-PRIV";
    let fund_universe = || -> Result<Universe> {
        let mut universe = Universe::new();
        universe.insert(private_fund("PRIV")?)?;
        Ok(universe)
    };
    let over = |at: Timestamp| -> Result<Platform> {
        let config = config().with_event_log_file(&path);
        let (context, _clock) = Context::deterministic(at, config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            fund_universe()?,
            leverage_only(),
        )
    };
    {
        let mut first = over(start())?;
        first.observe(tape("PRIV"));
        let report = first.run_cycle(start());
        assert!(report.traversed_every_stage(), "{}", report.summarise());
        assert!(
            first.positions_at_cost().contains_key(fund),
            "premise: the cycle opened a position in obj-PRIV: {}",
            report.summarise()
        );
        // Premise: the hypothesis the fund was sized on, and its horizon,
        // read from the platform's own prediction rather than restated.
        let prediction = first
            .predictions()
            .iter()
            .find(|p| p.claim.as_ref().is_some_and(|c| c.subject == fund))
            .cloned()
            .expect("premise: the approved thesis recorded a prediction on obj-PRIV");
        let claim = prediction.claim.expect("the prediction carries its claim");
        let approved_horizon = claim.resolves_at.since(claim.formed_at);
        assert!(approved_horizon.as_nanos() > 0);

        let record = first.position_holding(fund, start())?;
        assert_eq!(record.hypothesis(), prediction.hypothesis);
        assert_eq!(record.horizon(), approved_horizon);
        let sizing = first.sizing_confidence(fund, start())?;
        assert_eq!(record.valuation_confidence(), sizing);
        assert!(
            record.valuation_confidence() < Decimal::ONE,
            "premise: the fund's mark is decayed, so the confidence is not trivially one"
        );
        // The lockup has expired, so the expected exit is the catalogue's
        // ninety days (ADR 0183: a live lockup longer than the thesis is
        // refused at construction and never reaches a held record).
        assert!(
            (record.expected_exit_days() - 90.0).abs() < 1e-9,
            "a fund whose lockup has expired exits on the catalogue's estimate: {} days",
            record.expected_exit_days()
        );
        assert!(
            report
                .stage(Stage::Learn)
                .is_some_and(|learn| learn.detail.contains(
                    "1 of 1 held position(s) carry valuation confidence, horizon and expected \
                     liquidity"
                )),
            "the LEARN stage did not count the record: {}",
            report.summarise()
        );
    }

    let later = start().saturating_add(Duration::from_hours(1));
    let second = over(later)?;
    assert!(
        second.positions_at_cost().contains_key(fund),
        "premise: the resumed book holds obj-PRIV"
    );
    let refusal = second
        .position_holding(fund, later)
        .err()
        .map(|error| error.message().to_string())
        .unwrap_or_default();
    assert!(
        refusal.starts_with("obj-PRIV is held with no hypothesis horizon on record"),
        "a resumed position with no approved hypothesis stated a horizon: {refusal:?}"
    );
    // And a holding nobody has is refused, not recorded.
    assert!(second.position_holding("obj-NONE", later).is_err());
    Ok(())
}

/// R12-ASSET-4069 (ADR 0183): a position cannot be opened without an exit
/// plan its instrument's stated terms admit. The same fund on the same tape
/// is sized when its lockup has expired, and refused at construction when
/// its seven-year lockup is still running — the thesis is twenty days, so
/// any exit it could plan falls inside the lockup — with the lockup named,
/// and nothing is held.
///
/// The failure prevented: before this, a short thesis on a locked-up fund
/// opened a position whose only exit was one the fund's terms forbid, and
/// the platform carried no plan saying when or how it would leave.
///
/// Mutation: in `Platform::exit_plan_for`, pass `None` for the lockup — the
/// locked-up fund is sized and the refusal assertion fails.
#[test]
fn a_short_thesis_on_a_fund_inside_its_lockup_is_refused_with_the_lockup_named_and_nothing_is_held()
-> Result<()> {
    let fund = "obj-PRIV";
    let run = |lockup_years: f64| -> Result<(Platform, String)> {
        let mut universe = Universe::new();
        universe.insert(private_fund_locked_for("PRIV", lockup_years)?)?;
        let mut platform = in_memory(universe, config())?;
        platform.observe(tape("PRIV"));
        let report = platform.run_cycle(start());
        assert!(report.traversed_every_stage(), "{}", report.summarise());
        let summary = report.summarise();
        Ok((platform, summary))
    };

    // Premise: with the lockup expired, the same thesis sizes the fund and
    // dates an exit plan for it.
    let (free, summary) = run(1.0)?;
    assert!(
        free.positions_at_cost().contains_key(fund),
        "premise: an unlocked fund is sized: {summary}"
    );
    let plan = free
        .position_exit_plan(fund)
        .expect("a sized leg carries the exit plan it was admitted with");
    assert!(
        plan.exit_at() > start(),
        "the plan is dated after the decision"
    );

    let (locked, summary) = run(7.0)?;
    assert!(
        !locked.positions_at_cost().contains_key(fund),
        "a fund inside its lockup was sized on a thesis shorter than the lockup: {summary}"
    );
    assert!(
        locked.position_exit_plan(fund).is_none(),
        "no exit plan is recorded for a refused leg"
    );
    assert!(
        summary.contains("falls inside the lockup"),
        "the refusal names the lockup: {summary}"
    );
    Ok(())
}

/// R12-ASSET-3019: the desk's book slot — the one the Portfolio/Asset Brain
/// and construction read through `read_portfolio` — holds the position the
/// cycle opened, marked at its latest close, once the next REASON has run.
/// Before this the desk held the empty portfolio it was assembled with for
/// the life of the process, so the brain would have read an empty book in
/// every running binary.
///
/// Mutation: delete `self.brief_the_desk_on_book();` from REASON — the desk
/// still reads an empty book after the second cycle.
#[test]
fn the_desk_reads_the_book_the_cycle_opened_marked_at_its_latest_close() -> Result<()> {
    let aaa = object("AAA");
    let mut platform = platform_over(&log_path("desk-book"), start())?;
    platform.observe(tape("AAA"));
    assert_eq!(
        platform.desk_book_view().portfolio.positions().count(),
        0,
        "premise: the desk's book starts empty"
    );
    let first = platform.run_cycle(start());
    assert!(first.traversed_every_stage(), "{}", first.summarise());
    assert!(
        platform.positions_at_cost().contains_key(aaa.as_str()),
        "premise: the cycle opened a position in obj-AAA: {}",
        first.summarise()
    );
    let second = platform.run_cycle(start().saturating_add(Duration::from_secs(60)));
    assert!(second.traversed_every_stage(), "{}", second.summarise());
    let view = platform.desk_book_view();
    let held: Vec<&str> = view
        .portfolio
        .positions()
        .map(|position| position.object_id.as_str())
        .collect();
    assert_eq!(held, vec![aaa.as_str()], "the desk reads the held book");
    assert!(
        view.marks
            .get(aaa.as_str())
            .is_some_and(|mark| mark.is_positive()),
        "the held instrument is marked at its latest close: {:?}",
        view.marks
    );
    Ok(())
}

/// R12-TICK-2090: the kernel keeps every instrument it has seen trade in a
/// universe history, and a delisting dates the end of its membership
/// without removing it — so a date before the delisting still finds the
/// instrument, and a date after does not. Before this the history type
/// existed with no caller, and the only universe a backtest could ask was
/// the current one.
///
/// Mutation: drop the `universe_history.delist` arm in
/// `Platform::apply_corporate_action` — the instrument is still a member
/// after its delisting and the last assertion fails.
#[test]
fn the_kernels_universe_history_keeps_a_delisted_instrument_for_the_dates_it_traded() -> Result<()>
{
    let aaa = object("AAA");
    let mut platform = platform_over(&log_path("universe-history"), start())?;
    platform.observe(tape("AAA"));
    let delisted = start().saturating_sub(Duration::from_days(10));
    platform.observe(vec![SensedRecord::CorporateAction(Box::new(
        CorporateAction {
            object_id: aaa.clone(),
            ex_date: delisted,
            record_date: None,
            payment_date: None,
            kind: CorporateActionKind::Delisting {
                reason: "acquired for cash".to_string(),
            },
            announced_at: start().saturating_sub(Duration::from_days(40)),
        },
    ))]);
    let report = platform.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());

    let history = platform.universe_history();
    assert!(history.is_named(&aaa), "premise: the instrument is listed");
    let before = start().saturating_sub(Duration::from_days(20));
    assert!(
        history
            .members_as_of(before)
            .iter()
            .any(|listing| listing.object_id == aaa),
        "a date before the delisting finds the instrument that later went"
    );
    assert!(
        !history.is_member_at(&aaa, start().saturating_sub(Duration::from_days(5))),
        "a date after the delisting does not"
    );
    Ok(())
}

/// R12-ASSET-1015: a desk position is booked through the adapter and
/// lifecycle engine its asset class names in the registry — an equity
/// through the venue order book and the portfolio stages — resolved at the
/// fill on the cycle's own path. Before this `lifecycle_of` had no caller and
/// the registry named no adapter at all.
///
/// Mutation: drop the `position_bookings.insert` in
/// `Platform::book_desk_fill` — the position carries no booking and this
/// fails.
#[test]
fn a_position_is_booked_through_the_lifecycle_engine_and_adapter_its_asset_class_names()
-> Result<()> {
    use qip_kernel::asset_class_registry::{ClassAdapter, LifecycleEngine};
    let aaa = object("AAA");
    let mut platform = platform_over(&log_path("booking"), start())?;
    assert!(platform.position_booking(aaa.as_str()).is_none());
    platform.observe(tape("AAA"));
    let report = platform.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    assert!(
        platform.positions_at_cost().contains_key(aaa.as_str()),
        "premise: the cycle opened a position in obj-AAA: {}",
        report.summarise()
    );
    assert_eq!(
        platform.position_booking(aaa.as_str()),
        Some((
            ClassAdapter::VenueOrderBook,
            LifecycleEngine::PortfolioStages
        )),
        "an equity is booked through the venue order book and the portfolio stages"
    );
    Ok(())
}

/// R12-ASSET-1160, the divestment item: an order that sells the whole long
/// moves the position to `Unwinding` before any fill exists, and a trim
/// leaves it `Held`.
///
/// The failure prevented: `Portfolio::begin_unwind` had no production
/// caller, so a book being sold down read as `Held` until its last lot
/// closed, and the Portfolio/Asset Brain counted zero divestments on every
/// book. Both orders go through `Platform::submit_order`, the one seam every
/// desk order (the ACT stage's included) passes.
///
/// Mutation: delete the `begin_deliberate_close` call in
/// `Platform::submit_order` — the full sell leaves the position `Held` and
/// the second assertion fails. Mutation: drop `order.quantity < long` from
/// its guard — the trim unwinds the position and the first assertion fails.
#[test]
fn selling_the_whole_long_unwinds_the_position_and_a_trim_leaves_it_held() -> Result<()> {
    use qip_execution_engine::order::Side;
    let aaa = object("AAA");
    let mut platform = platform_over(&log_path("unwind"), start())?;
    platform.observe(tape("AAA"));
    let report = platform.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    let (held, _) = platform
        .positions_at_cost()
        .get(aaa.as_str())
        .copied()
        .expect("premise: the cycle opened a position in obj-AAA");
    assert!(
        held > dec!("1"),
        "premise: a long of more than one unit: {held}"
    );
    assert_eq!(
        platform.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Held),
        "premise: the opened position is held"
    );

    let now = start().saturating_add(Duration::from_mins(5));
    let price = dec!("90");
    let trim = platform.order_from(
        aaa.clone(),
        Side::Sell,
        dec!("1"),
        price,
        "prop-trim",
        vec!["hyp-trim".to_string()],
        now,
    );
    let _ = platform.submit_order(trim, now);
    assert_eq!(
        platform.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Held),
        "a sell smaller than the long is a trim, not a divestment"
    );

    let (remaining, _) = platform
        .positions_at_cost()
        .get(aaa.as_str())
        .copied()
        .expect("premise: a trim leaves the position open");
    let close = platform.order_from(
        aaa.clone(),
        Side::Sell,
        remaining,
        price,
        "prop-close",
        vec!["hyp-close".to_string()],
        now,
    );
    let _ = platform.submit_order(close, now);
    assert_eq!(
        platform.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Unwinding),
        "a sell of the whole long, partly filled, was not recorded as a divestment"
    );
    Ok(())
}
