//! Envelope expiry, per strategy. When a strategy's capital envelope
//! expires, that strategy is refused every new intent — and the strategies
//! beside it whose envelopes are live keep trading.
//!
//! ADR 0008: every envelope expires, so a partitioned cell is bounded by time
//! as well as by size; and the bound is each envelope's, because each
//! strategy was granted its own. f7ba812 halted the whole cell when *any*
//! deployed envelope lapsed, which stopped every live strategy on one
//! strategy's clock and set `WorkReport::halted` while `Cell::is_halted`
//! stayed false — a report disagreeing with the cell it reports on, which
//! is the exact thing the acceptance chaos suite's invariant 5 refuses. The
//! two tests that commit added asserted that disagreement (`after.halted` on
//! a cell whose `is_halted` answered false); they now assert what the cell
//! does: no order after expiry, the refusal under `envelope_expiry` naming
//! the expiry, on every later pass, and a report that agrees with the cell.
//!
//! REFLEX-067's "reports halted and withdraws its resting orders" is not
//! implemented here; REFLEX.json's notes for it say what remains.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::Metrics;
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
const ENVELOPE_KEY: &[u8] = b"envelope-key-tests";
const POLICY_KEY: &[u8] = b"policy-key-tests";

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
    Decimal::parse(literal).expect("decimal")
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

fn strategy(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
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

fn envelope(strategy_id: &str, expires_at: Timestamp) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy_id),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            expires_at,
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

fn fresh_policy(version: u64, at: Timestamp) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(version, CELL, at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, at)
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

#[test]
fn when_an_envelopes_expiry_is_reached_its_strategy_is_refused_new_intents() -> Result<()> {
    // REFLEX-067's fail-closed minimum: past its envelope's expiry a strategy
    // takes no new risk, and the cell says which gate stopped it.

    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));

    cell.apply_policy(fresh_policy(1, t(5))?, t(5))?;
    cell.track(book()?);

    let (compiled, program) = strategy("alpha", SignalKind::Enter, "10")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope("alpha", t(3600))?,
        PricingPolicy::Marketable,
    )?;

    let mut gateway = PaperGateway;

    // Premise: before expiry the strategy does trade, so the silence below
    // is the envelope's and not a strategy that never would have sent.
    let before = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        before.orders.len(),
        1,
        "the strategy should place an order before expiry: {before:?}"
    );
    assert!(
        refusals_under(&before, "envelope_expiry").is_empty(),
        "a live envelope was refused: {:?}",
        before.refusals
    );

    let after = cell.work(t(3700), &mut gateway)?;
    assert!(
        after.orders.is_empty(),
        "an order was sent on an expired envelope: {after:?}"
    );
    let expired = refusals_under(&after, "envelope_expiry");
    assert_eq!(
        expired.len(),
        1,
        "the expired envelope was not what refused: {:?}",
        after.refusals
    );
    assert!(
        expired[0].contains("expired"),
        "the refusal does not name the expiry: {:?}",
        expired[0]
    );
    // And the refusal was the strategy's own: it did raise a signal, so
    // the gate judged an intent rather than a strategy that stayed quiet.
    assert!(
        after.signals.iter().any(|s| s.strategy.as_str() == "alpha"),
        "premise: alpha raised no signal: {after:?}"
    );
    assert_eq!(
        after.halted,
        cell.is_halted(),
        "the report disagrees with the cell about whether it is halted"
    );
    Ok(())
}

#[test]
fn an_expired_envelope_keeps_refusing_on_every_later_pass() -> Result<()> {
    // Expiry is checked at every use, not once on arrival: a backstop that
    // fired on one pass and then forgot would let the next pass trade.

    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));

    cell.apply_policy(fresh_policy(1, t(5))?, t(5))?;
    cell.track(book()?);

    let (compiled, program) = strategy("alpha", SignalKind::Enter, "10")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope("alpha", t(100))?,
        PricingPolicy::Marketable,
    )?;

    let mut gateway = PaperGateway;

    let live = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        live.orders.len(),
        1,
        "premise: a live envelope sent nothing"
    );

    for at in [t(150), t(200), t(7200)] {
        let pass = cell.work(at, &mut gateway)?;
        assert!(
            pass.orders.is_empty(),
            "an order was sent on an expired envelope at {at:?}: {pass:?}"
        );
        assert_eq!(
            refusals_under(&pass, "envelope_expiry").len(),
            1,
            "the expired envelope did not refuse at {at:?}: {:?}",
            pass.refusals
        );
    }
    Ok(())
}

#[test]
fn one_strategys_expired_envelope_stops_that_strategy_and_not_the_one_beside_it() -> Result<()> {
    // ADR 0008: each strategy holds its own grant, and the worst a cell does
    // is spend what somebody approved for as long as *that* grant runs. One
    // lapsed envelope is not a reason to stop a strategy whose envelope is
    // live — f7ba812 halted the whole cell on it, and this is the test that
    // would have caught it.

    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));

    cell.apply_policy(fresh_policy(1, t(5))?, t(5))?;
    cell.track(book()?);

    for (id, expires_at) in [("alpha", t(100)), ("beta", t(3600))] {
        let (compiled, program) = strategy(id, SignalKind::Enter, "10")?;
        cell.deploy_with_pricing(
            compiled,
            program,
            envelope(id, expires_at)?,
            PricingPolicy::Marketable,
        )?;
    }

    let mut gateway = PaperGateway;
    let report = cell.work(t(150), &mut gateway)?;

    assert!(
        !report.halted && !cell.is_halted(),
        "one strategy's expired envelope halted the cell: {:?}",
        report.refusals
    );
    // Both strategies fired, so what follows is a gate's decision and not a
    // strategy that was never going to ask.
    for id in ["alpha", "beta"] {
        assert!(
            report.signals.iter().any(|s| s.strategy.as_str() == id),
            "premise: {id} raised no signal: {report:?}"
        );
    }
    let expired = refusals_under(&report, "envelope_expiry");
    assert_eq!(
        expired.len(),
        1,
        "alpha's lapsed envelope was not refused exactly once: {:?}",
        report.refusals
    );
    assert_eq!(report.orders.len(), 1, "{report:?}");
    assert_eq!(
        report.orders[0]
            .contributors
            .iter()
            .map(|c| c.strategy.as_str())
            .collect::<Vec<_>>(),
        vec!["beta"],
        "the order did not carry beta alone: alpha rode it on a lapsed envelope, or beta \
         was stopped by alpha's"
    );
    Ok(())
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}
