//! Blueprint §34.3 and §34.4 across the seams no single crate can see.
//!
//! Three questions this file exists to answer, each of which needs both sides
//! of a boundary in view:
//!
//! 1. **Does the MEV estimate actually change what the platform will send?**
//!    `qip_brokers::dex` computes it and `qip_execution_engine`'s order
//!    manager enforces a minimum notional; only a test holding both can show
//!    that the number computed in the first is the number the second refuses
//!    on. A figure nothing reads is the failure this repository names most
//!    often, and an estimate in a report reads exactly like one that decides.
//!
//! 2. **Can the venue promotion ladder enable a live venue?** It cannot, and
//!    proving it needs the ladder (`qip_lifecycle::venue_ladder`) and the
//!    submission path (`qip_execution_engine::oms`) in the same test, because
//!    the claim is about what a *promoted* venue can still not do.
//!
//! 3. **Does a decentralised venue's contract risk land on its own exposure
//!    axis?** `qip_financial::pool` names the axis and `qip_risk`'s limit set
//!    is what charges against it; the two never meet inside one crate.
//!
//! # The paper-trading boundary, stated
//!
//! Nothing in this file creates, enables or eases a live-order path. The
//! §34.4 ladder's ceiling is the *simulator* rung, every DEX venue here is
//! `AdapterClass::Simulated`, and the one test that names a live-class broker
//! asserts that an order to it is **refused** — by
//! `RefusalReason::LiveVenueBelowLiveAutonomy`, the step in
//! `OrderManager::submit` that this lane leaves exactly as it found it.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_brokers::VenueCredential;
use qip_brokers::adapter::{AdapterClass, VenueAdapter};
use qip_brokers::credential::{RequirementKind, requirements_of_kind, standard_requirements};
use qip_brokers::dex::DexVenue;
use qip_brokers::exchange::{ExchangeSettings, SimulatedExchange};
use qip_contracts::gate::GateStage;
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::Result;
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_execution_engine::broker::{
    Broker, LiveBroker, LiveVenueConfig, SimulatedBroker, SimulationSettings,
};
use qip_execution_engine::feasibility::GATE_MINIMUM_NOTIONAL;
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Fill, Order, OrderType, Side};
use qip_execution_engine::session::{RecordedInstruction, SessionRecorder};
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::pool::{
    BlockExecution, CONTRACT_RISK_AXIS, ContractRisk, DexModel, PoolCurve, PoolState,
};
use qip_financial::quality::Provenance;
use qip_lifecycle::venue_ladder::{
    SimulationEvidence, VENUE_PROMOTION_CEILING, VenueDeclaration, VenueEvidence, VenueLadder,
    VenueMeasurement, VenuePromotionPolicy, attempt_promotion,
};
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_risk_engine::pretrade::PreTradeChecker;
use std::collections::{BTreeMap, BTreeSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn limits() -> LimitSet {
    LimitSet::new("decentralised-venue").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 4.0 })
            .with_rationale("gross exposure is capped so pre-trade risk is not the binding gate"),
    )
}

fn funded() -> RiskState {
    RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    }
}

/// A deep, cheap pool: the curve's own slippage at the clips below is small,
/// so what moves between the two venues in the first test is the headroom and
/// nothing else.
fn pool() -> PoolState {
    PoolState::new(
        PoolCurve::ConstantProduct,
        dec!("1000000"),
        dec!("1000000"),
        dec!("5"),
    )
    .expect("a valid pool")
}

fn model(tolerance_bps: &str, contract: &str) -> DexModel {
    DexModel::new()
        .with_pool(pool())
        .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a valid mode"))
        .with_tolerance_bps(Decimal::parse(tolerance_bps).expect("a decimal literal"))
        .expect("a valid tolerance")
        .with_contract(ContractRisk::new(contract, None, true).expect("a valid flag"))
}

/// Gas of one input unit, a two-hundred basis point budget, lot 0.01.
fn dex_venue(name: &str, tolerance_bps: &str, contract: &str) -> DexVenue {
    DexVenue::new(
        VenueId::new(name),
        model(tolerance_bps, contract),
        dec!("1"),
        dec!("200"),
        dec!("0.01"),
    )
    .expect("a valid venue")
}

fn order(id: &str, quantity: Decimal, price: Decimal) -> Order {
    Order::new(
        OrderId::from_string(id),
        ObjectId::from_string("POOL-TOKEN"),
        Side::Buy,
        quantity,
        OrderType::Market,
        price,
        "prop-dex",
        vec!["hyp-dex".to_string()],
        "momentum",
        start(),
    )
}

#[test]
fn a_mev_estimate_a_decentralised_venue_computes_is_the_number_the_feasibility_gate_refuses_on()
-> Result<()> {
    // §34.3's third row, end to end through the production submission path.
    // Two venues identical in pool, gas, cost budget and lot size; the only
    // difference is the slippage tolerance their orders are submitted under,
    // which is what bounds a sandwich. If the platform's feasibility gate
    // read slippage alone — or read nothing at all, which is where this row
    // started — the same order would be admitted at both.
    let tight = dex_venue("XTIGHT", "20", "0xtight");
    let wide = dex_venue("XWIDE", "150", "0xwide");
    let clip = dec!("100");

    let tight_quote = tight.quote(clip)?;
    let wide_quote = wide.quote(clip)?;
    assert_eq!(
        tight_quote.pool.slippage_bps, wide_quote.pool.slippage_bps,
        "the premise: the curve charges both venues identically, so anything that differs below \
         is the MEV headroom and not the pool"
    );
    assert!(
        wide_quote.mev.extractable_bps > tight_quote.mev.extractable_bps,
        "the premise: the wider tolerance leaves more to the mempool"
    );

    let tight_model = tight.feasibility_model(clip)?;
    let wide_model = wide.feasibility_model(clip)?;
    assert!(
        wide_model.minimum_notional() > tight_model.minimum_notional(),
        "the estimate did not reach the gate's threshold: {} vs {}",
        wide_model.minimum_notional(),
        tight_model.minimum_notional()
    );

    // One order, priced so it clears the tight venue's floor and not the
    // wide one's. Asserted rather than assumed, because a test whose fixture
    // sits outside both floors would pass on a gate that refused everything.
    let notional = dec!("60");
    assert!(
        notional >= tight_model.minimum_notional(),
        "the premise: this order is large enough for the tight venue ({} needed)",
        tight_model.minimum_notional()
    );
    assert!(
        notional < wide_model.minimum_notional(),
        "the premise: this order is too small for the wide venue ({} needed)",
        wide_model.minimum_notional()
    );

    let autonomy = AutonomyController::new();
    assert!(
        autonomy.level().executes(),
        "the premise: the default autonomy level executes, so a refusal below is the \
         feasibility gate's and not the autonomy gate's"
    );

    let mut tight_broker = SimulatedBroker::new(SimulationSettings::frictionless(), 11);
    // The order manager keys its feasibility models on the *broker's* name —
    // the string a refusal and a fill are both charged to — so the model is
    // installed under the name of the broker the order actually reaches.
    let mut admitted_manager = OrderManager::new(PreTradeChecker::new(limits()))
        .with_venue_feasibility(tight_broker.name(), tight_model);
    let accepted = admitted_manager.submit(
        order("ord-tight", dec!("10"), dec!("6")),
        &mut tight_broker,
        &autonomy,
        &funded(),
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(
        accepted.accepted,
        "an order above the tight venue's floor was refused: {:?}",
        accepted.refusal.map(|reason| reason.describe())
    );

    let mut wide_broker = SimulatedBroker::new(SimulationSettings::frictionless(), 11);
    let mut refusing_manager = OrderManager::new(PreTradeChecker::new(limits()))
        .with_venue_feasibility(wide_broker.name(), wide_model);
    let refused = refusing_manager.submit(
        order("ord-wide", dec!("10"), dec!("6")),
        &mut wide_broker,
        &autonomy,
        &funded(),
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(
        !refused.accepted,
        "the same order was admitted at a venue offering 150 basis points to the mempool"
    );
    let reason = refused
        .refusal
        .expect("a refusal without a reason is not one");
    match reason {
        RefusalReason::Infeasible { gate, .. } => assert_eq!(
            gate, GATE_MINIMUM_NOTIONAL,
            "the refusal landed under a gate the centre's bounded label set does not expect"
        ),
        other => panic!("refused for the wrong reason: {}", other.describe()),
    }
    assert!(
        refused.fills.is_empty(),
        "a refused order produced {} fill(s)",
        refused.fills.len()
    );
    Ok(())
}

#[test]
fn a_venue_promoted_to_the_simulator_still_cannot_receive_an_order_at_a_live_class_broker()
-> Result<()> {
    // The §34.4 claim that matters most: promotion means eligible for the
    // simulator, and nothing else. A venue at the ceiling of this ladder is
    // still refused at a live-class broker by the step in
    // `OrderManager::submit` that this lane did not touch.
    let mut ladder = VenueLadder::new();
    let declaration = VenueDeclaration::new(
        VenueId::new("XVENUE"),
        VenueClass::CryptoExchange,
        dec!("10"),
        Duration::from_millis(50),
        ["limit".to_string()].into_iter().collect(),
    )?;
    let evidence = VenueEvidence::new()
        .with_measurement(VenueMeasurement {
            fee_bps: Some(dec!("10")),
            latency: Some(Duration::from_millis(50)),
            accepted_order_types: ["limit".to_string()].into_iter().collect(),
            rejected_order_types: BTreeSet::new(),
            observations: 40,
        })
        .with_simulation(SimulationEvidence {
            replayed_sessions: 10,
            reconciliation_breaks: 0,
            reference_clip: dec!("10"),
        });
    for _ in 0..2 {
        attempt_promotion(
            &mut ladder,
            &declaration,
            &evidence,
            VenuePromotionPolicy::default(),
            None,
            "measured read-only, then replayed and reconciled",
            start(),
        )?;
    }
    assert_eq!(
        ladder.stage_of("XVENUE"),
        VENUE_PROMOTION_CEILING,
        "the premise: the venue is as promoted as this platform can make it"
    );
    assert!(ladder.admits("XVENUE"));
    assert!(
        !VENUE_PROMOTION_CEILING.holds_capital(),
        "the ladder's ceiling is a rung that holds capital"
    );

    // And the rung above it is refused, with an approver present, on evidence
    // that passed every gate. There is no input that makes this succeed.
    let refused = attempt_promotion(
        &mut ladder,
        &declaration,
        &evidence,
        VenuePromotionPolicy::default(),
        Some("an operator".to_string()),
        "an operator asked for it",
        start(),
    );
    // Not merely that it errored, and this is a code-review finding rather
    // than caution: with the ceiling check deleted the promotion still
    // failed, because `attempt_promotion`'s trailing arm has no gate for the
    // shadow rung and refuses on that ground instead. A test asserting only
    // `is_err()` would have passed a build whose ceiling had been removed.
    // So the refusal is asserted by the reason it gives.
    let message = refused
        .err()
        .map(|error| error.message().to_string())
        .unwrap_or_default();
    assert!(
        message.contains("ADR 0003"),
        "the promotion was refused by something other than the paper-trading ceiling: {message}"
    );
    assert!(
        message.contains("autonomy ceiling is paper trading"),
        "{message}"
    );
    assert_eq!(ladder.stage_of("XVENUE"), VENUE_PROMOTION_CEILING);

    // Now the submission path, which does not consult the ladder at all and
    // refuses anyway.
    let mut live = LiveBroker::configured(
        LiveVenueConfig {
            venue: "XVENUE".to_string(),
            credential_env: "QIP_XVENUE_CREDENTIAL_FILE".to_string(),
            endpoint: "https://example.invalid".to_string(),
            account: "account-1".to_string(),
            required_autonomy: "supervised_live".to_string(),
        },
        true,
        true,
    );
    assert!(
        !live.is_simulated(),
        "the premise: this broker is live-class, or the refusal proves nothing"
    );
    let autonomy = AutonomyController::new();
    assert!(
        !autonomy.level().is_live(),
        "the premise: the platform's autonomy level is not live"
    );
    let mut manager = OrderManager::new(PreTradeChecker::new(limits()));
    let result = manager.submit(
        order("ord-live", dec!("10"), dec!("100")),
        &mut live,
        &autonomy,
        &funded(),
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(!result.accepted, "an order reached a live-class venue");
    let reason = result
        .refusal
        .expect("a refusal without a reason is not one");
    match reason {
        RefusalReason::LiveVenueBelowLiveAutonomy { level, venue } => {
            assert_eq!(venue, "XVENUE");
            assert_ne!(
                level,
                AutonomyLevel::SupervisedLive,
                "the refusal reported a live level"
            );
        }
        other => panic!(
            "a live venue was refused for something other than the live gate: {}",
            other.describe()
        ),
    }
    assert!(result.fills.is_empty());
    Ok(())
}

#[test]
fn a_decentralised_venues_contract_risk_is_its_own_exposure_axis_and_not_the_counterparty_one()
-> Result<()> {
    // §34.3's last row. A contract cannot be called, sued or asked to explain
    // itself, so netting it against counterparties would let a book held
    // entirely through one unaudited contract read as diversified across many
    // dealers. The axis a DEX venue produces is the key a limit is set on,
    // and it is not `counterparty`.
    let venue = dex_venue("XPOOL", "20", "0xpool");
    let axes = venue.exposure_axes();
    assert_eq!(
        axes.get(CONTRACT_RISK_AXIS).map(String::as_str),
        Some("0xpool"),
        "the venue produced no contract bucket to charge exposure to"
    );
    assert!(
        !axes.contains_key(qip_risk::limits::COUNTERPARTY_AXIS),
        "contract risk was filed under the counterparty axis, where it would net against \
         dealers that can actually be called"
    );
    assert_eq!(
        axes.len(),
        1,
        "the venue produced axes nobody asked for: {axes:?}"
    );

    // And the axis is the shape `OrderManager::submit` already carries into
    // pre-trade risk: a map of axis to bucket, merged with whatever else the
    // caller has. Proven by passing it through the real submission path.
    let autonomy = AutonomyController::new();
    let mut manager = OrderManager::new(PreTradeChecker::new(limits()));
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 13);
    let accepted = manager.submit(
        order("ord-axis", dec!("10"), dec!("100")),
        &mut broker,
        &autonomy,
        &funded(),
        axes.clone(),
        None,
        start(),
    );
    assert!(
        accepted.accepted,
        "an order carrying a contract axis was refused: {:?}",
        accepted.refusal.map(|reason| reason.describe())
    );
    assert_eq!(
        venue.adapter_class(),
        AdapterClass::Simulated,
        "a decentralised venue reported a class other than simulated"
    );
    Ok(())
}

#[test]
fn a_decentralised_venue_missing_any_of_the_four_pieces_is_observe_only_and_prices_nothing()
-> Result<()> {
    // §34.3's closing sentence, as a refusal a caller cannot route around.
    // The dangerous shape would be a model that answered with a price for
    // three of four pieces, because the missing piece is usually the one
    // nobody would have thought to check.
    let incomplete = DexModel::new()
        .with_pool(pool())
        .with_execution(BlockExecution::new(Duration::from_secs(12), 2).expect("a mode"))
        .with_tolerance_bps(dec!("20"))
        .expect("a tolerance");
    let venue = DexVenue::new(
        VenueId::new("XPOOL"),
        incomplete,
        dec!("1"),
        dec!("200"),
        dec!("0.01"),
    )?;
    let reason = venue
        .observe_only_reason()
        .expect("a venue owing a piece must say so");
    assert!(reason.contains("contract_risk"), "{reason}");
    assert!(
        venue.quote(dec!("100")).is_err(),
        "an observe-only venue priced a trade"
    );
    assert!(
        venue.feasibility_model(dec!("100")).is_err(),
        "an observe-only venue produced a feasibility model"
    );

    // The premise: the same venue with the flag present does all three, so
    // the refusals above are about the missing piece and not about a venue
    // that never works.
    let complete = dex_venue("XPOOL", "20", "0xpool");
    assert_eq!(complete.observe_only_reason(), None);
    assert!(complete.quote(dec!("100")).is_ok());
    assert!(complete.feasibility_model(dec!("100")).is_ok());
    Ok(())
}

#[test]
fn the_kernels_venue_admission_review_names_a_reachable_venue_that_never_earned_a_rung()
-> Result<()> {
    // The kernel-side composition, exercised across the crate boundary it
    // spans: `qip_kernel::venue_admission` reads a ladder built by
    // `qip_lifecycle` and reports the gap. It reports and never withdraws —
    // see the module's own documentation for why an empty ladder must not be
    // allowed to withdraw a deployment's entire venue list — and this test
    // asserts both halves.
    let reachable: BTreeSet<String> = ["XKNOWN".to_string(), "XSTRANGER".to_string()]
        .into_iter()
        .collect();
    let mut ladder = VenueLadder::new();
    let declaration = VenueDeclaration::new(
        VenueId::new("XKNOWN"),
        VenueClass::CryptoExchange,
        dec!("10"),
        Duration::from_millis(50),
        ["limit".to_string()].into_iter().collect(),
    )?;
    let evidence = VenueEvidence::new()
        .with_measurement(VenueMeasurement {
            fee_bps: Some(dec!("10")),
            latency: Some(Duration::from_millis(50)),
            accepted_order_types: ["limit".to_string()].into_iter().collect(),
            rejected_order_types: BTreeSet::new(),
            observations: 40,
        })
        .with_simulation(SimulationEvidence {
            replayed_sessions: 10,
            reconciliation_breaks: 0,
            reference_clip: dec!("10"),
        });
    for _ in 0..2 {
        attempt_promotion(
            &mut ladder,
            &declaration,
            &evidence,
            VenuePromotionPolicy::default(),
            None,
            "measured, replayed and reconciled",
            start(),
        )?;
    }
    assert_eq!(
        ladder.stage_of("XKNOWN"),
        GateStage::Paper,
        "the premise: one of the two venues earned the rung"
    );

    let (summary, problems) =
        qip_kernel::venue_admission::review(&ladder, &reachable, &BTreeSet::new());
    assert_eq!(
        summary.as_deref(),
        Some("1 of 2 reachable venue(s) have cleared the simulated rung")
    );
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("XSTRANGER"), "{}", problems[0]);
    assert!(
        problems[0].contains("holds no record of it"),
        "{}",
        problems[0]
    );
    // And the distinction that keeps this a problem rather than noise: the
    // ladder holds a venue, so this one was *missed* rather than never
    // configured. An empty ladder is the platform's state today — nothing
    // declares a venue to it — and reporting that as a problem put one on
    // every cycle of every deployment, which is how an operator learns that
    // problems are noise. It is reported in the summary instead, and the
    // second half below asserts that it still reaches a surface.
    assert!(
        problems[0].contains("other venue(s)"),
        "the problem does not distinguish a missed venue from an unconfigured ladder: {}",
        problems[0]
    );
    // The restraint: the review returns strings and nothing else. There is no
    // value here a caller could mistake for an instruction to enable a venue.
    assert!(
        !problems[0].contains("enable"),
        "the review suggested enabling something: {}",
        problems[0]
    );

    // The other branch, which is the one every deployment is actually in: an
    // empty ladder raises no problem and is still said out loud. A review that
    // fell silent here would be indistinguishable from one nobody wired in.
    let (empty_summary, empty_problems) =
        qip_kernel::venue_admission::review(&VenueLadder::new(), &reachable, &BTreeSet::new());
    assert!(
        empty_problems.is_empty(),
        "an unconfigured ladder raised a problem on a cycle nobody can act on: {empty_problems:?}"
    );
    let empty_summary = empty_summary.expect("an empty ladder is reported, not passed over");
    assert!(
        empty_summary.contains("empty promotion ladder"),
        "the summary does not say the ladder is empty: {empty_summary}"
    );
    assert!(
        empty_summary.contains('2'),
        "the summary does not say how many venues stand against it: {empty_summary}"
    );
    Ok(())
}

// --- §34.4's measurement seam, across the three crates it spans -------------

const MEASURED_VENUE: &str = "XMEAS";
const MEASURED_ACCOUNT: &str = "book-under-measurement";

fn measured_object() -> ObjectId {
    ObjectId::from_string("OBJ00000000000000000000MEA")
}

/// A listed name for the measured venue. `LiquidityProfile` has no `Default`
/// on purpose: the controls that veto trading read exactly these two figures.
fn measured_instrument() -> FinancialObject {
    FinancialObject::builder(
        measured_object(),
        "MEA",
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .name("Measured instrument")
    .venue(MEASURED_VENUE)
    .price(dec!("100"))
    .lot_size(Decimal::ONE)
    .tick_size(dec!("0.01"))
    .provenance(Provenance::synthetic("qip-acceptance §34.4 seam", start()))
    .build(start())
    .expect("a structurally valid instrument")
}

fn measured_credential() -> VenueCredential {
    let enforced = requirements_of_kind(
        &standard_requirements(&VenueId::new(MEASURED_VENUE)),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    VenueCredential::satisfying(MEASURED_VENUE, MEASURED_ACCOUNT, &enforced)
        .expect("a named venue and account")
}

fn measured_order(label: &str, order_type: OrderType) -> Order {
    Order::new(
        OrderId::from_string(label),
        measured_object(),
        Side::Buy,
        Decimal::from_int(10),
        order_type,
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    )
}

#[test]
fn a_venue_that_times_itself_crosses_the_broker_port_and_earns_the_observed_rung_on_measurement()
-> Result<()> {
    // The seam §34.4 and §34.1 both waited on, end to end and across three
    // crates no one of which can see it: `qip-brokers` holds the facts,
    // `qip_execution_engine::broker::Broker::observation` is the port they
    // cross, `qip-kernel` composes them into a measurement and
    // `qip-lifecycle` judges it. Before this port existed the ladder had a
    // declaration to check and nothing to check it against, and
    // `attempt_promotion` had no caller in a binary at all.
    //
    // The venue here is one that genuinely measures: its round trip carries a
    // jitter drawn from the seed, so the figure it reports is one no reader
    // of its settings could have produced. That is what makes the comparison
    // a check rather than a number checking itself.
    let settings = ExchangeSettings {
        // The seeded coin-flip refusal carries no reason and would make this
        // test about luck rather than about measurement. The jitter stays,
        // because the jitter is the thing being measured.
        rejection_probability: 0.0,
        ..ExchangeSettings::default()
    };
    let declared_latency = settings.latency;
    let jitter = settings.latency_jitter;
    assert!(
        jitter.as_nanos() > 0,
        "the premise: this venue's round trip carries a jitter, so a measured latency is not \
         the declared one read back"
    );
    let mut exchange = SimulatedExchange::new(VenueId::new(MEASURED_VENUE), settings, 11, start());
    exchange.list(measured_instrument());
    exchange.seed_liquidity(
        &measured_object(),
        Side::Sell,
        dec!("100.00"),
        Decimal::from_int(400),
        start(),
    )?;
    exchange.bring_up(&measured_credential(), start())?;

    // A market order and a limit order, so both declared types are exercised
    // and something actually fills — a fee rate is a quotient and a venue
    // that has filled nothing has no denominator.
    let ticket = exchange.ready(start())?;
    exchange.submit_order(&ticket, &measured_order("mkt", OrderType::Market), start())?;
    let ticket = exchange.ready(start())?;
    exchange.submit_order(
        &ticket,
        &measured_order("lmt", OrderType::Limit { price: dec!("101") }),
        start(),
    )?;

    // And one order type the venue refuses *for being that type*. This is the
    // second fact the widened port carries and the one no capability message
    // can establish: the venue said it accepts market and limit, and this is
    // what it did when something else arrived.
    let ticket = exchange.ready(start())?;
    let refused = exchange.submit_order(
        &ticket,
        &measured_order("algo", OrderType::TimeWeighted { minutes: 30 }),
        start(),
    );
    assert!(
        refused.is_err(),
        "the premise: this venue refuses an execution algorithm"
    );

    // Enough answers that a latency figure has a shape. Heartbeats are
    // acknowledgements like any other instruction, and counting them is
    // honest: the round trip is the round trip.
    for _ in 0..40 {
        exchange.heartbeat(start())?;
    }

    let observation = exchange
        .observation()
        .expect("a venue that has answered reports what it saw");
    assert_eq!(observation.venue.as_str(), MEASURED_VENUE);
    let measured_latency = observation
        .observed
        .acknowledgement_latency
        .expect("this venue times its acknowledgements");
    // The assertion that distinguishes a measurement from a setting read
    // back. A venue reporting exactly its declared latency has told us
    // nothing; this one reports the jitter it actually applied.
    assert!(
        measured_latency.as_nanos() > declared_latency.as_nanos(),
        "the measured latency equals the declared one, so nothing independent crossed the port: \
         declared {} ns, measured {} ns",
        declared_latency.as_nanos(),
        measured_latency.as_nanos()
    );
    assert!(
        observation.observed.rejected_order_types.contains("twap"),
        "the type the venue refused did not cross the port: {:?}",
        observation.observed.rejected_order_types
    );
    assert!(
        observation.observed.accepted_order_types.contains("market")
            && observation.observed.accepted_order_types.contains("limit"),
        "the types the venue accepted did not cross the port: {:?}",
        observation.observed.accepted_order_types
    );
    assert!(
        observation.observed.fee_bps().is_some(),
        "the venue filled and still reports no rate, so the fee half of the port is inert"
    );

    // Now the judgement, through the production seam rather than through a
    // fixture. One rung per pass, and the rung is earned on the measurement
    // above and nothing else.
    let mut ladder = VenueLadder::new();
    let outcome = qip_kernel::venue_measurement::measure(
        &mut ladder,
        MEASURED_VENUE,
        Some(observation),
        // No replayed session in *this* test: what it is about is the
        // observed rung, and the rung above is exercised by
        // `a_venue_whose_recorded_sessions_replay_clean_reaches_the_simulator_and_stops`.
        None,
        VenuePromotionPolicy::default(),
        start(),
    );
    assert!(outcome.problems.is_empty(), "{:?}", outcome.problems);
    assert!(
        outcome.unmeasured.is_empty(),
        "a venue that reported every fact was excused as unmeasurable"
    );
    assert_eq!(
        ladder.stage_of(MEASURED_VENUE),
        GateStage::Holdout,
        "a venue that measured as it declared did not earn the observed rung"
    );

    // And the ceiling, from the same seam: a hundred further passes of the
    // same perfect evidence never reach a rung that holds capital. No
    // simulation evidence is offered here, so the venue stops one rung below
    // the ceiling — and above the ceiling there is no rung at all, which the
    // next test proves with the evidence supplied.
    for _ in 0..100 {
        let again = exchange.observation();
        qip_kernel::venue_measurement::measure(
            &mut ladder,
            MEASURED_VENUE,
            again,
            None,
            VenuePromotionPolicy::default(),
            start(),
        );
    }
    let reached = ladder.stage_of(MEASURED_VENUE);
    assert!(
        reached <= VENUE_PROMOTION_CEILING,
        "the measurement seam walked a venue past the simulator"
    );
    assert!(
        !reached.holds_capital(),
        "the rung the measurement seam reached holds capital"
    );
    assert!(
        !reached.may_reach_a_venue(),
        "the rung the measurement seam reached may reach a venue"
    );
    assert_eq!(reached, GateStage::Holdout);
    Ok(())
}
#[test]
fn a_venue_whose_recorded_sessions_replay_clean_reaches_the_simulator_and_stops() -> Result<()> {
    // The ceiling rung of §34.4's ladder, reached by evidence for the first
    // time, and then refused for ever after. Until the session recorder and
    // its replay existed, **nothing in any binary constructed a
    // `SimulationEvidence`**: the simulated rung could only refuse, so the
    // ladder's ceiling was unreachable and the suite's only proof that it was
    // a ceiling came from handing `attempt_promotion` a literal. A rung no
    // production path can reach is the same defect as a limit that cannot
    // fire.
    //
    // Four crates are in view and no one of them can see this: `qip-brokers`
    // answers the instructions, `qip-execution-engine`'s order manager issues
    // them and seals the session, `qip-kernel` replays it and composes the
    // judgement, `qip-lifecycle` judges.
    let settings = ExchangeSettings {
        // The seeded coin-flip refusal carries no reason, so an unlucky order
        // would make this test about luck. The jitter stays: it is what makes
        // the latency a measurement rather than the setting read back.
        rejection_probability: 0.0,
        ..ExchangeSettings::default()
    };
    let mut exchange = SimulatedExchange::new(VenueId::new(MEASURED_VENUE), settings, 11, start());
    exchange.list(measured_instrument());
    exchange.seed_liquidity(
        &measured_object(),
        Side::Sell,
        dec!("100.00"),
        Decimal::from_int(4_000),
        start(),
    )?;
    exchange.bring_up(&measured_credential(), start())?;

    let autonomy = AutonomyController::new();
    assert!(
        !autonomy.level().is_live(),
        "the premise: the platform's autonomy level is not live, so nothing below is about a \
         live path"
    );
    let mut manager = OrderManager::new(PreTradeChecker::new(limits()));

    // Six passes, each issuing a market order and a limit order and each
    // sealed at its own instant. Six because the simulated rung asks for
    // five, and a test supplying exactly the minimum cannot tell a gate that
    // counts from one that does not.
    for pass in 0..6i64 {
        let at = start().saturating_add(Duration::from_secs(pass + 1));
        for (label, order_type) in [
            (format!("mkt-{pass}"), OrderType::Market),
            (
                format!("lmt-{pass}"),
                OrderType::Limit { price: dec!("101") },
            ),
        ] {
            let result = manager.submit(
                measured_order(&label, order_type),
                &mut exchange,
                &autonomy,
                &funded(),
                BTreeMap::new(),
                Some(MEASURED_VENUE.to_string()),
                at,
            );
            assert!(
                result.accepted,
                "the premise: the venue took order {label}; {:?}",
                result.refusal.map(|reason| reason.describe())
            );
        }
        manager.close_sessions(at);
    }

    // Enough answers that a latency figure has a shape — the observed rung's
    // own minimum, and the rung below the one under test here.
    for _ in 0..40 {
        exchange.heartbeat(start())?;
    }

    // The recording, and what a replay of it found. Asserted before the
    // ladder is touched, because a promotion on evidence nobody looked at is
    // exactly what this row exists to stop.
    let sessions = manager.sessions(MEASURED_VENUE);
    assert_eq!(
        sessions.len(),
        6,
        "the premise: six distinct sessions were sealed, not one repeated"
    );
    let replayed = qip_kernel::session_replay::replay(&sessions, MEASURED_VENUE);
    assert!(
        replayed.problems.is_empty(),
        "the replay found something to raise: {:?}",
        replayed.problems
    );
    let evidence = replayed
        .evidence
        .expect("six sessions in which a venue filled are evidence");
    assert_eq!(evidence.replayed_sessions, 6);
    assert_eq!(
        evidence.reconciliation_breaks, 0,
        "the desk's instructions and the venue's answers disagree"
    );
    assert!(
        evidence.reference_clip > Decimal::ZERO,
        "a crossing cost measured at a clip of zero is measured at a clip nobody sent"
    );

    // Now the ladder, through the production seam, one rung per pass.
    let mut ladder = VenueLadder::new();
    let first = qip_kernel::venue_measurement::measure(
        &mut ladder,
        MEASURED_VENUE,
        exchange.observation(),
        Some(evidence),
        VenuePromotionPolicy::default(),
        start(),
    );
    assert!(first.problems.is_empty(), "{:?}", first.problems);
    assert_eq!(
        ladder.stage_of(MEASURED_VENUE),
        GateStage::Holdout,
        "the venue did not earn the observed rung on its measurement"
    );

    let second = qip_kernel::venue_measurement::measure(
        &mut ladder,
        MEASURED_VENUE,
        exchange.observation(),
        Some(evidence),
        VenuePromotionPolicy::default(),
        start(),
    );
    assert!(second.problems.is_empty(), "{:?}", second.problems);
    assert!(
        second.unmeasured.is_empty(),
        "a venue that supplied a replayed session was still excused as unmeasurable: {:?}",
        second.unmeasured
    );
    assert_eq!(
        ladder.stage_of(MEASURED_VENUE),
        VENUE_PROMOTION_CEILING,
        "the replayed session did not carry the venue to the simulated rung, so the rest of \
         this test would prove a ceiling nothing can reach"
    );

    // And the ceiling. A thousand further passes of evidence a venue could
    // only dream of — every session clean, the corpus enormous — and the rung
    // does not move, because `attempt_promotion` computes its target from the
    // rung below and refuses anything above `VENUE_PROMOTION_CEILING`. There
    // is no argument to this seam that names a rung.
    let abundant = SimulationEvidence {
        replayed_sessions: 10_000,
        reconciliation_breaks: 0,
        reference_clip: dec!("1"),
    };
    for _ in 0..1_000 {
        qip_kernel::venue_measurement::measure(
            &mut ladder,
            MEASURED_VENUE,
            exchange.observation(),
            Some(abundant),
            VenuePromotionPolicy::default(),
            start(),
        );
    }
    let reached = ladder.stage_of(MEASURED_VENUE);
    assert_eq!(
        reached, VENUE_PROMOTION_CEILING,
        "replayed evidence walked a venue off the ceiling"
    );
    assert!(
        !reached.holds_capital(),
        "the rung replayed evidence reached holds capital"
    );
    assert!(
        !reached.may_reach_a_venue(),
        "the rung replayed evidence reached may reach a venue"
    );
    Ok(())
}

#[test]
fn a_recorded_session_holding_a_fill_the_venue_did_not_call_simulated_promotes_nothing()
-> Result<()> {
    // Fail closed and fail whole. The refusal here is not a reconciliation
    // break among reconciliation breaks — it is the alarm the three
    // paper-trading layers exist to make impossible, and a ladder able to
    // weigh a live fill against clean sessions would be a fourth way in that
    // none of the three watches.
    //
    // The recording is built through the recorder rather than fabricated,
    // because `RecordedSession` has no public constructor: a caller cannot
    // hand the replay a session the platform never had.
    let mut recorder = SessionRecorder::default();
    for pass in 0..8i64 {
        let at = start().saturating_add(Duration::from_secs(pass + 1));
        let order = format!("ord-{pass}");
        recorder.instruct(RecordedInstruction {
            order_id: OrderId::from_string(&order),
            object_id: measured_object(),
            side: Side::Buy,
            quantity: Decimal::from_int(10),
            venue: MEASURED_VENUE.to_string(),
            at,
        });
        recorder.answer(
            MEASURED_VENUE,
            Fill {
                fill_id: FillId::from_string(format!("fill-{pass}")),
                order_id: OrderId::from_string(&order),
                at,
                quantity: Decimal::from_int(10),
                price: dec!("100"),
                costs: dec!("0.1"),
                venue: MEASURED_VENUE.to_string(),
                // One of eight, and it is enough.
                simulated: pass != 7,
            },
            at,
        );
        recorder.close(MEASURED_VENUE, at);
    }
    let sessions = recorder.sessions(MEASURED_VENUE);
    assert_eq!(sessions.len(), 8, "the premise: eight sessions were sealed");
    let replayed = qip_kernel::session_replay::replay(&sessions, MEASURED_VENUE);
    assert!(
        replayed.evidence.is_none(),
        "a corpus holding a fill the venue did not report as simulated produced promotion \
         evidence: {:?}",
        replayed.evidence
    );
    assert!(
        replayed
            .problems
            .iter()
            .any(|problem| problem.contains("not simulated")),
        "the live fill was discarded without being raised: {:?}",
        replayed.problems
    );

    // And the consequence at the ladder: no evidence, no rung. The venue
    // clears the rung below on measurement, so what stops it below the
    // ceiling is the missing simulation evidence and nothing else.
    let mut ladder = VenueLadder::new();
    let declaration = VenueDeclaration::new(
        VenueId::new(MEASURED_VENUE),
        VenueClass::Exchange,
        dec!("10"),
        Duration::from_millis(50),
        ["limit".to_string()].into_iter().collect(),
    )?;
    ladder.register(&declaration, start());
    let measurement = VenueMeasurement {
        fee_bps: Some(dec!("10")),
        latency: Some(Duration::from_millis(50)),
        accepted_order_types: ["limit".to_string()].into_iter().collect(),
        rejected_order_types: BTreeSet::new(),
        observations: 40,
    };
    attempt_promotion(
        &mut ladder,
        &declaration,
        &VenueEvidence::new().with_measurement(measurement.clone()),
        VenuePromotionPolicy::default(),
        None,
        "measured read-only",
        start(),
    )?;
    assert_eq!(ladder.stage_of(MEASURED_VENUE), GateStage::Holdout);
    let refused = attempt_promotion(
        &mut ladder,
        &declaration,
        &VenueEvidence::new().with_measurement(measurement),
        VenuePromotionPolicy::default(),
        None,
        "nothing replayed",
        start(),
    );
    let message = refused
        .err()
        .map(|error| error.message().to_string())
        .unwrap_or_default();
    assert!(
        message.contains("has not been traded in the simulator"),
        "the simulated rung refused for some other reason: {message}"
    );
    assert_eq!(ladder.stage_of(MEASURED_VENUE), GateStage::Holdout);
    Ok(())
}
