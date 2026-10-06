//! Envelope expiry as a halt trigger. When a strategy's capital envelope
//! expires, the cell must halt and refuse new intents.
//!
//! This is REFLEX-067: when its freshness limits expire, a degraded cell must
//! exit or halt. It must stop taking new risk rather than keep trading under an
//! envelope it can no longer renew.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy};
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
fn when_an_envelopes_expiry_is_reached_the_cell_halts_and_refuses_new_intents() -> Result<()> {
    // REFLEX-067: when an envelope expires, the cell must stop trading.
    // It should halt and refuse new intents rather than continue trading
    // under an expired authorization.

    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));

    cell.apply_policy(fresh_policy(1, t(5))?, t(5))?;
    cell.track(book()?); // track() doesn't return Result

    // Deploy a strategy with an envelope that expires at t(3600).
    let (compiled, program) = strategy("alpha", SignalKind::Enter, "10")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope("alpha", t(3600))?,
        PricingPolicy::Marketable,
    )?;

    let mut gateway = PaperGateway;

    // Before expiry at t(10), the strategy should place an order.
    let before = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        before.orders.len(),
        1,
        "the strategy should place an order before expiry: {before:?}"
    );
    assert!(!before.halted, "cell should not be halted before expiry");

    // After expiry at t(3700), the cell should halt and refuse new intents
    // under "envelope_halt" gate.
    let after = cell.work(t(3700), &mut gateway)?;
    assert!(
        after.halted,
        "cell should halt when envelope expires: {after:?}"
    );
    assert!(
        after.orders.is_empty(),
        "no new orders should be placed after expiry: {after:?}"
    );

    // Check that envelope_halt refusal is recorded.
    let envelope_halt_refusals: Vec<_> = after
        .refusals
        .iter()
        .filter(|(gate, _)| gate == "envelope_halt")
        .collect();
    assert!(
        !envelope_halt_refusals.is_empty(),
        "envelope_halt refusal should be recorded: {:?}",
        after.refusals
    );
    assert!(
        envelope_halt_refusals[0].1.contains("expired"),
        "refusal reason should mention expiry"
    );

    Ok(())
}

#[test]
fn envelope_expiry_persists_across_passes() -> Result<()> {
    // Once an envelope expires, the cell should remain halted on subsequent
    // passes until the envelope is renewed (out of scope for this test) or
    // the cell is reconfigured.

    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));

    cell.apply_policy(fresh_policy(1, t(5))?, t(5))?;
    cell.track(book()?); // track() doesn't return Result

    let (compiled, program) = strategy("alpha", SignalKind::Enter, "10")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        envelope("alpha", t(100))?,
        PricingPolicy::Marketable,
    )?;

    let mut gateway = PaperGateway;

    // At t(50), envelope is still live.
    let live = cell.work(t(50), &mut gateway)?;
    assert!(!live.halted, "should not be halted while envelope is live");

    // At t(150), envelope has expired.
    let expired = cell.work(t(150), &mut gateway)?;
    assert!(expired.halted, "should halt when envelope expires");

    // At t(200), should still be halted.
    let still_expired = cell.work(t(200), &mut gateway)?;
    assert!(
        still_expired.halted,
        "should remain halted after envelope expiry"
    );

    Ok(())
}
