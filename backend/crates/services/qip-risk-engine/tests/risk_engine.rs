//! Tests for the control function.
//!
//! Nearly every test here tries to get the platform to do something it must
//! not: trade above its autonomy level, escalate itself, pass an order that
//! breaches a limit, or restart after a halt without an operator.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel, KillSwitch, OperatorIdentity};
use qip_risk_engine::monitor::{MonitorAction, MonitorPolicy, RiskMonitor};
use qip_risk_engine::pretrade::{PreTradeChecker, PreTradeDecision, ProposedOrder};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn operator() -> OperatorIdentity {
    OperatorIdentity::verified("alice@example.com", "hardware-token", now())
}

fn two_operators() -> OperatorIdentity {
    operator().with_second_approver("bob@example.com")
}

// --- autonomy levels --------------------------------------------------------

#[test]
fn the_platform_starts_in_paper_trading() {
    // The single most important default in the system.
    let controller = AutonomyController::new();
    assert_eq!(controller.level(), AutonomyLevel::PaperTrading);
    assert_eq!(AutonomyLevel::DEFAULT, AutonomyLevel::PaperTrading);
    assert!(!controller.is_live());
    assert!(controller.level().executes());
}

#[test]
fn a_default_deployment_cannot_reach_a_live_level_at_all() {
    // The ceiling starts at paper trading, so a deployment that was never
    // configured for live trading cannot get there even with two operators.
    let mut controller = AutonomyController::new();
    assert_eq!(controller.ceiling(), AutonomyLevel::PaperTrading);

    for level in [
        AutonomyLevel::SupervisedLive,
        AutonomyLevel::LimitedAutonomousLive,
        AutonomyLevel::AutonomousLive,
    ] {
        let error = controller
            .request_change(level, &two_operators(), "enabling live trading", now())
            .unwrap_err();
        assert!(
            error.message().contains("ceiling"),
            "reaching {level} was refused for the wrong reason: {}",
            error.message()
        );
        assert!(!controller.is_live());
    }
}

#[test]
fn one_operator_cannot_enable_live_trading_alone() {
    // A single compromised session must not be enough.
    let mut controller = AutonomyController::with_live_ceiling(AutonomyLevel::AutonomousLive);
    let error = controller
        .request_change(
            AutonomyLevel::SupervisedLive,
            &operator(),
            "enabling supervised live trading for the pilot",
            now(),
        )
        .unwrap_err();
    assert!(
        error.message().contains("second approver"),
        "{}",
        error.message()
    );
    assert!(!controller.is_live());
}

#[test]
fn a_second_approver_who_is_the_same_person_is_not_a_second_approver() {
    let identity = OperatorIdentity::verified("alice@example.com", "oidc", now())
        .with_second_approver("alice@example.com");
    assert!(!identity.has_second_approver());
}

#[test]
fn two_operators_can_enable_live_trading_where_the_ceiling_permits_it() {
    let mut controller = AutonomyController::with_live_ceiling(AutonomyLevel::SupervisedLive);
    controller
        .request_change(
            AutonomyLevel::SupervisedLive,
            &two_operators(),
            "enabling supervised live trading for the pilot",
            now(),
        )
        .unwrap();
    assert!(controller.is_live());
    assert!(controller.level().requires_human_release());

    let change = controller.history().last().unwrap();
    assert_eq!(change.operator, "alice@example.com");
    assert_eq!(change.second_approver.as_deref(), Some("bob@example.com"));
    assert!(!change.reason.is_empty());
}

#[test]
fn a_stale_credential_cannot_authorise_a_change() {
    // A session token from an hour ago is not evidence anyone is at the
    // keyboard now.
    let mut controller = AutonomyController::with_live_ceiling(AutonomyLevel::AutonomousLive);
    let stale = two_operators();
    let much_later = now().saturating_add(Duration::from_hours(1));
    let error = controller
        .request_change(
            AutonomyLevel::SupervisedLive,
            &stale,
            "enabling live trading after lunch",
            much_later,
        )
        .unwrap_err();
    assert!(
        error.message().contains("re-authenticate"),
        "{}",
        error.message()
    );
}

#[test]
fn a_change_without_a_stated_reason_is_refused() {
    let mut controller = AutonomyController::new();
    assert!(
        controller
            .request_change(AutonomyLevel::Advisory, &operator(), "why", now())
            .is_err(),
        "the audit trail is the point"
    );
}

#[test]
fn reducing_autonomy_needs_no_operator() {
    // Reducing is always safe, and requiring authority to stop would be a way
    // for the platform to keep trading when it should not.
    let mut controller = AutonomyController::new();
    controller.reduce_to(
        AutonomyLevel::Observation,
        "data feed quality degraded",
        now(),
    );
    assert_eq!(controller.level(), AutonomyLevel::Observation);
    assert!(!controller.level().executes());
}

#[test]
fn reduce_to_never_raises() {
    let mut controller = AutonomyController::new();
    controller.reduce_to(AutonomyLevel::AutonomousLive, "trying it on", now());
    assert_eq!(controller.level(), AutonomyLevel::PaperTrading);
}

#[test]
fn every_autonomy_level_round_trips_and_describes_itself() {
    for level in AutonomyLevel::all() {
        assert_eq!(AutonomyLevel::parse(level.as_str()).unwrap(), level);
        assert!(!level.describe().is_empty());
    }
    assert!(AutonomyLevel::parse("god_mode").is_err());
    // The ordering is what the ceiling comparison relies on.
    assert!(AutonomyLevel::Observation < AutonomyLevel::PaperTrading);
    assert!(AutonomyLevel::PaperTrading < AutonomyLevel::SupervisedLive);
}

// --- the kill switch --------------------------------------------------------

#[test]
fn tripping_the_kill_switch_needs_no_authority() {
    // A false stop costs far less than a missed one.
    let mut switch = KillSwitch::new();
    switch.trip_global(now(), "risk-monitor", "drawdown limit breached");
    assert!(switch.is_globally_tripped());
    assert!(switch.is_halted("any-scope"));
}

#[test]
fn clearing_the_kill_switch_needs_an_operator() {
    let mut switch = KillSwitch::new();
    switch.trip_global(now(), "risk-monitor", "drawdown limit breached");
    switch.clear_global(&operator(), now()).unwrap();
    assert!(!switch.is_globally_tripped());
}

#[test]
fn a_tripped_kill_switch_overrides_the_autonomy_level() {
    let mut controller = AutonomyController::new();
    assert!(controller.may_execute("momentum"));

    controller
        .kill_switch_mut()
        .trip_global(now(), "risk-monitor", "daily loss limit breached");

    assert_eq!(controller.level(), AutonomyLevel::Observation);
    assert!(!controller.may_execute("momentum"));
    // The configured level is untouched, so clearing restores exactly what was
    // there rather than whatever the last operator happened to set.
    assert_eq!(controller.configured_level(), AutonomyLevel::PaperTrading);

    controller
        .kill_switch_mut()
        .clear_global(&operator(), now())
        .unwrap();
    assert_eq!(controller.level(), AutonomyLevel::PaperTrading);
}

#[test]
fn autonomy_cannot_be_raised_while_the_kill_switch_is_tripped() {
    // Clear the stop deliberately, then raise. Not both at once.
    let mut controller = AutonomyController::with_live_ceiling(AutonomyLevel::SupervisedLive);
    controller
        .kill_switch_mut()
        .trip_global(now(), "risk-monitor", "position limit breached");

    let error = controller
        .request_change(
            AutonomyLevel::SupervisedLive,
            &two_operators(),
            "resuming after the incident",
            now(),
        )
        .unwrap_err();
    assert!(
        error.message().contains("kill switch is tripped"),
        "{}",
        error.message()
    );
}

#[test]
fn a_scoped_halt_stops_one_strategy_and_leaves_the_rest() {
    let mut switch = KillSwitch::new();
    switch.trip_scope("momentum", now(), "risk-monitor", "position limit breached");
    assert!(switch.is_halted("momentum"));
    assert!(!switch.is_halted("carry"));
    assert_eq!(switch.halted_scopes(), vec!["momentum"]);

    switch.clear_scope("momentum", &operator(), now()).unwrap();
    assert!(!switch.is_halted("momentum"));
}

#[test]
fn the_first_reason_for_a_halt_is_the_one_kept() {
    // An incident review wants the trigger, not the last thing to notice.
    let mut switch = KillSwitch::new();
    switch.trip_global(now(), "risk-monitor", "drawdown limit breached");
    switch.trip_global(
        now().saturating_add(Duration::from_secs(1)),
        "execution-engine",
        "orders rejected",
    );
    assert_eq!(
        switch.global_trip().unwrap().reason,
        "drawdown limit breached"
    );
    assert_eq!(switch.history().len(), 2, "both are kept in the history");
}

// --- pre-trade checks -------------------------------------------------------

fn limits() -> LimitSet {
    LimitSet::new("test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 1.5 })
                .with_rationale("gross exposure is capped at 1.5x equity"),
        )
        .with(
            Limit::new(
                "max-order-notional",
                LimitKind::MaxOrderNotional {
                    limit: dec!("2000000"),
                },
            )
            .with_rationale("a single order cannot move more than 2m"),
        )
}

fn state(equity: &str, gross: &str) -> RiskState {
    RiskState {
        equity: Decimal::parse(equity).unwrap(),
        cash: Decimal::parse(equity).unwrap(),
        gross_exposure: Decimal::parse(gross).unwrap(),
        ..RiskState::default()
    }
}

fn order(symbol: &str, quantity: &str, price: &str) -> ProposedOrder {
    ProposedOrder {
        object_id: object(symbol),
        quantity: Decimal::parse(quantity).unwrap(),
        reference_price: Decimal::parse(price).unwrap(),
        axes: BTreeMap::from([("sector".to_string(), "technology".to_string())]),
        counterparty: Some("prime-broker".to_string()),
        scope: "momentum".to_string(),
    }
}

#[test]
fn an_order_within_the_limits_is_approved() -> Result<()> {
    let checker = PreTradeChecker::new(limits());
    let result = checker.check(&order("AAA", "1000", "100"), &state("10000000", "0"), now())?;
    assert!(result.is_approved());
    assert!(
        !result.checks_run.is_empty(),
        "a clean result must be evidence"
    );
    Ok(())
}

#[test]
fn the_check_runs_against_the_state_the_order_would_produce() -> Result<()> {
    // Checking the state *before* the trade is the classic mistake: it passes
    // every order right up to the one that breaches, and then passes that one
    // too. A flat book plus an oversized order must be refused.
    let checker = PreTradeChecker::new(limits());
    let flat = state("10000000", "0");
    let oversized = order("AAA", "20000", "100");

    // Nothing is wrong with the current state.
    assert!(!limits().check(&flat).is_blocked());
    // But the state the order would produce breaches the position limit.
    let result = checker.check(&oversized, &flat, now())?;
    assert!(!result.is_approved(), "{}", result.decision.describe());
    assert!(result.decision.describe().contains("max-position-weight"));
    Ok(())
}

#[test]
fn an_oversized_single_order_is_refused() -> Result<()> {
    let checker = PreTradeChecker::new(limits());
    let result = checker.check(
        &order("AAA", "30000", "100"),
        &state("100000000", "0"),
        now(),
    )?;
    assert!(!result.is_approved());
    assert!(
        result.decision.describe().contains("max-order-notional"),
        "{}",
        result.decision.describe()
    );
    Ok(())
}

#[test]
fn a_refused_order_permits_nothing_by_default() -> Result<()> {
    // Silently resizing means the executed trade is not the reviewed one.
    let checker = PreTradeChecker::new(limits());
    let result = checker.check(
        &order("AAA", "20000", "100"),
        &state("10000000", "0"),
        now(),
    )?;
    assert!(!result.decision.permits_anything());
    assert_eq!(
        result.decision.permitted_quantity(dec!("20000")),
        Decimal::ZERO
    );
    Ok(())
}

#[test]
fn reduction_is_opt_in_and_finds_a_permissible_size() -> Result<()> {
    let checker = PreTradeChecker::new(limits()).allowing_reduction();
    let result = checker.check(
        &order("AAA", "20000", "100"),
        &state("10000000", "0"),
        now(),
    )?;
    match &result.decision {
        PreTradeDecision::Reduced {
            permitted_quantity,
            limiting_constraint,
        } => {
            assert!(*permitted_quantity > Decimal::ZERO);
            assert!(*permitted_quantity < dec!("20000"));
            assert!(!limiting_constraint.is_empty());
            // And the reduced size must actually pass.
            let mut reduced = order("AAA", "20000", "100");
            reduced.quantity = *permitted_quantity;
            let recheck = checker.check(&reduced, &state("10000000", "0"), now())?;
            assert!(recheck.decision.permits_anything());
        }
        other => panic!("expected a reduction, got {other:?}"),
    }
    Ok(())
}

#[test]
fn the_permitted_quantity_is_the_exact_boundary_and_one_more_unit_breaches() -> Result<()> {
    // The bisection used to rebuild each trial quantity through
    // `Decimal::from_f64(full.to_f64() * mid)`, so the size it handed back was
    // a binary-floating-point neighbour of the true boundary rather than the
    // boundary. That is the failure this pins: a size the limits would refuse,
    // returned by the control whose job is to produce a size they accept.
    //
    // The price is deliberately 3 and not 100. At a round price the boundary
    // lands on a dyadic fraction of the order — exactly half of it — which the
    // old `f64` bisection hit on its very first midpoint, so a test written at
    // 100 passes against the defect it was written to catch. It did, when this
    // test was first drafted. At 3 the boundary is a third of the order and
    // the two implementations diverge.
    let checker = PreTradeChecker::new(limits()).allowing_reduction();
    let current = state("10000000", "0");
    let requested = order("AAA", "1000000", "3");

    let result = checker.check(&requested, &current, now())?;
    let PreTradeDecision::Reduced {
        permitted_quantity, ..
    } = &result.decision
    else {
        panic!("expected a reduction, got {:?}", result.decision);
    };
    // Premise: a reduction actually happened, so there is a boundary to test.
    assert!(*permitted_quantity > Decimal::ZERO);
    assert!(*permitted_quantity < dec!("1000000"));

    let at = |quantity: Decimal| -> Result<bool> {
        let mut trial = requested.clone();
        trial.quantity = quantity;
        Ok(checker.check(&trial, &current, now())?.is_approved())
    };

    assert!(
        at(*permitted_quantity)?,
        "the permitted quantity {permitted_quantity} does not itself pass the limits"
    );
    // One scaled unit is the smallest quantity `Decimal` can express, so this
    // is the tightest possible statement that the answer is the boundary and
    // not merely near it.
    let one_more = Decimal::from_raw(permitted_quantity.raw() + 1);
    assert!(
        !at(one_more)?,
        "{one_more} also passes, so {permitted_quantity} is not the largest permissible size"
    );
    Ok(())
}

#[test]
fn the_permitted_quantity_is_identical_across_runs_and_across_order_direction() -> Result<()> {
    // Reproducibility from the event log is the whole product. A bisection in
    // `f64` gave an answer whose last digits depended on the magnitude of the
    // order rather than on the limits, which is not something a reader of the
    // log could ever reconstruct.
    let checker = PreTradeChecker::new(limits()).allowing_reduction();
    let current = state("10000000", "0");

    // Price 3 rather than 100 for the reason given in the boundary test above:
    // at a round price the boundary is a dyadic fraction of the order and both
    // implementations agree by accident.
    let buy = checker
        .check(&order("AAA", "1000000", "3"), &current, now())?
        .decision
        .permitted_quantity(dec!("1000000"));
    let again = checker
        .check(&order("AAA", "1000000", "3"), &current, now())?
        .decision
        .permitted_quantity(dec!("1000000"));
    // Premise: something was actually reduced, or the equality is vacuous.
    assert!(
        buy > Decimal::ZERO && buy < dec!("1000000"),
        "no reduction: {buy}"
    );
    assert_eq!(buy, again, "the same order gave two different answers");

    let mut sell = order("AAA", "1000000", "3");
    sell.quantity = dec!("-1000000");
    let short = checker
        .check(&sell, &current, now())?
        .decision
        .permitted_quantity(dec!("-1000000"));
    assert!(
        short < Decimal::ZERO,
        "a sale must reduce to a sale: {short}"
    );
    assert_eq!(
        short, -buy,
        "the limits here are symmetric, so the permitted sale must mirror the permitted purchase exactly"
    );
    Ok(())
}

#[test]
fn a_reducing_order_shrinks_gross_exposure_rather_than_growing_it() -> Result<()> {
    // The projection has to account for a sale reducing an existing position;
    // adding the order's notional to gross regardless would refuse the very
    // trades that fix a breach.
    let checker = PreTradeChecker::new(limits());
    let mut current = state("10000000", "1000000");
    current
        .position_notionals
        .insert(object("AAA").as_str().to_string(), dec!("1000000"));

    let sale = ProposedOrder {
        object_id: object("AAA"),
        quantity: dec!("-5000"),
        reference_price: dec!("100"),
        axes: BTreeMap::new(),
        counterparty: None,
        scope: "momentum".to_string(),
    };
    let projected = checker.project(&sale, &current);
    assert!(
        projected.gross_exposure < current.gross_exposure,
        "selling must reduce gross: {} vs {}",
        projected.gross_exposure,
        current.gross_exposure
    );
    Ok(())
}

#[test]
fn closing_a_position_through_a_counterparty_flattens_that_counterparty_exposure_rather_than_doubling_it()
-> Result<()> {
    // `counterparty_exposures` is documented in `qip-risk` as *gross exposure
    // per counterparty* — a current balance, not a running total of notional
    // ever routed through that counterparty. Projecting it by adding
    // `order.notional()` unconditionally, the way `gross_exposure` is
    // deliberately *not* computed a few lines above this in `project`, meant
    // a trade that fully closes a position still added to the recorded
    // exposure instead of removing it: a book flattened to zero through one
    // counterparty would report double the true exposure, and a
    // `MaxCounterpartyExposure` limit reading that number could never be
    // satisfied by closing the position that caused the breach.
    let checker = PreTradeChecker::new(limits());
    let mut current = state("10000000", "1000000");
    current
        .position_notionals
        .insert(object("AAA").as_str().to_string(), dec!("1000000"));
    current
        .counterparty_exposures
        .insert("prime-broker".to_string(), dec!("1000000"));

    // A sale that exactly flattens the existing long.
    let close = ProposedOrder {
        object_id: object("AAA"),
        quantity: dec!("-10000"),
        reference_price: dec!("100"),
        axes: BTreeMap::new(),
        counterparty: Some("prime-broker".to_string()),
        scope: "momentum".to_string(),
    };
    let projected = checker.project(&close, &current);

    // Premise: the position is actually closed to flat, so there is no
    // remaining exposure of any kind left to report.
    assert_eq!(
        *projected
            .position_notionals
            .get(object("AAA").as_str())
            .unwrap(),
        Decimal::ZERO,
        "the close must flatten the position for this test to mean anything"
    );
    assert_eq!(
        *projected
            .counterparty_exposures
            .get("prime-broker")
            .unwrap(),
        Decimal::ZERO,
        "a position closed to flat must show zero exposure to the counterparty that closed it, \
         not the sum of the open and the closing trade"
    );
    Ok(())
}

#[test]
fn an_order_with_no_scope_is_refused() {
    let checker = PreTradeChecker::new(limits());
    let mut unscoped = order("AAA", "1000", "100");
    unscoped.scope = String::new();
    let error = checker
        .check(&unscoped, &state("10000000", "0"), now())
        .unwrap_err();
    assert!(
        error.message().contains("scoped kill switch"),
        "{}",
        error.message()
    );
}

#[test]
fn an_order_for_zero_quantity_is_refused() {
    let checker = PreTradeChecker::new(limits());
    let mut empty = order("AAA", "1000", "100");
    empty.quantity = Decimal::ZERO;
    assert!(
        checker
            .check(&empty, &state("10000000", "0"), now())
            .is_err()
    );
}

// --- continuous monitoring --------------------------------------------------

#[test]
fn a_clean_book_continues() -> Result<()> {
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let action = monitor.observe(
        &state("10000000", "0"),
        "momentum",
        AutonomyLevel::PaperTrading,
        now(),
    );
    assert_eq!(action, MonitorAction::Continue);
    assert!(action.permits_new_risk());
    Ok(())
}

#[test]
fn a_drawdown_past_the_threshold_halts_everything_immediately() -> Result<()> {
    // No consecutive-observation count: a 20% drawdown is not a bad print.
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let mut breached = state("10000000", "0");
    breached.drawdown = 0.25;

    let action = monitor.observe(&breached, "momentum", AutonomyLevel::PaperTrading, now());
    assert!(matches!(action, MonitorAction::HaltGlobally { .. }));
    assert!(!action.permits_new_risk());
    assert!(
        !action.permits_reduction(),
        "a global halt stops everything"
    );

    let mut switch = KillSwitch::new();
    monitor.enforce(&action, &mut switch, now());
    assert!(switch.is_globally_tripped());
    Ok(())
}

#[test]
fn a_single_day_loss_past_the_threshold_halts_everything() -> Result<()> {
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let mut breached = state("10000000", "0");
    breached.daily_loss = 0.08;
    let action = monitor.observe(&breached, "momentum", AutonomyLevel::PaperTrading, now());
    assert!(matches!(action, MonitorAction::HaltGlobally { .. }));
    Ok(())
}

#[test]
fn a_first_limit_breach_goes_reduce_only_rather_than_halting() -> Result<()> {
    // One reading can be a stale mark; halting on it would make the platform
    // stoppable by a single bad tick.
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let breached = state("10000000", "20000000");

    let action = monitor.observe(&breached, "momentum", AutonomyLevel::PaperTrading, now());
    assert!(
        matches!(action, MonitorAction::ReduceOnly { .. }),
        "{action:?}"
    );
    assert!(!action.permits_new_risk());
    assert!(
        action.permits_reduction(),
        "a reduce-only state that blocked reductions could never fix the breach"
    );
    Ok(())
}

#[test]
fn a_persistent_breach_escalates_to_a_scoped_halt() -> Result<()> {
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let breached = state("10000000", "20000000");

    let mut last = MonitorAction::Continue;
    for i in 0..3 {
        last = monitor.observe(
            &breached,
            "momentum",
            AutonomyLevel::PaperTrading,
            now().saturating_add(Duration::from_mins(i)),
        );
    }
    match last {
        MonitorAction::HaltScope { scope, .. } => assert_eq!(scope, "momentum"),
        other => panic!("expected a scoped halt after three breaches, got {other:?}"),
    }
    assert_eq!(monitor.consecutive_breaches(), 3);
    Ok(())
}

#[test]
fn the_same_breach_halts_immediately_at_a_live_autonomy_level() -> Result<()> {
    // The asymmetry is deliberate: a breach is more serious when real money is
    // moving, and the platform should favour stopping.
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let breached = state("10000000", "20000000");
    let action = monitor.observe(&breached, "momentum", AutonomyLevel::AutonomousLive, now());
    assert!(
        matches!(action, MonitorAction::HaltScope { .. }),
        "{action:?}"
    );
    Ok(())
}

#[test]
fn a_resolved_breach_resets_the_counter() -> Result<()> {
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    monitor.observe(
        &state("10000000", "20000000"),
        "momentum",
        AutonomyLevel::PaperTrading,
        now(),
    );
    assert_eq!(monitor.consecutive_breaches(), 1);

    monitor.observe(
        &state("10000000", "0"),
        "momentum",
        AutonomyLevel::PaperTrading,
        now().saturating_add(Duration::from_mins(1)),
    );
    assert_eq!(monitor.consecutive_breaches(), 0);
    Ok(())
}

#[test]
fn deciding_and_enforcing_are_separate_so_a_dry_run_is_possible() -> Result<()> {
    // An operator can see what the monitor would do without it happening.
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    let mut breached = state("10000000", "0");
    breached.drawdown = 0.30;

    let action = monitor.observe(&breached, "momentum", AutonomyLevel::PaperTrading, now());
    let mut switch = KillSwitch::new();
    assert!(!switch.is_globally_tripped(), "observing must not enforce");
    monitor.enforce(&action, &mut switch, now());
    assert!(switch.is_globally_tripped());
    Ok(())
}

#[test]
fn every_observation_is_recorded() -> Result<()> {
    let mut monitor = RiskMonitor::new(limits(), MonitorPolicy::default());
    for i in 0..4 {
        monitor.observe(
            &state("10000000", "0"),
            "momentum",
            AutonomyLevel::PaperTrading,
            now().saturating_add(Duration::from_mins(i)),
        );
    }
    assert_eq!(monitor.observations().len(), 4);
    Ok(())
}

// --- the pre-trade and monitor gate rules, both halves ----------------------
//
// | Rule | Pass fixture | Veto fixture |
// |---|---|---|
// | order quantity non-zero | `an_order_within_the_limits_is_approved` | `an_order_for_zero_quantity_is_refused` |
// | order has a usable reference price | `an_order_within_the_limits_is_approved` | `an_order_with_no_usable_reference_price_is_refused` |
// | order names a scope | `an_order_within_the_limits_is_approved` | `an_order_with_no_scope_is_refused` |
// | post-trade limits | `an_order_within_the_limits_is_approved` | `the_check_runs_against_the_state_the_order_would_produce`, `an_oversized_single_order_is_refused` |
// | reduction is opt-in | `reduction_is_opt_in_and_finds_a_permissible_size` | `a_refused_order_permits_nothing_by_default` |
// | drawdown halt | `a_clean_book_continues` | `a_drawdown_past_the_threshold_halts_everything_immediately` |
// | daily-loss halt | `a_clean_book_continues` | `a_single_day_loss_past_the_threshold_halts_everything` |
// | breach count before a scoped halt | `a_first_limit_breach_goes_reduce_only_rather_than_halting` | `a_persistent_breach_escalates_to_a_scoped_halt` |
// | breach grace period | `a_breach_that_outlives_the_grace_period_halts_before_the_third_reading` (its first half) | the same test's second half |
// | live breach halts at once | `a_first_limit_breach_goes_reduce_only_rather_than_halting` | `the_same_breach_halts_immediately_at_a_live_autonomy_level` |

#[test]
fn an_order_with_no_usable_reference_price_is_refused() -> Result<()> {
    let checker = PreTradeChecker::new(limits());
    let current = state("10000000", "0");
    // Premise: the same order at a real price is approved, so the price rule
    // is the only thing that can refuse it below.
    assert!(
        checker
            .check(&order("AAA", "1000", "100"), &current, now())?
            .is_approved()
    );

    for price in ["0", "-100"] {
        let mut unpriced = order("AAA", "1000", "100");
        unpriced.reference_price = Decimal::parse(price).unwrap();
        let error = checker
            .check(&unpriced, &current, now())
            .expect_err("an order with no usable price was checked");
        assert!(
            error.message().contains("reference price"),
            "{}",
            error.message()
        );
    }
    Ok(())
}

#[test]
fn a_breach_that_outlives_the_grace_period_halts_before_the_third_reading() -> Result<()> {
    // Two rules escalate a breach to a scoped halt: three consecutive
    // readings, or one that has persisted past the grace period. The count
    // rule has its own test; this one isolates the clock, by reading twice
    // — one short of the count — first inside the grace period and then
    // beyond it.
    let policy = MonitorPolicy::default();
    assert_eq!(
        policy.breaches_before_halt, 3,
        "premise: two readings are under the count"
    );
    let breached = state("10000000", "20000000");

    // Inside the grace period: still reduce-only on the second reading.
    let mut inside = RiskMonitor::new(limits(), policy);
    inside.observe(&breached, "momentum", AutonomyLevel::PaperTrading, now());
    let second = inside.observe(
        &breached,
        "momentum",
        AutonomyLevel::PaperTrading,
        now().saturating_add(Duration::from_hours(1)),
    );
    assert!(
        matches!(second, MonitorAction::ReduceOnly { .. }),
        "{second:?}"
    );

    // Beyond it: the second reading halts the scope, and the count is still
    // two, so it was the clock that fired.
    let mut beyond = RiskMonitor::new(limits(), policy);
    beyond.observe(&breached, "momentum", AutonomyLevel::PaperTrading, now());
    let second = beyond.observe(
        &breached,
        "momentum",
        AutonomyLevel::PaperTrading,
        now().saturating_add(Duration::from_hours(5)),
    );
    assert_eq!(beyond.consecutive_breaches(), 2);
    match second {
        MonitorAction::HaltScope { scope, .. } => assert_eq!(scope, "momentum"),
        other => panic!("a breach five hours old was not halted: {other:?}"),
    }
    Ok(())
}
