//! Every route by which an order reaches a venue, driven through the code
//! that runs it, and each shown to refuse a limit breach before the venue
//! hears of it (R12-RISK-4093, R12-RISK-7205).
//!
//! This file used to be a source-text walk: it found each call that hands an
//! order to a venue and checked that the gate's call appeared above it. An
//! independent audit found that it passed with the gate absent from the
//! running binary — `Cell::send` held the call behind an `Option` that
//! `qip-edge-node` never filled, so the text was there and the gate never ran
//! — and that the repricer's replacement route was ungated outright, carried
//! in the table as a row that said so. A grep cannot tell installed from
//! written.
//!
//! So each route is now driven:
//!
//! * **The reflex net path** — a strategy firing in a node assembled exactly
//!   as `qip-edge-node`'s `main.rs` assembles it (`assemble_with_risk`, the
//!   limits from `CellRiskLimits`), run through `run_pass` against the
//!   simulated gateway the binary holds. A breaching order is refused and the
//!   venue's submission count stays at zero.
//! * **The requote replacement** — the same node with the binary's repricer:
//!   a resting order goes stale, is withdrawn, and its replacement at the new
//!   touch breaches the order-notional limit. The withdrawal stands and no
//!   replacement reaches the venue.
//! * **The desk path** — `OrderManager::submit` with a breaching order: the
//!   broker is never called.
//!
//! The arbitrage desk's legs leave through `Cell::send` as the net path does
//! (`Cell::send` is "the one place a `Placer` is called"); the enumeration
//! below is what holds that, by failing when a call that hands an order to a
//! venue appears anywhere that is not a row here.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::GATE_RISK;
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::telemetry::CellMetrics;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::reprice::{Requote, Requoter};
use qip_edge_node::risk::CellRiskLimits;
use qip_edge_node::{NodeAssembly, assemble, assemble_with_risk};
use qip_execution_engine::broker::{SimulatedBroker, SimulationSettings};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::pretrade::PreTradeChecker;
use qip_routing::reprice::RepricePolicy;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// --- the running cell ------------------------------------------------------

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"route-gate-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// A strategy whose one rule always holds, so a pass raises exactly one
/// signal; what is under test is the gate, not the strategy.
fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
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
    // Bounded as the centre's issuer bounds every grant (R12-CAPITAL-4088):
    // the node `assemble` builds refuses an envelope without exposure
    // bounds, and this suite is about the risk gate, not that refusal.
    let unsigned = build("")?.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        qip_capital::DEFAULT_MAX_PARTICIPATION_BP,
        qip_capital::DEFAULT_MAX_LEVERAGE,
        dec!("1000000"),
    )?)?;
    // ADR 0126 decision 2: the assembled cell requires a drawdown stop, as
    // the centre's issuer signs one at the loss limit into every grant.
    let loss_limit = unsigned.loss_limit();
    let unsigned = unsigned.with_drawdown(qip_contracts::DrawdownBounds::bounded(
        loss_limit, None, None,
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))
}

/// A node assembled the way `main.rs` assembles it — `assemble_with_risk`
/// under `limits` — funded with one firing strategy's grant, resting its
/// orders at the mid for a minute. Its venue may list `object()` on demand,
/// the one instrument this suite seeds and trades, and nothing else; it may
/// seed it only once `seed_book` has permitted that too, so a test here that
/// seeds nothing runs under the listing permission alone.
fn node(limits: &CellRiskLimits) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble_with_risk(
        config,
        features,
        Arc::new(SystemClock),
        allocation,
        None,
        limits,
    )?;
    let gateway = permitting(SimulatedGateway::new(venue(), 7, t(0))?, &[&object()])?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell.deploy_with_pricing(
        compiled,
        program,
        grant()?,
        PricingPolicy::rest_at_mid(Duration::from_secs(60))?,
    )?;
    let named = grant()?.signature().to_string();
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;
    Ok((node, gateway, feed))
}

/// The shipped two limits, plus an order-notional ceiling at `bound` — a
/// document of the form a deployment mounts at `QIP_CELL_RISK_LIMITS`.
fn limits_with_order_notional(bound: Decimal) -> Result<CellRiskLimits> {
    let mut set = CellRiskLimits::shipped_set();
    set.name = "route-test".to_string();
    set = set.with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional { limit: bound },
        )
        .with_rationale("the bound this suite breaches on purpose"),
    );
    let text = serde_json::to_string(&set).expect("a limit set serialises");
    CellRiskLimits::from_document(&text, "route-test-limits.json")
}

fn risk_refusals(node: &NodeAssembly) -> Vec<String> {
    node.cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == GATE_RISK => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn seed_book(gateway: &mut SimulatedGateway) -> Result<()> {
    permit_seeding(gateway, &[&object()])?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    Ok(())
}

/// The quantity the node rests on its first pass under the shipped limits,
/// so a breaching bound can be set off the order the pass actually sends
/// rather than a figure the sizing could move under the test.
fn resting_quantity_under_the_shipped_limits() -> Result<Decimal> {
    let (mut node, mut gateway, mut feed) = node(&CellRiskLimits::shipped())?;
    seed_book(&mut gateway)?;
    let mut stats = PassStats::default();
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    let PassOutcome::Ran { report, .. } = outcome else {
        panic!("premise: a running node reported its pass as halted");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "premise: under the shipped limits the pass sends one order: {:?}",
        report.refusals
    );
    assert_eq!(
        gateway.submitted_count(),
        1,
        "premise: the venue saw the one order"
    );
    assert!(
        risk_refusals(&node).is_empty(),
        "premise: the shipped gate refused nothing"
    );
    let order = &report.orders[0];
    assert_eq!(
        order.price,
        dec!("100"),
        "premise: the order rests at the mid"
    );
    assert!(order.quantity.is_positive());
    Ok(order.quantity)
}

/// Every cell the node's library can assemble carries a gate: the binary's
/// door (`assemble_with_risk`) and the convenience one (`assemble`) alike.
///
/// Mutation: delete `.with_risk_gate(risk_gate)` from `assemble_with_risk`
/// and both assertions fail.
#[test]
fn every_cell_the_edge_node_assembles_carries_the_deterministic_risk_gate() -> Result<()> {
    let shipped = CellRiskLimits::shipped();
    let (gated, _, _) = node(&shipped)?;
    let gate = gated
        .cell
        .risk_gate()
        .expect("the binary's assembly installs a risk gate");
    assert_eq!(gate.limits(), shipped.limits());
    assert_eq!(gate.capital(), dec!("1000000000"));

    let plain = assemble(
        CellConfig::new(CELL, REGION).with_venue(venue()),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
        Arc::new(SystemClock),
        RegionCapital::read(Some("1000"))?,
        None,
    )?;
    assert!(
        plain.cell.risk_gate().is_some(),
        "`assemble` built a cell with no risk gate"
    );
    Ok(())
}

/// The reflex net path: a strategy's order that breaches the cell's limit is
/// refused at `Cell::send`, journaled under the gate, and the simulated venue
/// never receives it.
///
/// Mutation: delete the `self.risk_admit(None, …)?` line from `Cell::send`
/// (or the `.with_risk_gate` in `assemble_with_risk`) and the venue's
/// submission count is one.
#[test]
fn a_strategy_order_that_breaches_the_cells_limit_never_reaches_the_venue() -> Result<()> {
    let quantity = resting_quantity_under_the_shipped_limits()?;
    // Half the order's notional at the mid: the order is twice the bound.
    let bound = quantity * dec!("50");
    let (mut node, mut gateway, mut feed) = node(&limits_with_order_notional(bound)?)?;
    seed_book(&mut gateway)?;
    let mut stats = PassStats::default();
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    );

    assert_eq!(
        gateway.submitted_count(),
        0,
        "an order the cell's risk gate refuses reached the venue"
    );
    let error = outcome.expect_err("a refused order fails the pass, as at the cell");
    assert!(
        error.message().contains("risk gate"),
        "the refusal must name the gate: {}",
        error.message()
    );
    let refusals = risk_refusals(&node);
    assert_eq!(refusals.len(), 1, "one refusal journaled under {GATE_RISK}");
    // The refusal names the limit by the name the limit set gave it —
    // `order-notional`, this suite's own bound, since the shipped set holds
    // no limit of that name — not by `LimitKind::label`'s
    // `max_order_notional`, which this assertion asked for when it was
    // written in round 2 and never run. Matched delimited on both sides
    // (`: order-notional: `), so a neighbour such as `max-order-notional`
    // cannot satisfy it, and beside the kind's own description so the name
    // is shown to be the limit that bound and not a word in the prose.
    assert!(
        refusals[0].contains(": order-notional: ") && refusals[0].contains("single order notional"),
        "the refusal names the limit that bound: {}",
        refusals[0]
    );
    Ok(())
}

/// The requote replacement: the order rests at 100 inside the bound, the
/// touch moves to 100.5, and the replacement there breaches it. The venue
/// withdraws the original and receives nothing new.
///
/// Mutation: delete the `cell.admit_replacement(…)` block from
/// `Requoter::replace` and the venue receives the replacement — its
/// submission count rises by one more than this asserts.
#[test]
fn a_requote_whose_replacement_breaches_the_cells_limit_withdraws_and_sends_nothing_new()
-> Result<()> {
    let quantity = resting_quantity_under_the_shipped_limits()?;
    // Between the resting order (100 a unit) and its replacement (100.5).
    let bound = quantity * dec!("100.25");
    let (mut node, mut gateway, mut feed) = node(&limits_with_order_notional(bound)?)?;
    let mut requoter = Requoter::new(
        RepricePolicy::new(dec!("0.01"), 5, 50.0),
        CellMetrics::new(Arc::clone(node.scrape_registry()), CELL, REGION),
    )?;
    seed_book(&mut gateway)?;
    let mut stats = PassStats::default();
    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(10),
    )?;
    let PassOutcome::Ran { report, .. } = first else {
        panic!("premise: a running node reported its pass as halted");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "premise: one order rests inside the bound"
    );
    let resting = report.orders[0].clone();
    assert_eq!(
        resting.quantity, quantity,
        "premise: the sizing did not move"
    );
    assert!(gateway.venue_holds_open(&resting.order_id));
    assert!(
        risk_refusals(&node).is_empty(),
        "premise: nothing refused yet"
    );

    // Somebody bids 100.50 above the cell's 100: stale past five ticks.
    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), t(15))?;
    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(20),
    );

    // Whatever the strategy's own order on this pass did — at the new mid it
    // breaches the bound too, so it is refused at `Cell::send` and the pass
    // fails — the replacement must not have been sent.
    let sent_by_work = match &second {
        Ok(PassOutcome::Ran {
            report, requotes, ..
        }) => {
            assert!(
                requotes
                    .iter()
                    .any(|requote| matches!(requote, Requote::RiskRefused { .. })),
                "the requote outcome does not name the risk gate: {requotes:?}"
            );
            report.orders.len() as u64
        }
        Ok(PassOutcome::Halted { .. }) => panic!("the node halted: {second:?}"),
        Err(_) => 0,
    };
    assert!(
        !gateway.venue_holds_open(&resting.order_id),
        "the stale order was not withdrawn"
    );
    assert_eq!(
        gateway.submitted_count(),
        1 + sent_by_work,
        "the venue received a replacement the cell's risk gate refuses"
    );
    let refusals = risk_refusals(&node);
    assert!(
        refusals
            .iter()
            .any(|reason| reason.contains("the replacement for")),
        "no refusal of the replacement was journaled under {GATE_RISK}: {refusals:?}"
    );
    Ok(())
}

/// The desk path: the order manager refuses a breaching order before the
/// broker is called.
///
/// Mutation: skip `.check_with_working(` in `OrderManager::submit` and the
/// broker's submission count is one.
#[test]
fn a_desk_order_that_breaches_a_limit_never_reaches_the_broker() {
    let limits = LimitSet::new("desk").with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional {
                limit: dec!("1000"),
            },
        )
        .with_rationale("the bound this suite breaches on purpose"),
    );
    let mut manager = OrderManager::new(PreTradeChecker::new(limits));
    let mut broker = SimulatedBroker::new(
        SimulationSettings::default()
            .with_rejection_probability(0.0)
            .and_then(|s| s.with_immediate_fill_fraction(Decimal::ONE))
            .expect("0.0 and 1.0 are inside the permitted ranges"),
        3,
    );
    let autonomy = AutonomyController::new();
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    };
    let order = |quantity: &str, id: &str| {
        Order::new(
            OrderId::from_string(id),
            object(),
            Side::Buy,
            Decimal::parse(quantity).expect("a decimal literal"),
            OrderType::Market,
            dec!("100"),
            "prop-1",
            vec!["hyp-1".to_string()],
            "momentum",
            t(0),
        )
    };
    // Premise: an order inside the bound reaches the broker.
    let inside = manager.submit(
        &qip_risk::gate::DeterministicGate::minted_by_router_determination(
            "a test of the order manager",
        ),
        order("5", "ord-inside"),
        &mut broker,
        &autonomy,
        &state,
        BTreeMap::new(),
        None,
        t(0),
    );
    assert!(inside.accepted, "premise: {:?}", inside.refusal);
    assert_eq!(
        broker.submitted_count(),
        1,
        "premise: the broker was called"
    );

    let breach = manager.submit(
        &qip_risk::gate::DeterministicGate::minted_by_router_determination(
            "a test of the order manager",
        ),
        order("50", "ord-breach"),
        &mut broker,
        &autonomy,
        &state,
        BTreeMap::new(),
        None,
        t(0),
    );
    assert!(!breach.accepted);
    assert!(
        matches!(breach.refusal, Some(RefusalReason::RiskRejected { .. })),
        "the refusal is not the risk gate's: {:?}",
        breach.refusal
    );
    assert_eq!(
        broker.submitted_count(),
        1,
        "a desk order the risk gate refuses reached the broker"
    );
}

// --- the enumeration -------------------------------------------------------
//
// What the behavioural tests above cannot do is notice a *new* route. This
// walk finds every call that hands an order to a venue and requires it to be
// a row below; a new call site fails here until somebody decides, in this
// table, how it is gated — and then writes the behavioural test above for it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// `gate` appears in the enclosing function before the venue call, and a
    /// test above drives the route.
    Gated {
        gate: &'static str,
    },
    Delegation,
    Release,
    /// Lists an instrument at the venue with an *empty* book: nothing rests,
    /// so there is nothing to gate. The walk checks the call's first
    /// argument is a book built with `OrderBook::new(`, so a listing that
    /// starts carrying levels stops matching this row.
    EmptyListing,
}

/// `(path under backend/crates, enclosing function, how it is gated)`.
const SITES: &[(&str, &str, Route)] = &[
    (
        "edge/qip-edge/src/cell.rs",
        "send",
        Route::Gated {
            gate: "self.risk_admit(",
        },
    ),
    ("edge/qip-edge/src/cell.rs", "try_place", Route::Delegation),
    (
        "services/qip-execution-engine/src/oms.rs",
        "submit",
        Route::Gated {
            // `check_with_working` since round 3: the same checker, over the
            // book plus the remainders of orders already working.
            gate: ".check_with_working(",
        },
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        "submit_now",
        Route::Release,
    ),
    // R12-RISK-4093: the Market Factory rests a quote ladder through
    // `ListingVenue::list_book`, which this walk did not look for, so the
    // route had no row and no gate. `seed` now takes a `LadderAdmission`,
    // whose only mint is `OrderManager::admit_ladder`'s limit check.
    (
        "services/qip-execution-engine/src/market_creation.rs",
        "seed",
        Route::Gated {
            gate: "ladder: &LadderAdmission",
        },
    ),
    (
        "services/qip-execution-engine/src/market_creation.rs",
        "list",
        Route::EmptyListing,
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        "place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        "try_place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        "place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        "try_place",
        Route::Delegation,
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        "replace",
        Route::Gated {
            gate: "cell.admit_replacement(",
        },
    ),
];

#[derive(Debug)]
struct Site {
    path: String,
    line: usize,
    function: String,
    preamble: String,
}

fn crates_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend/crates")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn declared_fn(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = ["pub(crate) fn ", "pub fn ", "fn "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn is_order_call(lines: &[&str], index: usize) -> bool {
    let line = lines[index].trim();
    if line.starts_with("//") {
        return false;
    }
    if ["fn place(", "fn try_place(", "fn submit("]
        .iter()
        .any(|declaration| line.contains(declaration))
    {
        return false;
    }
    if line.contains(".submit(&order") {
        return true;
    }
    // Resting a book at a venue is placing every order in it.
    if line.contains(".list_book(") {
        return true;
    }
    if line.contains("try_place(") || line.contains(".place(") {
        let call: String = lines[index..lines.len().min(index + 10)].join(" ");
        let arguments = call
            .split_once("place(")
            .map(|(_, after)| after.split(')').next().unwrap_or(""))
            .unwrap_or("");
        return arguments.contains("quantity") && arguments.contains("price");
    }
    false
}

fn order_call_sites() -> Vec<Site> {
    let root = crates_root();
    let mut files = Vec::new();
    for group in [
        "libs", "services", "runtime", "apps", "edge", "agents", "quant",
    ] {
        let Ok(members) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for member in members.flatten() {
            rust_files(&member.path().join("src"), &mut files);
        }
    }
    files.sort();
    let mut sites = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("a source file is readable");
        let all: Vec<&str> = text.lines().collect();
        let end = all
            .iter()
            .position(|line| *line == "#[cfg(test)]")
            .unwrap_or(all.len());
        let lines = &all[..end];
        for index in 0..lines.len() {
            if !is_order_call(lines, index) {
                continue;
            }
            let start = (0..=index)
                .rev()
                .find(|i| declared_fn(lines[*i]).is_some())
                .unwrap_or(0);
            let function = declared_fn(lines[start]).unwrap_or_else(|| "?".to_string());
            let path = file
                .strip_prefix(&root)
                .expect("under backend/crates")
                .to_string_lossy()
                .replace('\\', "/");
            sites.push(Site {
                path,
                line: index + 1,
                function,
                preamble: lines[start..index].join("\n"),
            });
        }
    }
    sites
}

#[test]
fn every_call_that_hands_an_order_to_a_venue_is_a_row_in_the_gate_table() {
    let sites = order_call_sites();
    for (path, function) in [
        ("edge/qip-edge/src/cell.rs", "send"),
        ("services/qip-execution-engine/src/oms.rs", "submit"),
        ("apps/qip-edge-node/src/reprice.rs", "replace"),
    ] {
        assert!(
            sites
                .iter()
                .any(|site| site.path == path && site.function == function),
            "premise: the walk did not find {path}::{function}; found {sites:?}"
        );
    }
    let found: BTreeSet<(String, String)> = sites
        .iter()
        .map(|site| (site.path.clone(), site.function.clone()))
        .collect();
    let pinned: BTreeSet<(String, String)> = SITES
        .iter()
        .map(|(path, function, _)| ((*path).to_string(), (*function).to_string()))
        .collect();
    let unclassified: Vec<String> = sites
        .iter()
        .filter(|site| !pinned.contains(&(site.path.clone(), site.function.clone())))
        .map(|site| format!("{}:{} in fn {}", site.path, site.line, site.function))
        .collect();
    assert!(
        unclassified.is_empty(),
        "an order reaches a venue at a call site nobody has classified; route it through \
         `Cell::send` or `OrderManager::submit`, or add a row and a behavioural test above: \
         {unclassified:?}"
    );
    let stale: Vec<&(String, String)> = pinned.difference(&found).collect();
    assert!(
        stale.is_empty(),
        "rows naming a call site that no longer exists: {stale:?}"
    );
}

#[test]
fn each_gated_route_runs_its_gate_before_the_venue_call_and_no_route_is_ungated() {
    let sites = order_call_sites();
    let mut gated = 0;
    for site in &sites {
        let Some((_, _, route)) = SITES
            .iter()
            .find(|(path, function, _)| *path == site.path && *function == site.function)
        else {
            continue;
        };
        match route {
            Route::Gated { gate } => {
                gated += 1;
                assert!(
                    site.preamble.contains(gate),
                    "{}:{} hands an order to a venue in fn {} without `{gate}` before it",
                    site.path,
                    site.line,
                    site.function
                );
            }
            Route::Delegation => assert!(
                ["place", "try_place", "submit"].contains(&site.function.as_str()),
                "{}:{} is listed as a delegation but sits in fn {}, not in a venue method",
                site.path,
                site.line,
                site.function
            ),
            Route::Release => {}
            Route::EmptyListing => {
                // The call and the two lines after it, whitespace removed:
                // since R12-COVERAGE-5107 `list_book` takes the admission
                // as its second argument, so rustfmt breaks the call and its
                // first argument onto lines of their own. What is checked is
                // unchanged — the first thing handed to `list_book` is a
                // new, empty book.
                let call: String = std::fs::read_to_string(crates_root().join(&site.path))
                    .expect("a source file is readable")
                    .lines()
                    .skip(site.line - 1)
                    .take(3)
                    .collect::<String>()
                    .split_whitespace()
                    .collect();
                assert!(
                    call.contains(".list_book(OrderBook::new("),
                    "{}:{} is listed as an empty listing but rests something other than a new \
                     empty book: {call}",
                    site.path,
                    site.line
                );
            }
        }
    }
    assert!(
        gated >= 4,
        "premise: the four gated seams were examined, {gated} were"
    );
}

#[test]
fn the_desk_and_the_cell_gate_through_the_same_deterministic_checker() {
    // One gate, not two: the cell's gate is `qip_risk_engine`'s
    // `PreTradeChecker`, the same type the order manager holds. And neither
    // reaches a model.
    let gate = std::fs::read_to_string(crates_root().join("edge/qip-edge/src/risk_gate.rs"))
        .expect("the cell risk gate exists");
    assert!(
        gate.contains("use qip_risk_engine::pretrade::{PreTradeChecker"),
        "the cell's gate must be the desk's PreTradeChecker"
    );
    for forbidden in [
        "qip_cost_router",
        "qip_reasoning_engine",
        "qip_investment_agents",
        "qip_ai",
    ] {
        assert!(
            !gate.contains(forbidden),
            "the cell's risk gate names {forbidden}; a model in the gate is not a gate"
        );
    }
}

/// `gateway`, permitted to list each of `objects` on demand: a recorded
/// venue, product and jurisdiction permission for each (R12-COVERAGE-5107).
/// A fixture; the refusal without one is proven in qip-edge-node's
/// tests/gateway.rs.
fn permitting(gateway: SimulatedGateway, objects: &[&ObjectId]) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        for action in [CreationAction::Create, CreationAction::List] {
            let request = CreationRequest::new(action, venue.as_str(), object.as_str(), "paper");
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "test fixture")?;
            }
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

/// Permit `gateway` to seed each of `objects` as well as to list it, in
/// place, where it is about to be seeded (R12-COVERAGE-5107: permission to
/// list is not permission to seed). Called only there, so a test here that
/// seeds nothing runs under `Create` and `List` alone. It replaces the register
/// `permitting` installed, so `objects` names every instrument the gateway
/// lists, not only the one seeded. A fixture; the refusals without either
/// permission are proven in qip-edge-node's tests/gateway.rs.
fn permit_seeding(gateway: &mut SimulatedGateway, objects: &[&ObjectId]) -> Result<()> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        for action in [
            CreationAction::Create,
            CreationAction::List,
            CreationAction::Seed,
        ] {
            let request = CreationRequest::new(action, venue.as_str(), object.as_str(), "paper");
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "test fixture")?;
            }
        }
    }
    gateway.permit_listings(permissions, "paper")
}

/// Whether `signature` declares a parameter named exactly `gate` of type
/// `&DeterministicGate` — delimited, so `_gate`, `pregate` or an
/// `Option<&DeterministicGate>` do not count.
fn demands_the_gate(signature: &str) -> bool {
    signature
        .split(['(', ','])
        .any(|parameter| parameter.trim() == "gate: &DeterministicGate")
}

/// R12-RISK-3022 on the desk path: every function in the order manager that
/// runs the pre-trade limit check demands, by signature, the deterministic
/// gate only the cost router mints (ADR 0430), and every submission result it
/// builds carries that gate's rationale.
///
/// The failure this prevents: the kernel asked the router before
/// `OrderManager::submit` and kept the routing as a record beside the call,
/// so a second path into the limit check written without asking the router
/// compiled and ran exactly as well. A function added to the manager that
/// reaches `check_with_working` without the gate fails here, as does a
/// result that drops the rationale.
#[test]
fn every_order_manager_limit_check_demands_the_deterministic_gate() {
    let source = qip_acceptance::read("backend/crates/services/qip-execution-engine/src/oms.rs");
    let production = source
        .find("#[cfg(test)]")
        .map_or(source.as_str(), |at| &source[..at]);

    // Each function at any visibility — `pub(crate) fn`, `const fn` and the
    // rest split an item as `fn` does — and a limit check by either name:
    // `PreTradeChecker::check` is the same evaluation as
    // `check_with_working`, and a method reaching it would bypass nothing a
    // scan for only one name could see.
    let mut checking = Vec::new();
    let mut ungated = Vec::new();
    for (name, item) in function_items(production) {
        if !item.contains(".check_with_working(") && !item.contains(".check(") {
            continue;
        }
        if !demands_the_gate(signature_of(item)) {
            ungated.push(name.clone());
        }
        checking.push(name);
    }
    // Premise: the scan found the two functions that run the check, or it
    // asserts something of nothing.
    assert!(
        checking.iter().any(|name| name == "submit")
            && checking.iter().any(|name| name == "admit_ladder"),
        "premise: submit and admit_ladder run the limit check; found {checking:?}"
    );
    assert!(
        ungated.is_empty(),
        "these order-manager functions run the pre-trade limit check without demanding \
         `gate: &DeterministicGate`: {ungated:?}. Take the gate from \
         `Platform::determine_pretrade(..)?.gate()` (R12-RISK-3022, ADR 0430)"
    );

    // Every `SubmissionResult {` is a literal except the declaration, the
    // `impl` and a `-> SubmissionResult {` return type, so a literal after
    // `return`, `Ok(` or anything else is counted too.
    let literals = production.matches("SubmissionResult {").count()
        - production.matches("pub struct SubmissionResult {").count()
        - production.matches("impl SubmissionResult {").count()
        - production.matches("-> SubmissionResult {").count();
    assert!(
        literals > 0,
        "premise: the manager builds submission results"
    );
    assert_eq!(
        production
            .matches("routed: gate.rationale().to_string(),")
            .count(),
        literals + 1,
        "every SubmissionResult the manager builds, and the ladder admission, must carry the \
         gate's rationale"
    );
}
/// Whether `line` declares a function, at any visibility and with any of the
/// qualifiers Rust allows before `fn`, and its name if so.
///
/// The order-manager scan once split items only on `    pub fn ` and
/// `    fn `, so a `pub(crate) fn`, `pub(super) fn` or `const fn` written after
/// `OrderManager::submit` was read as part of `submit`, and an ungated limit
/// check inside it passed under `submit`'s gate (round 80's review).
fn function_name(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("pub") {
        rest = after.trim_start();
        if let Some(after) = rest.strip_prefix('(') {
            rest = after.split_once(')')?.1.trim_start();
        }
    }
    for qualifier in ["const ", "async ", "unsafe ", "extern \"C\" "] {
        if let Some(after) = rest.strip_prefix(qualifier) {
            rest = after.trim_start();
        }
    }
    let rest = rest.strip_prefix("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Every function in `source`, as its name and its text from the line that
/// declares it to the line before the next declaration.
fn function_items(source: &str) -> Vec<(String, &str)> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if let Some(name) = function_name(line) {
            starts.push((offset, name));
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, (start, name))| {
            let end = starts
                .get(index + 1)
                .map_or(source.len(), |(next, _)| *next);
            (name.clone(), &source[*start..end])
        })
        .collect()
}

/// A function's signature: its text up to the brace that opens its body.
fn signature_of(item: &str) -> &str {
    &item[..item.find('{').unwrap_or(item.len())]
}

/// `code` with the text of every function blanked — from the line that
/// declares it through the brace that closes its body, or the `;` that ends
/// a declaration with none — newlines kept. What is left is the item-level
/// text no function's item holds: `const`s, `static`s, `impl` and trait
/// headers, `use` lines, macro bodies.
///
/// [`function_items`] starts each item at a declaration line, so the text
/// before a file's first function belongs to no item, and text between two
/// functions belongs to the one above it. A module `const RELEASE: fn(..) =
/// Proposal::release;` at the top of a file, called as `RELEASE(p, now)`
/// from anywhere, was therefore invisible to the one-caller scan, and an
/// associated `const` placed just after the one allowed caller was read as
/// that caller's own text (second round-81 review). Assumes [`code_only`]
/// text, so no brace or `;` sits inside a comment or a literal.
fn outside_functions(code: &str) -> String {
    let bytes = code.as_bytes();
    let mut blank = vec![false; bytes.len()];
    let mut offset = 0;
    let mut covered = 0;
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if start < covered || function_name(line).is_none() {
            continue;
        }
        // The body opens at the first `{` outside the signature's `()` and
        // `[]`; a `;` there first ends a declaration with no body.
        let mut nesting = 0usize;
        let mut braces = 0usize;
        let mut end = bytes.len();
        for (at, &byte) in bytes.iter().enumerate().skip(start) {
            match byte {
                b'(' | b'[' if braces == 0 => nesting += 1,
                b')' | b']' if braces == 0 => nesting = nesting.saturating_sub(1),
                b';' if braces == 0 && nesting == 0 => {
                    end = at + 1;
                    break;
                }
                b'{' if nesting == 0 || braces > 0 => braces += 1,
                b'}' if braces > 0 => {
                    braces -= 1;
                    if braces == 0 {
                        end = at + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        for flag in &mut blank[start..end] {
            *flag = true;
        }
        covered = end;
    }
    code.char_indices()
        .map(|(at, c)| if blank[at] && c != '\n' { ' ' } else { c })
        .collect()
}

#[test]
fn the_function_splitter_reads_every_visibility_and_qualifier() {
    let source = "    pub fn a(gate: &DeterministicGate) {\n        x.check(y);\n    }\n\
                  \x20   pub(crate) fn b() {\n        x.check(y);\n    }\n\
                  \x20   const fn c() {}\n    pub(super) async fn d() {}\n    fn e() {}\n";
    let names: Vec<String> = function_items(source)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["a", "b", "c", "d", "e"]);
    let (_, b) = &function_items(source)[1];
    assert!(
        !demands_the_gate(signature_of(b)),
        "b's item must not inherit a's gate"
    );
}

/// The kernel functions that make a book-level hard-limit decision, and the
/// method by which each one makes it (R12-RISK-3022): the function, the type
/// that owns the method, the method's name, and how many arguments a call of
/// it takes after the receiver.
///
/// Each is the only caller of its method in the kernel's production source, and
/// each must demand `gate: &DeterministicGate` by signature and record the
/// gate's rationale. The calls that only *restrict* — `Proposal::veto`,
/// `SurvivalGovernor::release_nothing`, which lowers the survival ceiling to
/// what the book stands in — are deliberately not here: a restriction needs
/// no witness, and demanding one would let a router refusal stop a control
/// from tightening.
///
/// Matched by structure, not by receiver (round-81 review). The scan once
/// looked for `self.governor.size(` and `proposal.approve(` as text, so
/// `p.approve(..)` in a loop, a rebinding `g.size(..)`, or a path call
/// `Proposal::release(p, now)` was a second caller it could not see. Now the
/// method is matched as a delimited token on any receiver and in path form,
/// and the argument count is what tells it from a same-named method of
/// another type — `ReservationLedger::release(id, now)` takes two,
/// `Proposal::release(now)` one — because the count is part of the
/// signature and a receiver's name is not.
const KERNEL_HARD_LIMIT_CALLS: [(&str, &str, &str, usize); 4] = [
    ("size_survival", "SurvivalGovernor", "size", 5),
    ("seed_survival_ceiling", "SurvivalGovernor", "seed", 4),
    ("sign_off_drafts", "Proposal", "approve", 2),
    ("advance_released", "Proposal", "release", 1),
];

/// Production text with every comment and the contents of every string and
/// character literal removed, line structure kept, so a method named in a
/// doc comment or an error message neither satisfies nor trips a scan.
/// String state is carried across lines, because a message continued with a
/// trailing `\\` is still a string on the next line.
fn code_only(source: &str) -> String {
    let production = qip_acceptance::production_text(source);
    let chars: Vec<char> = production.chars().collect();
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(production.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
            continue;
        }
        if c == 'r' && (i == 0 || chars[i - 1] == 'b' || !ident(chars[i - 1])) {
            let mut hashes = 0;
            while chars.get(i + 1 + hashes) == Some(&'#') {
                hashes += 1;
            }
            if chars.get(i + 1 + hashes) == Some(&'"') {
                out.push_str("\"\"");
                i += 2 + hashes;
                while i < chars.len() {
                    if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
                        i += 1 + hashes;
                        break;
                    }
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                continue;
            }
        }
        if c == '"' {
            out.push_str("\"\"");
            i += 1;
            while i < chars.len() {
                match chars[i] {
                    '\\' => {
                        if chars.get(i + 1) == Some(&'\n') {
                            out.push('\n');
                        }
                        i += 2;
                    }
                    '"' => {
                        i += 1;
                        break;
                    }
                    '\n' => {
                        out.push('\n');
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            continue;
        }
        if c == '\'' {
            // A character literal, `'x'` or an escape; a lifetime has no
            // closing quote and is left as code.
            let close = if next == Some('\\') {
                chars
                    .get(i + 3..)
                    .and_then(|rest| rest.iter().position(|&ch| ch == '\''))
                    .map(|at| i + 3 + at)
            } else {
                (chars.get(i + 2) == Some(&'\'')).then_some(i + 2)
            };
            if let Some(close) = close {
                out.push_str("' '");
                i = close + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Every offset where `token` begins as a whole identifier, so `release`
/// is not found inside `release_nothing` nor `approve` inside `approve_all`.
fn token_offsets(text: &str, token: &str) -> Vec<usize> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(token)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + token.len()..].chars().next();
            !before.is_some_and(ident) && !after.is_some_and(ident)
        })
        .map(|(at, _)| at)
        .collect()
}

/// How many arguments the call whose parenthesis opens `text` passes: commas
/// at the outermost depth of `()`, `[]` and `{}` and outside every generic
/// argument list, a trailing comma not counted. `None` if the parenthesis
/// never closes. Closure parameters written directly as an argument would be
/// miscounted; none of the methods held here takes a closure.
///
/// A `<` opens a generic list where an expression can hold one: after `::`
/// (a turbofish, `BTreeMap::<A, B>::new()`), at the start of an argument (a
/// qualified path, `<T as Default>::default()`), or inside a list already
/// open. Anywhere else it is a comparison or a shift, and a `>` closes only an
/// open list and never as part of `->`. Without this a turbofish comma was
/// counted as an argument, so `p.approve(now, BTreeMap::<A, B>::new())` read
/// as three arguments and was not taken for the two-argument sign-off
/// (second round-81 review).
fn call_arity(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut generics = 0usize;
    let mut commas = 0usize;
    let mut since_comma = false;
    let mut previous: Option<char> = None;
    for c in text.chars() {
        let generic_open = generics > 0 || matches!(previous, Some(':' | '(' | '[' | '{' | ','));
        match c {
            '<' if depth >= 1 && generic_open => {
                generics += 1;
                since_comma = true;
            }
            '>' if generics > 0 && previous != Some('-') => {
                generics -= 1;
                since_comma = true;
            }
            '(' | '[' | '{' => {
                depth += 1;
                if depth > 1 {
                    since_comma = true;
                }
            }
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(commas + usize::from(since_comma));
                }
                since_comma = true;
            }
            ',' if depth == 1 && generics == 0 => {
                commas += 1;
                since_comma = false;
            }
            c if depth >= 1 && !c.is_whitespace() => since_comma = true,
            _ => {}
        }
        if !c.is_whitespace() {
            previous = Some(c);
        }
    }
    None
}

/// The argument count, on the turbofish and qualified-path forms a
/// bracket-only count read wrong, and on the comparisons and shifts it must
/// not take for generics. Mutation: count commas inside a generic list again
/// (drop `&& generics == 0`) — the turbofish cases read one argument too
/// many and the sign-off with a turbofish argument is not found.
#[test]
fn the_arity_count_reads_a_turbofish_comma_as_part_of_one_argument() {
    assert_eq!(
        call_arity("(now, BTreeMap::<String, Vec<u8>>::new())"),
        Some(2)
    );
    assert_eq!(call_arity("(now, Vec::<(u8, u8)>::new(), x)"), Some(3));
    assert_eq!(call_arity("(now, Vec::<fn(u8, u8) -> u8>::new())"), Some(2));
    assert_eq!(
        call_arity("(<Vec<u8> as Default>::default(), now)"),
        Some(2)
    );
    assert_eq!(call_arity("(a < b, c > d)"), Some(2));
    assert_eq!(call_arity("(x << 2, y >> 1, z)"), Some(3));
    assert_eq!(call_arity("(f::<A, B>(x), y,)"), Some(2));
    let code = code_only(
        "fn f() {\n    p.approve(now, BTreeMap::<String, String>::new());\n    \
         Proposal::release(&mut p, HashMap::<A, B>::default());\n}\n",
    );
    assert_eq!(
        reaches(&code, "approve", 2),
        1,
        "a sign-off with a turbofish argument was not found"
    );
    assert_eq!(
        reaches(&code, "release", 1),
        1,
        "a path-form release with a turbofish argument was not found"
    );
}

/// How many times `code` reaches `method`: a call on any receiver
/// (`x.method(..)` with `arity` arguments), a path call (`Type::method(..)`
/// with the receiver as an extra first argument), or a path naming it
/// without calling it (`Type::method` handed on as a function value, counted
/// whatever it is later called with). A bare token — the definition, a
/// binding, a field — is not a call, and nor is a path segment with a name
/// after it (`qip_confidential::release::Policy`): that is a module on the
/// way to something else, which a method cannot be. A turbofish after the
/// segment (`Type::method::<T>`) still names the method.
fn reaches(code: &str, method: &str, arity: usize) -> usize {
    token_offsets(code, method)
        .into_iter()
        .filter(|at| {
            let before = code[..*at].trim_end();
            let after = code[at + method.len()..].trim_start();
            if before.ends_with("::") {
                if let Some(rest) = after.strip_prefix("::")
                    && !rest.trim_start().starts_with('<')
                {
                    return false;
                }
                !after.starts_with('(') || call_arity(after) == Some(arity + 1)
            } else if before.ends_with('.') && !before.ends_with("..") {
                after.starts_with('(') && call_arity(after) == Some(arity)
            } else {
                false
            }
        })
        .count()
}

/// The matcher, on a fixture that holds each evasion the receiver-spelled
/// scan let through and each neighbour it must not take for a caller.
/// Mutations: match `self.governor.size(` as text again, or drop the
/// path-call arm of `reaches`, or the arity filter — each fails here.
#[test]
fn the_hard_limit_matcher_finds_any_receiver_and_path_calls_and_nothing_else() {
    let fixture = "fn a() {\n    for p in &mut self.proposals { p.approve(now, vec![x]); }\n}\n\
                   fn b() {\n    let g = &mut self.governor;\n    g.size(f, s, t, v, now);\n}\n\
                   fn c() {\n    Proposal::release(&mut p, now);\n}\n\
                   fn d() {\n    let f = SurvivalGovernor::seed;\n}\n\
                   fn e() {\n    x.approve_all(now, v);\n    self.reservations.release(id, now);\n    \
                   self.governor.release_nothing(g, h);\n    let s = config.seed;\n    \
                   context.seed();\n    // p.approve(now, v)\n    m(\"p.release(now)\");\n}\n";
    let code = code_only(fixture);
    let items = function_items(&code);
    let reached = |method: &str, arity: usize| -> Vec<String> {
        items
            .iter()
            .filter(|(_, item)| reaches(item, method, arity) > 0)
            .map(|(name, _)| name.clone())
            .collect()
    };
    assert_eq!(items.len(), 5, "premise: the fixture's five functions");
    assert_eq!(reached("approve", 2), ["a"]);
    assert_eq!(reached("size", 5), ["b"]);
    assert_eq!(reached("release", 1), ["c"]);
    assert_eq!(reached("seed", 4), ["d"]);
}

/// Who reaches each method of [`KERNEL_HARD_LIMIT_CALLS`] in `sources`,
/// each a file's name and its [`code_only`] text.
#[derive(Default)]
struct HardLimitReach {
    /// By `Owner::method`: every function that reaches it, by name, then
    /// `{file}: item-level` for each file whose text outside any function
    /// does.
    found: BTreeMap<String, Vec<String>>,
    /// Reachers that do not demand the gate by signature. Item-level text
    /// has no signature, so a guarded method named there is always here.
    ungated: Vec<String>,
    /// Functions that take the gate and do not record its rationale.
    unrecorded: Vec<String>,
}

fn hard_limit_reach(sources: &[(String, String)]) -> HardLimitReach {
    let mut reach = HardLimitReach::default();
    for (file, code) in sources {
        for (name, item) in function_items(code) {
            for (_, owner, method, arity) in KERNEL_HARD_LIMIT_CALLS {
                if reaches(item, method, arity) == 0 {
                    continue;
                }
                let call = format!("{owner}::{method}");
                reach
                    .found
                    .entry(call.clone())
                    .or_default()
                    .push(name.clone());
                if !demands_the_gate(signature_of(item)) {
                    reach.ungated.push(format!("{name} ({call})"));
                }
                if !item.contains("gate.rationale().to_string()") {
                    reach.unrecorded.push(format!("{name} ({call})"));
                }
            }
        }
        // A guarded method bound or called outside every function — a
        // `const` or `static` holding it as a function value, an associated
        // `const` in an `impl`, a macro body — is a caller no signature can
        // gate, whoever later calls the binding.
        let outside = outside_functions(code);
        for (_, owner, method, arity) in KERNEL_HARD_LIMIT_CALLS {
            if reaches(&outside, method, arity) == 0 {
                continue;
            }
            let call = format!("{owner}::{method}");
            let site = format!("{file}: item-level");
            reach
                .found
                .entry(call.clone())
                .or_default()
                .push(site.clone());
            reach.ungated.push(format!("{site} ({call})"));
        }
    }
    reach
}

/// The one-caller scan, on a fixture holding the evasions the function-only
/// split let through (second round-81 review): a guarded method bound as a
/// function value outside every function — a module `const` before the
/// first function, a `static`, and an associated `const` placed just after
/// the one allowed caller, where the split read it as that caller's own text
/// — and then called through the binding from a function that never names
/// the method. Each binding is reported as an ungated item-level caller,
/// while a doc comment naming the method, a same-named method of another
/// type, and a module of the same name in a `use` line are not — the last
/// is `central/insights.rs`'s `use qip_confidential::release::{..}`, which
/// the item-level scan read as `Proposal::release` until path segments were
/// told apart from methods.
///
/// Mutations: drop the item-level arm of `hard_limit_reach` —
/// `advance_released` looks like the only caller of `Proposal::release` and
/// nothing reaches `seed` or `approve`; drop the path-segment exclusion in
/// `reaches` — the `size` module is read as the method.
#[test]
fn the_one_caller_scan_sees_a_guarded_method_bound_outside_every_function() {
    let fixture = "use crate::proposal::Proposal;\n\
                   use qip_confidential::release::{Policy, Release};\n\
                   use qip_layout::size::Bounds;\n\
                   /// Proposal::release(p, now), named in a doc comment.\n\
                   const SEED: fn(&mut SurvivalGovernor, F, Option<&W>, f64, Timestamp) = \
                   SurvivalGovernor::seed;\n\
                   static APPROVE: fn(&mut Proposal, Timestamp, Vec<String>) = Proposal::approve;\n\
                   impl Platform {\n    \
                   fn advance_released(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   self.routings.insert(gate.rationale().to_string());\n        \
                   self.proposal.release(now);\n    }\n    \
                   const RELEASE: fn(&mut Proposal, Timestamp) = Proposal::release;\n    \
                   fn later(&mut self, now: Timestamp) {\n        \
                   Self::RELEASE(&mut self.proposal, now);\n        \
                   APPROVE(&mut self.proposal, now, Vec::new());\n        \
                   self.reservations.release(id, now);\n    }\n}\n";
    let reach = hard_limit_reach(&[("fixture.rs".to_string(), code_only(fixture))]);
    let site = "fixture.rs: item-level".to_string();
    // Premise: the allowed caller is found as a function, so the item-level
    // entries below are added to a scan that still works.
    assert_eq!(
        reach.found.get("Proposal::release").map(Vec::as_slice),
        Some(&["advance_released".to_string(), site.clone()][..]),
        "the associated const after the allowed caller was read as that caller's text"
    );
    assert_eq!(
        reach.found.get("SurvivalGovernor::seed").map(Vec::as_slice),
        Some(&[site.clone()][..]),
        "a module const before the first function was invisible"
    );
    assert_eq!(
        reach.found.get("Proposal::approve").map(Vec::as_slice),
        Some(&[site.clone()][..]),
        "a static holding the method was invisible"
    );
    assert_eq!(
        reach.ungated,
        [
            format!("{site} (SurvivalGovernor::seed)"),
            format!("{site} (Proposal::approve)"),
            format!("{site} (Proposal::release)"),
        ],
        "an item-level binding was not reported as a caller no gate holds"
    );
    assert!(reach.unrecorded.is_empty(), "{:?}", reach.unrecorded);
    assert!(
        !reach.found.contains_key("SurvivalGovernor::size"),
        "a module named `size` in a `use` line was read as the method: {:?}",
        reach.found.get("SurvivalGovernor::size")
    );
}

/// R12-RISK-3022 beyond the order manager: the survival waterfall, the
/// sign-off of a draft and the release of an approved proposal are each made
/// only by a kernel function that demands, by signature, the deterministic
/// gate only the cost router mints, and records its rationale.
///
/// The failure this prevents: until this change the three decisions were
/// made inline in DECIDE and ACT with no routing at all — deterministic by
/// the absence of a model in the call, not by anything a reader could check —
/// so a second path to `Proposal::approve` or `SurvivalGovernor::size`
/// written without asking the router compiled and ran exactly as well.
#[test]
fn every_kernel_hard_limit_decision_demands_the_deterministic_gate() {
    let mut kernel_files = Vec::new();
    rust_files(
        &crates_root().join("runtime/qip-kernel/src"),
        &mut kernel_files,
    );
    assert!(
        kernel_files.len() > 10,
        "premise: the kernel's sources were found"
    );
    let sources: Vec<(String, String)> = kernel_files
        .iter()
        .map(|path| {
            let source = std::fs::read_to_string(path).expect("kernel source is readable");
            (path.display().to_string(), code_only(&source))
        })
        .collect();
    let HardLimitReach {
        found,
        ungated,
        unrecorded,
    } = hard_limit_reach(&sources);
    // Premise, and the exact placement: each method is reached by exactly the
    // one function named for it, on any receiver and in path form, so a
    // second caller is a failure here whether or not it took a gate.
    for (function, owner, method, _) in KERNEL_HARD_LIMIT_CALLS {
        let call = format!("{owner}::{method}");
        assert_eq!(
            found.get(&call).map(Vec::as_slice),
            Some(&[function.to_string()][..]),
            "`{call}` must be reached by `{function}` alone in the kernel's production source"
        );
    }
    assert!(
        ungated.is_empty(),
        "these kernel functions make a hard-limit decision without demanding \
         `gate: &DeterministicGate`: {ungated:?}. Take the gate from \
         `Platform::determine_book_decision(..)?.gate()` (R12-RISK-3022, ADR 0430)"
    );
    assert!(
        unrecorded.is_empty(),
        "these kernel functions take the gate but do not record its rationale: {unrecorded:?}"
    );
}

/// R12-RISK-3022 on the kill-switch path beyond `RiskMonitor::enforce`: no
/// kernel production source trips the kill switch through the ungated
/// `KillSwitch::trip_scope` or `trip_global`, and every kernel function that
/// trips a scope under a gate demands that gate by signature.
///
/// The failure this prevents: the central plane halted a cell whose records
/// did not reconcile with `kill_switch.trip_scope(..)`, a hard-limit decision
/// made with no routing, and nothing distinguished it from an operator's
/// trip. A kernel halt now goes through `trip_scope_under`, or through the
/// monitor's `enforce`, which acts on a verdict the routed monitor gave.
#[test]
fn every_kernel_kill_switch_trip_demands_the_deterministic_gate() {
    let mut kernel_files = Vec::new();
    rust_files(
        &crates_root().join("runtime/qip-kernel/src"),
        &mut kernel_files,
    );
    let mut ungated_trips = Vec::new();
    let mut gated = Vec::new();
    let mut ungated = Vec::new();
    // Every mention of the ungated trips as a delimited token, on any
    // receiver, in path form (`KillSwitch::trip_scope(&mut switch, ..)`) or
    // handed on as a function value. The scan once matched `.trip_scope(` as
    // text, which a path call does not contain (round-81 review). The kernel
    // defines neither name, so any mention in its code is a route to one.
    for path in &kernel_files {
        let source = std::fs::read_to_string(path).expect("kernel source is readable");
        let code = code_only(&source);
        for method in ["trip_scope", "trip_global"] {
            let mentions = token_offsets(&code, method).len();
            if mentions > 0 {
                ungated_trips.push(format!("{} ({method} x{mentions})", path.display()));
            }
        }
        for (name, item) in function_items(&code) {
            if !token_offsets(item, "trip_scope_under").is_empty() {
                if !demands_the_gate(signature_of(item)) {
                    ungated.push(name.clone());
                }
                gated.push(name);
            }
        }
    }
    assert!(
        ungated_trips.is_empty(),
        "the kernel trips the kill switch without a deterministic gate: {ungated_trips:?}. \
         Use `KillSwitch::trip_scope_under` with a gate from \
         `Platform::determine_book_decision` (R12-RISK-3022)"
    );
    assert_eq!(
        gated,
        ["ingest"],
        "premise: the central plane's ingest is the kernel's one gated trip"
    );
    assert!(
        ungated.is_empty(),
        "these kernel functions trip a scope under a gate they do not demand: {ungated:?}"
    );
}

// --- the edge cell's pre-trade check (DECISION, R12-RISK-3022) -------------

/// The crates that can put a model on a decision: the language-model crate,
/// its providers, and the cost router, whose `Judged` arm names a model rung.
///
/// The edge cell's `CellRiskGate` calls `PreTradeChecker::check`, and a cell
/// cannot hold a router-minted `DeterministicGate`: the router links
/// `qip-ai`. So the cell's pre-trade check is held to never route to a model
/// by its dependency closure instead — neither `qip-edge` nor the binary that
/// hosts it links any of these, directly or through any crate they link.
const MODEL_CRATES: [&str; 3] = ["qip-ai", "qip-language-providers", "qip-cost-router"];

/// Each workspace crate's normal dependencies on other workspace crates, read
/// from its `Cargo.toml`. Dev-dependencies are left out: they never reach the
/// binary a node runs.
fn workspace_dependencies() -> BTreeMap<String, BTreeSet<String>> {
    let root = std::fs::read_to_string(crates_root().join("../Cargo.toml"))
        .expect("the workspace manifest is readable");
    let inherited = workspace_packages(&root);
    assert!(
        inherited.get("qip-ai").map(String::as_str) == Some("qip-ai"),
        "premise: the workspace manifest's dependency table was read"
    );
    let mut manifests = BTreeMap::new();
    for group in std::fs::read_dir(crates_root()).expect("the crates root is readable") {
        let group = group.expect("a crate group").path();
        let Ok(crates) = std::fs::read_dir(&group) else {
            continue;
        };
        for krate in crates.flatten() {
            let manifest = krate.path().join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let name = krate.file_name().to_string_lossy().to_string();
            manifests.insert(name, normal_dependencies(&text, &inherited));
        }
    }
    manifests
}

/// One dependency as a manifest declares it: the key it is written under,
/// the `package = "…"` it renames, and whether it inherits the workspace's
/// entry for that key.
#[derive(Default)]
struct Declared {
    key: String,
    package: Option<String>,
    workspace: bool,
}

impl Declared {
    /// The package that links: a rename's `package`, else the workspace's
    /// package for an inherited key, else the key itself.
    fn package(&self, inherited: &BTreeMap<String, String>) -> String {
        self.package.clone().unwrap_or_else(|| {
            self.workspace
                .then(|| inherited.get(&self.key).cloned())
                .flatten()
                .unwrap_or_else(|| self.key.clone())
        })
    }
}

/// The table a manifest header opens, for the dependency scan.
enum Table {
    /// Not a table this scan reads.
    Other,
    /// `[dependencies]`, `[target.*.dependencies]` or
    /// `[workspace.dependencies]`: one dependency per key.
    Entries,
    /// `[dependencies.name]` (or a target's, or the workspace's): one
    /// dependency, its fields one per line.
    Entry(Declared),
}

/// The line with a trailing `#` comment removed, outside a quoted string.
fn without_comment(line: &str) -> &str {
    let mut quoted = false;
    for (at, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..at],
            _ => {}
        }
    }
    line
}

/// The key a manifest line opens with — `qip-ai` in `qip-ai = …` and in
/// `qip-ai.workspace = true` — quotes removed.
fn leading_key(line: &str) -> String {
    line.trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// The quoted value of `field = "…"` inside `text`, as a delimited field.
fn quoted_field(text: &str, field: &str) -> Option<String> {
    for (at, _) in text.match_indices(field) {
        let before = text[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let rest = text[at + field.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('"')?;
        return rest.split_once('"').map(|(value, _)| value.to_string());
    }
    None
}

/// Whether `text` sets `workspace = true` (inline) or is `key.workspace = true`.
fn inherits(text: &str) -> bool {
    text.split([',', '{', '}'])
        .map(|part| part.split_whitespace().collect::<String>())
        .any(|part| part == "workspace=true" || part.ends_with(".workspace=true"))
}

/// What table `header` (between the brackets) opens.
fn table_of(header: &str) -> Table {
    let header = header.trim();
    let rest = if let Some(target) = header.strip_prefix("target.") {
        // A target spec is quoted (`'cfg(unix)'`, `"cfg(..)"`) or one bare
        // segment (a triple, which holds no dot).
        let after = match target.chars().next() {
            Some(quote @ ('\'' | '"')) => target[1..]
                .split_once(quote)
                .map(|(_, after)| after.trim_start_matches('.')),
            _ => target.split_once('.').map(|(_, after)| after),
        };
        let Some(after) = after else {
            return Table::Other;
        };
        after
    } else {
        header.strip_prefix("workspace.").unwrap_or(header)
    };
    if rest == "dependencies" {
        Table::Entries
    } else if let Some(name) = rest.strip_prefix("dependencies.") {
        Table::Entry(Declared {
            key: name.trim_matches(['"', '\'']).to_string(),
            ..Declared::default()
        })
    } else {
        Table::Other
    }
}

/// Every dependency `manifest` declares for its build, as declared: entries
/// under `[dependencies]` and `[target.*.dependencies]`, each dependency
/// given its own `[dependencies.name]` table, and the workspace's own
/// `[workspace.dependencies]`.
fn declared_dependencies(manifest: &str) -> Vec<Declared> {
    let mut declared = Vec::new();
    let mut table = Table::Other;
    for line in manifest.lines() {
        let line = without_comment(line).trim();
        if line.starts_with('[') {
            if let Table::Entry(entry) = std::mem::replace(&mut table, Table::Other) {
                declared.push(entry);
            }
            table = table_of(line.trim_start_matches('[').trim_end_matches(']'));
            continue;
        }
        match &mut table {
            Table::Other => {}
            Table::Entries => {
                let key = leading_key(line);
                if !key.is_empty() {
                    declared.push(Declared {
                        package: quoted_field(line, "package"),
                        workspace: inherits(line),
                        key,
                    });
                }
            }
            Table::Entry(entry) => {
                if leading_key(line) == "package" {
                    entry.package = quoted_field(line, "package");
                }
                if inherits(line) {
                    entry.workspace = true;
                }
            }
        }
    }
    if let Table::Entry(entry) = table {
        declared.push(entry);
    }
    declared
}

/// The workspace manifest's `[workspace.dependencies]`, key to package, so a
/// member's `key.workspace = true` resolves to what links even when the
/// workspace renames it.
fn workspace_packages(root: &str) -> BTreeMap<String, String> {
    let mut packages = BTreeMap::new();
    let mut in_workspace = false;
    let mut kept = String::new();
    for line in root.lines() {
        let header = without_comment(line).trim();
        if header.starts_with('[') {
            in_workspace = header.starts_with("[workspace.dependencies");
        }
        if in_workspace {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    for entry in declared_dependencies(&kept) {
        let package = entry.package.clone().unwrap_or_else(|| entry.key.clone());
        packages.insert(entry.key, package);
    }
    packages
}

/// The `qip-*` packages a member manifest links for its build — by the
/// package, not the key, so a rename (`models = { package = "qip-ai" }`) or a
/// `[dependencies.qip-ai]` table is seen. The scan once read only keys under
/// a `[dependencies]` header, and both forms passed it (round-81 review).
fn normal_dependencies(manifest: &str, inherited: &BTreeMap<String, String>) -> BTreeSet<String> {
    declared_dependencies(manifest)
        .iter()
        .map(|entry| entry.package(inherited))
        .filter(|package| package.starts_with("qip-"))
        .collect()
}

/// The manifest parser, on a fixture holding each form a key-only scan
/// missed and each it must not take for a normal dependency. Mutations: read
/// only `[dependencies]` headers again, or take the key over `package` —
/// each fails here.
#[test]
fn the_dependency_scan_reads_tables_renames_and_inherited_renames() {
    let inherited = BTreeMap::from([
        ("qip-core".to_string(), "qip-core".to_string()),
        ("lm".to_string(), "qip-language-providers".to_string()),
    ]);
    let manifest = "[package]\nname = \"x\"\n\n[dependencies]\nqip-core.workspace = true\n\
                    router = { package = \"qip-cost-router\", path = \"../r\" } # renamed\n\
                    lm = { workspace = true }\nserde = \"1\"\n\n\
                    [dependencies.qip-ai]\npath = \"../qip-ai\"\n\n\
                    [target.'cfg(unix)'.dependencies.models]\npackage = \"qip-models\"\n\n\
                    [dev-dependencies]\nqip-dev-only = { path = \"../d\" }\n\n\
                    [dev-dependencies.qip-dev-table]\npath = \"../e\"\n";
    let found = normal_dependencies(manifest, &inherited);
    let expected: BTreeSet<String> = [
        "qip-ai",
        "qip-core",
        "qip-cost-router",
        "qip-language-providers",
        "qip-models",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(found, expected);
}

/// Every model crate reachable from `root`, with the path that reaches it.
fn model_reach(manifests: &BTreeMap<String, BTreeSet<String>>, root: &str) -> Vec<String> {
    let mut reached = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![vec![root.to_string()]];
    while let Some(path) = stack.pop() {
        let at = path.last().cloned().unwrap_or_default();
        if !seen.insert(at.clone()) {
            continue;
        }
        if MODEL_CRATES.contains(&at.as_str()) {
            reached.push(path.join(" -> "));
            continue;
        }
        for next in manifests.get(&at).into_iter().flatten() {
            let mut longer = path.clone();
            longer.push(next.clone());
            stack.push(longer);
        }
    }
    reached
}

#[test]
fn the_edge_cells_pre_trade_check_links_no_crate_that_can_route_to_a_model() {
    let manifests = workspace_dependencies();
    // Premise: the manifests were read and the walk sees what it must. Every
    // model crate is a workspace member, the cost router reaches `qip-ai`, and
    // the cell links the crate whose checker it calls.
    for model in MODEL_CRATES {
        assert!(
            manifests.contains_key(model),
            "premise: {model} is a workspace crate"
        );
    }
    assert!(
        manifests["qip-cost-router"].contains("qip-ai"),
        "premise: the cost router links the language-model crate"
    );
    assert!(
        manifests["qip-edge"].contains("qip-risk-engine"),
        "premise: the cell links the crate whose PreTradeChecker its risk gate calls"
    );
    assert!(
        manifests["qip-edge-node"].contains("qip-edge"),
        "premise: the node links the cell"
    );

    for root in ["qip-edge", "qip-edge-node"] {
        let reached = model_reach(&manifests, root);
        assert!(
            reached.is_empty(),
            "{root} can reach a crate that routes to a model, so the cell's pre-trade check \
             is no longer held deterministic by its closure: {reached:?}"
        );
    }

    // The same walk on a scratch copy of the manifest data with one model
    // edge added two links below the cell: it must be found, or the clean
    // result above is the walk failing to look.
    let mut scratch = manifests.clone();
    scratch
        .get_mut("qip-risk")
        .expect("qip-risk is a workspace crate")
        .insert("qip-cost-router".to_string());
    let reached = model_reach(&scratch, "qip-edge-node");
    assert!(
        reached
            .iter()
            .any(|path| path.ends_with("-> qip-risk -> qip-cost-router")),
        "a model edge added below the cell went unseen: {reached:?}"
    );
}
