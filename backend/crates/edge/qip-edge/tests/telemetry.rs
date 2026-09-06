//! What a cell given a registry must record, proven by driving the cell.
//!
//! Every test here takes a `Cell` through the event that produces a fact — a
//! central halt, a policy going stale, a signal becoming an order, two
//! strategies cancelling, a drop-copy break — and asserts that the *specific*
//! series moved. None asserts a counter is zero after doing nothing: a
//! recording site that was never wired passes that test forever, and a
//! recording site nobody has proven fires is the same shape as a control that
//! cannot — present to a reader, silent in fact.
//!
//! The registry is the one the composition root would hand over. It is read
//! through `Metrics::snapshot`, which is what `qip-edge-node` serves at
//! `/metrics`, so what these tests see is what a scraper would.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::degradation::Capability;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, EpisodicDigest, GrantManifest, HaltCommand, PolicyPayload, Slot,
};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy, WorkReport};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::{VerifiedHalt, VerifiedPolicy};
use qip_edge::reservation::RegionTable;
use qip_edge::telemetry::{EDGE_REGION_SHARE_APPLIED, EDGE_REGION_SHARE_BOUND};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// The two labels every cell series carries, plus one more.
fn by(key: &str, value: &str) -> Labels {
    labels([("cell", CELL), ("region", REGION), (key, value)])
}

fn base() -> Labels {
    labels([("cell", CELL), ("region", REGION)])
}

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100, built from messages because there is
/// deliberately no setter that bypasses the feed.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        state.apply(&level(index as u64, *side, price, size, t(index as i64)))?;
    }
    Ok(state)
}

/// A strategy whose one rule always holds, so a pass raises exactly the
/// signal the test asked for. What is under test is what the cell records
/// about a signal, not how a strategy decides to raise one.
fn firing_strategy(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ),
    );
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// An envelope signed the way the central allocator would sign it, wide
/// enough that capital is never the gate under test.
fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

/// A gateway that accepts everything and is honest that it is simulated.
#[derive(Debug, Default)]
struct PaperGateway {
    placed: usize,
}

impl Placer for PaperGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed += 1;
        Ok(())
    }
}

/// A cell wired the way `qip-edge-node` wires it: the registry handle is
/// taken first and the cell records into that same handle.
fn wired_cell() -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    Ok((cell, metrics))
}

/// A wired cell with a priced book and the named strategies deployed.
fn trading_cell(strategies: &[(&str, SignalKind, &str)]) -> Result<(Cell, Arc<Metrics>)> {
    let (mut cell, metrics) = wired_cell()?;
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy_with_pricing(
            compiled,
            program,
            signed_envelope(id)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok((cell, metrics))
}

/// A payload whose three capability-bearing slots were produced at
/// `issued_at`, so at that instant every policy-fed capability is fresh and
/// the sizing multiplier is one.
fn fresh_policy(sequence: u64, issued_at: Timestamp) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

fn halt(issued_at: Timestamp) -> Result<VerifiedHalt> {
    let command = HaltCommand::new(CELL, issued_at, "operator halt").signed(POLICY_KEY)?;
    VerifiedHalt::verify(command, POLICY_KEY, CELL, issued_at)
}

fn work(cell: &mut Cell, at: Timestamp) -> Result<WorkReport> {
    let mut gateway = PaperGateway::default();
    cell.work(at, &mut gateway)
}

// --- the halt gauge ----------------------------------------------------------

#[test]
fn a_wired_cell_reports_not_halted_before_its_first_pass_and_a_central_halt_moves_the_gauge()
-> Result<()> {
    // The failure this prevents: a gauge written only inside `work`. A cell
    // halted by the centre before its first pass never runs one, so the
    // series would be absent — and absent reads as "running" on every chart.
    let (mut cell, metrics) = wired_cell()?;

    let before = metrics.snapshot();
    assert_eq!(
        before.gauge(names::EDGE_HALTED, &by("source", "policy")),
        Some(0.0),
        "the premise failed: a freshly wired cell published no policy-halt gauge"
    );
    assert_eq!(
        before.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(0.0),
        "the premise failed: a freshly wired cell published no kill-switch gauge"
    );

    cell.apply_halt(halt(t(10))?, t(10));
    assert!(
        cell.is_halted(),
        "the premise failed: the halt did not take"
    );
    let halted = metrics.snapshot();
    assert_eq!(
        halted.gauge(names::EDGE_HALTED, &by("source", "policy")),
        Some(1.0),
        "a central halt left the policy-halt gauge at zero"
    );
    assert_eq!(
        halted.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(0.0),
        "a central halt was attributed to the kill switch, which has a different release"
    );

    // A newer payload issued after the barrier releases it. The gauge must
    // fall, not stop being written.
    cell.apply_policy(fresh_policy(7, t(20))?, t(20))?;
    assert!(
        !cell.is_halted(),
        "the premise failed: the release did not take"
    );
    let released = metrics.snapshot();
    assert_eq!(
        released.gauge(names::EDGE_HALTED, &by("source", "policy")),
        Some(0.0),
        "a released halt still reads as halted"
    );
    assert_eq!(
        released.gauge(names::EDGE_POLICY_SEQUENCE, &base()),
        Some(7.0),
        "the applied policy sequence was not published"
    );
    Ok(())
}

#[test]
fn a_pass_while_halted_is_counted_and_refused_under_the_halt_that_stopped_it() -> Result<()> {
    // A refusal count with no pass count underneath it cannot tell "nothing
    // was refused" from "the cell never ran". The pass must be counted even
    // though it returned at the halt check.
    let (mut cell, metrics) = wired_cell()?;
    cell.apply_halt(halt(t(10))?, t(10));

    let report = work(&mut cell, t(11))?;
    assert!(
        report.halted,
        "the premise failed: the pass did not see the halt"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_WORK_PASSES, &base()),
        1,
        "a halted pass was not counted as a pass"
    );
    assert_eq!(
        snapshot.counter(names::EDGE_REFUSALS, &by("gate", "policy_halt")),
        1,
        "the halted pass was not refused under the policy-halt gate"
    );
    Ok(())
}

// --- freshness and the sizing floor -----------------------------------------

#[test]
fn a_policy_going_stale_narrows_the_cell_to_the_floor_and_the_gauges_move_with_it() -> Result<()> {
    // Three states, each asserted: no policy at all (unavailable, the 0.375
    // floor), a fresh payload (fresh, 1.0), and the same payload past its
    // window (stale, back at the floor). The failure this prevents: a gauge
    // recorded when the payload was applied rather than when the pass sized
    // against it, which would report 1.0 for as long as the payload sat
    // there going stale.
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;
    let fed = [
        Capability::CausalGraph,
        Capability::EpisodicMemory,
        Capability::BeliefState,
    ];

    work(&mut cell, t(50))?;
    let blind = metrics.snapshot();
    for capability in fed {
        assert_eq!(
            blind.gauge(
                names::EDGE_CAPABILITY_FRESHNESS,
                &by("capability", capability.as_str())
            ),
            Some(2.0),
            "{} should read unavailable with no policy applied",
            capability.as_str()
        );
    }
    assert_eq!(
        blind.gauge(names::EDGE_SIZING_MULTIPLIER, &base()),
        Some(0.375),
        "a cell with no policy should size at the 0.375 floor"
    );
    // The two capabilities the cell never measures must not appear at all:
    // `nothing_known()` calls ingestion unavailable by default, not by
    // observation, and a permanent `2` on a chart whose purpose is a `max`
    // teaches an operator to ignore the one series that pages on a real
    // narrowing.
    for unmeasured in [Capability::Ingestion, Capability::CounterfactualScoring] {
        assert_eq!(
            blind.gauge(
                names::EDGE_CAPABILITY_FRESHNESS,
                &by("capability", unmeasured.as_str())
            ),
            None,
            "{} was published although the cell never measures it",
            unmeasured.as_str()
        );
    }

    cell.apply_policy(fresh_policy(1, t(100))?, t(100))?;
    work(&mut cell, t(100))?;
    let fresh = metrics.snapshot();
    for capability in fed {
        assert_eq!(
            fresh.gauge(
                names::EDGE_CAPABILITY_FRESHNESS,
                &by("capability", capability.as_str())
            ),
            Some(0.0),
            "{} should read fresh at the instant the payload was produced",
            capability.as_str()
        );
    }
    assert_eq!(
        fresh.gauge(names::EDGE_SIZING_MULTIPLIER, &base()),
        Some(1.0),
        "the premise failed: a fresh payload did not lift the multiplier to one"
    );

    // Past `valid_for` (300 s) every slot is at best stale, whatever its
    // own TTL says. Nothing was applied; only the clock moved.
    work(&mut cell, t(500))?;
    let stale = metrics.snapshot();
    for capability in fed {
        assert_eq!(
            stale.gauge(
                names::EDGE_CAPABILITY_FRESHNESS,
                &by("capability", capability.as_str())
            ),
            Some(1.0),
            "{} should read stale once the payload's window has passed",
            capability.as_str()
        );
    }
    assert_eq!(
        stale.gauge(names::EDGE_SIZING_MULTIPLIER, &base()),
        Some(0.375),
        "a stale payload should narrow the cell back to the 0.375 floor"
    );
    Ok(())
}

// --- signals, orders, netting and crossing ----------------------------------

#[test]
fn a_signal_that_becomes_an_order_moves_the_signal_order_and_netting_series() -> Result<()> {
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;

    let report = work(&mut cell, t(50))?;
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: the cell placed no order: {:?}",
        report.refusals
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_SIGNALS_RAISED, &by("kind", "enter")),
        1,
        "the signal was not counted by kind"
    );
    assert_eq!(
        snapshot.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        1,
        "the order was not counted on the venue that received it"
    );
    let ratio = snapshot
        .histogram(names::EDGE_NETTING_RATIO, &base())
        .ok_or_else(|| Error::not_found("the netting ratio histogram"))?;
    assert_eq!(ratio.count, 1, "one pass should be one netting observation");
    assert_eq!(
        ratio.counts[0], 1,
        "a single strategy nets nothing, so the ratio is exactly 1.0 and lands in the first bucket"
    );
    Ok(())
}

#[test]
fn two_strategies_that_cancel_to_zero_count_a_cancellation_and_a_cap_refusal_and_no_order()
-> Result<()> {
    // An entry and an exit of equal size on the same instrument net to
    // nothing. Three facts must be recorded and one must not: the
    // cancellation, the cap's refusal of the cross (a full cancellation is
    // always over the forty percent cap — see `Cell::cross_internally`), no
    // order, and *no* netting-ratio observation, because the ratio is
    // unbounded there and a sentinel would be a number nobody computed.
    let (mut cell, metrics) = trading_cell(&[
        ("alpha", SignalKind::Enter, "100"),
        ("beta", SignalKind::Exit, "100"),
    ])?;

    let report = work(&mut cell, t(50))?;
    assert_eq!(
        report.cancelled.len(),
        1,
        "the premise failed: the two intents did not cancel: {:?}",
        report.refusals
    );
    assert!(
        report.orders.is_empty(),
        "a cancelled net reached the venue"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_INTENTS_CANCELLED, &base()),
        1,
        "the cancellation was not counted"
    );
    assert_eq!(
        snapshot.counter(names::EDGE_REFUSALS, &by("gate", "internal_cross_cap")),
        1,
        "the cap's refusal of the cross was not counted under its gate"
    );
    assert_eq!(
        snapshot.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        0,
        "an order was counted for a net that never reached the venue"
    );
    assert!(
        snapshot
            .histogram(names::EDGE_NETTING_RATIO, &base())
            .is_none(),
        "a pass with no net volume observed a netting ratio nobody computed"
    );
    Ok(())
}

#[test]
fn an_offsetting_portion_under_the_cap_is_counted_as_a_cross_on_the_venue_that_priced_it()
-> Result<()> {
    // One hundred against forty: forty crosses internally (under the cap,
    // since 40 * 5 < 140 * 2) and sixty goes to the venue as one order.
    let (mut cell, metrics) = trading_cell(&[
        ("alpha", SignalKind::Enter, "100"),
        ("beta", SignalKind::Exit, "40"),
    ])?;

    let report = work(&mut cell, t(50))?;
    assert_eq!(
        report.crosses.len(),
        1,
        "the premise failed: no cross was booked: {:?}",
        report.refusals
    );
    assert_eq!(
        report.orders.len(),
        1,
        "the residual did not reach the venue"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_INTERNAL_CROSSES, &by("venue", VENUE)),
        1,
        "the internal cross was not counted on the venue whose mid priced it"
    );
    assert_eq!(
        snapshot.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        1,
        "the residual order was not counted"
    );
    Ok(())
}

// --- the polled halt wire ----------------------------------------------------

#[test]
fn a_polled_halt_moves_its_own_gauge_refuses_the_pass_under_its_own_gate_and_no_payload_releases_it()
-> Result<()> {
    // §46.2's second wire, seen from the series and the chain. It must chart
    // under `source="polled"` and not under either of the other two, so an
    // incident can tell which path stopped the cell; a pass must refuse
    // under `polled_halt`; and a fresh, verified policy payload — the thing
    // that releases the *broadcast* halt — must leave it engaged, because a
    // wire the mesh can release is a wire that shares the mesh's failure.
    use qip_edge::cell::PolledHalt;
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "10")])?;
    assert!(!cell.is_halted(), "the premise is a running cell");

    cell.apply_polled_halt(PolledHalt::Engaged("drill".to_string()), t(10));
    assert!(
        cell.is_halted(),
        "the premise failed: the polled halt did not take"
    );
    let halted = metrics.snapshot();
    assert_eq!(
        halted.gauge(names::EDGE_HALTED, &by("source", "polled")),
        Some(1.0),
        "a polled halt left its own gauge at zero"
    );
    assert_eq!(
        halted.gauge(names::EDGE_HALTED, &by("source", "policy")),
        Some(0.0),
        "a polled halt was attributed to the broadcast, which has a different release"
    );
    assert_eq!(
        halted.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(0.0),
        "a polled halt was attributed to the kill switch, which has a different release"
    );

    let report = work(&mut cell, t(11))?;
    assert!(report.halted && report.orders.is_empty());
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by("gate", "polled_halt")),
        1,
        "the halted pass was not refused under the polled-halt gate"
    );

    // A newer signed payload that is not halting: releases the broadcast
    // halt, and must not release this one.
    cell.apply_policy(fresh_policy(1, t(12))?, t(12))?;
    assert!(
        cell.is_halted(),
        "a policy payload released the polled halt, so the two wires share a release"
    );
    assert_eq!(
        metrics
            .snapshot()
            .gauge(names::EDGE_HALTED, &by("source", "polled")),
        Some(1.0),
        "the polled gauge fell on a payload that cannot release it"
    );

    cell.apply_polled_halt(PolledHalt::Absent, t(13));
    assert!(
        !cell.is_halted(),
        "an absent flag did not release the polled halt"
    );
    assert_eq!(
        metrics
            .snapshot()
            .gauge(names::EDGE_HALTED, &by("source", "polled")),
        Some(0.0),
        "a released polled halt still reads as halted"
    );
    Ok(())
}

#[test]
fn clearing_the_kill_switch_while_the_polled_flag_is_present_leaves_the_cell_halted() -> Result<()>
{
    // The third independence direction. The other two are proven above: a
    // policy payload does not release the polled halt, and the polled flag
    // does not release the others. What neither covers is the operator
    // credential that clears the kill switch — the one release that carries
    // authority — reaching past its own wire. If it did, an operator
    // clearing a drop-copy trip while the halt flag was still on the disk
    // would resume a cell somebody else had stopped by hand, and the flag
    // they were relying on would have been released by a credential that
    // never named it.
    use qip_edge::cell::PolledHalt;
    use qip_risk_engine::autonomy::OperatorIdentity;
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "10")])?;
    assert!(!cell.is_halted(), "the premise is a running cell");

    cell.autonomy_mut()
        .kill_switch_mut()
        .trip_global(t(10), "drop-copy", "a break");
    cell.apply_polled_halt(PolledHalt::Engaged("drill".to_string()), t(11));
    assert!(
        cell.autonomy().kill_switch().is_globally_tripped() && cell.polled_halt().is_some(),
        "the premise is a cell held by both wires"
    );

    let operator = OperatorIdentity::verified("alice@example.com", "hardware-key", t(12));
    cell.autonomy_mut()
        .kill_switch_mut()
        .clear_global(&operator, t(12))?;
    assert!(
        !cell.autonomy().kill_switch().is_globally_tripped(),
        "the premise failed: the credential did not clear the kill switch"
    );

    assert!(
        cell.is_halted(),
        "clearing the kill switch released the polled halt, so the two wires share a release"
    );
    assert!(
        cell.polled_halt().is_some(),
        "the polled halt's own state was cleared by a credential that never named it"
    );
    let report = work(&mut cell, t(13))?;
    assert!(
        report.halted && report.orders.is_empty(),
        "a cell whose flag is still present ran a pass"
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by("gate", "polled_halt")),
        1,
        "the pass was not refused under the wire that still holds the cell"
    );
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(0.0),
        "the cleared kill switch still charts as engaged"
    );
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "polled")),
        Some(1.0),
        "the polled halt fell on a clearance that cannot release it"
    );
    Ok(())
}

// --- reconciliation ----------------------------------------------------------

#[test]
fn a_reconciliation_break_is_counted_and_raises_the_kill_switch_gauge() -> Result<()> {
    // The venue's own account says half the order traded. That is a break,
    // the break trips the kill switch, and the kill switch stops passes —
    // so the gauge must be written by the break itself, not by the next
    // pass, which will never run.
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;
    let report = work(&mut cell, t(50))?;
    let order = report
        .orders
        .first()
        .cloned()
        .ok_or_else(|| Error::not_found("an order from a cell that signalled"))?;

    cell.observe_drop_copy(DropCopyFill {
        order_id: order.order_id.clone(),
        venue: order.venue.clone(),
        quantity: order.quantity / dec!("2"),
        price: order.price,
        at: t(55),
    });
    let breaks = cell.reconcile(t(60));
    assert_eq!(
        breaks.len(),
        1,
        "the premise failed: a half fill reconciled clean"
    );
    assert!(
        cell.is_halted(),
        "the premise failed: the break did not halt the cell"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_RECONCILIATION_BREAKS, &base()),
        1,
        "the break was not counted"
    );
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(1.0),
        "a break tripped the kill switch and the gauge did not say so"
    );

    // The pass that follows is refused under the switch, and counted.
    let halted = work(&mut cell, t(61))?;
    assert!(halted.halted);
    let after = metrics.snapshot();
    assert_eq!(
        after.counter(names::EDGE_REFUSALS, &by("gate", "kill_switch")),
        1,
        "the halted pass was not refused under the kill-switch gate"
    );
    assert_eq!(
        after.counter(names::EDGE_WORK_PASSES, &base()),
        2,
        "the halted pass was not counted"
    );
    Ok(())
}

// --- ADR 0039: the cell's share of its region's grant ------------------------

/// The operator's ceiling on every table below. High enough that what the
/// bound gauge reports is always the centre's share and never the local
/// backstop, which would make a narrowed cell and a capped one identical.
const CEILING: &str = "100000000";

/// One grant, sized exactly, so the share a manifest naming it derives is
/// this number and every assertion below is against something the test
/// computed rather than against whatever the cell happened to produce.
const GRANT: &str = "250000";

/// An envelope whose gross, per-order and loss limits are all `gross`.
///
/// The signature is deterministic over the envelope's terms, so signing the
/// same terms twice names the grant the cell already holds. That is how a
/// manifest names a grant without the test reaching inside the cell for it —
/// and it is how the centre does it too.
fn envelope_of(strategy: &str, gross: Decimal) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            gross,
            gross,
            gross,
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

fn deploy_under(cell: &mut Cell, strategy: &str, envelope: VerifiedEnvelope) -> Result<()> {
    let (compiled, program) = firing_strategy(strategy, SignalKind::Enter, "10")?;
    cell.deploy_with_pricing(compiled, program, envelope, PricingPolicy::Marketable)
}

/// A wired cell over `table`, holding a grant of `GRANT` for each named
/// strategy. Deploying before any payload arrives moves nothing on the share
/// series — `Cell::deploy` re-derives only under a payload already applied —
/// so every test below starts from a cell that has recorded no share.
fn share_cell(table: &RegionTable, strategies: &[&str]) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_region_table(table.clone());
    cell.track(book()?);
    for strategy in strategies {
        deploy_under(&mut cell, strategy, envelope_of(strategy, d(GRANT))?)?;
    }
    Ok((cell, metrics))
}

/// A verified payload for this cell whose `capital_grants` slot names
/// `grants`, or is unproduced when `None` — the centre withholding it.
fn share_policy(
    sequence: u64,
    issued_at: Timestamp,
    grants: Option<Vec<String>>,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    if let Some(live_grants) = grants {
        payload.capital_grants = Slot::produced(GrantManifest { live_grants }, issued_at);
    }
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

fn shares(metrics: &Metrics, outcome: &str) -> u64 {
    metrics
        .snapshot()
        .counter(EDGE_REGION_SHARE_APPLIED, &by("outcome", outcome))
}

fn share_bound(metrics: &Metrics) -> Option<f64> {
    metrics.snapshot().gauge(EDGE_REGION_SHARE_BOUND, &base())
}

#[test]
fn a_payload_naming_this_cells_grant_charts_the_bound_and_counts_the_share_as_applied() -> Result<()>
{
    // The failure this prevents: a bound that moves only in the journal. A
    // cell the centre has narrowed to nothing and a cell that has spent its
    // share both report a free balance of zero, and with no bound beside it
    // nothing on a chart tells those two apart.
    let table = RegionTable::unfunded(d(CEILING))?;
    let (mut cell, metrics) = share_cell(&table, &["alpha"])?;
    let signature = envelope_of("alpha", d(GRANT))?.signature().to_string();

    assert_eq!(
        share_bound(&metrics),
        None,
        "the premise failed: a cell nobody has named a share for already published a bound"
    );
    assert_eq!(
        shares(&metrics, "applied"),
        0,
        "the premise failed: a share was counted before any payload arrived"
    );

    cell.apply_policy(share_policy(1, t(10), Some(vec![signature]))?, t(10))?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(d(GRANT)),
        "the premise failed: the payload did not re-base the table to the grant it named"
    );

    assert_eq!(
        share_bound(&metrics),
        Some(d(GRANT).to_f64()),
        "the applied share left the bound uncharted"
    );
    assert_eq!(
        shares(&metrics, "applied"),
        1,
        "the applied share was not counted under its own outcome"
    );
    for other in ["rederived", "refused_lower_sequence", "withheld"] {
        assert_eq!(
            shares(&metrics, other),
            0,
            "a payload's re-base was counted as {other}"
        );
    }
    Ok(())
}

#[test]
fn a_grant_landing_after_the_payload_naming_it_raises_the_bound_and_counts_a_rederivation()
-> Result<()> {
    // A node applies a payload before it deploys the plan that payload names,
    // so the first sum over the manifest finds no grant and the table funds
    // nothing. What happens when the grant lands is invisible otherwise: the
    // bound rises with no payload behind it, and an operator reading only
    // `applied` sees a cell whose share never moved after sequence one.
    let table = RegionTable::unfunded(d(CEILING))?;
    let (mut cell, metrics) = share_cell(&table, &[])?;
    let envelope = envelope_of("alpha", d(GRANT))?;
    let signature = envelope.signature().to_string();

    cell.apply_policy(share_policy(1, t(10), Some(vec![signature]))?, t(10))?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise failed: a manifest naming a grant nobody holds yet funded the table"
    );
    assert_eq!(
        share_bound(&metrics),
        Some(0.0),
        "the premise failed: the first sum over the manifest charted no bound"
    );
    assert_eq!(
        shares(&metrics, "rederived"),
        0,
        "the premise failed: a re-derivation was counted before the grant landed"
    );

    deploy_under(&mut cell, "alpha", envelope)?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(d(GRANT)),
        "the premise failed: the landed grant did not fund the table"
    );

    assert_eq!(
        share_bound(&metrics),
        Some(d(GRANT).to_f64()),
        "the bound charted still reads the nothing the payload alone summed to"
    );
    assert_eq!(
        shares(&metrics, "rederived"),
        1,
        "the re-derivation was not counted under its own outcome"
    );
    assert_eq!(
        shares(&metrics, "applied"),
        1,
        "the re-derivation was counted as a fresh re-base, so the two cannot be told apart"
    );
    Ok(())
}

#[test]
fn a_share_offered_under_a_sequence_the_ledger_has_passed_is_counted_as_a_replay() -> Result<()> {
    // The ledger keeps the sequence guard itself so the guarantee does not
    // rest on `Cell::apply_policy` remembering to check — and here the cell's
    // own guard cannot fire, because this cell has applied no payload at all
    // and the table it was handed already carries a newer share. Without the
    // counter the refusal is one line in a journal nobody reads, and
    // something re-sending captured payloads at a cell looks like silence.
    let table = RegionTable::unfunded(d(CEILING))?;
    let held = d("500000");
    table.rebase(CELL, held, 9)?;
    let (mut cell, metrics) = share_cell(&table, &["alpha"])?;

    assert_eq!(
        shares(&metrics, "refused_lower_sequence"),
        0,
        "the premise failed: a replay was counted before one was offered"
    );
    assert_eq!(
        share_bound(&metrics),
        None,
        "the premise failed: the cell charted a bound it never applied"
    );

    let signature = envelope_of("alpha", d(GRANT))?.signature().to_string();
    cell.apply_policy(share_policy(5, t(10), Some(vec![signature]))?, t(10))?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(held),
        "the premise failed: a share under a sequence the ledger has passed moved the bound"
    );

    assert_eq!(
        shares(&metrics, "refused_lower_sequence"),
        1,
        "the replayed share was not counted under its own outcome"
    );
    assert_eq!(
        shares(&metrics, "applied"),
        0,
        "a share the ledger refused was counted as applied"
    );
    assert_eq!(
        share_bound(&metrics),
        None,
        "a refused share published a bound the ledger never took"
    );
    Ok(())
}

#[test]
fn a_payload_that_says_nothing_about_capital_is_counted_as_withheld_and_moves_no_bound()
-> Result<()> {
    // "The centre narrowed us to nothing" and "the centre has stopped saying
    // anything about capital" leave the same table and the same balance. One
    // is a plan and the other is an outage, and the only thing separating
    // them on a chart is that the second was counted.
    let table = RegionTable::unfunded(d(CEILING))?;
    let (mut cell, metrics) = share_cell(&table, &["alpha"])?;
    assert_eq!(
        shares(&metrics, "withheld"),
        0,
        "the premise failed: a withheld manifest was counted before any payload arrived"
    );

    // `fresh_policy` produces the three capability slots and leaves
    // `capital_grants` unproduced, which is the centre saying nothing.
    cell.apply_policy(fresh_policy(1, t(10))?, t(10))?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise failed: a payload carrying no manifest moved the bound"
    );

    assert_eq!(
        shares(&metrics, "withheld"),
        1,
        "a payload that said nothing about capital was not counted as withheld"
    );
    assert_eq!(
        share_bound(&metrics),
        None,
        "a withheld manifest published a bound nobody applied"
    );
    for other in ["applied", "rederived", "refused_lower_sequence"] {
        assert_eq!(
            shares(&metrics, other),
            0,
            "a withheld manifest was counted as {other}"
        );
    }

    // A cell holding no table counts nothing: it has no share for the centre
    // to withhold, and counting one would put every table-less cell in the
    // tree on a series meant to say a downlink has gone quiet about capital.
    let (mut tableless, tableless_metrics) = wired_cell()?;
    assert!(
        tableless.region_allocation_bound().is_none(),
        "the premise failed: the contrast cell holds a region table"
    );
    tableless.apply_policy(fresh_policy(1, t(10))?, t(10))?;
    assert_eq!(
        shares(&tableless_metrics, "withheld"),
        0,
        "a cell with no region table counted a share the centre withheld from nobody"
    );
    Ok(())
}
