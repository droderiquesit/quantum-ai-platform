//! The deterministic Risk Gate at `Cell::send` (W1.9, R12-RISK-4093).
//!
//! Until this file the cell's order seam checked the envelope, the region
//! hold, feasibility, the mode gate and the venue class, and never the cell's
//! aggregate book: grants each inside their own envelope could together carry
//! the cell to any leverage. These tests drive a real pass through
//! `Cell::work` into a recording `Placer` and assert the gate decides whether
//! the placer is called at all — and, as premise, that the same pass with a
//! generous gate does call it, so a refusal below is the gate and not the
//! fixture.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::risk_gate::{CellRiskGate, GATE_RISK};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: Decimal::parse(price).expect("a decimal literal"),
            quantity: Decimal::parse(size).expect("a decimal literal"),
            order_count: None,
        },
        when,
        when,
    )
}

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

fn firing_strategy(id: &str, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(Decimal::parse(size).expect("a decimal literal")),
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
struct RecordingGateway {
    placed: Vec<(BookSide, Decimal)>,
}

impl Placer for RecordingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((side, quantity));
        Ok(())
    }
}

/// A cell with one strategy that buys 100 of ACME every pass, and `gate`.
fn gated_cell(gate: CellRiskGate) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_risk_gate(gate);
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha", "100")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope("alpha")?,
        PricingPolicy::Marketable,
    )?;
    Ok(cell)
}

fn leverage_gate(capital: &str, limit: f64) -> Result<CellRiskGate> {
    CellRiskGate::new(
        LimitSet::new("cell").with(Limit::new("leverage", LimitKind::MaxLeverage { limit })),
        Decimal::parse(capital).expect("a decimal literal"),
    )
}

fn risk_refusals(cell: &Cell) -> Vec<String> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == GATE_RISK => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_order_that_would_lever_the_cell_past_its_limit_never_reaches_the_placer() -> Result<()> {
    // Premise: the same pass under a generous gate sends exactly one order —
    // at most 100 at about 101 (the degradation floor of a cell with no
    // policy sizes it down), against a million of capital.
    let mut open = gated_cell(leverage_gate("1000000", 2.0)?)?;
    let mut open_gateway = RecordingGateway::default();
    let report = open.work(t(50), &mut open_gateway)?;
    assert_eq!(
        open_gateway.placed.len(),
        1,
        "premise: a generous gate admits the order: {:?}",
        report.refusals
    );
    assert!(
        risk_refusals(&open).is_empty(),
        "premise: the generous gate refused nothing"
    );

    // Capital of 25 per unit the pass sends, at a 1.0 leverage limit: an
    // order priced near 101 is about four times the capital and must not be
    // sent. Sized off the premise's own order so the degradation floor cannot
    // make the test vacuous.
    let sent = open_gateway.placed[0].1;
    assert!(sent.is_positive(), "premise: a real size was sent");
    let capital = (sent * dec!("25")).to_string();
    let mut tight = gated_cell(leverage_gate(&capital, 1.0)?)?;
    let mut tight_gateway = RecordingGateway::default();
    let outcome = tight.work(t(50), &mut tight_gateway);
    assert!(
        tight_gateway.placed.is_empty(),
        "the placer was called for an order the risk gate refuses: {:?}",
        tight_gateway.placed
    );
    let error = outcome.expect_err("a refused order fails the pass, as a live-venue refusal does");
    assert!(
        error.message().contains("risk gate"),
        "the refusal must name the gate: {}",
        error.message()
    );
    let refusals = risk_refusals(&tight);
    assert_eq!(refusals.len(), 1, "one journaled refusal under {GATE_RISK}");
    assert!(
        refusals[0].contains("max_leverage") || refusals[0].contains("leverage"),
        "the refusal names the limit that bound: {}",
        refusals[0]
    );
    Ok(())
}

#[test]
fn a_cell_gate_refuses_at_installation_a_limit_it_has_no_figure_for() {
    // A value-at-risk limit at the edge would take its silent absent-figure
    // arm and pass every order; refused when installed instead.
    let tail = LimitSet::new("cell").with(Limit::new(
        "var",
        LimitKind::MaxValueAtRisk {
            confidence: 0.99,
            limit: 0.05,
        },
    ));
    let error = CellRiskGate::new(tail, dec!("1000000")).expect_err("an unevaluable limit");
    assert!(error.message().contains("var"), "{}", error.message());
    // Premise: an evaluable set installs.
    assert!(
        CellRiskGate::new(
            LimitSet::new("cell").with(Limit::new(
                "survival",
                LimitKind::MaxSurvivalGross { limit: 1.0 }
            )),
            dec!("1000000")
        )
        .is_ok()
    );
    assert!(CellRiskGate::new(LimitSet::new("empty"), dec!("1000000")).is_err());
    assert!(
        CellRiskGate::new(
            LimitSet::new("cell").with(Limit::new(
                "leverage",
                LimitKind::MaxLeverage { limit: 1.0 }
            )),
            Decimal::ZERO
        )
        .is_err()
    );
}

#[test]
fn the_survival_ceiling_is_the_gross_the_cells_live_grants_sum_to() -> Result<()> {
    // The grant admits a million of gross. At the full grant the pass is far
    // inside it; at a bound of 25 per unit sent, over the million, the
    // ceiling is about a quarter of the order's gross. Only the bound moves,
    // so the refusal is the survival limit reading the grant.
    let survival = |limit: f64| {
        CellRiskGate::new(
            LimitSet::new("cell").with(Limit::new(
                "survival",
                LimitKind::MaxSurvivalGross { limit },
            )),
            dec!("1000000"),
        )
    };
    let mut open = gated_cell(survival(1.0)?)?;
    let mut open_gateway = RecordingGateway::default();
    open.work(t(50), &mut open_gateway)?;
    assert_eq!(open_gateway.placed.len(), 1, "premise: inside the grant");

    let sent = open_gateway.placed[0].1.to_f64();
    let mut tight = gated_cell(survival(sent * 25.0 / 1_000_000.0)?)?;
    let mut tight_gateway = RecordingGateway::default();
    assert!(tight.work(t(50), &mut tight_gateway).is_err());
    assert!(tight_gateway.placed.is_empty());
    assert_eq!(risk_refusals(&tight).len(), 1);
    Ok(())
}

#[test]
fn a_second_order_inside_the_ceiling_alone_is_refused_while_the_first_still_works() -> Result<()> {
    // The gateway here reports no fills, so the first pass's order is still
    // working when the second pass checks its own. Each is inside the
    // survival ceiling alone and the two together are not; a gate that
    // measured only filled exposure would send both, and both could fill.
    let survival = |limit: f64| {
        CellRiskGate::new(
            LimitSet::new("cell").with(Limit::new(
                "survival",
                LimitKind::MaxSurvivalGross { limit },
            )),
            dec!("1000000"),
        )
    };

    // Premise: under a generous ceiling both passes send, so nothing but the
    // ceiling stands between the second order and the venue below.
    let mut open = gated_cell(survival(1.0)?)?;
    let mut open_gateway = RecordingGateway::default();
    open.work(t(50), &mut open_gateway)?;
    open.work(t(51), &mut open_gateway)?;
    assert_eq!(
        open_gateway.placed.len(),
        2,
        "premise: a generous ceiling sends both passes' orders"
    );
    assert_eq!(
        open_gateway.placed[0].1, open_gateway.placed[1].1,
        "premise: the two passes send the same size"
    );

    // A ceiling of one and a half orders' gross at the touch of 101.
    let one = open_gateway.placed[0].1 * dec!("101");
    let mut tight = gated_cell(survival(one.to_f64() * 1.5 / 1_000_000.0)?)?;
    let mut tight_gateway = RecordingGateway::default();
    tight.work(t(50), &mut tight_gateway)?;
    assert_eq!(
        tight_gateway.placed.len(),
        1,
        "premise: the first order is inside the ceiling alone"
    );
    assert!(tight.work(t(51), &mut tight_gateway).is_err());
    assert_eq!(
        tight_gateway.placed.len(),
        1,
        "the second order reached the placer while the first still holds the ceiling"
    );
    let refusals = risk_refusals(&tight);
    assert_eq!(refusals.len(), 1, "one journaled refusal under {GATE_RISK}");
    assert!(
        refusals[0].contains("survival"),
        "the refusal names the survival limit: {}",
        refusals[0]
    );
    Ok(())
}
