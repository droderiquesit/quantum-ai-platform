//! The feasibility gate as the cell applies it: ahead of netting, counted by
//! rule, fed from the venue model and from the policy payload's item 11.
//!
//! The pure rules are held beside the gate in `feasibility.rs`. What is held
//! here is the seam: that an infeasible intent is refused *before* the netting
//! set is built, so it never rides a feasible strategy's order to the venue;
//! that the refusal reaches the metric series under its own gate literal; and
//! that a constraint the centre ships is the one the cell judges by.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, EpisodicDigest, FeasibilityConstraints, PolicyPayload, Slot,
};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::feasibility::{Granularity, VenueModel};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
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
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
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

/// 99 bid for 500, 101 offered for 400: a mid of 100 and a known touch size
/// on each side, which is what the depth rule is judged against.
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

#[derive(Debug, Default)]
struct PaperGateway;

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
        Ok(())
    }
}

/// Lot 1, tick 0.01, no minimum, no fee — a model whose only bite is the
/// grid, so a test that names the lot rule is refused by the lot rule.
fn lot_model() -> Result<VenueModel> {
    VenueModel::new(
        VenueClass::Exchange,
        Granularity::new(dec!("1"), dec!("0.01"), Decimal::ZERO)?,
        Decimal::ZERO,
        None,
    )
}

fn trading_cell(
    model: Option<VenueModel>,
    strategies: &[(&str, SignalKind, &str)],
) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    if let Some(model) = model {
        config = config.with_feasibility(&venue(), model);
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    // With no policy the degradation table sizes at its conservative floor,
    // and a size of 10 would reach the gate as 3.75. Every size a test names
    // below is the size the gate judges, so the payload's capability slots
    // are produced and fresh.
    let fresh = fresh_payload(1, t(5)).signed(POLICY_KEY)?;
    cell.apply_policy(VerifiedPolicy::verify(fresh, POLICY_KEY, CELL, t(5))?, t(5))?;
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy(compiled, program, signed_envelope(id)?)?;
    }
    Ok((cell, metrics))
}

fn work(cell: &mut Cell) -> Result<WorkReport> {
    cell.work(t(10), &mut PaperGateway)
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

#[test]
fn an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order()
-> Result<()> {
    // The failure this prevents: the gate placed after `net`. Alpha wants 10
    // and beta wants 10.5; netted first they become one order for 20.5, the
    // venue rejects it for being off-lot, and alpha — whose intent was fine —
    // trades nothing. Judged first, beta is refused alone and alpha's order
    // goes out carrying alpha and nobody else.
    //
    // Premise first: with no model both strategies net into one order, so
    // the refusal below is the model's doing and not something else's.
    let (mut cell, _) = trading_cell(
        None,
        &[
            ("alpha", SignalKind::Enter, "10"),
            ("beta", SignalKind::Enter, "10.5"),
        ],
    )?;
    let unmodelled = work(&mut cell)?;
    assert_eq!(
        unmodelled.orders.len(),
        1,
        "the premise failed: without a model the two intents did not net: {unmodelled:?}"
    );
    assert_eq!(unmodelled.orders[0].quantity, dec!("20.5"));
    assert_eq!(unmodelled.orders[0].contributors.len(), 2);

    let (mut cell, metrics) = trading_cell(
        Some(lot_model()?),
        &[
            ("alpha", SignalKind::Enter, "10"),
            ("beta", SignalKind::Enter, "10.5"),
        ],
    )?;
    let report = work(&mut cell)?;

    let lot = refusals_under(&report, "feasibility_lot");
    assert_eq!(
        lot.len(),
        1,
        "beta was not refused by the lot rule: {report:?}"
    );
    assert!(
        lot[0].contains("10.5") && lot[0].contains("refused rather than rounded"),
        "the refusal does not name the size or the policy: {}",
        lot[0]
    );
    assert_eq!(
        report.orders.len(),
        1,
        "alpha's order did not go out: {report:?}"
    );
    let order = &report.orders[0];
    assert_eq!(order.quantity, dec!("10"), "the order carries beta's size");
    assert_eq!(
        order
            .contributors
            .iter()
            .map(|c| c.strategy.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"],
        "beta entered the netting set despite being refused"
    );

    // The series moved under the rule's own literal, which is what makes the
    // distribution of feasibility refusals chartable at all.
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", "feasibility_lot")
            ])
        ),
        1,
        "the refusal did not reach qip_edge_refusals_total{{gate=feasibility_lot}}"
    );
    Ok(())
}

#[test]
fn an_intent_larger_than_the_touch_is_refused_without_any_model_at_all() -> Result<()> {
    // Depth is the rule that needs only the book, so it must bind on a venue
    // nobody modelled. 400 is offered; 401 is asked for.
    let (mut cell, _) = trading_cell(None, &[("alpha", SignalKind::Enter, "401")])?;
    let report = work(&mut cell)?;
    assert!(
        report.orders.is_empty(),
        "an order larger than the touch went out: {report:?}"
    );
    let depth = refusals_under(&report, "feasibility_depth");
    assert_eq!(depth.len(), 1, "the depth rule did not fire: {report:?}");
    assert!(
        depth[0].contains("401") && depth[0].contains("400"),
        "{}",
        depth[0]
    );

    // And the size that fits goes out, so the rule is a bound and not a wall.
    let (mut cell, _) = trading_cell(None, &[("alpha", SignalKind::Enter, "400")])?;
    let report = work(&mut cell)?;
    assert_eq!(
        report.orders.len(),
        1,
        "a size at the touch was refused: {report:?}"
    );
    Ok(())
}

#[test]
fn a_sell_is_judged_against_the_bid_side_and_a_buy_against_the_ask() -> Result<()> {
    // The touch size read for the wrong side would let a 450 sell through
    // on the strength of the 400 offered, and refuse a 450 buy on the 500
    // bid. 450 sells fit the 500 bid; 450 buys exceed the 400 offer.
    let (mut cell, _) = trading_cell(None, &[("alpha", SignalKind::Exit, "450")])?;
    let sold = work(&mut cell)?;
    assert_eq!(
        sold.orders.len(),
        1,
        "a sell inside the bid was refused: {sold:?}"
    );
    assert_eq!(sold.orders[0].side, BookSide::Bid);

    let (mut cell, _) = trading_cell(None, &[("alpha", SignalKind::Enter, "450")])?;
    let bought = work(&mut cell)?;
    assert!(
        bought.orders.is_empty(),
        "a buy larger than the offer went out: {bought:?}"
    );
    assert_eq!(refusals_under(&bought, "feasibility_depth").len(), 1);
    Ok(())
}

/// A payload whose three capability-bearing slots were produced at
/// `issued_at`, so the sizing multiplier is one and the feasibility slot is
/// still unproduced.
fn fresh_payload(sequence: u64, issued_at: Timestamp) -> PolicyPayload {
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
    payload
}

/// A fresh payload whose feasibility slot names a minimum order for the venue.
fn policy_with_minimum(
    sequence: u64,
    issued_at: Timestamp,
    minimum: &str,
) -> Result<VerifiedPolicy> {
    let mut payload = fresh_payload(sequence, issued_at);
    let mut minimum_order = BTreeMap::new();
    minimum_order.insert(VENUE.to_string(), d(minimum));
    payload.feasibility_constraints = Slot::produced(
        FeasibilityConstraints {
            minimum_order,
            fee_floor: BTreeMap::new(),
            tick: BTreeMap::new(),
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

#[test]
fn the_policy_payloads_feasibility_slot_is_the_constraint_the_cell_judges_by() -> Result<()> {
    // Item 11 of the payload was shipped and read by nothing. This proves the
    // cell now reads it: the same intent goes out before the payload and is
    // refused under the minimum-notional gate after it, on the centre's
    // number and with no venue model configured at all.
    let (mut cell, metrics) = trading_cell(None, &[("alpha", SignalKind::Enter, "10")])?;
    let before = work(&mut cell)?;
    assert_eq!(
        before.orders.len(),
        1,
        "the premise failed: the intent was refused before any constraint applied: {before:?}"
    );

    // 10 × 100 = 1000 notional; the centre says 5000 is the least accepted.
    cell.apply_policy(policy_with_minimum(2, t(6), "5000")?, t(6))?;
    let after = work(&mut cell)?;
    assert!(
        after.orders.is_empty(),
        "the centre's minimum was not applied: {after:?}"
    );
    let minimum = refusals_under(&after, "feasibility_minimum_notional");
    assert_eq!(minimum.len(), 1, "{after:?}");
    assert!(
        minimum[0].contains("5000") && minimum[0].contains("1000"),
        "the refusal names neither the centre's minimum nor the notional: {}",
        minimum[0]
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", "feasibility_minimum_notional")
            ])
        ),
        1
    );
    Ok(())
}
