//! The vertical slice, DECIDE to the booked fill: an order the ACT stage
//! releases, reached through the public API only.
//!
//! `vertical_slice.rs` (finding G6) could not get a tradable leg out of the
//! DECIDE stage on its synthetic tape, so its chain builds the order by hand
//! with `Platform::order_from` and `submit_order` — the function ACT calls,
//! but not ACT. This file closes that gap:
//!
//! ```text
//! observe(macro releases, a declared catalyst, a quiet tape with one sharp drop)
//!   -> run_cycle: DISCOVER (a catalyst-linked price move)
//!   -> REASON  (panel convened; the shipped review admits the hypothesis)
//!   -> DECIDE  (one long leg sized, a capital hold taken)
//!   -> ACT     (signed by risk-monitor and compliance, pre-trade risk, hold committed,
//!               order released to the simulated venue, filled)
//!   -> outcome row captured, fill booked into cash and position
//! ```
//!
//! # Why the earlier fixture released nothing
//!
//! It was a fixture that could not reach the review's bar, not missing wiring
//! and not a defect in the DECIDE stage. The panel's confidence on a bare
//! price drop plateaus near 0.36 (`ReviewPolicy::default()` requires 0.50), and
//! three things in the fixture were responsible, each a property of the
//! *data* and none of the policy:
//!
//! 1. **No catalyst.** With a macro calendar present and nothing explaining the
//!    drop the catalyst detector classifies it `UnexplainedMove`, which
//!    `mechanism_for` deliberately maps to no mechanism, so REASON forms no
//!    hypothesis at all (`a_move_nothing_explains_...` below pins that). With no
//!    event data at all the detector declines to say "unexplained" and the move
//!    is a plain `PriceMove`, which the panel scores near 0.36.
//! 2. **One supporter.** The equity analyst alone backs a buy; the simulation
//!    analyst dissents. The review's bar is only cleared when a second,
//!    independent, well-grounded origin agrees, here the macro analyst reading
//!    four published series for the instrument's economy.
//! 3. **No geography** on the instrument, so the macro analyst has no economy
//!    to read and answers `NoView`.
//!
//! Nothing was relaxed: `PlatformConfig::default()`'s review policy (0.50
//! floor), the action bar, the risk limits and the autonomy ceiling are the
//! shipped ones, and the negatives below prove each still bites. The margin is
//! thin (about 0.53 against 0.50) because the panel is conservative by design;
//! that is the finding, not a number to widen.
//!
//! # A production finding this fixture surfaced
//!
//! `SimulatedBroker::submit` fills `immediate_fill_fraction` (0.6) of an order
//! at a price that carries nine fractional digits, so a fill of 1028.4 shares
//! has a notional of ten, and the desk ledger refuses to post a cash leg it
//! would have to round. The fill happened at the venue and is in
//! `orders().fills()`, but it is booked nowhere: no `Filled` row, no position,
//! no cash movement. The refusal is loud and correct (`unledgered_fills`, a
//! LEARN problem), so the platform fails safe; the defect is that the
//! simulated venue emits a fill the platform's own ledger cannot represent.
//! `a_fill_the_ledger_cannot_represent_is_refused_loudly_and_moves_nothing`
//! pins the platform's side. The headline test uses an equity for which the
//! sized order is a multiple of five shares, so the 60% partial fill is a
//! whole number; that is a fixture choice made to get past the finding, and it
//! is said here so nobody reads the headline test as proof the finding does
//! not exist.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, OrderId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::intelligence::MacroObservation;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::cycle::{CycleReport, Stage};
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::{AutonomyLevel, Identification, OperatorIdentity};
use qip_twin::capture::Action;
use qip_world_model::vocabulary::MacroSeries;
use std::collections::BTreeMap;

// --- the fixture ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// The equity for which the sized order is 1715 shares, a multiple of five, so
/// that the simulated venue's 60% partial fill (1029 shares) is whole and its
/// notional is ledger-representable. See the module note: this is the fixture
/// stepping around a production finding, not a neutral choice.
const BOOK: i64 = 10_006_000;

/// The same book at which the order is 1714 shares, whose 60% fill (1028.4)
/// carries a notional the ledger refuses.
const BOOK_WITH_FRACTIONAL_FILL: i64 = 10_000_000;

/// A book too small to buy one share at the sized weight.
const BOOK_TOO_SMALL_FOR_A_LOT: i64 = 5_000;

/// The instrument is **listed**: an order for an id outside the universe is
/// refused, and the macro analyst reads the economy from `geography`.
fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                acme(),
                "ACME",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .geography("US")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("vertical-slice-decide", start()))
            .build(start())
            .expect("valid instrument"),
        )
        .expect("insertable");
    universe
}

/// The shipped-shaped limits of `vertical_slice.rs`, plus an optional cap on a
/// single order. A cap is only ever added: a stricter set is still policy.
fn limits(order_cap: Option<Decimal>) -> LimitSet {
    let mut set = LimitSet::new("vertical-slice-decide")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        );
    if let Some(cap) = order_cap {
        set = set.with(
            Limit::new(
                "max-order-notional",
                LimitKind::MaxOrderNotional { limit: cap },
            )
            .with_rationale("no single order may exceed the desk's per-order notional cap"),
        );
    }
    set
}

fn bar(at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: acme(),
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

const TAPE_BARS: usize = 400;

/// A quiet daily tape that drops twelve percent on its final bar. The noise is
/// a fixed irrational rotation so the scenario reproduces exactly. The drop is
/// last on purpose: a drop earlier in the tape leaves a volatility-shift
/// anomaly at the head of the queue, whose claim is about the option and not
/// the underlying (`valuation_seam.rs` records the same trap).
fn tape() -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE_BARS)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.003;
            let drop = if i == TAPE_BARS - 1 { -0.12 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE_BARS - i) as i64));
            bar(at, open, price)
        })
        .collect()
}

/// Forty prints of each of the four series the macro analyst reads, for the
/// instrument's economy, ending on a step that reads as supportive for risk
/// assets (policy rate, inflation and spreads down; growth up) by `steps`
/// noise-widths. Series are knowable on the day they refer to.
fn macro_calendar(steps: f64) -> Vec<SensedRecord> {
    const PRINTS: usize = 40;
    let mut records = Vec::new();
    // (series, level, noise amplitude, sign of a risk-supportive move)
    for (series, level, amplitude, supportive) in [
        (MacroSeries::PolicyRate, 4.0, 0.05, -1.0),
        (MacroSeries::InflationYoy, 3.0, 0.1, -1.0),
        (MacroSeries::GrowthYoy, 2.0, 0.2, 1.0),
        (MacroSeries::CreditSpreadBps, 120.0, 4.0, -1.0),
    ] {
        for i in 0..PRINTS {
            let noise = ((i as f64 * 0.7548776662 + 0.3) % 1.0 - 0.5) * 2.0 * amplitude;
            let step = if i == PRINTS - 1 {
                supportive * steps * amplitude
            } else {
                0.0
            };
            let at = start().saturating_sub(Duration::from_days((PRINTS - i) as i64 + 1));
            records.push(SensedRecord::Macro(Box::new(MacroObservation {
                series_id: series.series_id("US"),
                region: "US".to_string(),
                value: level + noise + step,
                unit: "pct".to_string(),
                reference_date: at,
                consensus: None,
                previous: None,
                is_revision: false,
                provenance: Provenance::synthetic("macro-fixture", at)
                    .with_licensing(LicensingClass::Synthetic),
                quality: DataQuality::default(),
                evidence_unretrievable: false,
            })));
        }
    }
    records
}

/// A dividend declaration three days before the drop: a knowable event that
/// precedes the move, which lets the catalyst detector link the two. Its
/// ex-date is a month out, so no adjustment of the tape is triggered.
fn declaration() -> Vec<SensedRecord> {
    vec![SensedRecord::CorporateAction(Box::new(CorporateAction {
        object_id: acme(),
        ex_date: start().saturating_add(Duration::from_days(30)),
        record_date: None,
        payment_date: None,
        kind: CorporateActionKind::CashDividend {
            amount: dec!("0.10"),
        },
        announced_at: start().saturating_sub(Duration::from_days(3)),
    }))]
}

/// What the scenario feeds the platform. Every field is data; none is policy.
#[derive(Clone)]
struct Spec {
    book: i64,
    catalyst: bool,
    macro_support: bool,
    order_cap: Option<Decimal>,
}

impl Spec {
    fn funded() -> Self {
        Self {
            book: BOOK,
            catalyst: true,
            macro_support: true,
            order_cap: None,
        }
    }
}

struct Run {
    platform: Platform,
    report: CycleReport,
    opening: Book,
}

/// Everything a refused or unreached order must not have moved.
#[derive(Debug, PartialEq)]
struct Book {
    cash: Decimal,
    equity: Decimal,
    costs: Decimal,
    positions: BTreeMap<String, (Decimal, Decimal)>,
    fills: usize,
    committed: Decimal,
}

fn book(platform: &Platform) -> Book {
    Book {
        cash: platform.cash(),
        equity: platform.equity(),
        costs: platform.trading_costs(),
        positions: platform.positions_at_cost(),
        fills: platform.orders().fills().len(),
        committed: platform.reservations().committed_total(),
    }
}

/// Assemble the shipped configuration (only the book size differs), feed it the
/// scenario through `observe`, let `prepare` act on the platform, and run one
/// whole cycle.
fn run(spec: &Spec, prepare: impl FnOnce(&mut Platform)) -> Result<Run> {
    let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(spec.book));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        limits(spec.order_cap),
    )?;
    if spec.macro_support {
        platform.observe(macro_calendar(6.0));
    }
    platform.observe(tape());
    if spec.catalyst {
        platform.observe(declaration());
    }
    prepare(&mut platform);
    let opening = book(&platform);
    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a stage did not run: {}",
        report.summarise()
    );
    Ok(Run {
        platform,
        report,
        opening,
    })
}

fn stage(run: &Run, which: Stage) -> &qip_kernel::cycle::StageOutcome {
    run.report.stage(which).expect("the stage ran")
}

/// The number the stage printed after `marker`: the platform's own account
/// rather than a recomputation beside it.
fn reported_number(detail: &str, marker: &str) -> f64 {
    let (_, rest) = detail
        .split_once(marker)
        .unwrap_or_else(|| panic!("no `{marker}` in: {detail}"));
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("`{marker}` was followed by `{digits}`: {error}"))
}

/// The premise every negative stands on: the same scenario, unperturbed,
/// really does fill. A refusal proves nothing about a platform that trades
/// nothing at all.
fn assert_control_fills() -> Result<()> {
    let control = run(&Spec::funded(), |_| {})?;
    assert_eq!(
        control.platform.orders().fills().len(),
        1,
        "premise: the unperturbed scenario must fill, or every refusal below is vacuous: {}",
        control.report.summarise()
    );
    Ok(())
}

fn refusal_gates(platform: &Platform) -> Vec<String> {
    platform
        .outcomes()
        .refusals()
        .iter()
        .filter_map(|row| match &row.decision.action {
            Action::Rejected { gate, .. } => Some(gate.clone()),
            _ => None,
        })
        .collect()
}

fn assert_nothing_moved(run: &Run) {
    assert_eq!(
        book(&run.platform),
        run.opening,
        "the book moved although the order was refused"
    );
    assert!(run.platform.orders().fills().is_empty());
    assert!(run.platform.positions_at_cost().is_empty());
    assert_eq!(stage(run, Stage::Act).produced, 0, "an order was released");
    run.platform
        .outcomes()
        .verify()
        .expect("the outcome chain still verifies");
}

// --- the headline: every stage, one eligible decision ----------------------------

#[test]
fn an_eligible_decision_is_sized_signed_checked_funded_released_filled_and_booked() -> Result<()> {
    // The policy the run stands on is the shipped one. If a later change loosens
    // the review floor this test must fail on its premise, not pass on a
    // fixture that no longer has to earn its approval.
    assert_eq!(
        PlatformConfig::default().review,
        ReviewPolicy::default(),
        "premise: the platform's default review is not the review policy's default"
    );
    assert!(
        (ReviewPolicy::default().minimum_surviving_confidence - 0.50).abs() < f64::EPSILON,
        "premise: the review floor is no longer 0.50"
    );

    let Run {
        platform,
        report,
        opening,
    } = run(&Spec::funded(), |_| {})?;
    let initial_equity = opening.equity;
    let run = |which: Stage| report.stage(which).expect("the stage ran");

    // REASON: the panel convened and the shipped review admitted the thesis.
    let reason = run(Stage::Reason);
    assert!(
        reason.detail.contains("hypothesis approved at confidence "),
        "premise: the review did not approve, so nothing below is reached: {} {:?}",
        reason.detail,
        reason.problems
    );
    let admitted = reported_number(&reason.detail, "hypothesis approved at confidence ");
    assert!(
        admitted >= 0.50,
        "approved at {admitted}, below the floor the review claims to enforce"
    );
    assert!(
        !reason
            .problems
            .iter()
            .any(|problem| problem.starts_with("rejected on review")),
        "{:?}",
        reason.problems
    );

    // DECIDE: one long leg, sized inside the position-weight limit, traceable
    // to the hypothesis, and signed by both controls.
    assert_eq!(
        run(Stage::Decide).produced,
        1,
        "{}",
        run(Stage::Decide).detail
    );
    let proposal = platform
        .proposals()
        .last()
        .expect("DECIDE records a proposal");
    assert_eq!(proposal.len(), 1);
    let leg = &proposal.legs[0];
    assert_eq!(leg.object_id, acme());
    assert!(leg.side == qip_portfolio_engine::proposal::Side::Buy);
    assert!(
        leg.target_weight > 0.0 && leg.target_weight <= 0.10,
        "sized at weight {} against a 0.10 limit",
        leg.target_weight
    );
    assert_eq!(leg.hypotheses.len(), 1);
    assert!(
        leg.hypotheses[0].starts_with("hyp-") && leg.hypotheses[0].ends_with("obj-ACME"),
        "the leg does not name the hypothesis REASON approved: {:?}",
        leg.hypotheses
    );
    assert_eq!(
        proposal.checks_passed,
        vec!["risk-monitor".to_string(), "compliance".to_string()],
        "the proposal was not signed by exactly the two controls"
    );
    assert!(
        proposal.status().as_str() == "released",
        "ACT released the proposal: {}",
        proposal.status().as_str()
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(snapshot.counter_total(names::PROPOSALS_SIGNED), 1);

    // RISK: the limits were evaluated against the tracked book and none fired.
    assert!(
        snapshot.counter_total(names::RISK_EVALUATIONS) >= 1,
        "no risk evaluation was recorded"
    );
    assert_eq!(
        snapshot.gauge(names::LIMIT_BREACHES, &labels([])),
        Some(0.0),
        "a limit was breached on the way to a fill"
    );

    // CAPITAL: DECIDE held the proposal's notional and ACT committed it.
    assert_eq!(platform.reservations().reserved_total(), Decimal::ZERO);
    assert!(
        platform.reservations().committed_total().is_positive(),
        "ACT released an order and committed no capital"
    );

    // ACT: the order was released by the stage, not built by hand, and traded
    // on the simulator.
    let act = run(Stage::Act);
    assert!(
        act.detail
            .starts_with("1 order(s) released from 1 approved proposal(s), 0 refused"),
        "{}",
        act.detail
    );
    assert_eq!(act.produced, 1);
    assert_eq!(snapshot.counter_total(names::ORDERS_SUBMITTED), 1);
    let fills = platform.orders().fills();
    assert_eq!(fills.len(), 1, "one released order, one fill");
    let fill = &fills[0];
    assert!(fill.simulated, "the fill is a paper fill");
    assert_eq!(fill.venue, "simulated-venue");
    assert!(!platform.orders().has_live_fills());
    assert!(!platform.is_live_capable());
    assert_eq!(platform.autonomy().level(), AutonomyLevel::PaperTrading);

    // The outcome row: the order placed and the fill, on a chain that verifies.
    let outcomes = platform.outcomes();
    outcomes.verify()?;
    let taken = outcomes.taken();
    let placed: Vec<&OrderId> = taken
        .iter()
        .filter_map(|row| match &row.decision.action {
            Action::OrderPlaced { order_id, .. } => Some(order_id),
            _ => None,
        })
        .collect();
    assert_eq!(placed, vec![&fill.order_id], "exactly the released order");
    assert!(
        taken.iter().any(|row| matches!(&row.decision.action,
            Action::Filled { order_id, quantity, price, .. }
                if *order_id == fill.order_id && *quantity == fill.quantity && *price == fill.price)),
        "no Filled row carrying the venue's quantity and price: {:?}",
        outcomes.tally()
    );

    // The fill is booked, to the unit: position, cash, costs and equity.
    assert!(
        platform.unledgered_fills().is_empty(),
        "{:?}",
        platform.unledgered_fills()
    );
    let (held, average) = platform
        .positions_at_cost()
        .get("obj-ACME")
        .copied()
        .expect("the fill opened a position");
    assert_eq!(held, fill.quantity);
    assert_eq!(average, fill.price);
    assert_eq!(
        platform.cash(),
        initial_equity - fill.quantity * fill.price - fill.costs
    );
    assert_eq!(platform.trading_costs(), fill.costs);
    assert_eq!(platform.equity(), initial_equity - fill.costs);
    assert!(
        run(Stage::Learn).problems.is_empty(),
        "LEARN reported a booking problem: {:?}",
        run(Stage::Learn).problems
    );
    Ok(())
}

// --- the same scenario family, refused ---------------------------------------------

#[test]
fn a_hypothesis_below_the_reviews_bar_releases_nothing_and_says_why() -> Result<()> {
    assert_control_fills()?;
    // Same tape and catalyst, but the macro analyst has no series to read: the
    // one origin backing the buy cannot carry the review's 0.50.
    let run = run(
        &Spec {
            macro_support: false,
            ..Spec::funded()
        },
        |_| {},
    )?;
    let reason = stage(&run, Stage::Reason);
    assert!(
        reason.detail.contains("hypothesis rejected at confidence "),
        "premise: a hypothesis was formed and reviewed: {}",
        reason.detail
    );
    let rejection = reason
        .problems
        .iter()
        .find(|problem| problem.starts_with("rejected on review: confidence fell from "))
        .unwrap_or_else(|| panic!("REASON gave no reason: {:?}", reason.problems));
    assert!(
        rejection.ends_with(", below the 0.500 required"),
        "{rejection}"
    );
    let after = reported_number(rejection, " to ");
    assert!(after < 0.50, "rejected at {after}, which is not below 0.50");
    let decide = stage(&run, Stage::Decide);
    assert!(
        decide
            .detail
            .starts_with("no thesis cleared the action bar; nothing to propose"),
        "{}",
        decide.detail
    );
    assert_eq!(decide.produced, 0);
    assert!(
        run.platform
            .proposals()
            .last()
            .expect("recorded")
            .is_empty()
    );
    assert!(
        stage(&run, Stage::Act)
            .detail
            .starts_with("no approved proposal to release")
    );
    assert_nothing_moved(&run);
    Ok(())
}

#[test]
fn a_move_nothing_explains_forms_no_hypothesis_so_nothing_is_sized() -> Result<()> {
    assert_control_fills()?;
    // With a macro calendar present and no event explaining the drop, the
    // catalyst detector calls it unexplained. That is information leaking or a
    // data error, not a mechanism, and REASON declines to invent one. This is
    // the first of the three reasons the earlier fixture released nothing.
    let run = run(
        &Spec {
            catalyst: false,
            ..Spec::funded()
        },
        |_| {},
    )?;
    let reason = stage(&run, Stage::Reason);
    assert!(
        reason
            .detail
            .ends_with("but the anomaly implies no mechanism, so no hypothesis was formed"),
        "{}",
        reason.detail
    );
    assert!(
        run.platform
            .proposals()
            .last()
            .expect("recorded")
            .is_empty(),
        "a leg was sized on a move nothing explains"
    );
    assert_nothing_moved(&run);
    Ok(())
}

#[test]
fn an_order_over_the_notional_cap_is_refused_and_recorded() -> Result<()> {
    assert_control_fills()?;
    let uncapped = run(&Spec::funded(), |_| {})?;
    let fill = &uncapped.platform.orders().fills()[0];
    // A cap below what the fill traded: 60% of the sized order was filled, so
    // the order's notional is at least the fill's.
    let cap = (fill.quantity * fill.price) / dec!("2");
    assert!(cap.is_positive());

    let run = run(
        &Spec {
            order_cap: Some(cap),
            ..Spec::funded()
        },
        |_| {},
    )?;
    // The thesis is still sized and signed: the cap is a pre-trade control on
    // the order, not a reason the platform declines to think.
    assert_eq!(stage(&run, Stage::Decide).produced, 1);
    assert_nothing_moved(&run);
    assert!(
        refusal_gates(&run.platform)
            .iter()
            .any(|gate| gate == "pre-trade-risk"),
        "the refusal left no pre-trade-risk row: {:?}",
        run.platform.outcomes().tally()
    );
    assert!(
        stage(&run, Stage::Act)
            .problems
            .iter()
            .chain(stage(&run, Stage::Learn).problems.iter())
            .any(|problem| problem.contains("max-order-notional")),
        "no stage names the limit that fired: act {:?} learn {:?}",
        stage(&run, Stage::Act).problems,
        stage(&run, Stage::Learn).problems
    );
    // The hold taken for the refused proposal went back to free.
    assert_eq!(run.platform.reservations().reserved_total(), Decimal::ZERO);
    assert_eq!(run.platform.reservations().committed_total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn insufficient_capital_releases_nothing() -> Result<()> {
    assert_control_fills()?;
    let run = run(
        &Spec {
            book: BOOK_TOO_SMALL_FOR_A_LOT,
            ..Spec::funded()
        },
        |_| {},
    )?;
    // Premise: DECIDE sized a leg and held capital for it, so what refuses is
    // ACT meeting a book that cannot pay for one lot, not a thesis never made.
    assert_eq!(stage(&run, Stage::Decide).produced, 1);
    let act = stage(&run, Stage::Act);
    assert!(
        act.detail
            .starts_with("0 order(s) released from 1 approved proposal(s), 1 refused"),
        "{}",
        act.detail
    );
    assert!(
        act.problems
            .iter()
            .any(|problem| problem.ends_with("is less than one lot; nothing to release")),
        "{:?}",
        act.problems
    );
    assert_nothing_moved(&run);
    assert_eq!(run.platform.reservations().reserved_total(), Decimal::ZERO);
    assert_eq!(run.platform.reservations().committed_total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn a_tripped_kill_switch_halts_the_cycle_before_anything_trades() -> Result<()> {
    assert_control_fills()?;
    let run = run(&Spec::funded(), |platform| {
        platform.autonomy_mut().kill_switch_mut().trip_global(
            start(),
            "vertical-slice-decide",
            "an operator halted the platform",
        );
    })?;
    assert!(
        run.platform.autonomy().kill_switch().is_globally_tripped(),
        "premise: the switch is tripped"
    );
    assert_nothing_moved(&run);
    assert_eq!(run.platform.reservations().reserved_total(), Decimal::ZERO);
    assert_eq!(
        refusal_gates(&run.platform),
        vec!["kill-switch".to_string()]
    );
    assert!(
        stage(&run, Stage::Act)
            .problems
            .iter()
            .any(|problem| problem.contains("trading is halted for platform")),
        "{:?}",
        stage(&run, Stage::Act).problems
    );
    Ok(())
}

#[test]
fn an_autonomy_ceiling_below_execution_refuses_the_order_and_a_live_one_cannot_be_asked_for()
-> Result<()> {
    assert_control_fills()?;
    let run = run(&Spec::funded(), |platform| {
        let operator = OperatorIdentity::verified("desk-operator", "hardware-token", start());
        platform
            .autonomy_mut()
            .request_change(
                AutonomyLevel::Advisory,
                &operator,
                "stepping down to advisory for the review",
                Some(Identification::Identified),
                start(),
            )
            .expect("stepping down is permitted");
    })?;
    assert_eq!(
        run.platform.autonomy().level(),
        AutonomyLevel::Advisory,
        "premise"
    );
    assert_nothing_moved(&run);
    assert_eq!(refusal_gates(&run.platform), vec!["autonomy".to_string()]);
    assert!(
        stage(&run, Stage::Act)
            .problems
            .iter()
            .any(|problem| problem.ends_with(
                "the autonomy level is advisory, and execution needs at least paper_trading"
            )),
        "{:?}",
        stage(&run, Stage::Act).problems
    );
    assert_eq!(run.platform.reservations().reserved_total(), Decimal::ZERO);

    // And the other direction is closed: no operator, however approved, can
    // raise the ceiling past paper trading (layer two of the boundary).
    let mut platform = run.platform;
    let two = OperatorIdentity::verified("desk-operator", "hardware-token", start())
        .with_second_approver("desk-supervisor");
    let error = platform
        .autonomy_mut()
        .request_change(
            AutonomyLevel::AutonomousLive,
            &two,
            "attempting to enable fully autonomous live trading",
            Some(Identification::Identified),
            start(),
        )
        .unwrap_err();
    assert!(error.message().contains("ceiling"), "{}", error.message());
    assert!(!platform.is_live_capable());
    Ok(())
}

#[test]
fn a_fractional_fill_from_the_default_simulator_is_booked_exactly_and_queued_for_the_ledger()
-> Result<()> {
    // The production finding in the module note, fixed at its source. The
    // simulated venue filled 60% of a whole-share order (1028.4 shares) at a
    // nine-decimal price, a ten-digit notional the ledger will not round, so
    // the platform declined the fill and the book stopped following the venue.
    // The venue now rounds its price, against the taker, to the digits the
    // notional can carry, so the fill is booked and the ledger can post it.
    // This was this test's opposite until the venue was fixed.
    let run = run(
        &Spec {
            book: BOOK_WITH_FRACTIONAL_FILL,
            ..Spec::funded()
        },
        |_| {},
    )?;
    let fills = run.platform.orders().fills();
    assert_eq!(fills.len(), 1, "premise: the venue did fill");
    let fill = &fills[0];
    assert!(
        fill.quantity.to_string().contains('.'),
        "premise: the fill is fractional: {}",
        fill.quantity
    );
    // The notional is exact at the ledger's nine digits.
    assert_eq!(
        (fill.quantity.raw() * fill.price.raw()) % qip_core::decimal::SCALE,
        0,
        "the venue emitted a notional the ledger cannot hold: {} x {}",
        fill.quantity,
        fill.price
    );
    // Booked: the position, the cash and the Filled row follow the venue.
    let positions = run.platform.positions_at_cost();
    assert_eq!(positions.len(), 1, "{positions:?}");
    let (held, at_cost) = positions.values().next().copied().expect("one position");
    assert_eq!(held, fill.quantity);
    assert_eq!(at_cost, fill.price);
    assert_eq!(
        run.platform.cash(),
        run.opening.cash - fill.quantity * fill.price - fill.costs
    );
    assert!(
        run.platform
            .outcomes()
            .taken()
            .iter()
            .any(|row| matches!(row.decision.action, Action::Filled { .. })),
        "no Filled row exists for a fill the platform booked"
    );
    // And queued for the ledger, with nothing declined.
    assert!(
        run.platform.unledgered_fills().is_empty(),
        "{:?}",
        run.platform.unledgered_fills()
    );
    assert_eq!(run.platform.pending_desk_outcomes().len(), 1);
    assert!(
        !stage(&run, Stage::Learn)
            .problems
            .iter()
            .any(|problem| problem.contains("not booked")),
        "{:?}",
        stage(&run, Stage::Learn).problems
    );
    assert!(fills.iter().all(|fill| fill.simulated));
    Ok(())
}

/// R12-CAPITAL-4092 (ADR 0148 decision 5). The held move of a released,
/// partly filled order splits into the thesis, the sizing, the allocation and
/// the unfilled remainder, and the four add back to the fill's own held move
/// to the last digit.
///
/// The failure it guards: before the three quantities were stamped, "how much
/// of this came from the prediction, how much from the sizing" could only be
/// answered by choosing the numbers. Here every quantity is read off the stage
/// that produced it, through the production cycle (thesis, constructor,
/// governor, lot rounding, simulated venue, LEARN), and the premise asserts
/// they are three different numbers and the fill is partial so no term is
/// vacuous.
///
/// Mutations that must fail it: stamping the reference quantity as the sized
/// one collapses the prediction and sizing terms and fails the premise;
/// computing the sizing term as `(sized - released) * move` fails its
/// definition.
#[test]
fn the_four_terms_of_a_fills_held_move_sum_exactly_to_the_filled_quantity_times_the_move()
-> Result<()> {
    let Run { mut platform, .. } = run(&Spec::funded(), |_| {})?;
    let leg = platform
        .proposals()
        .last()
        .expect("DECIDE records a proposal")
        .legs[0]
        .clone();
    platform.observe(vec![bar(
        start().saturating_add(Duration::from_days(1)),
        110.0,
        110.0,
    )]);
    platform.run_cycle(start().saturating_add(Duration::from_days(5)));

    let fill = platform
        .orders()
        .fills()
        .into_iter()
        .next()
        .cloned()
        .expect("premise: the released order filled");
    let order_id = fill.order_id.as_str().to_string();
    let quantities = platform
        .release_quantities(&order_id)
        .expect("premise: the order came from a constructed leg and was stamped at release");

    // The stamped quantities are the stages' own figures, not recomputed.
    assert_eq!(quantities.sized, leg.quantity);
    assert_eq!(Some(quantities.reference), leg.reference_quantity);
    // Premise: three different numbers and a partial fill, so no term below is
    // a zero that proves nothing.
    assert!(
        quantities.reference > quantities.sized
            && quantities.sized > quantities.released
            && quantities.released > fill.quantity
            && fill.quantity.is_positive(),
        "premise: reference {} sized {} released {} filled {} are not strictly ordered",
        quantities.reference,
        quantities.sized,
        quantities.released,
        fill.quantity
    );

    let split = platform
        .last_attribution()
        .and_then(|pass| pass.held_moves.get(&order_id))
        .expect("the stamped order's held move was split");
    let held_move = dec!("110") - leg.reference_price;
    assert!(
        held_move.is_positive(),
        "premise: the tape moved in the buy's favour"
    );
    // The fill's own held move, computed here and not read from the split.
    assert_eq!(split.total, fill.quantity * held_move);
    assert_eq!(
        split.sum(),
        split.total,
        "the four terms do not add to the fill's held move"
    );
    assert_eq!(split.prediction, quantities.reference * held_move);
    assert_eq!(
        split.sizing,
        (quantities.sized - quantities.reference) * held_move,
        "the sizing term is not what narrowing took"
    );
    assert_eq!(
        split.allocation,
        (quantities.released - quantities.sized) * held_move
    );
    // The unfilled term balances the others; it must still be what it says,
    // to the rounding `Decimal` applies to each of four products.
    let stated = -((quantities.released - fill.quantity) * held_move);
    assert!(
        (split.unfilled - stated).abs() <= Decimal::from_raw(4),
        "unfilled {} is not the unfilled share of the move {stated}",
        split.unfilled
    );
    // And with the cost of getting the fill, the five components are the
    // order's whole outcome — which the per-fill attribution measured on its
    // own, from the fill price and the commission, without any of the above.
    let position = platform
        .last_attribution()
        .and_then(|pass| {
            pass.attribution
                .positions
                .iter()
                .find(|position| position.object_id == order_id)
        })
        .expect("the fill is in the per-fill attribution");
    assert!(
        split.execution.is_negative(),
        "premise: getting the fill cost something: {}",
        split.execution
    );
    assert!(
        (split.outcome() - position.total).abs() <= Decimal::from_raw(4),
        "the five components {} are not the order's outcome {}",
        split.outcome(),
        position.total
    );
    assert!(
        split.prediction.is_positive()
            && split.sizing.is_negative()
            && split.unfilled.is_negative(),
        "premise: each term has its own sign, so a swapped pair is visible: {split:?}"
    );
    Ok(())
}

#[test]
fn an_order_submitted_without_a_leg_is_stamped_with_no_quantities_rather_than_zeros() -> Result<()>
{
    let Run { mut platform, .. } = run(&Spec::funded(), |_| {})?;
    let order = platform.order_from(
        acme(),
        qip_execution_engine::order::Side::Buy,
        dec!("5"),
        dec!("100"),
        "prop-direct",
        vec!["hyp-direct".to_string()],
        start(),
    );
    let order_id = order.order_id.as_str().to_string();
    platform.submit_order(order, start())?;
    assert!(
        platform.release_quantities(&order_id).is_none(),
        "a direct order has no proposal to be narrowed from; a stamped zero would read as a measurement"
    );
    Ok(())
}
