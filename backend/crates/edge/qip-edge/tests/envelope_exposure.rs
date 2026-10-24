//! R12-CAPITAL-4088 at the cell: the envelope's liquidity and leverage
//! bounds refuse an order the money limits admit, before any intent or
//! region hold exists, and journal it under the gate that names the bound.
//!
//! Every test drives `Cell::work` into a venue that fills what it is sent,
//! and first shows the same pass under a looser grant *does* send — so a
//! refusal below is the bound and not a fixture that refuses everything.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::{AuthorityBounds, CapitalEnvelope, ExposureBounds};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, GATE_ENVELOPE_LEVERAGE, GATE_ENVELOPE_LIQUIDITY,
    GATE_ENVELOPE_PRODUCT, GATE_ENVELOPE_UNBOUNDED, Placer, PricingPolicy, WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::BTreeMap;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const ALPHA: &str = "alpha";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-exposure-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-exposure-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// Quote ACME at `bid` / `ask`, `size` deep on each side.
fn quote(cell: &mut Cell, bid: &str, ask: &str, size: &str) -> Result<()> {
    let mut state = VenueState::aggregated(acme(), venue(), VenueStatus::Open);
    for (index, (side, price)) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            acme(),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    cell.track(state);
    Ok(())
}

fn firing(kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(ALPHA), acme(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A grant of `gross` (order limit `order`, loss limit `loss`), with
/// `exposure` signed into it when given.
fn grant(
    gross: &str,
    order: &str,
    loss: &str,
    exposure: Option<ExposureBounds>,
) -> Result<VerifiedEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new(ALPHA),
        CELL,
        d(gross),
        d(order),
        d(loss),
        vec![venue()],
        t(0),
        t(3600),
        "alice@example.com",
        "",
    )?;
    let unsigned = match exposure {
        Some(bounds) => unsigned.with_exposure(bounds)?,
        None => unsigned,
    };
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

/// Fresh sizing slots, so the multiplier is one and every order is exactly
/// the size its signal asked for.
fn fresh_policy(issued_at: Timestamp) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, issued_at);
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

fn cell_with(config: CellConfig) -> Result<Cell> {
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    Ok(cell)
}

fn deploy(
    cell: &mut Cell,
    kind: SignalKind,
    size: &str,
    envelope: &VerifiedEnvelope,
) -> Result<()> {
    let (compiled, program) = firing(kind, size)?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope.clone(),
        PricingPolicy::Marketable,
    )
}

/// Fills every order it is sent, whole, at the order's own price.
#[derive(Debug, Default)]
struct FillingVenue {
    placed: usize,
    reports: Vec<ExecutionReport>,
}

impl Placer for FillingVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.placed += 1;
        self.reports.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue.clone(),
            quantity,
            price,
            at,
        });
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

/// The reasons refused under exactly `gate`.
fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(refused, _)| refused == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

/// Whether the cell's journal holds a refusal under exactly `gate`.
fn journaled(cell: &Cell, gate: &str) -> bool {
    cell.journal()
        .entries()
        .iter()
        .any(|entry| matches!(&entry.decision, Decision::Refused { gate: g, .. } if g == gate))
}

/// One pass of a ten-lot buy against a 1,000-deep book under a grant whose
/// participation bound is `bp`.
fn one_buy(bp: u32) -> Result<(Cell, FillingVenue, WorkReport)> {
    let envelope = grant(
        "1000000",
        "100000",
        "50000",
        Some(ExposureBounds::bounded(bp, Decimal::ONE, d("1000000"))?),
    )?;
    let mut cell = cell_with(CellConfig::new(CELL, REGION).with_venue(venue()))?;
    quote(&mut cell, "99", "101", "1000")?;
    deploy(&mut cell, SignalKind::Enter, "10", &envelope)?;
    let mut gateway = FillingVenue::default();
    let report = cell.work(t(10), &mut gateway)?;
    Ok((cell, gateway, report))
}

/// Mutation: in `Cell::intent_for`, never act on the exposure verdict — the
/// order goes out, and this test and both below fail.
#[test]
fn an_order_taking_more_of_the_visible_depth_than_its_grant_allows_never_reaches_the_venue()
-> Result<()> {
    // Premise: a quarter of the visible side admits ten of a thousand.
    let (_, open_gateway, open) = one_buy(2_500)?;
    assert_eq!(open_gateway.placed, 1, "premise: {:?}", open.refusals);
    assert!(refused_under(&open, GATE_ENVELOPE_LIQUIDITY).is_empty());

    // Fifty basis points of a thousand is five, and the order asks for ten.
    let (cell, gateway, report) = one_buy(50)?;
    assert_eq!(
        gateway.placed, 0,
        "the order reached the venue: {:?}",
        report.orders
    );
    let refusals = refused_under(&report, GATE_ENVELOPE_LIQUIDITY);
    assert_eq!(refusals.len(), 1, "{:?}", report.refusals);
    assert!(
        refusals[0].contains("50 bp"),
        "the refusal does not name the bound: {}",
        refusals[0]
    );
    assert!(
        journaled(&cell, GATE_ENVELOPE_LIQUIDITY),
        "the refusal is not in the journal a replay reads"
    );
    Ok(())
}

/// The leverage bound binds once a realised loss has eaten into the equity
/// the grant was backed by: the money limits still admit the next order,
/// and the bound refuses it.
///
/// A grant of 1,100 gross on 1,100 of capital, unlevered. One losing round
/// trip — buy ten at 101, then, redeployed as an exit under the same grant,
/// sell ten at 89 — realises 120 of loss, which a redeploy under the same
/// grant keeps. Redeployed to buy again, the order of ten is sized at the
/// mid of 100 — 1,000 of notional, inside the 1,100 gross and order limits
/// and the 500 loss limit, and past the 980 of equity the loss left.
///
/// Mutation: in `CapitalEnvelope::check_exposure`, measure on `capital`
/// rather than `capital - realised_loss` — the third order goes out.
#[test]
fn an_order_past_the_grants_leverage_on_the_equity_its_loss_left_is_refused() -> Result<()> {
    let envelope = grant(
        "1100",
        "1100",
        "500",
        Some(ExposureBounds::bounded(10_000, Decimal::ONE, d("1100"))?),
    )?;
    let mut cell = cell_with(CellConfig::new(CELL, REGION).with_venue(venue()))?;
    let mut gateway = FillingVenue::default();

    quote(&mut cell, "99", "101", "1000")?;
    deploy(&mut cell, SignalKind::Enter, "10", &envelope)?;
    let bought = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        bought.fills.len(),
        1,
        "premise: the buy filled: {:?}",
        bought.refusals
    );

    quote(&mut cell, "89", "91", "1000")?;
    deploy(&mut cell, SignalKind::Exit, "10", &envelope)?;
    let sold = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        sold.fills.len(),
        1,
        "premise: the sell filled: {:?}",
        sold.refusals
    );
    assert_eq!(gateway.placed, 2);
    let after = cell
        .state_delta(&sold, t(21))
        .utilisation
        .into_iter()
        .find(|entry| entry.strategy.as_str() == ALPHA)
        .map(|entry| entry.utilisation)
        .expect("alpha's utilisation");
    assert_eq!(
        (after.gross_committed, after.realised_loss),
        (d("890"), d("120")),
        "premise: the exit's gross is committed and the trip's loss is booked"
    );

    quote(&mut cell, "99", "101", "1000")?;
    deploy(&mut cell, SignalKind::Enter, "10", &envelope)?;
    let next = cell.work(t(30), &mut gateway)?;
    assert_eq!(next.signals.len(), 1, "premise: the strategy still signals");
    assert!(
        refused_under(&next, "capital").is_empty(),
        "premise: the money limits admit the order, so a refusal is the leverage bound: {:?}",
        next.refusals
    );
    assert_eq!(
        gateway.placed, 2,
        "the third order went out: {:?}",
        next.orders
    );
    let refusals = refused_under(&next, GATE_ENVELOPE_LEVERAGE);
    assert_eq!(refusals.len(), 1, "{:?}", next.refusals);
    assert!(
        refusals[0].split_whitespace().any(|word| word == "1000")
            && refusals[0].split_whitespace().any(|word| word == "980"),
        "the refusal does not name the gross and the equity: {}",
        refusals[0]
    );
    assert!(journaled(&cell, GATE_ENVELOPE_LEVERAGE));
    Ok(())
}

/// A cell that requires the bounds refuses an envelope without them, which
/// is the fail-closed reading of a grant signed over no participation and no
/// leverage; the same envelope at a cell that does not require them trades,
/// so the refusal is the requirement.
///
/// Mutation: in `exposure_verdict`, return `None` for an unbounded envelope
/// whatever `required` says — the order goes out.
#[test]
fn a_cell_requiring_exposure_bounds_refuses_an_envelope_without_them() -> Result<()> {
    let unbounded = grant("1000000", "100000", "50000", None)?;
    for (config, expected) in [
        (CellConfig::new(CELL, REGION).with_venue(venue()), 1),
        (
            CellConfig::new(CELL, REGION)
                .with_venue(venue())
                .with_exposure_bounds_required(),
            0,
        ),
    ] {
        let mut cell = cell_with(config)?;
        quote(&mut cell, "99", "101", "1000")?;
        deploy(&mut cell, SignalKind::Enter, "10", &unbounded)?;
        let mut gateway = FillingVenue::default();
        let report = cell.work(t(10), &mut gateway)?;
        assert_eq!(gateway.placed, expected, "{:?}", report.refusals);
        assert_eq!(
            refused_under(&report, GATE_ENVELOPE_UNBOUNDED).len(),
            1 - expected,
            "{:?}",
            report.refusals
        );
    }
    Ok(())
}

/// A grant carrying both blocks: exposure bounds at unit leverage and
/// authority bounds naming `classes` and `functions` at the same leverage
/// (ADR 0155 section 4 makes two different claims about one ratio a refusal).
fn authority_grant(classes: &[&str], functions: &[&str]) -> Result<VerifiedEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new(ALPHA),
        CELL,
        d("1000000"),
        d("100000"),
        d("50000"),
        vec![venue()],
        t(0),
        t(3600),
        "alice@example.com",
        "",
    )?
    .with_exposure(ExposureBounds::bounded(2_500, Decimal::ONE, d("1000000"))?)?
    .bound(AuthorityBounds::new(
        Decimal::ONE,
        classes.iter().map(|c| (*c).to_string()).collect(),
        functions.iter().map(|f| (*f).to_string()).collect(),
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

fn armed_pass(config: CellConfig, envelope: &VerifiedEnvelope) -> Result<(usize, WorkReport)> {
    let mut cell = cell_with(config)?;
    quote(&mut cell, "99", "101", "1000")?;
    deploy(&mut cell, SignalKind::Enter, "10", envelope)?;
    let mut gateway = FillingVenue::default();
    let report = cell.work(t(10), &mut gateway)?;
    Ok((gateway.placed, report))
}

/// Mutation: in `Cell::intent_for`, pass a fixed product class to the
/// authority check instead of the instrument's mapped class — the
/// out-of-class order is admitted and this test fails.
#[test]
fn an_armed_cell_admits_an_order_inside_a_bounded_envelope_and_refuses_one_outside_its_product_class_function_or_leverage_naming_which()
-> Result<()> {
    let armed_as = |class: &str| {
        CellConfig::new(CELL, REGION)
            .with_venue(venue())
            .with_product_class(acme(), class)
            .with_authority_bounds_required()
    };
    let armed = || armed_as("equity");

    // Premise: inside the bounds the order goes out, so every refusal below
    // is a bound and not an armed cell refusing everything.
    let inside = authority_grant(&["equity"], &["order_taking"])?;
    let (placed, report) = armed_pass(armed(), &inside)?;
    assert_eq!(placed, 1, "premise: {:?}", report.refusals);

    // Outside the product class: the instrument is mapped to a commodity and
    // the grant names equity only. Refused under its own gate, naming both.
    let (placed, report) = armed_pass(armed_as("commodity"), &inside)?;
    assert_eq!(placed, 0);
    let refusals = refused_under(&report, GATE_ENVELOPE_PRODUCT);
    assert_eq!(refusals.len(), 1, "{:?}", report.refusals);
    assert!(
        refusals[0].contains("commodity") && refusals[0].contains("equity"),
        "{}",
        refusals[0]
    );

    // Outside the regulated function: a net performs order taking.
    let (placed, report) = armed_pass(armed(), &authority_grant(&["equity"], &["routing"])?)?;
    assert_eq!(placed, 0, "{:?}", report.orders);
    assert!(
        report
            .refusals
            .iter()
            .any(|(_, reason)| reason.contains("order_taking")),
        "{:?}",
        report.refusals
    );

    // An instrument with no declared class is refused, never read as the
    // first class on the list.
    let (placed, report) = armed_pass(
        CellConfig::new(CELL, REGION)
            .with_venue(venue())
            .with_authority_bounds_required(),
        &inside,
    )?;
    assert_eq!(placed, 0);
    assert_eq!(refused_under(&report, GATE_ENVELOPE_PRODUCT).len(), 1);

    // The library default is unarmed: an envelope without the block trades.
    let unbounded = grant(
        "1000000",
        "100000",
        "50000",
        Some(ExposureBounds::bounded(2_500, Decimal::ONE, d("1000000"))?),
    )?;
    let (placed, report) = armed_pass(
        CellConfig::new(CELL, REGION).with_venue(venue()),
        &unbounded,
    )?;
    assert_eq!(placed, 1, "{:?}", report.refusals);
    Ok(())
}
