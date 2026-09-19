//! ADR 0080 at the cell: a retired strategy's lot is unwound reduce-only,
//! against the lot the cell holds, from the applied policy's dispositions
//! slot — and refused, under one literal, whenever the centre's claim and
//! this cell's book disagree.
//!
//! The failure every test here guards is the one the register scored §35.1
//! `PARTIAL` on: a retirement was journaled at the centre, the lot was listed
//! as awaiting its unwind, and no cell ever read the instruction, so the lot
//! stayed open for ever under a strategy that could never again be funded.
//! Each test drives a real `Cell` through a real pass with a gateway that
//! reports only what the test says the venue did, and asserts what entered
//! the netting set, what reached the venue, and what the book says after.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, Dispositions, EpisodicDigest, PolicyPayload, Slot,
};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, DispositionVerdict, ExecutionReport, GATE_DISPOSITION, Placer,
    PricingPolicy, WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
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
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-disposition-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-disposition-tests";
/// The strategy the centre retires in every test here.
const RETIRED: &str = "alpha";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn retired() -> StrategyId {
    StrategyId::new(RETIRED)
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

/// 99 bid for 500, 101 offered for 400: a mid of 100 and enough at the touch
/// on both sides that nothing here is refused for depth.
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

/// A strategy that fires the same signal on every pass.
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

/// An envelope live from `t(0)` until `until`, verified at `t(1)` so a test
/// can hold one that has expired by the pass it runs.
fn envelope_until(strategy: &str, until: Timestamp) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            until,
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

fn live_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    envelope_until(strategy, t(3600))
}

/// A gateway that accepts every order and reports only what the test tells
/// it the venue did — never a fill inferred from an order.
#[derive(Debug, Default)]
struct ReportingGateway {
    reports: Vec<ExecutionReport>,
}

impl ReportingGateway {
    fn report(&mut self, order_id: &str, quantity: Decimal, price: Decimal, at: Timestamp) {
        self.reports.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue(),
            quantity,
            price,
            at,
        });
    }
}

impl Placer for ReportingGateway {
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

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

/// A payload whose three capability slots are fresh at `issued_at`, so the
/// cell sizes at full multiplier and every size a test names is the size the
/// gates judge.
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

fn dispositions(lots: &[(&str, &str)]) -> Dispositions {
    let mut unwinds: BTreeMap<StrategyId, BTreeMap<String, Decimal>> = BTreeMap::new();
    for (strategy, flatten_by) in lots {
        unwinds
            .entry(StrategyId::new(*strategy))
            .or_default()
            .insert(object().as_str().to_string(), d(flatten_by));
    }
    Dispositions { unwinds }
}

/// A fresh payload carrying a dispositions slot, signed and verified for
/// this cell.
fn policy_with_dispositions(
    sequence: u64,
    issued_at: Timestamp,
    lots: &[(&str, &str)],
) -> Result<VerifiedPolicy> {
    let mut payload = fresh_payload(sequence, issued_at);
    payload.dispositions = Slot::produced(dispositions(lots), issued_at);
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

/// A cell with a fresh policy at sequence 1, a two-sided book, and the named
/// strategies deployed with live envelopes and marketable pricing.
fn trading_cell(strategies: &[(&str, SignalKind, &str)]) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    let fresh = fresh_payload(1, t(5)).signed(POLICY_KEY)?;
    cell.apply_policy(VerifiedPolicy::verify(fresh, POLICY_KEY, CELL, t(5))?, t(5))?;
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy_with_pricing(
            compiled,
            program,
            live_envelope(id)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok((cell, metrics))
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

/// Let the retired strategy buy `size` at the venue and have the venue
/// confirm every unit, so the cell holds a long lot for it. Asserts the
/// premise every disposition test needs: the lot is on the cell's own book.
fn build_long_lot(
    cell: &mut Cell,
    gateway: &mut ReportingGateway,
    at: Timestamp,
    size: &str,
) -> Result<()> {
    let report = cell.work(at, gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "premise: the strategy's buy did not go out: {:?}",
        report.refusals
    );
    let order = &report.orders[0];
    assert_eq!(order.side, BookSide::Ask, "premise: an Enter is a buy");
    assert_eq!(order.quantity, d(size));
    gateway.report(
        &order.order_id,
        order.quantity,
        order.price,
        at.saturating_add(Duration::from_secs(1)),
    );
    let confirmed =
        cell.confirm_execution_reports(gateway, at.saturating_add(Duration::from_secs(2)));
    assert_eq!(
        confirmed.len(),
        1,
        "premise: the venue's fill was not confirmed"
    );
    assert_eq!(
        cell.strategy_lot(&retired(), &object()),
        d(size),
        "premise: the strategy's share of the confirmed fill did not reach its lot at this cell"
    );
    Ok(())
}

fn disposition_intent_entries(cell: &Cell) -> Vec<(String, String, String)> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::DispositionIntent {
                flatten_by,
                held,
                signed_size,
                ..
            } => Some((flatten_by.clone(), held.clone(), signed_size.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_retired_strategys_long_lot_is_sold_reduce_only_attributed_to_it_and_the_book_goes_flat()
-> Result<()> {
    // The whole path, end to end: a lot built at the venue, an instruction
    // from the centre to flatten it, one sell of exactly the lot carrying the
    // retired strategy and nobody else, a fill the venue confirms, and a book
    // that reads zero — after which the same instruction is refused, because
    // there is nothing left to reduce.
    let (mut cell, _) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    build_long_lot(&mut cell, &mut gateway, t(10), "100")?;

    cell.apply_policy(
        policy_with_dispositions(2, t(15), &[(RETIRED, "-100")])?,
        t(15),
    )?;
    assert_eq!(
        cell.dispositions().map(Dispositions::len),
        Some(1),
        "premise: the applied payload carries the disposition"
    );

    let report = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "one sell for the lot and nothing else was expected: {report:?}"
    );
    let order = &report.orders[0];
    assert_eq!(
        order.side,
        BookSide::Bid,
        "the unwind of a long lot went to the venue as a buy — the intent's sign is wrong"
    );
    assert_eq!(
        order.quantity,
        dec!("100"),
        "the sell is not the lot's size"
    );
    assert_eq!(
        order
            .contributors
            .iter()
            .map(|c| c.strategy.as_str())
            .collect::<Vec<_>>(),
        vec![RETIRED],
        "the unwind is not attributed to the retired strategy alone"
    );
    assert_eq!(order.contributors[0].signed_size, dec!("-100"));

    // The report line pairs the instruction with the lot and the intent.
    assert_eq!(report.dispositions.len(), 1, "{:?}", report.dispositions);
    let line = &report.dispositions[0];
    assert_eq!(
        (line.strategy.as_str(), line.flatten_by, line.held),
        (RETIRED, dec!("-100"), dec!("100"))
    );
    assert_eq!(
        line.verdict,
        DispositionVerdict::Intent {
            signed_size: dec!("-100"),
            venue: venue()
        }
    );
    // And the journal carries the three quantities a partial unwind is read
    // against, as its own kind rather than a signal nobody raised.
    assert_eq!(
        disposition_intent_entries(&cell),
        vec![(
            dec!("-100").to_string(),
            dec!("100").to_string(),
            dec!("-100").to_string()
        )]
    );

    // The venue confirms the sell; the retired strategy's lot is flat.
    gateway.report(&order.order_id, order.quantity, order.price, t(21));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(22));
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].side, BookSide::Bid);
    assert_eq!(
        confirmed[0].shares,
        vec![(retired(), dec!("100"))],
        "the fill is not attributed to the retired strategy"
    );
    assert_eq!(
        cell.strategy_lot(&retired(), &object()),
        Decimal::ZERO,
        "the lot did not go flat on the confirmed sell"
    );

    // The instruction still stands in the applied payload, and now there is
    // nothing to reduce: refused, not sold again.
    let again = cell.work(t(30), &mut gateway)?;
    assert!(
        again.orders.is_empty(),
        "a flat lot was sold again on a standing instruction: {:?}",
        again.orders
    );
    let reasons = refusals_under(&again, GATE_DISPOSITION);
    assert!(
        reasons.iter().any(|r| r.contains("holds no lot")),
        "the second pass did not refuse for holding nothing: {reasons:?}"
    );
    Ok(())
}

#[test]
fn a_disposition_for_a_lot_the_cell_does_not_hold_is_refused_under_the_literal_and_sends_nothing()
-> Result<()> {
    // The centre's attribution says the cell holds a lot; the cell's own book
    // says it holds nothing. Nothing moves, the refusal names the
    // disagreement, and it is counted under the gate literal so the series
    // an operator would chart moves.
    let (mut cell, metrics) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    assert_eq!(
        cell.strategy_lot(&retired(), &object()),
        Decimal::ZERO,
        "premise: the cell holds nothing for the strategy"
    );
    cell.apply_policy(
        policy_with_dispositions(2, t(15), &[(RETIRED, "-100")])?,
        t(15),
    )?;

    let report = cell.work(t(20), &mut gateway)?;
    assert!(
        report.orders.is_empty(),
        "an order went out for a lot the cell does not hold: {:?}",
        report.orders
    );
    assert_eq!(report.dispositions.len(), 1, "{:?}", report.dispositions);
    let line = &report.dispositions[0];
    assert_eq!(line.held, Decimal::ZERO);
    let DispositionVerdict::Refused { gate, reason } = &line.verdict else {
        panic!("a disposition for nothing became an intent: {line:?}");
    };
    assert_eq!(gate, GATE_DISPOSITION);
    assert!(
        reason.contains("holds no lot"),
        "the refusal does not say the cell holds nothing: {reason}"
    );
    // The same reason is in the pass's refusals, under the same literal,
    // which is what carries it onto the delta.
    let reasons = refusals_under(&report, GATE_DISPOSITION);
    assert!(
        reasons.iter().any(|r| r.contains("holds no lot")),
        "the refusal is not on the report under the literal: {reasons:?}"
    );
    // Two refusals under the literal this pass: the disposition itself, and
    // the retired strategy's own signal, which evaluates nothing while the
    // applied policy names it.
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", GATE_DISPOSITION)
            ])
        ),
        2,
        "the refusals did not reach qip_edge_refusals_total{{gate={GATE_DISPOSITION}}}"
    );
    assert!(
        disposition_intent_entries(&cell).is_empty(),
        "an intent was journaled for a lot the cell does not hold"
    );
    Ok(())
}

#[test]
fn a_disposition_whose_sign_would_increase_the_lot_is_refused_on_its_sign() -> Result<()> {
    // A long lot and an instruction to buy more. The centre could only send
    // that from a book that disagrees with this one, and a payload is a wire
    // that authenticates the centre and nobody else: the sign check is what
    // makes this slot unable to add risk, so it fails on the sign and not
    // on anything downstream.
    let (mut cell, _) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    build_long_lot(&mut cell, &mut gateway, t(10), "100")?;
    cell.apply_policy(
        policy_with_dispositions(2, t(15), &[(RETIRED, "50")])?,
        t(15),
    )?;

    let report = cell.work(t(20), &mut gateway)?;
    assert!(
        report.orders.is_empty(),
        "an instruction to increase a lot reached the venue: {:?}",
        report.orders
    );
    assert_eq!(report.dispositions.len(), 1);
    let line = &report.dispositions[0];
    assert_eq!((line.flatten_by, line.held), (dec!("50"), dec!("100")));
    let DispositionVerdict::Refused { gate, reason } = &line.verdict else {
        panic!("an instruction to increase a lot became an intent: {line:?}");
    };
    assert_eq!(gate, GATE_DISPOSITION);
    assert!(
        reason.contains("increases the lot"),
        "the refusal is not on the sign: {reason}"
    );
    assert_eq!(
        cell.strategy_lot(&retired(), &object()),
        dec!("100"),
        "the lot moved without a fill"
    );
    Ok(())
}

#[test]
fn a_flatten_larger_than_the_lot_sends_the_lots_size_and_not_the_instructions() -> Result<()> {
    // The centre's book is one fill behind, or ahead, of this one. Whichever
    // claim is stale, the cell may not carry the lot through flat: the size
    // is the smaller of the two, and here that is the lot.
    let (mut cell, _) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    build_long_lot(&mut cell, &mut gateway, t(10), "100")?;
    cell.apply_policy(
        policy_with_dispositions(2, t(15), &[(RETIRED, "-250")])?,
        t(15),
    )?;

    let report = cell.work(t(20), &mut gateway)?;
    assert_eq!(report.orders.len(), 1, "{report:?}");
    let order = &report.orders[0];
    assert_eq!(order.side, BookSide::Bid);
    assert_eq!(
        order.quantity,
        dec!("100"),
        "the sell is the instruction's size rather than the lot's, and would carry the lot \
         through flat"
    );
    assert_eq!(report.dispositions.len(), 1);
    let line = &report.dispositions[0];
    assert_eq!((line.flatten_by, line.held), (dec!("-250"), dec!("100")));
    assert_eq!(
        line.verdict,
        DispositionVerdict::Intent {
            signed_size: dec!("-100"),
            venue: venue()
        }
    );
    // The journal shows the cell chose the lot over the instruction, as three
    // numbers rather than one.
    assert_eq!(
        disposition_intent_entries(&cell),
        vec![(
            dec!("-250").to_string(),
            dec!("100").to_string(),
            dec!("-100").to_string()
        )]
    );
    Ok(())
}

#[test]
fn a_live_strategy_with_an_expired_envelope_is_still_refused_while_the_disposition_beside_it_goes_out()
-> Result<()> {
    // The exemption is for the disposition path and nothing else. Beta is a
    // live strategy whose envelope has lapsed; its directional intent is
    // refused under the envelope gate exactly as before, on the same pass
    // that sends alpha's envelope-less unwind.
    let (mut cell, _) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    build_long_lot(&mut cell, &mut gateway, t(10), "100")?;
    let (compiled, program) = firing_strategy("beta", SignalKind::Enter, "10")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope_until("beta", t(15))?,
        PricingPolicy::Marketable,
    )?;
    cell.apply_policy(
        policy_with_dispositions(2, t(15), &[(RETIRED, "-100")])?,
        t(15),
    )?;

    let report = cell.work(t(20), &mut gateway)?;
    // Premise: beta did raise a signal this pass, so the refusal below is
    // about its envelope and not about a strategy that never fired.
    assert!(
        report.signals.iter().any(|s| s.strategy.as_str() == "beta"),
        "premise: beta raised no signal: {report:?}"
    );
    let expired = refusals_under(&report, "envelope_expiry");
    assert_eq!(
        expired.len(),
        1,
        "beta's directional intent was not refused for its lapsed envelope: {:?}",
        report.refusals
    );
    assert_eq!(report.orders.len(), 1, "{report:?}");
    let order = &report.orders[0];
    assert_eq!(order.side, BookSide::Bid);
    assert_eq!(order.quantity, dec!("100"));
    assert_eq!(
        order
            .contributors
            .iter()
            .map(|c| c.strategy.as_str())
            .collect::<Vec<_>>(),
        vec![RETIRED],
        "beta rode the unwind's order without an envelope"
    );
    Ok(())
}

#[test]
fn a_strategy_the_applied_slot_names_raises_no_directional_intent_of_its_own() -> Result<()> {
    // Alpha still fires `Enter 100` on every pass and still holds an
    // envelope that is live. Named as retired, it evaluates nothing: were
    // its buy admitted, it would net against its own sell to zero and the
    // unwind would never reach the venue while looking, on the report, like
    // an internal cross.
    let (mut cell, _) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    build_long_lot(&mut cell, &mut gateway, t(10), "100")?;
    cell.apply_policy(
        policy_with_dispositions(2, t(15), &[(RETIRED, "-100")])?,
        t(15),
    )?;

    let report = cell.work(t(20), &mut gateway)?;
    assert!(
        report.signals.is_empty(),
        "the retired strategy raised a signal: {:?}",
        report.signals
    );
    let reasons = refusals_under(&report, GATE_DISPOSITION);
    assert!(
        reasons.iter().any(|r| r.contains("evaluates no signal")),
        "the retired strategy's signal was not refused under the literal: {reasons:?}"
    );
    assert!(
        report.cancelled.is_empty(),
        "the unwind cancelled against the strategy's own intent: {:?}",
        report.cancelled
    );
    assert_eq!(report.orders.len(), 1, "{report:?}");
    assert_eq!(report.orders[0].side, BookSide::Bid);
    assert_eq!(report.orders[0].quantity, dec!("100"));
    Ok(())
}

#[test]
fn a_payload_at_or_below_the_applied_sequence_naming_a_disposition_is_refused_before_it_is_read()
-> Result<()> {
    // The replay discipline is the payload's, and the slot inherits it: a
    // captured payload naming a disposition, re-delivered at a sequence the
    // cell has already applied, is refused whole. The cell holds no
    // disposition afterwards and the next pass unwinds nothing.
    let (mut cell, _) = trading_cell(&[(RETIRED, SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    build_long_lot(&mut cell, &mut gateway, t(10), "100")?;
    assert_eq!(
        cell.policy_sequence(),
        Some(1),
        "premise: sequence 1 is applied"
    );

    let replayed = cell.apply_policy(
        policy_with_dispositions(1, t(15), &[(RETIRED, "-100")])?,
        t(15),
    );
    let error = match replayed {
        Ok(()) => panic!("a payload at the applied sequence was applied"),
        Err(error) => error,
    };
    assert!(
        error.message().contains("not newer than the applied 1"),
        "refused for a reason other than the sequence: {}",
        error.message()
    );
    assert!(
        cell.dispositions().is_none(),
        "a refused payload's disposition is readable at the cell"
    );

    let report = cell.work(t(20), &mut gateway)?;
    assert!(
        report.dispositions.is_empty(),
        "the cell acted on a disposition from a payload it refused: {:?}",
        report.dispositions
    );
    assert!(
        report
            .orders
            .iter()
            .all(|order| order.side == BookSide::Ask),
        "a sell went out with no applied disposition: {:?}",
        report.orders
    );
    assert!(
        refusals_under(&report, GATE_DISPOSITION).is_empty(),
        "the literal fired with no disposition applied"
    );

    // And a fresh sequence carrying the same instruction is read.
    cell.apply_policy(
        policy_with_dispositions(2, t(25), &[(RETIRED, "-100")])?,
        t(25),
    )?;
    assert_eq!(cell.dispositions().map(Dispositions::len), Some(1));
    Ok(())
}

/// Every hit for `flatten_by` in the edge crate's sources reads it from the
/// applied slot — ADR 0080's "what would make this wrong" check, kept as a
/// test so a second writer of the flatten intent fails here rather than in
/// review.
#[test]
fn the_only_source_of_a_flatten_intent_in_the_cell_is_the_applied_dispositions_slot() -> Result<()>
{
    let whole = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cell.rs"))
        .map_err(|error| Error::io(format!("reading cell.rs: {error}")))?;
    // The production half only: the unit tests at the bottom of the file
    // build intents of their own and are not a path an order can take.
    let source = whole
        .split_once("#[cfg(test)]")
        .map(|(production, _)| production)
        .ok_or_else(|| Error::invalid("cell.rs has no unit-test module to split at"))?;
    let builders: Vec<&str> = source
        .lines()
        .filter(|line| line.contains("Intent::new("))
        .collect();
    // Premise: the two constructors this crate has — the signal path and the
    // disposition path. A third is a third source of intents to read.
    assert_eq!(
        builders.len(),
        2,
        "the cell builds directional intents at a number of sites other than the two ADR 0080 \
         describes; read each and say which slot it reads from: {builders:?}"
    );
    let readers: Vec<&str> = source
        .lines()
        .filter(|line| line.contains("self.dispositions()"))
        .collect();
    assert_eq!(
        readers.len(),
        1,
        "the applied slot is read at more than one site; every reader must be the one that \
         applies the sign check: {readers:?}"
    );
    Ok(())
}
