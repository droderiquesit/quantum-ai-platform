//! Tests for the execution path.
//!
//! The most important assertions in the platform are here: that an order
//! cannot reach a live venue below a live autonomy level, that a halted scope
//! stops trading, and that a paper fill can never be mistaken for a real one.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::testing::approx_eq;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_execution_engine::broker::{
    Broker, LiveBroker, LiveVenueConfig, SimulatedBroker, SimulationSettings,
};
use qip_execution_engine::feasibility;
use qip_execution_engine::oms::{OrderManager, RefusalReason, order_type_for};
use qip_execution_engine::order::{Fill, Order, OrderState, OrderType, Side};
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel, OperatorIdentity};
use qip_risk_engine::pretrade::PreTradeChecker;
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn limits() -> LimitSet {
    LimitSet::new("execution-test")
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

fn risk_state() -> RiskState {
    RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    }
}

fn order(symbol: &str, side: Side, quantity: &str) -> Order {
    Order::new(
        OrderId::from_string(format!("ord-{symbol}")),
        object(symbol),
        side,
        Decimal::parse(quantity).unwrap(),
        OrderType::Market,
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        "momentum",
        now(),
    )
}

fn manager() -> OrderManager {
    OrderManager::new(PreTradeChecker::new(limits()))
}

/// A simulated venue that fills everything, so the tests are about the gates
/// rather than about fill mechanics.
fn simulator() -> SimulatedBroker {
    SimulatedBroker::new(fills_everything(), 3)
}

/// Settings that never reject and fill in full, so the gate tests are about
/// the gates rather than about fill mechanics.
fn fills_everything() -> SimulationSettings {
    SimulationSettings::default()
        .with_rejection_probability(0.0)
        .and_then(|s| s.with_immediate_fill_fraction(Decimal::ONE))
        .expect("0.0 and 1.0 are inside the permitted ranges")
}

fn live_venue(credential: bool, enabled: bool) -> LiveBroker {
    LiveBroker::configured(
        LiveVenueConfig {
            venue: "xnys-primary".to_string(),
            credential_env: "QIP_VENUE_TOKEN".to_string(),
            endpoint: "https://fix.example.com".to_string(),
            account: "FUND-1".to_string(),
            required_autonomy: "supervised_live".to_string(),
        },
        credential,
        enabled,
    )
}

fn submit(
    manager: &mut OrderManager,
    order: Order,
    broker: &mut dyn Broker,
    autonomy: &AutonomyController,
) -> qip_execution_engine::oms::SubmissionResult {
    manager.submit(
        order,
        broker,
        autonomy,
        &risk_state(),
        BTreeMap::new(),
        None,
        now(),
    )
}

// --- the live-trading gate --------------------------------------------------

#[test]
fn a_live_venue_is_refused_at_the_default_autonomy_level() {
    // The single most important assertion in the platform.
    let mut manager = manager();
    let mut venue = live_venue(true, true);
    let autonomy = AutonomyController::new();
    assert_eq!(autonomy.level(), AutonomyLevel::PaperTrading);

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut venue,
        &autonomy,
    );

    assert!(!result.accepted);
    assert!(result.fills.is_empty());
    match result.refusal.as_ref().unwrap() {
        RefusalReason::LiveVenueBelowLiveAutonomy { level, venue } => {
            assert_eq!(*level, AutonomyLevel::PaperTrading);
            assert_eq!(venue, "xnys-primary");
        }
        other => panic!("expected the live gate to refuse, got {other:?}"),
    }
    assert!(!manager.has_live_fills());
}

#[test]
fn a_live_autonomy_level_alone_does_not_make_an_unreachable_venue_usable() {
    // Neither implies the other, and treating either as sufficient is how an
    // order reaches a market nobody intended it to reach.
    let mut manager = manager();
    let mut controller = AutonomyController::with_live_ceiling(AutonomyLevel::SupervisedLive);
    controller
        .request_change(
            AutonomyLevel::SupervisedLive,
            &OperatorIdentity::verified("alice@example.com", "hardware-token", now())
                .with_second_approver("bob@example.com"),
            "enabling supervised live trading for the pilot",
            now(),
        )
        .unwrap();
    assert!(controller.is_live());

    // A credential but no operator enablement.
    let mut venue = live_venue(true, false);
    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut venue,
        &controller,
    );

    assert!(!result.accepted);
    match result.refusal.as_ref().unwrap() {
        RefusalReason::VenueUnavailable { detail, .. } => {
            assert!(detail.contains("operator enablement"), "{detail}");
            assert!(detail.contains("simulated venue"), "{detail}");
        }
        other => panic!("expected a venue-unavailable refusal, got {other:?}"),
    }
    assert!(!manager.has_live_fills());
}

#[test]
fn the_live_venue_states_exactly_what_it_is_missing() {
    let venue = live_venue(false, false);
    assert!(!venue.is_available());
    assert!(!venue.is_simulated());

    let requirement = venue.requirement();
    assert!(requirement.contains("QIP_VENUE_TOKEN"), "{requirement}");
    assert!(
        requirement.contains("FIX or REST transport"),
        "{requirement}"
    );
    assert!(requirement.contains("operator enablement"), "{requirement}");
    assert!(
        requirement.contains("simulated venue, which is the configured default"),
        "an operator needs to know what happens instead: {requirement}"
    );
}

#[test]
fn a_live_venue_refuses_to_submit_even_if_called_directly() {
    // Defence in depth: the OMS gate is not the only thing standing between a
    // decision and a real market.
    let mut venue = live_venue(true, true);
    let error = venue
        .submit(&order("AAA", Side::Buy, "1000"), now())
        .unwrap_err();
    assert!(error.message().contains("not usable"));
    assert!(
        venue
            .cancel(&order("AAA", Side::Buy, "1000"), now())
            .is_err()
    );
}

// --- feasibility at the central path -----------------------------------------

#[test]
fn an_off_lot_order_is_refused_at_the_venue_gate_before_it_ever_reaches_the_broker() {
    // The central path used to have no equivalent of `qip-edge`'s
    // feasibility gate: a well-formed order rode the kill switch, the
    // autonomy gate and pre-trade risk straight to the venue with no check
    // that the venue's own grid could accept it. Lot size 2 here, so a
    // quantity of 3 is off it.
    let mut manager = manager().with_venue_feasibility(
        "simulated-venue",
        feasibility::VenueFeasibility::new(dec!("2"), None, dec!("0"), dec!("0"))
            .expect("a valid model"),
    );
    let mut broker = simulator();
    let autonomy = AutonomyController::new();

    // Premise: nothing has reached the broker yet, so a zero count after the
    // refusal is evidence the gate stopped it rather than an artefact of an
    // empty run.
    assert_eq!(broker.submitted_count(), 0);

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "3"),
        &mut broker,
        &autonomy,
    );

    assert!(!result.accepted);
    assert!(result.fills.is_empty());
    match result.refusal.as_ref().unwrap() {
        // The gate reports through `Malformed` rather than a variant of its
        // own — see the module comment on why — naming the gate literal in
        // the detail text instead of a structured field.
        RefusalReason::Malformed { detail } => {
            let expected_prefix = format!("infeasible ({}):", feasibility::GATE_LOT);
            assert!(detail.starts_with(&expected_prefix), "{detail}");
            assert!(detail.contains("not a whole number of lots"), "{detail}");
        }
        other => panic!("expected a malformed (infeasible) refusal, got {other:?}"),
    }
    // The broker was never asked: the gate refused before submission, not
    // after a rejected send.
    assert_eq!(broker.submitted_count(), 0);
}

#[test]
fn an_order_on_the_venues_grid_still_reaches_it_once_a_feasibility_model_is_installed() {
    // The other half of the same fixture family: installing a grid must not
    // become a blanket refusal, or the gate would be indistinguishable from
    // one that rejects everything.
    let mut manager = manager().with_venue_feasibility(
        "simulated-venue",
        feasibility::VenueFeasibility::new(dec!("2"), None, dec!("0"), dec!("0"))
            .expect("a valid model"),
    );
    let mut broker = simulator();
    let autonomy = AutonomyController::new();

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "4"),
        &mut broker,
        &autonomy,
    );

    assert!(result.accepted, "{:?}", result.refusal);
    assert!(!result.fills.is_empty());
    assert_eq!(broker.submitted_count(), 1);
}

#[test]
fn a_venue_with_no_feasibility_model_installed_enforces_no_grid() {
    // Stated in the module comment: a venue nobody has modelled is checked
    // for nothing, rather than the gate inventing a grid to enforce. An
    // order at a size no plausible grid would admit (a fraction of a share)
    // still reaches the venue when no model was installed for it.
    let mut manager = manager();
    let mut broker = simulator();
    let autonomy = AutonomyController::new();

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "3.7"),
        &mut broker,
        &autonomy,
    );

    assert!(result.accepted, "{:?}", result.refusal);
    assert_eq!(broker.submitted_count(), 1);
}

// --- the kill switch in the execution path ----------------------------------

#[test]
fn a_halted_scope_stops_its_orders_and_leaves_the_others() {
    let mut manager = manager();
    let mut broker = simulator();
    let mut controller = AutonomyController::new();
    controller.kill_switch_mut().trip_scope(
        "momentum",
        now(),
        "risk-monitor",
        "position limit breached",
    );

    let halted = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );
    assert!(!halted.accepted);
    assert!(matches!(
        halted.refusal.as_ref().unwrap(),
        RefusalReason::Halted { .. }
    ));

    let mut other = order("BBB", Side::Buy, "1000");
    other.scope = "carry".to_string();
    let allowed = submit(&mut manager, other, &mut broker, &controller);
    assert!(allowed.accepted, "{:?}", allowed.refusal);
}

#[test]
fn a_global_halt_stops_every_scope() {
    let mut manager = manager();
    let mut broker = simulator();
    let mut controller = AutonomyController::new();
    controller
        .kill_switch_mut()
        .trip_global(now(), "risk-monitor", "drawdown limit breached");

    for scope in ["momentum", "carry", "value"] {
        let mut order = order("AAA", Side::Buy, "1000");
        order.order_id = OrderId::from_string(format!("ord-{scope}"));
        order.scope = scope.to_string();
        let result = submit(&mut manager, order, &mut broker, &controller);
        assert!(
            !result.accepted,
            "{scope} was allowed through a global halt"
        );
    }
}

#[test]
fn observation_level_permits_no_execution_at_all() {
    let mut manager = manager();
    let mut broker = simulator();
    let mut controller = AutonomyController::new();
    controller.reduce_to(AutonomyLevel::Observation, "data quality alert", now());

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );
    assert!(!result.accepted);
    match result.refusal.as_ref().unwrap() {
        RefusalReason::AutonomyTooLow { level, required } => {
            assert_eq!(*level, AutonomyLevel::Observation);
            assert_eq!(*required, AutonomyLevel::PaperTrading);
        }
        other => panic!("expected an autonomy refusal, got {other:?}"),
    }
}

#[test]
fn advisory_level_proposes_but_never_executes() {
    let mut manager = manager();
    let mut broker = simulator();
    let mut controller = AutonomyController::new();
    controller.reduce_to(AutonomyLevel::Advisory, "running in advisory mode", now());
    assert!(!controller.level().executes());

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );
    assert!(!result.accepted);
    assert!(manager.fills().is_empty());
}

// --- paper versus live ------------------------------------------------------

#[test]
fn a_paper_fill_is_marked_as_simulated_on_the_fill_itself() -> Result<()> {
    // A reconciliation must be able to tell paper from real without consulting
    // configuration, which is exactly what gets confused between a test and a
    // deployment.
    let mut manager = manager();
    let mut broker = simulator();
    let controller = AutonomyController::new();

    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );
    assert!(result.accepted, "{:?}", result.refusal);
    assert!(result.simulated);
    assert!(!result.fills.is_empty());
    for fill in &result.fills {
        assert!(fill.simulated, "a paper fill must say so");
    }
    assert!(!manager.has_live_fills());

    let stored = manager.order(&result.order_id).unwrap();
    assert!(stored.is_paper());
    Ok(())
}

#[test]
fn the_oms_does_not_take_the_brokers_word_for_the_simulation_flag() -> Result<()> {
    // A broker that reported the wrong flag would let a paper fill be counted
    // as real. The OMS sets it from `is_simulated()` rather than trusting the
    // fill it was handed.
    #[derive(Debug)]
    struct LyingSimulator;
    impl Broker for LyingSimulator {
        fn name(&self) -> &str {
            "lying-simulator"
        }
        fn is_simulated(&self) -> bool {
            true
        }
        fn is_available(&self) -> bool {
            true
        }
        fn capabilities(&self) -> qip_execution_engine::broker::VenueCapabilities {
            qip_execution_engine::broker::VenueCapabilities {
                name: "lying-simulator".to_string(),
                supported_types: vec!["market".to_string()],
                partial_fills: false,
                lot_size: Decimal::from_int(1),
                commission_rate: 0.0,
            }
        }
        fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>> {
            Ok(vec![Fill {
                fill_id: qip_core::ids::FillId::from_string("fill-lie"),
                order_id: order.order_id.clone(),
                at,
                quantity: order.quantity,
                price: order.arrival_price,
                costs: Decimal::ZERO,
                venue: "lying-simulator".to_string(),
                // Claims to be a real fill.
                simulated: false,
            }])
        }
        fn cancel(&mut self, _order: &Order, _at: Timestamp) -> Result<()> {
            Ok(())
        }
    }

    let mut manager = manager();
    let mut broker = LyingSimulator;
    let controller = AutonomyController::new();
    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );

    assert!(result.accepted);
    for fill in &result.fills {
        assert!(fill.simulated, "the OMS must correct the broker's claim");
    }
    assert!(!manager.has_live_fills());
    Ok(())
}

#[test]
fn a_fill_the_order_refuses_is_recorded_rather_than_dropped() -> Result<()> {
    // The most dangerous state an execution system can be in is one where the
    // venue and the book disagree and nothing says so. An over-fill is the
    // easiest way to get there: the order state machine refuses it, and if
    // that refusal is discarded the platform quietly holds a position the
    // venue does not agree with.
    #[derive(Debug)]
    struct OverFillingVenue;
    impl Broker for OverFillingVenue {
        fn name(&self) -> &str {
            "over-filling-venue"
        }
        fn is_simulated(&self) -> bool {
            true
        }
        fn is_available(&self) -> bool {
            true
        }
        fn capabilities(&self) -> qip_execution_engine::broker::VenueCapabilities {
            qip_execution_engine::broker::VenueCapabilities {
                name: "over-filling-venue".to_string(),
                supported_types: vec!["market".to_string()],
                partial_fills: false,
                lot_size: Decimal::from_int(1),
                commission_rate: 0.0,
            }
        }
        fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>> {
            // Twice what was asked for.
            Ok(vec![Fill {
                fill_id: qip_core::ids::FillId::from_string("fill-over"),
                order_id: order.order_id.clone(),
                at,
                quantity: order.quantity + order.quantity,
                price: order.arrival_price,
                costs: Decimal::ZERO,
                venue: "over-filling-venue".to_string(),
                simulated: true,
            }])
        }
        fn cancel(&mut self, _order: &Order, _at: Timestamp) -> Result<()> {
            Ok(())
        }
    }

    let mut manager = manager();
    let mut broker = OverFillingVenue;
    let controller = AutonomyController::new();
    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );

    // The over-fill is not applied: the book stays at what the order allows.
    assert!(
        result.fills.is_empty(),
        "an over-fill was applied to the book"
    );

    // And the disagreement is visible, both on the submission and on the
    // manager, so a monitor reading either one can halt.
    assert_eq!(
        result.reconciliation_breaks.len(),
        1,
        "the refused fill was dropped without a word"
    );
    assert!(
        result.reconciliation_breaks[0].contains("over-filling-venue"),
        "the break does not name the venue: {:?}",
        result.reconciliation_breaks
    );
    assert_eq!(manager.reconciliation_breaks().len(), 1);
    Ok(())
}

#[test]
fn an_ordinary_submission_records_no_disagreement() -> Result<()> {
    // The other half: a signal that fires on a normal day is one nobody acts
    // on when it matters.
    let mut manager = manager();
    let mut broker = simulator();
    let controller = AutonomyController::new();
    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );
    assert!(result.accepted);
    assert!(result.reconciliation_breaks.is_empty());
    assert!(manager.reconciliation_breaks().is_empty());
    Ok(())
}

// --- risk in the execution path ---------------------------------------------

#[test]
fn an_order_that_would_breach_a_limit_never_reaches_the_venue() -> Result<()> {
    let mut manager = manager();
    let mut broker = simulator();
    let controller = AutonomyController::new();

    // 20,000 shares at 100 is 2m against 10m of equity: 20% in one name
    // against a 10% cap.
    let result = submit(
        &mut manager,
        order("AAA", Side::Buy, "20000"),
        &mut broker,
        &controller,
    );
    assert!(!result.accepted);
    assert!(matches!(
        result.refusal.as_ref().unwrap(),
        RefusalReason::RiskRejected { .. }
    ));
    assert_eq!(
        broker.submitted_count(),
        0,
        "a refused order must not reach the venue at all"
    );
    Ok(())
}

#[test]
fn a_safety_refusal_is_distinguishable_from_a_transient_fault() -> Result<()> {
    // A safety refusal must never be retried automatically; a transient one may.
    let mut manager = manager();
    let mut broker = simulator();
    let controller = AutonomyController::new();
    submit(
        &mut manager,
        order("AAA", Side::Buy, "20000"),
        &mut broker,
        &controller,
    );

    assert_eq!(manager.safety_refusals().len(), 1);
    let refusal = manager.safety_refusals()[0];
    assert!(refusal.refusal.as_ref().unwrap().is_safety_control());
    Ok(())
}

#[test]
fn every_refusal_is_recorded_so_a_missing_position_can_be_explained() -> Result<()> {
    let mut manager = manager();
    let mut broker = simulator();
    let mut controller = AutonomyController::new();
    controller
        .kill_switch_mut()
        .trip_global(now(), "risk-monitor", "drawdown limit breached");

    submit(
        &mut manager,
        order("AAA", Side::Buy, "1000"),
        &mut broker,
        &controller,
    );
    assert_eq!(manager.refusals().len(), 1);
    let stored = manager.order(&OrderId::from_string("ord-AAA")).unwrap();
    assert!(matches!(stored.state, OrderState::Rejected { .. }));
    Ok(())
}

// --- order validity ---------------------------------------------------------

#[test]
fn an_order_that_traces_to_no_hypothesis_is_refused() {
    let mut manager = manager();
    let mut broker = simulator();
    let controller = AutonomyController::new();

    let mut untraceable = order("AAA", Side::Buy, "1000");
    untraceable.hypotheses.clear();
    let result = submit(&mut manager, untraceable, &mut broker, &controller);

    assert!(!result.accepted);
    match result.refusal.as_ref().unwrap() {
        RefusalReason::Malformed { detail } => {
            assert!(detail.contains("nobody can explain"), "{detail}");
        }
        other => panic!("expected a malformed refusal, got {other:?}"),
    }
}

#[test]
fn an_order_with_no_scope_cannot_be_stopped_and_is_refused() {
    let mut manager = manager();
    let mut broker = simulator();
    let controller = AutonomyController::new();

    let mut unscoped = order("AAA", Side::Buy, "1000");
    unscoped.scope = String::new();
    let result = submit(&mut manager, unscoped, &mut broker, &controller);
    assert!(!result.accepted);
}

// --- the order state machine ------------------------------------------------

#[test]
fn a_cancelled_order_cannot_be_filled_by_a_late_report() -> Result<()> {
    let mut order = order("AAA", Side::Buy, "1000");
    order.transition(OrderState::RiskApproved { at: now() }, now())?;
    order.transition(
        OrderState::Working {
            at: now(),
            venue: "simulated-venue".to_string(),
        },
        now(),
    )?;
    order.transition(
        OrderState::Cancelled {
            at: now(),
            reason: "risk halt".to_string(),
        },
        now(),
    )?;

    let late = Fill {
        fill_id: qip_core::ids::FillId::from_string("fill-late"),
        order_id: order.order_id.clone(),
        at: now().saturating_add(Duration::from_secs(1)),
        quantity: dec!("1000"),
        price: dec!("100"),
        costs: Decimal::ZERO,
        venue: "simulated-venue".to_string(),
        simulated: true,
    };
    assert!(
        order.apply_fill(late).is_err(),
        "a cancelled order that a late report can fill is a reconciliation break"
    );
    Ok(())
}

#[test]
fn an_order_cannot_start_working_without_a_risk_approval() {
    let mut order = order("AAA", Side::Buy, "1000");
    let error = order
        .transition(
            OrderState::Working {
                at: now(),
                venue: "simulated-venue".to_string(),
            },
            now(),
        )
        .unwrap_err();
    assert!(
        error
            .message()
            .contains("cannot move from created to working")
    );
}

#[test]
fn an_overfill_is_refused() -> Result<()> {
    // Accepting one would put the book out of step with reality in a way
    // nothing else would catch.
    let mut order = order("AAA", Side::Buy, "1000");
    order.transition(OrderState::RiskApproved { at: now() }, now())?;
    order.transition(
        OrderState::Working {
            at: now(),
            venue: "v".to_string(),
        },
        now(),
    )?;

    let too_much = Fill {
        fill_id: qip_core::ids::FillId::from_string("fill-big"),
        order_id: order.order_id.clone(),
        at: now(),
        quantity: dec!("1500"),
        price: dec!("100"),
        costs: Decimal::ZERO,
        venue: "v".to_string(),
        simulated: true,
    };
    assert!(
        order
            .apply_fill(too_much)
            .unwrap_err()
            .message()
            .contains("exceeds")
    );
    Ok(())
}

#[test]
fn a_redelivered_fill_report_does_not_double_book_the_order() -> Result<()> {
    // A retried connection can redeliver the same venue report. Nothing
    // upstream of `apply_fill` can tell that apart from a second, genuinely
    // distinct print at the same size and price, so the check has to live
    // here: the same fill id applied twice must not double the quantity
    // booked against the order.
    let mut order = order("AAA", Side::Buy, "1000");
    order.transition(OrderState::RiskApproved { at: now() }, now())?;
    order.transition(
        OrderState::Working {
            at: now(),
            venue: "v".to_string(),
        },
        now(),
    )?;

    let fill = Fill {
        fill_id: qip_core::ids::FillId::from_string("fill-once"),
        order_id: order.order_id.clone(),
        at: now(),
        quantity: dec!("400"),
        price: dec!("100"),
        costs: dec!("1"),
        venue: "v".to_string(),
        simulated: true,
    };
    // The premise: the first application succeeds and books the quantity.
    order.apply_fill(fill.clone())?;
    assert_eq!(order.filled_quantity(), dec!("400"));

    let error = order
        .apply_fill(fill)
        .expect_err("the same fill id was applied twice without complaint");
    assert!(
        error.message().contains("already applied"),
        "the refusal does not name the duplicate: {}",
        error.message()
    );
    assert_eq!(
        order.filled_quantity(),
        dec!("400"),
        "a redelivered report doubled the booked quantity"
    );
    Ok(())
}

#[test]
fn partial_fills_accumulate_and_complete() -> Result<()> {
    let mut order = order("AAA", Side::Buy, "1000");
    order.transition(OrderState::RiskApproved { at: now() }, now())?;
    order.transition(
        OrderState::Working {
            at: now(),
            venue: "v".to_string(),
        },
        now(),
    )?;

    for (i, quantity) in ["400", "400", "200"].iter().enumerate() {
        order.apply_fill(Fill {
            fill_id: qip_core::ids::FillId::from_string(format!("fill-{i}")),
            order_id: order.order_id.clone(),
            at: now(),
            quantity: Decimal::parse(quantity).unwrap(),
            price: dec!("100"),
            costs: dec!("1"),
            venue: "v".to_string(),
            simulated: true,
        })?;
    }
    assert!(matches!(order.state, OrderState::Filled { .. }));
    assert_eq!(order.filled_quantity(), dec!("1000"));
    assert_eq!(order.average_price().unwrap(), dec!("100"));
    assert_eq!(order.total_costs(), dec!("3"));
    Ok(())
}

#[test]
fn slippage_is_signed_so_positive_is_always_worse() -> Result<()> {
    // A buy filling above arrival and a sell filling below are both bad, and a
    // sign convention that made one negative would hide half the cost.
    let mut buy = order("AAA", Side::Buy, "1000");
    buy.transition(OrderState::RiskApproved { at: now() }, now())?;
    buy.transition(
        OrderState::Working {
            at: now(),
            venue: "v".to_string(),
        },
        now(),
    )?;
    buy.apply_fill(Fill {
        fill_id: qip_core::ids::FillId::from_string("f1"),
        order_id: buy.order_id.clone(),
        at: now(),
        quantity: dec!("1000"),
        price: dec!("101"),
        costs: Decimal::ZERO,
        venue: "v".to_string(),
        simulated: true,
    })?;
    assert!(approx_eq(buy.slippage_bps().unwrap(), 100.0, 1e-6));

    let mut sell = order("BBB", Side::Sell, "1000");
    sell.transition(OrderState::RiskApproved { at: now() }, now())?;
    sell.transition(
        OrderState::Working {
            at: now(),
            venue: "v".to_string(),
        },
        now(),
    )?;
    sell.apply_fill(Fill {
        fill_id: qip_core::ids::FillId::from_string("f2"),
        order_id: sell.order_id.clone(),
        at: now(),
        quantity: dec!("1000"),
        price: dec!("99"),
        costs: Decimal::ZERO,
        venue: "v".to_string(),
        simulated: true,
    })?;
    assert!(
        approx_eq(sell.slippage_bps().unwrap(), 100.0, 1e-6),
        "a sell below arrival is also a cost: {:?}",
        sell.slippage_bps()
    );
    Ok(())
}

// --- cancellation -----------------------------------------------------------

#[test]
fn cancelling_a_scope_stops_every_open_order_in_it() -> Result<()> {
    let mut manager = manager();
    // Partial fills leave the orders open, which is the case that matters for
    // a halt.
    let settings = SimulationSettings::default()
        .with_rejection_probability(0.0)?
        .with_immediate_fill_fraction(dec!("0.5"))?;
    let mut broker = SimulatedBroker::new(settings, 3);
    let controller = AutonomyController::new();

    for symbol in ["AAA", "BBB"] {
        submit(
            &mut manager,
            order(symbol, Side::Buy, "1000"),
            &mut broker,
            &controller,
        );
    }
    assert_eq!(manager.open_orders().len(), 2);

    let (cancelled, failed) = manager.cancel_scope(
        "momentum",
        &mut broker,
        "the kill switch tripped",
        now().saturating_add(Duration::from_secs(1)),
    );
    assert_eq!(cancelled.len(), 2);
    assert!(
        failed.is_empty(),
        "a halt that silently left orders working would be worse than no halt: {failed:?}"
    );
    assert!(manager.open_orders().is_empty());
    Ok(())
}

// --- the simulated venue ----------------------------------------------------

#[test]
fn the_simulator_does_not_fill_everything_at_once_by_default() -> Result<()> {
    // A simulator that always fills in full teaches a strategy that liquidity
    // is free, and the lesson is expensive to unlearn.
    let settings = SimulationSettings::default();
    assert!(settings.immediate_fill_fraction() < Decimal::ONE);
    assert!(
        settings.rejection_probability() > 0.0,
        "rejections must be exercised"
    );
    Ok(())
}

#[test]
fn a_market_order_pays_the_spread() -> Result<()> {
    let mut broker = SimulatedBroker::new(fills_everything(), 1);
    let buy = order("AAA", Side::Buy, "1000");
    let fills = broker.submit(&buy, now())?;
    assert_eq!(fills.len(), 1);
    assert!(
        fills[0].price > buy.arrival_price,
        "a buy must pay up: {} against {}",
        fills[0].price,
        buy.arrival_price
    );

    let sell = order("BBB", Side::Sell, "1000");
    let fills = broker.submit(&sell, now())?;
    assert!(
        fills[0].price < sell.arrival_price,
        "a sell must give up the spread too"
    );
    Ok(())
}

#[test]
fn a_limit_order_does_not_fill_through_its_price() -> Result<()> {
    let mut broker = SimulatedBroker::new(fills_everything(), 1);
    let mut buy = order("AAA", Side::Buy, "1000");
    // The simulator fills a buy slightly above 100; a limit below that must
    // not trade.
    buy.order_type = OrderType::Limit { price: dec!("99") };
    assert!(broker.submit(&buy, now())?.is_empty());

    buy.order_type = OrderType::Limit { price: dec!("110") };
    assert!(!broker.submit(&buy, now())?.is_empty());
    Ok(())
}

#[test]
fn an_immediate_fill_fraction_outside_its_range_is_refused_rather_than_clamped() -> Result<()> {
    // This used to be `clamp(0.0, 1.0)` at the point of use, so an operator who
    // configured 1.6 silently got 1.0 and one who configured -0.5 silently got
    // no fills at all. A value silently corrected is a caller bug that survives
    // into every backtest run afterwards.
    //
    // Premise first: the builder admits a good value, or refusing a bad one
    // proves nothing.
    let good = SimulationSettings::default().with_immediate_fill_fraction(dec!("0.25"))?;
    assert_eq!(good.immediate_fill_fraction(), dec!("0.25"));

    for bad in [dec!("1.6"), dec!("-0.5"), Decimal::ZERO] {
        let refused = SimulationSettings::default().with_immediate_fill_fraction(bad);
        let Err(error) = refused else {
            panic!("a fill fraction of {bad} was accepted");
        };
        assert!(
            error.message().contains("at most 1"),
            "the refusal must name the range the caller should have used: {}",
            error.message()
        );
    }

    // And the boundary itself is admitted: a gate that refuses everything is
    // not a gate.
    assert_eq!(
        SimulationSettings::default()
            .with_immediate_fill_fraction(Decimal::ONE)?
            .immediate_fill_fraction(),
        Decimal::ONE
    );
    Ok(())
}

#[test]
fn settings_deserialized_from_a_document_face_the_same_refusal_as_the_builder() -> Result<()> {
    // A settings blob read from a file is exactly the case the clamp used to
    // swallow, so the validation has to sit on the serde path too.
    let good = serde_json::to_string(&SimulationSettings::default())
        .map_err(|e| qip_core::error::Error::schema(e.to_string()))?;
    let parsed: SimulationSettings =
        serde_json::from_str(&good).map_err(|e| qip_core::error::Error::schema(e.to_string()))?;
    assert_eq!(parsed.immediate_fill_fraction(), dec!("0.6"));

    let bad = good.replace(
        r#""immediate_fill_fraction":"0.6""#,
        r#""immediate_fill_fraction":"1.6""#,
    );
    assert_ne!(bad, good, "the substitution did not fire");
    let refused: std::result::Result<SimulationSettings, _> = serde_json::from_str(&bad);
    assert!(
        refused.is_err(),
        "a document carrying an impossible fill fraction was accepted"
    );
    Ok(())
}

#[test]
fn a_fill_price_that_cannot_be_represented_is_refused_not_reported_as_the_arrival_price()
-> Result<()> {
    // The fallback used to be `.unwrap_or(arrival)`: a cost the conversion
    // could not represent became a fill at the arrival price, which is zero
    // slippage — the most flattering answer available, delivered silently.
    let buy = order("AAA", Side::Buy, "1000");

    // Premise: with sane settings this order fills, and away from arrival.
    let mut sane = SimulatedBroker::new(fills_everything(), 1);
    let fills = sane.submit(&buy, now())?;
    assert_eq!(fills.len(), 1);
    assert!(fills[0].price > buy.arrival_price);

    // A half-spread this large makes the price adjustment unrepresentable.
    let settings = fills_everything().with_half_spread_bps(1e35)?;
    let mut broken = SimulatedBroker::new(settings, 1);
    let Err(error) = broken.submit(&buy, now()) else {
        panic!("an unrepresentable fill price produced a fill instead of a refusal");
    };
    assert!(
        error.message().contains("half_spread_bps"),
        "the refusal must name the setting to change: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_commission_that_cannot_be_computed_is_refused_not_booked_as_zero() -> Result<()> {
    // `.unwrap_or(Decimal::ZERO)` on the commission booked a free trade
    // whenever the arithmetic failed. Free is the most favourable answer there
    // is, and a simulator that fails favourably flatters every strategy
    // measured against it.
    let mut broker = SimulatedBroker::new(fills_everything(), 1);

    // Premise: an ordinary fill books a strictly positive commission, so the
    // commission path is live and a zero would be visible.
    let ordinary = order("AAA", Side::Buy, "1000");
    let fills = broker.submit(&ordinary, now())?;
    assert_eq!(fills.len(), 1);
    assert!(
        fills[0].costs > Decimal::ZERO,
        "the ordinary case books no commission, so this test could not tell zero from a refusal"
    );

    // A notional beyond what the fixed-point type can express.
    let huge = Order::new(
        OrderId::from_string("ord-huge"),
        object("HUGE"),
        Side::Buy,
        dec!("1000000000000000"),
        OrderType::Market,
        dec!("1000000000000000"),
        "prop-1",
        vec!["hyp-1".to_string()],
        "momentum",
        now(),
    );
    let Err(error) = broker.submit(&huge, now()) else {
        panic!("an uncomputable commission produced a fill instead of a refusal");
    };
    assert!(
        error.message().contains("commission"),
        "the refusal must say the cost is what could not be computed: {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("must not be booked without its cost"),
        "the refusal must say why the fill was withheld: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_simulator_is_deterministic() -> Result<()> {
    let run = || -> Result<Vec<Fill>> {
        let mut broker = SimulatedBroker::new(SimulationSettings::default(), 42);
        broker.submit(&order("AAA", Side::Buy, "1000"), now())
    };
    assert_eq!(run()?, run()?);
    Ok(())
}

// --- order type selection ---------------------------------------------------

#[test]
fn a_large_order_is_worked_rather_than_taken() {
    // A market order in an illiquid name is how a small position becomes a
    // large loss.
    assert!(matches!(order_type_for(0.005, 30), OrderType::Market));
    assert!(matches!(
        order_type_for(0.03, 30),
        OrderType::TimeWeighted { .. }
    ));
    assert!(matches!(
        order_type_for(0.10, 30),
        OrderType::VolumeWeighted { .. }
    ));
    assert!(matches!(
        order_type_for(0.40, 30),
        OrderType::Participation { .. }
    ));
    assert!(OrderType::Market.is_unpriced());
    assert!(!OrderType::Limit { price: dec!("100") }.is_unpriced());
}
