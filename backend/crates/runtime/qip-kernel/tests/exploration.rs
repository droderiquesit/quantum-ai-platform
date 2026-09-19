//! The exploration budget where the cycle runs it (blueprint §13.2).
//!
//! Four properties, each driven through the thing that should reach it rather
//! than asserted about the module in isolation:
//!
//! * a deployment that sets no share aside still *says* so, every pass, in
//!   the cycle report and on a gauge — the idle state is the state a
//!   deployment is normally in, and a budget that reached a surface only when
//!   it was positive would be a control nobody could see was working;
//! * a deployment that does set a share aside has that capital **withheld
//!   from the book the construction is sized against**, which is the
//!   difference between a line item and a label;
//! * the desk's configured share is genuinely the ceiling user mandates are
//!   admitted under — it admits a share under it and refuses one above it,
//!   and both halves matter, because a gate that refuses everything looks
//!   identical to a working one from the refusing side;
//! * a probe nobody takes up settles as *observation* and never as evidence
//!   that probing works.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::exploration::ProbeKind;
use qip_capital::ledger::{
    Jurisdiction, Mandate, MandateId, MandateTerms, PermittedFamilies, UserId, UserLedger,
};
use qip_capital::reservation::ReservationLedger;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::{PlatformConfig, UserMandate};
use qip_kernel::cycle::Stage;
use qip_kernel::exploration::{ExplorationDesk, HOLD_ID, PROBE_VALIDITY, review};
use qip_kernel::platform::Platform;
use qip_learning_engine::self_model::{ComponentKey, ComponentKind, ScoredOutcome, SelfModel};
use qip_observability::Telemetry;
use qip_observability::metrics::labels;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::collections::BTreeMap;

const INSTRUMENT: &str = "EXPLORE-1";

/// The wire names, as a dashboard query spells them. Literals rather than the
/// `names` constants, so renaming a constant without renaming the series
/// fails here rather than silently retiring a chart.
const BUDGET_SERIES: &str = "qip_exploration_budget";
const SETTLED_SERIES: &str = "qip_exploration_probes_settled_total";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn universe() -> Result<Universe> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string(INSTRUMENT),
            "AAA",
            InstrumentType::CommonStock,
            fixture_liquidity(),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn limits() -> LimitSet {
    LimitSet::new("exploration-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe()?, limits())
}

/// A user mandate under the desk's, with a stated exploration share.
fn enrolment(user: &str, capital: Decimal, exploration_share: Decimal) -> Result<UserMandate> {
    Ok(UserMandate {
        user: UserId::new(user)?,
        id: MandateId::new(format!("mandate-{user}"))?,
        mandate: Mandate::new(MandateTerms {
            capital,
            currency: Currency::USD,
            risk_tolerance: Decimal::ONE,
            permitted_families: PermittedFamilies::Any,
            liquidity_floor: Decimal::ZERO,
            exploration_share,
            jurisdiction: Jurisdiction::new("GB")?,
        })?,
    })
}

fn gauge(platform: &Platform, name: &str) -> Option<f64> {
    platform
        .telemetry()
        .metrics
        .snapshot()
        .gauge(name, &labels([]))
}

#[test]
fn a_platform_that_sets_no_share_aside_still_reports_the_budget_it_did_not_take() -> Result<()> {
    // The failure this closes is the silent-when-idle one: exploration is
    // zero on every deployment that has not configured it, and a report that
    // said nothing there would leave "no share is configured" and "the budget
    // is not being computed at all" looking identical for the entire life of
    // the process.
    let mut platform = platform(PlatformConfig::default())?;
    let report = platform.run_cycle(start().saturating_add(Duration::from_secs(60)));
    let decide = report
        .stage(Stage::Decide)
        .expect("the premise is a cycle whose DECIDE ran");
    assert!(
        decide.ran,
        "DECIDE did not run, so its detail proves nothing: {decide:?}"
    );
    assert!(
        decide.detail.contains("exploration: no budget"),
        "the DECIDE detail does not say the platform explored with nothing: {}",
        decide.detail
    );
    // And on a series, not only in prose: zero written as zero, so a scrape
    // can tell an unconfigured budget from an unrecorded one.
    assert_eq!(
        gauge(&platform, BUDGET_SERIES),
        Some(0.0),
        "the exploration budget gauge was not written on a pass that set nothing aside"
    );
    Ok(())
}

#[test]
fn an_exploration_share_is_withheld_from_the_capital_the_book_is_sized_against() -> Result<()> {
    // The property that makes this a line item rather than a label: §13.2's
    // share is held out of the same free balance `deployable_capital` sizes
    // the book from. A budget reported beside sizing without being subtracted
    // from it is a side effect wearing a line item's name.
    let cycle_at = start().saturating_add(Duration::from_secs(60));

    // The premise, and the control: the same platform with no share set aside
    // holds nothing and deploys the whole book.
    let mut unexploring = platform(PlatformConfig::default())?;
    unexploring.run_cycle(cycle_at);
    assert!(
        unexploring.reservations().reservation(HOLD_ID).is_none(),
        "a platform with no exploration share took a hold anyway"
    );
    let whole_book = unexploring.deployable_capital(cycle_at)?;
    assert_eq!(
        whole_book,
        dec!("10000000"),
        "the premise is a book of ten million with nothing held against it"
    );

    // And with five percent set aside, five hundred thousand of that book is
    // held for information rather than return.
    let mut exploring = platform(PlatformConfig::default().with_exploration_share(dec!("0.05")))?;
    exploring.run_cycle(cycle_at);
    let hold = exploring
        .reservations()
        .reservation(HOLD_ID)
        .expect("the exploration budget is held against the book");
    assert_eq!(hold.amount, dec!("500000"));
    assert_eq!(
        exploring.deployable_capital(cycle_at)?,
        whole_book - dec!("500000"),
        "the exploration budget was not subtracted from the capital the book is sized against"
    );
    assert_eq!(gauge(&exploring, BUDGET_SERIES), Some(500_000.0));
    Ok(())
}

#[test]
fn the_configured_desk_share_is_the_ceiling_user_mandates_are_admitted_under() -> Result<()> {
    // Both halves, because a ceiling that refuses everything reads exactly
    // like a working one from the refusing side. The registry admits a user
    // share the desk's covers and refuses one above it — and before the desk
    // ceiling was configurable it refused *every* nonzero share, because
    // `Mandate::desk` pinned the desk at zero and nothing could raise it.
    let admitted = PlatformConfig::default()
        .with_exploration_share(dec!("0.05"))
        .with_user_mandates(vec![enrolment("alice", dec!("100000"), dec!("0.02"))?]);
    let assembled = platform(admitted)?;
    assert_eq!(
        assembled
            .user_ledger()
            .mandate(&UserId::new("alice")?)
            .map(Mandate::exploration_share),
        Some(dec!("0.02")),
        "the enrolled mandate did not keep the share it was admitted with"
    );

    let refused = PlatformConfig::default()
        .with_exploration_share(dec!("0.01"))
        .with_user_mandates(vec![enrolment("bob", dec!("100000"), dec!("0.02"))?]);
    let error = platform(refused).expect_err("a share above the desk's ceiling is refused");
    assert!(
        error.message().contains("exploration"),
        "the refusal does not name the term that exceeded the ceiling: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_probe_nobody_takes_up_settles_as_observation_and_never_as_evidence_that_probing_works()
-> Result<()> {
    // The failure this closes: with no exploration execution path, every
    // settlement the platform makes is the subject being re-measured while
    // the probe sat unexercised. A book that fed those into the selection
    // rule's measured reward would report that probing works on evidence that
    // no probing happened, and would then keep choosing whichever kind the
    // market happened to clarify on its own.
    let telemetry = Telemetry::silent();
    let ledger = UserLedger::with_desk_exploring(
        UserId::new("desk")?,
        dec!("1000000"),
        Currency::USD,
        dec!("0.02"),
    )?;
    let mut reservations = ReservationLedger::new(dec!("1000000"))?;
    let mut desk = ExplorationDesk::new();

    // Three components the platform knows too little about to estimate: the
    // stale-estimate arm, which is the one a young deployment actually has.
    let mut self_model = SelfModel::new();
    for id in ["gap", "drift", "spread"] {
        self_model.record(
            ComponentKey::new(ComponentKind::Detector, id)?,
            ScoredOutcome::new("hypothesis-1", 0.5, true, start())?,
        );
    }
    assert_eq!(
        self_model.len(),
        3,
        "the premise is three components with a record too thin to estimate"
    );

    let opened = review(
        &mut desk,
        &mut reservations,
        &telemetry.metrics,
        &ledger,
        &self_model,
        &[],
        &BTreeMap::new(),
        dec!("1000000"),
        start(),
    );
    assert!(
        desk.book().open_count() > 0,
        "nothing was probed, so the settlement below would prove nothing: {opened}"
    );
    assert_eq!(desk.held(), dec!("20000"), "the budget was not held");

    // A day later the questions close. Nothing took them up, so each is
    // settled against the subject as it now measures — which is observation.
    let later = start().saturating_add(PROBE_VALIDITY);
    let settled = review(
        &mut desk,
        &mut reservations,
        &telemetry.metrics,
        &ledger,
        &self_model,
        &[],
        &BTreeMap::new(),
        dec!("1000000"),
        later,
    );
    assert!(
        desk.book().settled_total() > 0,
        "no probe closed at its expiry: {settled}"
    );
    let record = desk
        .book()
        .record(ProbeKind::StaleEstimate)
        .expect("the stale-estimate kind has a record");
    assert!(record.observed > 0, "nothing settled as observation");
    assert_eq!(
        record.probed, 0,
        "a probe nothing took up was recorded as having been taken up"
    );
    assert_eq!(
        record.measured_gain(),
        None,
        "observation reached the measured reward the selection rule reads"
    );
    // Spending is billed from what ran: nothing ran, so nothing is billed —
    // not the bound, which is the number closest to hand.
    assert_eq!(desk.book().spend(), Decimal::ZERO);
    // And the settlement is on a series, by the evidence it rests on.
    assert_eq!(
        telemetry
            .metrics
            .snapshot()
            .get(SETTLED_SERIES, &labels([("evidence", "observed")]))
            .and_then(|value| match value {
                qip_observability::metrics::MetricValue::Counter(count) => Some(*count),
                _ => None,
            }),
        Some(record.observed),
        "the observed settlements were not counted on the series"
    );
    Ok(())
}
