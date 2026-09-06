//! The plan's strategies, deployed into the node's cell from the payload.
//!
//! `qip-edge`'s tests prove a cell runs whatever a test deploys. What only
//! the node can be wrong about is whether anything is ever deployed into a
//! *deployed* cell — and until this suite existed nothing was, so the pass
//! loop ran over an empty strategy set on every node that could have
//! existed. Each test here drives the installer with a payload built and
//! signed independently, a plan file on disk, and a grant, and asserts on
//! what the cell then does through the same pass loop the binary runs.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PlanDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::strategies::{PRICING_VARIABLE, StrategyInstaller, StrategyPlan, parse_pricing};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"strategies-test-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// A fresh directory for one test, so tests running in parallel never read
/// each other's plan.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-node-strategies-{}-{test}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A strategy whose one rule always holds, sized `size`, so a pass raises
/// exactly one signal; what is under test is the deployment, not the rule.
fn spec(id: &str, size: &str) -> StrategySpec {
    StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(Rule::new(
        "always",
        SignalKind::Enter,
        Expr::Flag(true),
        Expr::Exact(Decimal::parse(size).expect("a decimal literal")),
        Expr::Statistic(0.5),
        10,
    ))
}

/// Write a plan naming `specs` and return its path and digest.
fn plan_file(dir: &Path, specs: &[StrategySpec]) -> (PathBuf, String, u64) {
    let json = serde_json::json!({ "strategies": specs });
    let bytes = serde_json::to_vec(&json).expect("a plan serialises");
    let path = dir.join("plan.json");
    fs::write(&path, &bytes).expect("the plan is written");
    (path, StrategyPlan::digest_of(&bytes), specs.len() as u64)
}

fn grant(strategy: &str) -> Result<VerifiedEnvelope> {
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
    let signed = build(&sign_payload(KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, KEY, CELL, t(1))
}

/// A payload naming the plan by digest and count, signed and verified the
/// way the mesh's downlink verifies one.
fn policy(
    sequence: u64,
    issued_at: Timestamp,
    digest: &str,
    strategies: u64,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: digest.to_string(),
            strategies,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

/// [`policy`], with the `capital_grants` slot naming the grants for
/// `strategies` — the payload as the centre ships it once the region's
/// grant is partitioned (ADR 0039). The node applies this before the plan it
/// names deploys anything, so the share it carries is summed again when the
/// grant lands; a payload without the slot leaves the table unfunded, and
/// the plan's strategy would then fire and be refused under
/// `region_reservation`.
fn funded_policy(
    sequence: u64,
    issued_at: Timestamp,
    digest: &str,
    strategies: &[&str],
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: digest.to_string(),
            strategies: strategies.len() as u64,
        },
        issued_at,
    );
    let mut live_grants = Vec::new();
    for strategy in strategies {
        live_grants.push(grant(strategy)?.signature().to_string());
    }
    payload.capital_grants = Slot::produced(GrantManifest { live_grants }, issued_at);
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

fn node_with_feed() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    // Far above any grant this suite signs, so the plan's deployment decides.
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation)?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    Ok((node, gateway, feed))
}

fn rest(secs: i64) -> Result<PricingPolicy> {
    PricingPolicy::rest_at_mid(Duration::from_secs(secs))
}

#[test]
fn a_fresh_payload_naming_the_plan_deploys_it_and_the_next_pass_sends_an_order_the_venue_fills_later()
-> Result<()> {
    // The whole chain from the centre's decision to a confirmed fill: a
    // signed payload names the plan, the plan on disk digests to what it
    // names, a grant for the strategy has arrived, and the installer
    // deploys — then the pass loop raises the intent, gates it, nets it,
    // places it, and a later pass confirms what somebody else's flow filled.
    let dir = scratch("chain");
    let (path, digest, _count) = plan_file(&dir, &[spec(STRATEGY, "10")]);
    let (mut node, mut gateway, mut feed) = node_with_feed()?;
    let mut installer = StrategyInstaller::new(Some(path), Some(rest(60)?));
    installer.offer(grant(STRATEGY)?)?;
    // The payload carries the region share as well as the plan, in the
    // order the node's exchange applies them: payload first, deploy after.
    node.cell
        .apply_policy(funded_policy(1, t(10), &digest, &[STRATEGY])?, t(10))?;
    assert!(
        node.cell.deployed_strategies().is_empty(),
        "the premise is a cell running nothing until the installer acts"
    );
    assert_eq!(
        node.cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise is a share of nothing until the grant the manifest names is held"
    );

    let outcome = installer.install(&mut node.cell, t(10));
    assert_eq!(
        outcome.deployed,
        vec![STRATEGY.to_string()],
        "the plan's strategy was not deployed: {}",
        outcome.describe()
    );
    // The deployed grant is the one the manifest named, so the share the
    // payload carried is now summed against it and the table funds — under
    // the same sequence, without a second payload.
    assert_eq!(
        node.cell.region_allocation_bound(),
        Some(dec!("1000000")),
        "deploying the grant the manifest named did not fund the table"
    );
    assert_eq!(node.cell.region_share_sequence(), Some(1));
    assert!(outcome.refused.is_empty(), "{}", outcome.describe());
    assert_eq!(node.cell.deployed_strategies(), vec![STRATEGY]);
    assert_eq!(
        node.cell.pricing_of(STRATEGY),
        Some(rest(60)?),
        "the strategy was deployed under a pricing other than the node's"
    );
    assert!(
        installer.held_grants().is_empty(),
        "the grant was spent and still held: {:?}",
        installer.held_grants()
    );

    // Deployed twice is deployed once: the same plan on the next tick
    // touches nothing.
    let again = installer.install(&mut node.cell, t(11));
    assert!(
        again.deployed.is_empty() && again.withdrawn.is_empty(),
        "an unchanged plan redeployed or withdrew: {}",
        again.describe()
    );

    // The pass, against a two-sided book: the intent rests at the mid.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let mut stats = PassStats::default();
    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(20),
    )?;
    let PassOutcome::Ran { report, breaks, .. } = first else {
        panic!("a running node reported its pass as halted: {first:?}");
    };
    assert_eq!(
        report.signals.len(),
        1,
        "the deployed strategy did not fire"
    );
    assert!(
        report.refusals.is_empty(),
        "a fully-fed pass refused: {:?}",
        report.refusals
    );
    assert_eq!(report.orders.len(), 1, "the intent did not reach the venue");
    assert_eq!(gateway.submitted_count(), 1);
    assert!(breaks.is_empty(), "{breaks:?}");
    let resting = report.orders[0].clone();
    assert_eq!(
        resting.price,
        dec!("100"),
        "the order did not rest at the mid"
    );
    assert!(report.fills.is_empty(), "the premise is nothing filled yet");

    // Somebody else's flow takes the resting buy between passes; the next
    // pass confirms it through the order-entry channel and reconciles it
    // against the drop copy.
    let taken = gateway.seed_aggressor(&object(), Side::Sell, dec!("100"), dec!("400"), t(25))?;
    assert_eq!(
        taken, resting.quantity,
        "the flow did not fill the resting order"
    );
    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(30),
    )?;
    let PassOutcome::Ran { report, breaks, .. } = second else {
        panic!("the node halted on the pass after a fill: {second:?}");
    };
    let confirmed: Vec<_> = report
        .fills
        .iter()
        .filter(|fill| fill.order_id == resting.order_id)
        .collect();
    assert_eq!(
        confirmed.len(),
        1,
        "the fill was not confirmed: {:?}",
        report.fills
    );
    assert_eq!(confirmed[0].quantity, resting.quantity);
    assert_eq!(
        confirmed[0].shares,
        vec![(StrategyId::new(STRATEGY), resting.quantity)],
        "the fill is not attributed to the strategy the plan deployed"
    );
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(!node.cell.is_halted());
    assert_eq!(stats.fills, 1);
    Ok(())
}

#[test]
fn a_stale_payload_deploys_nothing_however_good_its_plan() -> Result<()> {
    // The payload's own age caps every slot. A plan the centre named in a
    // payload that has run past `valid_for` is a plan the centre has
    // stopped vouching for, and the installer must not read the file at
    // all: the grant is held, the cell runs nothing, and the outcome says
    // the plan is not fresh rather than that the file is wrong.
    let dir = scratch("stale");
    let (path, digest, count) = plan_file(&dir, &[spec(STRATEGY, "10")]);
    let (mut node, _gateway, _feed) = node_with_feed()?;
    let mut installer = StrategyInstaller::new(Some(path), Some(PricingPolicy::Marketable));
    installer.offer(grant(STRATEGY)?)?;
    // Issued at t(10) and applied then; `valid_for` is five minutes, so at
    // t(1000) the slot reads stale though the plan itself never changed.
    node.cell
        .apply_policy(policy(1, t(10), &digest, count)?, t(10))?;
    assert!(
        node.cell.compiled_plan(t(10)).is_some(),
        "the premise is a plan that was fresh when applied"
    );
    assert!(
        node.cell.compiled_plan(t(1000)).is_none(),
        "the premise is a plan gone stale"
    );

    let outcome = installer.install(&mut node.cell, t(1000));
    assert!(
        outcome.deployed.is_empty(),
        "a stale payload deployed: {}",
        outcome.describe()
    );
    assert!(
        outcome
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("no fresh compiled plan")),
        "the outcome does not say the plan is stale: {}",
        outcome.describe()
    );
    assert!(node.cell.deployed_strategies().is_empty());
    assert_eq!(
        installer.held_grants(),
        vec![STRATEGY],
        "the grant was spent on nothing"
    );
    Ok(())
}

#[test]
fn a_plan_whose_bytes_are_not_the_ones_the_payload_names_deploys_nothing() -> Result<()> {
    // The digest is the only thing tying the file on disk to the decision
    // the centre signed. A file at the configured path that digests to
    // anything else — a plan somebody edited, or the wrong plan — is
    // refused whole, the refusal names both digests, and the grant stays
    // held for the plan the payload actually means.
    let dir = scratch("digest");
    let (path, _, count) = plan_file(&dir, &[spec(STRATEGY, "10")]);
    let (mut node, _gateway, _feed) = node_with_feed()?;
    let mut installer = StrategyInstaller::new(Some(path), Some(PricingPolicy::Marketable));
    installer.offer(grant(STRATEGY)?)?;
    let other = StrategyPlan::digest_of(b"some other plan");
    node.cell
        .apply_policy(policy(1, t(10), &other, count)?, t(10))?;

    let outcome = installer.install(&mut node.cell, t(10));
    assert!(outcome.deployed.is_empty(), "{}", outcome.describe());
    let reason = outcome.blocked.as_deref().unwrap_or_default();
    assert!(
        reason.contains(&other) && reason.contains("digests to"),
        "the refusal does not name the digest the payload wanted: {reason}"
    );
    assert!(node.cell.deployed_strategies().is_empty());
    assert_eq!(installer.held_grants(), vec![STRATEGY]);
    Ok(())
}

#[test]
fn a_strategy_a_newer_plan_drops_is_withdrawn_and_one_it_changes_is_redeployed_under_its_grant()
-> Result<()> {
    // Two strategies deployed from one plan; the next payload names a plan
    // that drops one and resizes the other. The dropped one is withdrawn,
    // the changed one is withdrawn and redeployed under the grant it
    // already ran under — no second grant needed — and the cell's set is
    // exactly what the newer plan says.
    let dir = scratch("drop");
    let (path, digest, count) = plan_file(&dir, &[spec("alpha", "10"), spec("beta", "5")]);
    let (mut node, _gateway, _feed) = node_with_feed()?;
    let mut installer = StrategyInstaller::new(Some(path.clone()), Some(PricingPolicy::Marketable));
    installer.offer(grant("alpha")?)?;
    installer.offer(grant("beta")?)?;
    node.cell
        .apply_policy(policy(1, t(10), &digest, count)?, t(10))?;
    let first = installer.install(&mut node.cell, t(10));
    assert_eq!(
        first.deployed,
        vec!["alpha".to_string(), "beta".to_string()],
        "the premise is both deployed: {}",
        first.describe()
    );

    let (_, digest, count) = plan_file(&dir, &[spec("alpha", "20")]);
    node.cell
        .apply_policy(policy(2, t(20), &digest, count)?, t(20))?;
    let second = installer.install(&mut node.cell, t(20));
    assert_eq!(
        second.withdrawn,
        vec!["alpha".to_string(), "beta".to_string()],
        "the dropped and the changed strategies were not both withdrawn: {}",
        second.describe()
    );
    assert_eq!(
        second.deployed,
        vec!["alpha".to_string()],
        "the changed strategy was not redeployed under its own grant: {}",
        second.describe()
    );
    assert_eq!(node.cell.deployed_strategies(), vec!["alpha"]);
    assert!(
        installer.held_grants().is_empty(),
        "a grant for a dropped strategy is still held: {:?}",
        installer.held_grants()
    );
    assert_eq!(
        node.cell.journal().tally().get("strategy_withdrawn"),
        Some(&2),
        "the withdrawals are not in the chain"
    );
    Ok(())
}

#[test]
fn a_pricing_the_node_cannot_read_is_refused_at_start_and_unset_deploys_nothing() -> Result<()> {
    // The two forms, the unset case, and every plausible mistake. A wrong
    // value is a configuration error rather than a quiet cell, because an
    // operator who wrote `market` meant something and would otherwise be
    // reading a node that started and never deployed.
    assert_eq!(parse_pricing(None)?, None);
    assert_eq!(parse_pricing(Some("  "))?, None);
    assert_eq!(
        parse_pricing(Some("marketable"))?,
        Some(PricingPolicy::Marketable)
    );
    assert_eq!(parse_pricing(Some("rest-at-mid:30"))?, Some(rest(30)?));
    for value in [
        "market",
        "mid",
        "Marketable",
        "rest-at-mid",
        "rest-at-mid:",
        "rest-at-mid:30s",
        "rest-at-mid:0",
        "rest-at-mid:-5",
    ] {
        let error = match parse_pricing(Some(value)) {
            Ok(policy) => panic!("{PRICING_VARIABLE}={value} was accepted as {policy:?}"),
            Err(error) => error,
        };
        let message = error.message();
        assert!(
            message.starts_with("configuration:"),
            "the refusal of {value} is not a configuration error, so the node would exit as a \
             crash rather than a misdeployment: {message}"
        );
        assert!(
            message.contains(value),
            "the refusal does not echo the value: {message}"
        );
    }

    // Unset: the installer acts on a fresh plan and a held grant and still
    // deploys nothing, saying which variable is missing.
    let dir = scratch("unpriced");
    let (path, digest, count) = plan_file(&dir, &[spec(STRATEGY, "10")]);
    let (mut node, _gateway, _feed) = node_with_feed()?;
    let mut installer = StrategyInstaller::new(Some(path), None);
    installer.offer(grant(STRATEGY)?)?;
    node.cell
        .apply_policy(policy(1, t(10), &digest, count)?, t(10))?;
    let outcome = installer.install(&mut node.cell, t(10));
    assert!(outcome.deployed.is_empty(), "{}", outcome.describe());
    assert!(
        outcome
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains(PRICING_VARIABLE)),
        "the outcome does not name the unset variable: {}",
        outcome.describe()
    );
    assert!(node.cell.deployed_strategies().is_empty());
    Ok(())
}
