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
use qip_events::Topic;
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
use qip_kernel::position_register::PositionObligationsStated;
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_portfolio::PositionLifecycle;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::hedge::HedgeAxis;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;

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

/// Buy ten units of `symbol` through the desk control path, so the book
/// holds it; the premise is asserted rather than assumed.
fn hold(platform: &mut Platform, symbol: &str) -> Result<()> {
    let order = platform.order_for(
        object(symbol),
        qip_execution_engine::order::Side::Buy,
        dec!("10"),
        dec!("100"),
        "prop-hedge-leg",
        vec!["hyp-hedge-leg".to_string()],
        start(),
    )?;
    platform.submit_order(order, start())?;
    assert!(
        platform
            .positions_at_cost()
            .get(object(symbol).as_str())
            .is_some_and(|(quantity, _)| !quantity.is_zero()),
        "premise: obj-{symbol} is held"
    );
    Ok(())
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
        // Hedged with a distinct instrument the book holds: obj-HEDGE, an
        // information-technology name bought after the cycle opens obj-AAA.
        // Until 2026-11-12 this policy hedged with obj-AAA itself, and the
        // test read a position offsetting itself as hedged.
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
    let mut hedged_universe = universe()?;
    hedged_universe.insert(listed("HEDGE")?)?;
    let mut listed_book = in_memory(hedged_universe, config().with_hedge_policies(policies))?;
    listed_book.observe(tape("AAA"));
    let report = listed_book.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    assert!(
        listed_book.positions_at_cost().contains_key("obj-AAA"),
        "premise: the cycle opened a position in obj-AAA"
    );
    hold(&mut listed_book, "HEDGE")?;
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
    // The hedge leg itself: it hedges the bucket obj-AAA is held in, and
    // nothing else. Not hedged by tech-beta although tech-beta covers it — a
    // position is not its own hedge — and not the hedge for energy, where
    // nothing is held.
    let hedge_leg = obligations
        .get("obj-HEDGE")
        .expect("the held hedge leg states its obligations");
    assert_eq!(
        present(hedge_leg.hedges(), "obj-HEDGE's hedge relationship"),
        vec![format!(
            "tech-beta (this position is the hedge for sector {})",
            Sector::InformationTechnology.as_str()
        )],
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
    let (sized_hypothesis, sized_horizon, sized_plan) = {
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
        let plan = *first
            .position_exit_plan(fund)
            .expect("premise: the construction dated an exit plan for obj-PRIV");
        assert_eq!(plan.exit_at(), start().saturating_add(approved_horizon));
        (record.hypothesis().to_string(), record.horizon(), plan)
    };

    // A second process over the same log, which ran no construction: the
    // horizon and the exit plan are the journalled ones, not refused and not
    // re-derived from a thesis this process never approved (R12-ASSET-4067,
    // R12-ASSET-4069). Before the record was journalled, this process
    // refused the holding "no hypothesis horizon on record" and held no plan.
    //
    // Mutation: drop the `resume_position_horizons` call at assembly — the
    // resumed holding is refused and this assertion fails.
    let later = start().saturating_add(Duration::from_hours(1));
    let second = over(later)?;
    assert!(
        second.positions_at_cost().contains_key(fund),
        "premise: the resumed book holds obj-PRIV"
    );
    assert!(
        second.proposals().is_empty(),
        "premise: the second process constructed nothing, so what it states is resumed"
    );
    let resumed = second.position_holding(fund, later)?;
    assert_eq!(resumed.hypothesis(), sized_hypothesis);
    assert_eq!(resumed.horizon(), sized_horizon);
    assert_eq!(
        second.position_exit_plan(fund).copied(),
        Some(sized_plan),
        "the resumed exit plan is not the one construction dated"
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

/// R12-ASSET-3019 (ADR 0468): the desk's book slot carries what the held
/// position is held *for* — the hypothesis and horizon `construct_from`
/// recorded, the sizing instant beside its mandate, and the hedge standing
/// the register states — so the Portfolio/Asset Brain reads the register's
/// facts on the default cycle. With no hedge policy declared the position
/// is stated *unhedged*, with the reason, and never left undeclared.
///
/// Mutations: drop `view.purpose = purpose;` from `brief_the_desk_on_book`
/// — the slot keeps the empty purpose it was assembled with and the horizon
/// assertion fails; drop the hedge insert in `book_purpose` — the standing
/// reads `None`; drop the factor assignment — no factor allocation arrives.
#[test]
fn the_desk_reads_each_held_positions_horizon_and_hedge_standing() -> Result<()> {
    use qip_investment_agents::desk::HedgeStanding;
    let aaa = object("AAA");
    // AAA loads 1.2 on one catalogue factor, so the factor allocation has
    // something to carry; the shared fixture states none.
    let mut loaded = listed("AAA")?;
    loaded
        .risk
        .factor_exposures
        .loadings
        .insert("equity_market".to_string(), 1.2);
    let mut universe = Universe::new();
    universe.insert(loaded)?;
    let config = config().with_event_log_file(log_path("desk-book-purpose"));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        leverage_only(),
    )?;
    platform.observe(tape("AAA"));
    let first = platform.run_cycle(start());
    assert!(first.traversed_every_stage(), "{}", first.summarise());
    let (hypothesis, horizon) = {
        let holding = platform.position_holding(aaa.as_str(), start());
        let holding = holding.map_err(|e| {
            qip_core::error::Error::invalid(format!(
                "premise: the cycle sized obj-AAA on a hypothesis with a horizon: {}",
                e.message()
            ))
        })?;
        (holding.hypothesis().to_string(), holding.horizon())
    };
    let second = platform.run_cycle(start().saturating_add(Duration::from_secs(60)));
    assert!(second.traversed_every_stage(), "{}", second.summarise());
    let view = platform.desk_book_view();
    let held = view
        .purpose
        .horizons
        .get(aaa.as_str())
        .unwrap_or_else(|| panic!("no horizon reached the desk: {:?}", view.purpose));
    assert_eq!(
        held.hypothesis, hypothesis,
        "the driver is the sizing hypothesis"
    );
    assert_eq!(held.horizon, horizon);
    assert!(
        held.sized_at.is_some(),
        "the sizing instant beside the mandate reached the desk"
    );
    match view.purpose.hedges.get(aaa.as_str()) {
        Some(HedgeStanding::Unhedged { reason }) => assert!(
            reason.contains("no hedge policy is declared"),
            "the reason is the register's: {reason}"
        ),
        other => panic!("obj-AAA's hedge standing was not stated as unhedged: {other:?}"),
    }
    assert!(view.purpose.hedges_refused.is_none());
    assert!(view.purpose.factors_refused.is_none());
    let factor = view
        .purpose
        .factor_exposure
        .get("equity_market")
        .copied()
        .unwrap_or_else(|| panic!("no factor allocation reached the desk: {:?}", view.purpose));
    let quantity = view
        .portfolio
        .positions()
        .find(|position| position.object_id == aaa)
        .map(|position| position.quantity())
        .expect("premise: the desk's book holds obj-AAA");
    let mark = view
        .marks
        .get(aaa.as_str())
        .copied()
        .expect("premise: marked");
    assert_eq!(
        factor,
        dec!("1.2") * quantity * mark,
        "the factor exposure is the loading times the marked position"
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

/// R12-ASSET-1160, divestment on the default path: a cycle whose tape makes
/// construction target a zero weight on a long held at a gain moves the
/// position to `Unwinding`, although the sell it sizes is below the long.
///
/// The failure prevented: an exit leg is sized from narrowed free capital
/// times the held weight and floored to whole lots, so a full exit at or
/// above cost always sells less than the long. Judged by quantity alone, the
/// desk's decision to divest booked as a trim and the position stayed `Held`
/// (the gap an independent review found in the first version of this seam).
///
/// And the sell is sized to the long, so the decision to divest is not
/// executed as a trim that leaves part of the position held under it (the
/// second review's finding).
///
/// Mutations: pass `false` for `to_flat` to `submit_released` in
/// `Platform::stage_act_orders` — the position stays `Held`; size the order
/// from `leg.quantity` whatever `to_flat` says — less than the leg's quantity
/// leaves the book and the last assertion fails.
#[test]
fn a_cycle_whose_tape_makes_construction_target_a_zero_weight_on_a_long_held_at_a_gain_moves_the_position_to_unwinding_although_the_sized_sell_is_below_the_long()
-> Result<()> {
    let aaa = object("AAA");
    let mut platform = platform_over(&log_path("zero-target"), start())?;
    platform.observe(tape("AAA"));
    let report = platform.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    let (held, cost) = platform
        .positions_at_cost()
        .get(aaa.as_str())
        .copied()
        .expect("premise: the first cycle opened a long in obj-AAA");
    assert!(held.is_positive(), "premise: a long: {held}");
    assert_eq!(
        platform.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Held)
    );

    // A rally past the entry, so the long is held at a gain when the next
    // construction decides on it.
    let mut price = 92.0_f64;
    let mut bars = Vec::new();
    for i in 0..10 {
        let open = price;
        price *= if i == 9 { 1.15 } else { 1.002 };
        bars.push(bar(
            "AAA",
            start().saturating_add(Duration::from_days(i + 1)),
            open,
            price,
        ));
    }
    assert!(
        Decimal::from_f64(price).expect("a price") > cost,
        "premise: the position is at a gain ({price} against a cost of {cost})"
    );
    platform.observe(bars);
    let later = start().saturating_add(Duration::from_days(11));
    let report = platform.run_cycle(later);
    assert!(report.traversed_every_stage(), "{}", report.summarise());

    let exit = platform
        .proposals()
        .iter()
        .flat_map(|proposal| proposal.legs.iter())
        .find(|leg| {
            leg.object_id == aaa
                && leg.target_weight == 0.0
                && matches!(leg.side, qip_portfolio_engine::proposal::Side::Sell)
        })
        .expect("premise: the second construction targeted obj-AAA at zero weight");
    assert!(
        exit.quantity < held,
        "premise: the sized exit ({}) is below the long ({held}), so the quantity rule alone \
         would read it as a trim",
        exit.quantity
    );
    assert_eq!(
        platform.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Unwinding),
        "a sell the construction targeted at zero weight was not recorded as a divestment"
    );
    // The divestment sold the long, not the floored leg: what left the book
    // is more than the leg's whole quantity could have taken out of it.
    let remaining = platform
        .positions_at_cost()
        .get(aaa.as_str())
        .map_or(Decimal::ZERO, |(quantity, _)| *quantity);
    assert!(
        held - remaining > exit.quantity,
        "the exit was sized to the floored leg ({}) rather than the long ({held}): {} left \
         the book and {remaining} is still held",
        exit.quantity,
        held - remaining
    );
    Ok(())
}

/// R12-ASSET-1160, the refused divestment: a sell the construction targeted
/// at zero weight that the wallet halt stops before an order exists leaves
/// the position `Held` and the long whole.
///
/// The failure prevented: the unwind was once recorded before the controls
/// ruled, so a divestment the desk was not allowed to send still relabelled
/// the position `Unwinding` — a register saying the book was being sold
/// while the custodian disagreed about what the book held and nothing sold.
///
/// The borrow rule is the other refusal ahead of the unwind and cannot stop
/// this sell: a zero-target sell is sized to the long and never past it, so
/// it is never a short.
///
/// Mutation: call `begin_deliberate_close` before the wallet-halt check in
/// `Platform::submit_released` — the position reads `Unwinding`.
#[test]
fn a_zero_target_sell_the_wallet_halt_refuses_leaves_the_position_held_and_the_long_whole()
-> Result<()> {
    use qip_capital_fabric::wallet::{Asset, ReconciliationOutcome, VenueAsset};
    use qip_contracts::venue::VenueId;
    let aaa = object("AAA");
    let mut platform = platform_over(&log_path("zero-target-halted"), start())?;
    // A statement the desk's own books cannot agree with, read by the first
    // cycle's LEARN after its ACT opened the long.
    platform.observe_statement(
        VenueId::new("simulated-venue"),
        aaa.as_str(),
        dec!("999"),
        dec!("1"),
        start(),
    )?;
    platform.observe(tape("AAA"));
    let report = platform.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    let (held, _) = platform
        .positions_at_cost()
        .get(aaa.as_str())
        .copied()
        .expect("premise: the first cycle opened a long in obj-AAA");
    assert!(held.is_positive(), "premise: a long: {held}");
    let key = VenueAsset {
        venue: VenueId::new("simulated-venue"),
        asset: Asset::new(aaa.as_str())?,
    };
    assert!(
        platform
            .fabric_state()
            .reconciliations()
            .get(&key)
            .is_some_and(ReconciliationOutcome::is_halt),
        "premise: the wallet reconciliation halted the position"
    );

    let mut price = 92.0_f64;
    let mut bars = Vec::new();
    for i in 0..10 {
        let open = price;
        price *= if i == 9 { 1.15 } else { 1.002 };
        bars.push(bar(
            "AAA",
            start().saturating_add(Duration::from_days(i + 1)),
            open,
            price,
        ));
    }
    platform.observe(bars);
    let later = start().saturating_add(Duration::from_days(11));
    let report = platform.run_cycle(later);
    assert!(report.traversed_every_stage(), "{}", report.summarise());

    assert!(
        platform
            .proposals()
            .iter()
            .flat_map(|proposal| proposal.legs.iter())
            .any(|leg| {
                leg.object_id == aaa
                    && leg.target_weight == 0.0
                    && matches!(leg.side, qip_portfolio_engine::proposal::Side::Sell)
            }),
        "premise: the second construction targeted obj-AAA at zero weight"
    );
    assert!(
        report
            .summarise()
            .contains("halted by the wallet reconciliation"),
        "premise: the wallet halt refused the divestment: {}",
        report.summarise()
    );
    assert_eq!(
        platform.position_lifecycle(aaa.as_str()),
        Some(PositionLifecycle::Held),
        "a divestment the wallet halt refused was still recorded as an unwind"
    );
    assert_eq!(
        platform
            .positions_at_cost()
            .get(aaa.as_str())
            .map(|(quantity, _)| *quantity),
        Some(held),
        "a refused divestment moved the long"
    );
    Ok(())
}

/// R12-ASSET-1015, the private-stages half: a capital call on a private
/// fund's commitment moves its lifecycle from committed to funding, and the
/// stage is read through the class's own lifecycle engine.
///
/// The failure prevented: the registry named a private-stages engine for
/// the class and nothing in the kernel ever moved a private stage, so a
/// commitment that had been called read exactly like one never drawn.
///
/// Mutation: stop `private_stage` adding the capital-call event — the stage
/// stays committed and the last assertion fails.
#[test]
fn a_capital_call_on_a_private_funds_commitment_moves_its_lifecycle_from_committed_to_funding()
-> Result<()> {
    use qip_financial::cashflow::CallConsequence;
    use qip_financial::private_stage::PrivateStage;
    use qip_kernel::platform::CapitalCallNotice;
    use qip_risk_engine::autonomy::OperatorIdentity;

    // Undrawn, so the stage the call moves it to is visible.
    let fund = FinancialObject::builder(
        object("FUND"),
        "FUND",
        InstrumentType::PrivateEquityFund,
        LiquidityProfile::illiquid(90.0, 250.0),
    )
    .venue("OTC")
    .geography("US")
    .price(dec!("100"))
    .extension(Extension::PrivateAsset(PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: dec!("400000"),
        called_capital: Decimal::ZERO,
        distributed_capital: Decimal::ZERO,
        residual_value: dec!("160000"),
        stage: "buyout".to_string(),
        lockup_years: 1.0,
        capital_call_notice_days: 10,
        call_schedule: None,
    }))
    .provenance(Provenance::synthetic("administrator", start()))
    .build(start())?;
    let fund_id = object("FUND");
    let mut universe = Universe::new();
    universe.insert(fund)?;
    let mut platform = in_memory(universe, config())?;
    assert_eq!(
        platform.private_stage(fund_id.as_str()),
        Some(PrivateStage::Committed),
        "premise: the fund's commitment is recorded and nothing is drawn"
    );
    assert_eq!(
        platform.private_stage("obj-NOT-A-COMMITMENT"),
        None,
        "an instrument with no commitment has no private stage"
    );

    let now = start().saturating_add(Duration::from_mins(5));
    platform.record_capital_call(
        fund_id.as_str(),
        CapitalCallNotice {
            reference: "call-1".to_string(),
            amount: dec!("50000"),
            due: now.saturating_add(Duration::from_days(10)),
            consequence: CallConsequence::Interest {
                annual_rate_bps: 3_650,
            },
        },
        &OperatorIdentity::verified("ops-carol", "oidc", now),
        now,
    )?;
    assert_eq!(
        platform.private_stage(fund_id.as_str()),
        Some(PrivateStage::Funding),
        "a capital call did not move the commitment's lifecycle"
    );
    Ok(())
}

/// R12-ASSET-4068: a declared hedge policy is the position's hedge only where
/// its hedge instrument is actually held, and the per-position record reaches
/// the log rather than only the stage line's counts.
///
/// The failure this guards: the hedge relationship was resolved from the
/// declarations alone, so a book that declared "hedge information technology
/// with obj-HEDGE" and never bought obj-HEDGE read as hedged; and LEARN
/// counted each record and dropped it, so the log could not say which
/// position was unhedged on a cycle, or why.
///
/// Mutations: pass `|_| true` as `held` in `Platform::position_obligations`
/// and obj-AAA reads as hedged by the obj-HEDGE it does not hold; delete the
/// `journal_record` of `PositionObligationsStated` in LEARN and the log holds
/// no record of the cycle's obligations.
#[test]
fn a_declared_hedge_whose_instrument_is_not_held_is_not_a_hedge_and_the_record_is_journalled()
-> Result<()> {
    let policies = vec![HedgePolicyDeclaration::new(
        "tech-beta",
        HedgeAxis::Sector,
        Sector::InformationTechnology.as_str(),
        object("HEDGE"),
        dec!("1"),
    )];
    let mut book = in_memory(universe()?, config().with_hedge_policies(policies))?;
    book.observe(tape("AAA"));
    let report = book.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    let held = book.positions_at_cost();
    assert!(held.contains_key("obj-AAA"), "premise: obj-AAA is held");
    assert!(
        !held.contains_key("obj-HEDGE"),
        "premise: the hedge leg is not"
    );

    let obligations = book.position_obligations()?;
    let aaa = obligations
        .get("obj-AAA")
        .expect("the held position states its obligations");
    let reason = none_reason(aaa.hedges(), "obj-AAA's hedge relationship");
    assert!(
        reason
            .contains("hedge instrument is not held: tech-beta (hedged with obj-HEDGE on sector "),
        "the unhedged position does not name the declared hedge it lacks: {reason}"
    );

    let mut journalled = Vec::new();
    for record in book.event_log().by_topic(Topic::RiskEvaluated) {
        if let Ok(envelope) = StreamEnvelope::from_frame(record)
            && let Ok(decoded) = envelope.decode::<PositionObligationsStated>()
        {
            journalled.push(decoded.body);
        }
    }
    assert_eq!(journalled.len(), 1, "one obligations record for one cycle");
    let logged = journalled[0]
        .positions
        .get("obj-AAA")
        .expect("the journalled record carries the held position");
    assert_eq!(
        logged.hedges(),
        aaa.hedges(),
        "the log states the same hedge relationship the platform serves"
    );
    Ok(())
}

/// R12-ASSET-4068: a position named as its own hedge, with nothing else
/// held in the bucket it hedges, states no hedge and says why.
///
/// tech-beta hedges the information-technology sector with obj-AAA, and
/// obj-AAA is the only position. It is covered by the policy and it is the
/// policy's hedge instrument, so both arms used to read as a hedge: "hedged
/// with obj-AAA" — a long offsetting itself — and "the hedge for
/// information technology" — over a bucket in which nothing else is held.
/// Neither offsets anything.
///
/// Mutations: in `hedge_review::policies_relating_to`, push a self-covering
/// policy to `held` instead of `own` — obj-AAA reads as hedged by itself;
/// set `exposed = true` without looking at the book — obj-AAA reads as the
/// hedge for a bucket nothing else is held in. Either fails `none_reason`.
#[test]
fn a_position_named_as_its_own_hedge_over_a_bucket_holding_nothing_else_states_no_hedge()
-> Result<()> {
    let policies = vec![HedgePolicyDeclaration::new(
        "tech-beta",
        HedgeAxis::Sector,
        Sector::InformationTechnology.as_str(),
        object("AAA"),
        dec!("1"),
    )];
    let mut book = in_memory(universe()?, config().with_hedge_policies(policies))?;
    book.observe(tape("AAA"));
    let report = book.run_cycle(start());
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    let held = book.positions_at_cost();
    assert_eq!(
        held.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["obj-AAA"],
        "premise: obj-AAA is the only position"
    );
    let obligations = book.position_obligations()?;
    let aaa = obligations
        .get("obj-AAA")
        .expect("the held position states its obligations");
    let reason = none_reason(aaa.hedges(), "obj-AAA's hedge relationship");
    assert!(
        reason.contains("with this position as its own hedge, which offsets nothing: tech-beta (hedged with obj-AAA on sector "),
        "{reason}"
    );
    assert!(
        reason.contains("no other held position lies in the bucket it hedges: tech-beta (this position is the hedge for sector "),
        "{reason}"
    );
    Ok(())
}
