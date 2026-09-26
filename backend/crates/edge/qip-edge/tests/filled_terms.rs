//! A fill is journaled with the terms a ledger posts it from.
//!
//! Red-team B2: `ConfirmedFill` carried the side and the cell's `Filled`
//! entry dropped it, and nothing gave the entry a quote unit, so a ledger
//! reading the chain had a quantity and a price in no currency and no way to
//! tell a debit from a credit. These tests drive the cell through a real
//! pass and the order-entry channel and read the journal entry back: the
//! side is the side sent, the quote unit is the one the placer states or
//! absent, and the fee is absent because no venue here reports one — never
//! zero. The third test holds the signal seam: a conviction that is not a
//! number is refused where the signal enters the cell, not sized and sent
//! while the journal quietly declines to encode it.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::reflex::GATE_JOURNAL_ENCODING;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, GATE_SIGNAL_CONVICTION, PlacedOrder, Placer, PricingPolicy,
    QuoteTerms,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
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
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-filled-terms";

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

/// A two-sided book with a mid of 100.
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

fn strategy(kind: SignalKind, conviction: f64) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new("alpha"), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d("100")),
            Expr::Statistic(conviction),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn signed_envelope() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("alpha"),
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

fn cell_with(kind: SignalKind, conviction: f64) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(book()?);
    let (compiled, program) = strategy(kind, conviction)?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope()?,
        PricingPolicy::Marketable,
    )?;
    Ok(cell)
}

/// A gateway that reports only what the test says the venue did, and states
/// quote terms only when told to. `quote_unit: None` exercises the trait's
/// default: this fixture then does not override the answer, it forwards it.
#[derive(Debug, Default)]
struct TermsGateway {
    placed: Vec<(String, BookSide, Decimal, Decimal)>,
    reports: Vec<ExecutionReport>,
    quote_unit: Option<Currency>,
}

impl Placer for TermsGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed
            .push((order_id.to_string(), side, quantity, price));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }

    fn quote_terms(&self, object_id: &ObjectId, venue: &VenueId) -> Option<QuoteTerms> {
        match self.quote_unit {
            Some(quote_unit) if object_id == &object() && venue.as_str() == VENUE => {
                Some(QuoteTerms { quote_unit })
            }
            // Nothing stated: answer exactly what a gateway that never
            // overrides the method answers.
            _ => None,
        }
    }
}

/// One pass that sends exactly one order, then the venue's report that all
/// of it filled, confirmed through the order-entry channel.
fn send_and_fill(cell: &mut Cell, gateway: &mut TermsGateway) -> Result<PlacedOrder> {
    let report = cell.work(t(50), gateway)?;
    assert!(
        report.refusals.is_empty(),
        "the premise is a pass that refuses nothing: {:?}",
        report.refusals
    );
    let order = report
        .orders
        .first()
        .cloned()
        .ok_or_else(|| Error::not_found("an order from a cell that signalled"))?;
    assert_eq!(gateway.placed.len(), 1, "the premise is one order sent");
    gateway.reports.push(ExecutionReport {
        order_id: order.order_id.clone(),
        venue: venue(),
        quantity: order.quantity,
        price: order.price,
        at: t(55),
    });
    let confirmed = cell.confirm_execution_reports(gateway, t(56));
    assert_eq!(confirmed.len(), 1, "the premise is one confirmed fill");
    Ok(order)
}

/// The side, quote unit and fee of every `Filled` entry, in journal order.
type Terms = (Option<BookSide>, Option<String>, Option<String>);

fn filled_terms(cell: &Cell) -> Vec<Terms> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Filled {
                side,
                quote_unit,
                fee,
                ..
            } => Some((*side, quote_unit.clone(), fee.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_confirmed_sell_is_journaled_with_its_side_and_the_listings_quote_unit() -> Result<()> {
    // A sell, because a buy is the side a hard-coded default would write:
    // an entry that always said `Ask` would pass a test that only bought.
    let mut cell = cell_with(SignalKind::Exit, 0.5)?;
    let pounds = Currency::parse("GBP")?;
    let mut gateway = TermsGateway {
        quote_unit: Some(pounds),
        ..TermsGateway::default()
    };
    let order = send_and_fill(&mut cell, &mut gateway)?;
    assert_eq!(
        gateway.placed[0].1,
        BookSide::Bid,
        "the premise is an order sent hitting the bid, which is a sell"
    );
    assert_eq!(order.side, BookSide::Bid);

    let terms = filled_terms(&cell);
    assert_eq!(terms.len(), 1, "the fill did not reach the journal");
    assert_eq!(
        terms[0].0,
        Some(BookSide::Bid),
        "the journal does not carry the side that filled, so a ledger cannot tell the sale \
         from a purchase"
    );
    assert_eq!(
        terms[0].1.as_deref(),
        Some("GBP"),
        "the journal does not carry the quote unit the placer stated"
    );
    assert_eq!(
        terms[0].2, None,
        "a fee nobody reported was journaled; absent is unreported, and a zero here would be \
         a venue fact the cell invented"
    );
    assert_eq!(cell.journal().verify(), Ok(()), "the chain does not verify");
    Ok(())
}

#[test]
fn a_placer_that_states_no_quote_terms_leaves_the_fill_without_a_quote_unit_rather_than_guessing()
-> Result<()> {
    let mut cell = cell_with(SignalKind::Enter, 0.5)?;
    let mut gateway = TermsGateway::default();
    assert_eq!(
        gateway.quote_terms(&object(), &venue()),
        None,
        "the premise is a placer that states no terms"
    );
    let order = send_and_fill(&mut cell, &mut gateway)?;
    assert_eq!(order.side, BookSide::Ask, "the premise is a buy");

    let terms = filled_terms(&cell);
    assert_eq!(terms.len(), 1, "the fill did not reach the journal");
    // The side is still written: it is the cell's own record of what it
    // sent and does not depend on anything the placer states.
    assert_eq!(terms[0].0, Some(BookSide::Ask));
    assert_eq!(
        terms[0].1, None,
        "a quote unit nobody stated was journaled, so the fill's price now reads as a number in \
         a currency the cell assumed"
    );
    assert_eq!(terms[0].2, None, "a fee nobody reported was journaled");

    // And on the wire the key is absent, not `null`: the chain hashes the
    // serialised entry, and an absent term must read back as absent.
    let entry = cell
        .journal()
        .entries()
        .iter()
        .find(|entry| matches!(entry.decision, Decision::Filled { .. }))
        .ok_or_else(|| Error::not_found("the filled entry"))?;
    let json =
        serde_json::to_value(&entry.decision).map_err(|error| Error::invalid(error.to_string()))?;
    let body = &json["Filled"];
    assert!(body.is_object(), "the premise is a Filled body: {json}");
    assert!(
        body.get("quote_unit").is_none(),
        "an unstated quote unit was serialised: {body}"
    );
    assert!(
        body.get("fee").is_none(),
        "an unreported fee was serialised: {body}"
    );
    assert_eq!(cell.journal().verify(), Ok(()), "the chain does not verify");
    Ok(())
}

#[test]
fn a_signal_with_a_non_finite_conviction_is_refused_and_journaled_rather_than_hashed() -> Result<()>
{
    // The premise, on an identical cell: with a finite conviction this
    // strategy fires and an order goes out. Without it, an empty report
    // below would say nothing about the check.
    let mut control = cell_with(SignalKind::Enter, 0.5)?;
    let mut control_gateway = TermsGateway::default();
    let control_report = control.work(t(50), &mut control_gateway)?;
    assert_eq!(
        control_report.signals.len(),
        1,
        "the premise failed: the strategy does not fire with a finite conviction: {:?}",
        control_report.refusals
    );
    assert_eq!(
        control_gateway.placed.len(),
        1,
        "the premise failed: no order"
    );

    // `Conviction::new` clamps, and `NaN` survives a clamp.
    let mut cell = cell_with(SignalKind::Enter, f64::NAN)?;
    let mut gateway = TermsGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    assert!(
        report.signals.is_empty(),
        "a signal whose conviction is not a number was acted on: {:?}",
        report.signals
    );
    assert!(
        gateway.placed.is_empty(),
        "an order was sent on a conviction that is not a number"
    );
    let refused: Vec<&(String, String)> = report
        .refusals
        .iter()
        .filter(|(gate, _)| gate == GATE_SIGNAL_CONVICTION)
        .collect();
    assert_eq!(
        refused.len(),
        1,
        "the signal was not refused under its own gate: {:?}",
        report.refusals
    );
    assert!(
        refused[0].1.starts_with("strategy alpha "),
        "the refusal does not name the strategy: {}",
        refused[0].1
    );

    let decisions: Vec<&Decision> = cell
        .journal()
        .entries()
        .iter()
        .map(|entry| &entry.decision)
        .collect();
    assert!(
        !decisions
            .iter()
            .any(|decision| matches!(decision, Decision::SignalRaised { .. })),
        "a non-finite conviction reached the chain as a signal"
    );
    assert!(
        decisions.iter().any(|decision| matches!(
            decision,
            Decision::Refused { gate, .. } if gate == GATE_SIGNAL_CONVICTION
        )),
        "the refusal was not journaled"
    );
    assert!(
        !decisions.iter().any(|decision| matches!(
            decision,
            Decision::Refused { gate, .. } if gate == GATE_JOURNAL_ENCODING
        )),
        "the journal's codec caught what the signal seam should have"
    );
    assert_eq!(cell.journal().verify(), Ok(()), "the chain does not verify");
    Ok(())
}
