//! Blueprint §36.3's node-crash row, as the cell runs it.
//!
//! *"That region dark; reconciles against every venue before resuming"*, and
//! §48's degradation matrix spells out what against: *"Reconciles against
//! every venue including resting orders and quotes before resuming"*.
//!
//! # The failure these tests hold
//!
//! A cell rebuilds its books from the feed on every start and chains its
//! journal onto genesis, so a restarted process knows nothing about the
//! orders the dead one left resting at a venue. Until this existed the new
//! process simply began trading, while the venue was still holding size for
//! it that nothing in this platform could see, withdraw or attribute a fill
//! on. The ordinary drop-copy reconciler could never find it either: the
//! cell's side of that comparison was empty and stayed empty, so the two
//! records agreed on nothing and therefore agreed.
//!
//! Every test here asserts its premise first — that the cell *would* have
//! traded — because a test that arms a gate and finds no order is passed by a
//! cell that was never going to send one.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, GATE_AWAITING_RECONCILIATION, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::resume::VenueAccount;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
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

/// A strategy whose one rule always holds, so a pass raises exactly one
/// signal. What is under test is the gate, not how a strategy decides.
fn firing_strategy(id: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(d("10")),
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

/// A cell with a priced book and one strategy that always fires.
fn trading_cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope("alpha")?,
        PricingPolicy::Marketable,
    )?;
    Ok(cell)
}

#[test]
fn a_restarted_cell_forms_no_order_until_every_venue_has_answered_for_what_it_holds_open()
-> Result<()> {
    let mut cell = trading_cell()?;

    // The premise, asserted first and on its own cell: this fixture really
    // does send. Without it, "no order was placed" is passed by a cell that
    // was never going to place one.
    let mut proof = PaperGateway::default();
    let would_have = trading_cell()?.work(t(10), &mut proof)?;
    assert_eq!(
        would_have.orders.len(),
        1,
        "the premise failed: an unarmed cell placed {} orders",
        would_have.orders.len()
    );
    assert_eq!(proof.placed, 1);

    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(5))?;
    let mut gateway = PaperGateway::default();
    let refused = cell.work(t(10), &mut gateway)?;
    assert!(
        refused.orders.is_empty(),
        "a cell that has not reconciled sent {} order(s)",
        refused.orders.len()
    );
    assert!(
        refused.signals.is_empty(),
        "a cell that has not reconciled evaluated a strategy and raised {} signal(s); the pass \
         must stop before a strategy prices against a venue that may be holding size this \
         process cannot see",
        refused.signals.len()
    );
    assert_eq!(
        gateway.placed, 0,
        "the gateway was reached while reconciling"
    );
    let gates: Vec<&str> = refused
        .refusals
        .iter()
        .map(|(gate, _)| gate.as_str())
        .collect();
    assert!(
        gates.contains(&GATE_AWAITING_RECONCILIATION),
        "the pass was refused under {gates:?} rather than under the reconciliation gate"
    );

    // The venue answers with nothing open, which is the ordinary clean
    // answer, and the cell may form an order again.
    cell.observe_venue_account(VenueAccount::empty(venue(), t(11)), t(11))?;
    assert!(
        cell.awaiting_reconciliation().is_none(),
        "the last venue answered and the cell is still waiting"
    );
    let mut after = PaperGateway::default();
    let resumed = cell.work(t(12), &mut after)?;
    assert_eq!(
        resumed.orders.len(),
        1,
        "a reconciled cell placed {} orders: {:?}",
        resumed.orders.len(),
        resumed.refusals
    );
    assert_eq!(after.placed, 1);

    // And the chain says so, so the moment the cell was allowed to trade
    // again is replayable rather than only visible in a report a caller
    // happened to hold.
    let resumed_entry = cell
        .journal()
        .entries()
        .iter()
        .find_map(|entry| match &entry.decision {
            Decision::VenueReconciled {
                venue,
                pending,
                resumed,
                ..
            } => Some((venue.clone(), pending.clone(), *resumed)),
            _ => None,
        })
        .expect("the chain holds the venue that answered");
    assert_eq!(resumed_entry.0, VENUE);
    assert!(resumed_entry.1.is_empty());
    assert!(resumed_entry.2, "the entry does not say the cell resumed");
    Ok(())
}

#[test]
fn an_order_the_venue_holds_that_a_restarted_cell_never_sent_halts_it_rather_than_resuming()
-> Result<()> {
    // The order with no owner: the dead process left it resting, and the new
    // one cannot withdraw it or attribute a fill on it. Never auto-corrected
    // — §36.3's own row for a break, and §48's "human investigation".
    let mut cell = trading_cell()?;
    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(5))?;
    // Premise: the cell really holds nothing open, so the disagreement below
    // is the venue's order and not a quantity mismatch on one of its own.
    assert!(cell.open_orders().is_empty());
    assert!(!cell.is_halted());

    cell.observe_venue_account(
        VenueAccount::empty(venue(), t(11)).with_open("ORD-FROM-THE-DEAD-SESSION", d("40"))?,
        t(11),
    )?;

    assert!(
        cell.is_halted(),
        "a venue holding an order this cell never sent left it running"
    );
    let breaks = cell.reconciliation_breaks();
    assert_eq!(breaks.len(), 1, "breaks: {breaks:?}");
    assert!(
        breaks[0].contains("ORD-FROM-THE-DEAD-SESSION"),
        "the break should name the order: {}",
        breaks[0]
    );
    assert!(
        cell.awaiting_reconciliation().is_some(),
        "a disagreement cleared the venue it disagreed about"
    );
    Ok(())
}

#[test]
fn a_venue_that_says_a_different_quantity_is_open_is_a_break_rather_than_an_agreement() -> Result<()>
{
    // The other direction of the same comparison, and the one a count of
    // orders would miss: both records name the order, and they disagree
    // about how much of it is still live.
    let mut cell = trading_cell()?;
    let mut gateway = PaperGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    // Premise: the cell really holds one order open, or the comparison below
    // would be testing the unknown-order arm again.
    assert_eq!(report.orders.len(), 1);
    let open = cell.open_orders();
    assert_eq!(open.len(), 1, "the cell holds no open order to compare");
    let order_id = open[0].order_id.clone();
    let remaining = open[0].remaining();

    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(11))?;
    cell.observe_venue_account(
        VenueAccount::empty(venue(), t(12)).with_open(order_id.clone(), remaining + d("1"))?,
        t(12),
    )?;
    assert!(
        cell.is_halted(),
        "a quantity disagreement left the cell running"
    );
    let breaks = cell.reconciliation_breaks();
    assert!(
        breaks.iter().any(|detail| detail.contains(&order_id)),
        "the break should name the order: {breaks:?}"
    );
    Ok(())
}

#[test]
fn a_quote_the_venue_holds_in_the_cells_name_is_a_break_because_the_cell_keeps_no_quote()
-> Result<()> {
    // §48 asks for "resting orders and quotes". The cell keeps no quote
    // inventory, so a quote the venue holds live for it is exposure with no
    // owner in this process — and a comparison with no field for it would
    // report agreement on half the question.
    let mut cell = trading_cell()?;
    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(5))?;
    cell.observe_venue_account(VenueAccount::empty(venue(), t(11)).with_quotes(2), t(11))?;
    assert!(
        cell.is_halted(),
        "a live quote in the cell's name left it running"
    );
    let breaks = cell.reconciliation_breaks();
    assert!(
        breaks.iter().any(|detail| detail.contains("quote(s) live")),
        "the break should name the quotes: {breaks:?}"
    );
    Ok(())
}

#[test]
fn a_venue_account_offered_outside_the_resume_window_is_refused_rather_than_compared() -> Result<()>
{
    // The working set moves within a pass — an order sent, a fill booked —
    // so an account taken at some other instant would halt a healthy cell on
    // a difference that is only the clock.
    let mut cell = trading_cell()?;
    assert!(cell.awaiting_reconciliation().is_none());
    let refusal = cell
        .observe_venue_account(VenueAccount::empty(venue(), t(11)), t(11))
        .expect_err("an account outside the window is not compared");
    assert_eq!(refusal.code(), "denied");
    assert!(
        refusal
            .message()
            .contains("not reconciling before it resumes"),
        "the refusal should say why: {}",
        refusal.message()
    );
    // The half that proves it admits a good value.
    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(5))?;
    assert!(
        cell.observe_venue_account(VenueAccount::empty(venue(), t(11)), t(11))
            .is_ok()
    );
    Ok(())
}

#[test]
fn an_account_from_a_venue_this_cell_was_never_configured_for_is_refused() -> Result<()> {
    // An account from a venue the cell cannot reach says nothing about what
    // this cell left open, and accepting one would clear nothing while
    // reading as evidence.
    let mut cell = trading_cell()?;
    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(5))?;
    let refusal = cell
        .observe_venue_account(VenueAccount::empty(VenueId::new("XPAR"), t(11)), t(11))
        .expect_err("a venue the cell may not trade is not evidence about it");
    assert_eq!(refusal.code(), "invalid");
    assert!(
        cell.awaiting_reconciliation().is_some(),
        "an account from a foreign venue cleared the discipline"
    );
    Ok(())
}

#[test]
fn arming_the_discipline_twice_is_refused_so_the_venues_that_answered_are_not_forgotten()
-> Result<()> {
    let mut cell = trading_cell()?;
    cell.require_reconciliation_before_resuming("a prior session was found in the store", t(5))?;
    let refusal = cell
        .require_reconciliation_before_resuming("and again", t(6))
        .expect_err("a second arming would discard what has already been agreed");
    assert_eq!(refusal.code(), "denied");
    assert!(
        refusal.message().contains("already reconciling"),
        "the refusal should say why: {}",
        refusal.message()
    );
    Ok(())
}
