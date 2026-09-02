//! The arbitrage desk, installed into the assembled cell from the payload's
//! whitelist.
//!
//! The scanner was wired into the cell and no composition root gave a cell a
//! desk, because the whitelist carried strings. These tests drive the node's
//! installer with a signed payload whose whitelist carries conversions, a
//! verified grant for the desk's strategy, and the assembled cell — and
//! prove what installs and what is refused. That a payload signed before the
//! structured whitelist existed still verifies is held beside the verifier,
//! in `qip-edge/tests/whitelist.rs`.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::BookSide;
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, CycleWhitelist, EpisodicDigest, PolicyPayload, Slot,
    WhitelistedConversion,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, SystemClock, Timestamp, dec};
use qip_edge::cell::CellConfig;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::arbitrage::{ArbitrageInstaller, Installation, graph_from_whitelist};
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use std::collections::BTreeMap;
use std::sync::Arc;

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "CX";
const DESK: &str = "arbitrage-desk";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn assembled() -> NodeAssembly {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new(VENUE));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    assemble(config, features, Arc::new(SystemClock)).expect("a well-formed cell assembles")
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![VenueId::new(VENUE)],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, KEY, CELL, t(1))
}

fn conversion(
    venue: &str,
    market: &str,
    from: &str,
    to: &str,
    side: BookSide,
) -> WhitelistedConversion {
    WhitelistedConversion {
        venue: venue.to_string(),
        venue_class: VenueClass::CryptoExchange,
        market: market.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        side,
        cost_fraction: dec!("0.0004"),
    }
}

/// The ETH/BTC/USDT triangle at one venue, sized at every start.
fn triangle(venue: &str) -> CycleWhitelist {
    CycleWhitelist {
        cycles: BTreeMap::from([("eth-triangle".to_string(), "1".to_string())]),
        conversions: vec![
            conversion(venue, "ETHUSDT", "USDT", "ETH", BookSide::Ask),
            conversion(venue, "ETHBTC", "ETH", "BTC", BookSide::Bid),
            conversion(venue, "BTCUSDT", "BTC", "USDT", BookSide::Bid),
        ],
        start_sizes: BTreeMap::from([
            ("USDT".to_string(), dec!("10000")),
            ("ETH".to_string(), dec!("3.3")),
            ("BTC".to_string(), dec!("0.16")),
        ]),
    }
}

/// A payload whose capability slots are fresh (so the cell is not degraded)
/// and whose whitelist is `whitelist`, when one is given.
fn policy(
    sequence: u64,
    issued_at: Timestamp,
    fresh_capabilities: bool,
    whitelist: Option<CycleWhitelist>,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    if fresh_capabilities {
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
    }
    if let Some(whitelist) = whitelist {
        payload.cycle_whitelist = Slot::produced(whitelist, issued_at);
    }
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

fn installer() -> ArbitrageInstaller {
    ArbitrageInstaller::new(StrategyId::new(DESK), vec![VenueId::new(VENUE)])
}

#[test]
fn the_node_installs_a_desk_from_the_payloads_whitelist_once_capital_for_it_has_arrived()
-> Result<()> {
    let mut node = assembled();
    let mut installer = installer();
    assert!(
        node.cell.arbitrage().is_none(),
        "the premise is a cell with no desk"
    );

    // Neither input yet: nothing installs, and the reason is named.
    assert_eq!(
        installer.install(&mut node.cell, t(10)),
        Installation::NoWhitelist
    );

    node.cell
        .apply_policy(policy(1, t(10), true, Some(triangle(VENUE)))?, t(10))?;
    assert_eq!(
        installer.install(&mut node.cell, t(11)),
        Installation::NoEnvelope,
        "a whitelist with no grant behind it installed a desk with no capital"
    );

    installer.offer(signed_envelope(DESK)?)?;
    assert_eq!(
        installer.install(&mut node.cell, t(12)),
        Installation::Installed(3)
    );
    let desk = node.cell.arbitrage().expect("the desk is installed");
    assert_eq!(desk.strategy().as_str(), DESK);
    assert_eq!(
        desk.graph().edge_count(),
        3,
        "the three conversions are the three edges"
    );
    assert!(
        desk.graph()
            .edges()
            .iter()
            .all(|edge| edge.from.venue.as_str() == VENUE && edge.to.venue.as_str() == VENUE),
        "an edge reaches a venue other than the whitelist's"
    );
    // Spent, and a second attempt does not build a second desk.
    assert!(
        !installer.holds_envelope(),
        "the grant was kept after the desk spent it"
    );
    assert_eq!(
        installer.install(&mut node.cell, t(13)),
        Installation::AlreadyInstalled
    );
    Ok(())
}

#[test]
fn a_whitelist_naming_a_venue_outside_the_configured_list_is_refused_and_installs_nothing()
-> Result<()> {
    let mut node = assembled();
    let mut installer = installer();
    installer.offer(signed_envelope(DESK)?)?;
    // Premise: the same whitelist at the configured venue would install, so
    // what refuses below is the venue and not the shape.
    assert!(
        graph_from_whitelist(&triangle(VENUE), &[VenueId::new(VENUE)]).is_ok(),
        "the premise failed: the fixture whitelist does not build at its own venue"
    );

    let mut foreign = triangle(VENUE);
    foreign.conversions[1].venue = "ZZZ".to_string();
    node.cell
        .apply_policy(policy(1, t(10), true, Some(foreign))?, t(10))?;
    let outcome = installer.install(&mut node.cell, t(11));
    match &outcome {
        Installation::Refused(reason) => assert!(
            reason.contains("ZZZ") && reason.contains("conversion 1"),
            "the refusal names neither the venue nor the entry: {reason}"
        ),
        other => panic!("a whitelist naming an unknown venue was not refused: {other:?}"),
    }
    assert!(
        node.cell.arbitrage().is_none(),
        "a desk was installed through a refused whitelist"
    );
    // The grant is kept for a whitelist that does not name a venue this cell
    // cannot reach; refusing the whitelist is not refusing the capital.
    assert!(
        installer.holds_envelope(),
        "a refused whitelist discarded the desk's grant"
    );
    Ok(())
}

#[test]
fn a_degraded_cell_and_an_empty_whitelist_install_no_desk() -> Result<()> {
    let mut node = assembled();
    let mut installer = installer();
    installer.offer(signed_envelope(DESK)?)?;

    // A whitelist with nothing else produced: every capability is
    // unavailable, the multiplier is at its floor, and a desk would refuse
    // to scan on every pass.
    node.cell
        .apply_policy(policy(1, t(10), false, Some(triangle(VENUE)))?, t(10))?;
    assert!(
        node.cell.narrowing(t(11)).sizing_multiplier() < Decimal::ONE,
        "the premise failed: the cell is not degraded"
    );
    assert_eq!(
        installer.install(&mut node.cell, t(11)),
        Installation::Degraded
    );
    assert!(node.cell.arbitrage().is_none());

    // Fresh capabilities but a whitelist with conversions stripped: the
    // string map alone is not a graph.
    let mut bare = triangle(VENUE);
    bare.conversions.clear();
    node.cell
        .apply_policy(policy(2, t(20), true, Some(bare))?, t(20))?;
    assert_eq!(
        installer.install(&mut node.cell, t(21)),
        Installation::EmptyWhitelist
    );
    assert!(node.cell.arbitrage().is_none());

    // And a whitelist whose slot has gone stale reads as none: a desk built
    // from a whitelist the centre stopped republishing prices a graph it
    // may have withdrawn.
    node.cell
        .apply_policy(policy(3, t(30), true, Some(triangle(VENUE)))?, t(30))?;
    assert_eq!(
        installer.install(&mut node.cell, t(30 + 3600)),
        Installation::NoWhitelist,
        "a stale whitelist was read as fresh"
    );
    Ok(())
}

#[test]
fn a_grant_for_another_strategy_is_refused_by_the_installer_rather_than_held() -> Result<()> {
    let mut installer = installer();
    let outcome = installer.offer(signed_envelope("momentum-9")?);
    assert!(
        outcome.is_err(),
        "a grant for another strategy was held for the desk"
    );
    assert!(!installer.holds_envelope());
    Ok(())
}
