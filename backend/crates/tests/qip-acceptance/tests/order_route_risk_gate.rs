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
        let end = item_end(bytes, start);
        for flag in &mut blank[start..end] {
            *flag = true;
        }
        covered = end;
    }
    code.char_indices()
        .map(|(at, c)| if blank[at] && c != '\n' { ' ' } else { c })
        .collect()
}

/// Where the function declared at `start` ends: the brace that closes its
/// body, or the `;` that ends a declaration with none. The body opens at the
/// first `{` outside the signature's `()` and `[]`. Assumes [`code_only`]
/// text, so no brace or `;` sits inside a comment or a literal.
fn item_end(bytes: &[u8], start: usize) -> usize {
    let mut nesting = 0usize;
    let mut braces = 0usize;
    for (at, &byte) in bytes.iter().enumerate().skip(start) {
        match byte {
            b'(' | b'[' if braces == 0 => nesting += 1,
            b')' | b']' if braces == 0 => nesting = nesting.saturating_sub(1),
            b';' if braces == 0 && nesting == 0 => return at + 1,
            b'{' if nesting == 0 || braces > 0 => braces += 1,
            b'}' if braces > 0 => {
                braces -= 1;
                if braces == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
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
///
/// `Treasury::check_leverage` joined the table with R12-RISK-3022 (c):
/// `Treasury::first_breach` called `LeverageBook::check` with no routing, a
/// hard limit on every financed requirement that the cycle reached with no
/// gate. `check` takes four arguments (strategy, region, notional, capital),
/// which is what tells it from the kernel's other `check`s.
const KERNEL_HARD_LIMIT_CALLS: [(&str, &str, &str, usize); 5] = [
    ("size_survival", "SurvivalGovernor", "size", 5),
    ("seed_survival_ceiling", "SurvivalGovernor", "seed", 4),
    ("sign_off_drafts", "Proposal", "approve", 2),
    ("advance_released", "Proposal", "release", 1),
    ("check_leverage", "LeverageBook", "check", 4),
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
    // An arrow's `>` closes no generic list: with a comma after the arrow
    // at the generic list's own depth, reading `->` as a close would count
    // the comma as a third argument and miss a second sign-off caller.
    assert_eq!(
        call_arity("(now, HashMap::<fn() -> u8, u8>::new())"),
        Some(2)
    );
    // A qualified path opening an argument is a generic list whose comma
    // sits at the list's own depth; without the rule that `<` after `(` or
    // `,` opens one, that comma is counted as an argument.
    assert_eq!(
        call_arity("(<HashMap<A, B> as Default>::default(), now)"),
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

/// The line with a trailing `#` comment removed, outside a quoted string —
/// a basic `"…"` or a literal `'…'` one, each closed only by its own quote,
/// so a `'` inside `"it's"` closes nothing. A `#` inside a single-quoted
/// value cut the line there while only `"` was read, and a `package` after
/// it was lost.
fn without_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (at, c) in line.char_indices() {
        match (c, quote) {
            ('"' | '\'', None) => quote = Some(c),
            (c, Some(open)) if c == open => quote = None,
            ('#', None) => return &line[..at],
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

/// The quoted value of `field = "…"` or `field = '…'` inside `text`, as a
/// delimited field. TOML takes either; reading only `"` let `models = {
/// package = 'qip-ai' }` link the language-model crate under the key
/// `models`, which the scan does not treat as a model crate (second round-81
/// review).
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
        let rest = rest.trim_start();
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        return rest[1..]
            .split_once(quote)
            .map(|(value, _)| value.to_string());
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

/// The manifest parser on single-quoted TOML strings, which a parser reading
/// only `"` missed (second round-81 review): an inline rename, a rename in
/// its own `[dependencies.name]` table, and an inline rename whose `package`
/// follows a single-quoted value holding a `#`, which the comment stripper
/// cut at while it read only `"`. A `'` inside a double-quoted value opens
/// nothing, so the `#` after it is still inside that value.
///
/// Mutations: read only `"` in `quoted_field` (the three single-quoted
/// renames fall back to keys outside `qip-*`); read only `"` in
/// `without_comment` (the rename after the single-quoted `#` is lost); let
/// either quote close the other (the rename after `it's#` is lost).
#[test]
fn the_dependency_scan_reads_single_quoted_package_renames() {
    let inherited = BTreeMap::from([("qip-core".to_string(), "qip-core".to_string())]);
    let manifest = "[package]\nname = 'x'\n\n\
                    [dependencies]\nrouter = { package = 'qip-cost-router', path = '../r' }\n\
                    hashed = { path = '../#h', package = 'qip-ai' } # renamed\n\
                    apostrophe = { path = \"../it's#here\", package = \"qip-models\" }\n\
                    qip-core.workspace = true\nserde = '1'\n\n\
                    [dependencies.lm]\npackage = 'qip-language-providers'\npath = '../lm'\n";
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
    crates_reached(manifests, root, &MODEL_CRATES)
}

/// Every crate of `targets` reachable from `root` through normal
/// dependencies, with the path that reaches it.
fn crates_reached(
    manifests: &BTreeMap<String, BTreeSet<String>>,
    root: &str,
    targets: &[&str],
) -> Vec<String> {
    let mut reached = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![vec![root.to_string()]];
    while let Some(path) = stack.pop() {
        let at = path.last().cloned().unwrap_or_default();
        if !seen.insert(at.clone()) {
            continue;
        }
        if targets.contains(&at.as_str()) {
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
    // The same closure for every crate that holds a hard limit outside the
    // kernel, each shown to be a workspace crate first: a root the manifests
    // do not hold reaches nothing, and would pass here for that reason.
    for (krate, _, _) in HARD_LIMIT_CRATES {
        assert!(
            manifests.contains_key(krate),
            "premise: {krate} is a workspace crate"
        );
        let reached = model_reach(&manifests, krate);
        assert!(
            reached.is_empty(),
            "{krate} holds a hard limit and can reach a crate that routes to a model: \
             {reached:?}"
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

// --- what a gated kernel decision reaches (R12-RISK-3022) ------------------

/// A kernel production function: its file, the type whose `impl` (or trait)
/// declares it — `None` for a free function — its name, and its
/// [`code_only`] text from its declaration through the end of its body.
struct KernelFn {
    file: String,
    owner: Option<String>,
    name: String,
    body: String,
}

/// The type an `impl` or `trait` header names: `impl<T> Book<T>` is `Book`,
/// `impl Default for Treasury` is `Treasury`, `pub trait Gate: Debug` is
/// `Gate`, and `impl crate::x::Plane` is `Plane`.
fn header_owner(header: &str) -> Option<String> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let header = header.split('{').next()?.split(" where").next()?.trim();
    let target = if let Some(rest) = header
        .strip_prefix("pub ")
        .unwrap_or(header)
        .strip_prefix("trait ")
    {
        rest
    } else {
        let rest = header.strip_prefix("impl")?;
        let rest = match rest.trim_start().strip_prefix('<') {
            // Skip the impl's own generic list.
            Some(generics) => {
                let mut depth = 1usize;
                let close = generics.char_indices().find(|(_, c)| {
                    match c {
                        '<' => depth += 1,
                        '>' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                })?;
                &generics[close.0 + 1..]
            }
            None => rest,
        };
        rest.rsplit_once(" for ").map_or(rest, |(_, target)| target)
    };
    let path = target.trim().split('<').next()?;
    let name: String = path
        .rsplit("::")
        .next()?
        .chars()
        .take_while(|c| ident(*c))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Every function in `code` with the type that declares it, each cut to its
/// own extent — from the line that declares it through the brace that closes
/// its body — rather than to the next declaration as [`function_items`]
/// cuts it, so a struct, an `impl` header or a `const` written after a
/// function is not read as its text. The owner is the `impl` or `trait`
/// opened at the start of a line and closed by a `}` there.
fn owned_function_bodies(code: &str) -> Vec<(Option<String>, String, &str)> {
    let bytes = code.as_bytes();
    let mut bodies = Vec::new();
    let mut owner: Option<String> = None;
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if line.starts_with('}') {
            owner = None;
        }
        let opens_impl = line.starts_with("impl")
            || line.starts_with("trait ")
            || line.starts_with("pub trait ")
            || line.starts_with("unsafe impl");
        if opens_impl {
            owner = header_owner(line.strip_prefix("unsafe ").unwrap_or(line));
        }
        if let Some(name) = function_name(line) {
            let owner = if line.starts_with(|c: char| !c.is_whitespace()) {
                None
            } else {
                owner.clone()
            };
            bodies.push((owner, name, &code[start..item_end(bytes, start)]));
        }
    }
    bodies
}

/// The kernel's production sources, each a file's path and its
/// [`code_only`] text.
fn kernel_sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    rust_files(&crates_root().join("runtime/qip-kernel/src"), &mut files);
    assert!(files.len() > 10, "premise: the kernel's sources were found");
    files
        .iter()
        .map(|path| {
            let source = std::fs::read_to_string(path).expect("kernel source is readable");
            (path.display().to_string(), code_only(&source))
        })
        .collect()
}

fn kernel_functions(sources: &[(String, String)]) -> Vec<KernelFn> {
    sources
        .iter()
        .flat_map(|(file, code)| {
            owned_function_bodies(code)
                .into_iter()
                .map(|(owner, name, body)| KernelFn {
                    file: file.clone(),
                    owner,
                    name,
                    body: body.to_string(),
                })
        })
        .collect()
}

/// `code` with the whitespace on either side of every `.` removed, line
/// breaks included, so a method chain reads the same however it is wrapped:
/// `self\n    .cost_router\n    .determine(` is `self.cost_router.determine(`.
///
/// rustfmt breaks a chain before each `.` once it outgrows a line, and the
/// scans matched the receiver against the text immediately before the `.`.
/// `calls_in`'s `self.` arm stripped a trailing `self` from text that still
/// ended in the newline and indent, so it never matched, and every wrapped
/// own-method call in the kernel was a call the walk did not follow —
/// twenty-seven in production on the day this was written, `Treasury`'s
/// `self\n.check_leverage(` among them — and the model-surface scan's
/// `cost_router.` arm read `self\n.cost_router\n.select(` as no route at
/// all. Assumes
/// [`code_only`] text: no `.` is left inside a comment or a literal for this
/// to join.
fn joined_chains(code: &str) -> String {
    let mut joined = String::with_capacity(code.len());
    let mut held = String::new();
    let mut after_dot = false;
    for c in code.chars() {
        if c.is_whitespace() {
            if !after_dot {
                held.push(c);
            }
            continue;
        }
        if c == '.' {
            after_dot = true;
        } else {
            joined.push_str(&held);
            after_dot = false;
        }
        held.clear();
        joined.push(c);
    }
    joined.push_str(&held);
    joined
}

/// A call [`calls_in`] can follow, and where its definition is looked for.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Call {
    /// `self.name(..)` or `Self::name(..)`: a function of the caller's own
    /// type.
    Own(String),
    /// `Type::name(..)`: a function of the named type.
    Of(String, String),
    /// `name(..)`, `crate::module::name(..)`, `super::name(..)`: a free
    /// function.
    Free(String),
}

/// The calls `body` makes in a form the scan can follow to a kernel
/// function, each with or without a turbofish: a method on `self`, a path
/// through `Self` or a named type, a path through `crate`, `super` or
/// `self`, or a bare call.
///
/// A method on any other receiver — `self.treasury.fund(..)`, `x.check(..)`
/// — is not followed. Its type is not in the text, and following every
/// kernel function of the same name would read `Vec::push` or
/// `LeverageBook::check` as the kernel's own. That is the scan's stated
/// limit: it follows by name as far as the text names the owner. The
/// declaration itself (`fn name(`) is not a call.
///
/// The body is read through [`joined_chains`] first, so a call rustfmt
/// wrapped across lines (`self\n    .helper(..)`) is the same call as its
/// one-line form.
fn calls_in(body: &str) -> BTreeSet<Call> {
    let body = joined_chains(body);
    let body = body.as_str();
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let preceded_by_ident = |text: &str| text.chars().next_back().is_some_and(ident);
    let mut calls = BTreeSet::new();
    let mut cursor = 0;
    while let Some(offset) = body[cursor..].find(|c: char| c.is_ascii_alphabetic() || c == '_') {
        let at = cursor + offset;
        let end = body[at..]
            .find(|c: char| !ident(c))
            .map_or(body.len(), |len| at + len);
        cursor = end;
        if preceded_by_ident(&body[..at]) {
            continue;
        }
        let after = body[end..].trim_start();
        if !(after.starts_with('(') || after.starts_with("::<")) {
            continue;
        }
        let name = body[at..end].to_string();
        let before = body[..at].trim_end();
        if let Some(path) = before.strip_suffix("::") {
            let start = path
                .rfind(|c: char| !(ident(c) || c == ':'))
                .map_or(0, |at| at + 1);
            let path = &path[start..];
            let first = path.split("::").next().unwrap_or_default();
            let last = path.rsplit("::").next().unwrap_or_default();
            if last == "Self" {
                calls.insert(Call::Own(name));
            } else if matches!(first, "crate" | "super" | "self")
                && !last.starts_with(|c: char| c.is_ascii_uppercase())
            {
                calls.insert(Call::Free(name));
            } else if last.starts_with(|c: char| c.is_ascii_uppercase()) {
                calls.insert(Call::Of(last.to_string(), name));
            }
        } else if let Some(receiver) = before.strip_suffix('.') {
            if let Some(head) = receiver.strip_suffix("self")
                && !preceded_by_ident(head)
                && !head.ends_with('.')
            {
                calls.insert(Call::Own(name));
            }
        } else if before.strip_suffix("fn").is_none_or(preceded_by_ident) {
            calls.insert(Call::Free(name));
        }
    }
    calls
}

/// Every kernel function reachable from `roots` through [`calls_in`], each
/// with the chain of names that reaches it, the root first. A call resolves
/// to every kernel function of that name and owner, so a name defined twice
/// on one type (in two `impl` blocks, or behind two `cfg`s) is followed into
/// both.
fn kernel_closure<'a>(
    functions: &'a [KernelFn],
    roots: &[&'a KernelFn],
) -> Vec<(Vec<String>, &'a KernelFn)> {
    let mut index: BTreeMap<(Option<&str>, &str), Vec<usize>> = BTreeMap::new();
    for (at, function) in functions.iter().enumerate() {
        index
            .entry((function.owner.as_deref(), function.name.as_str()))
            .or_default()
            .push(at);
    }
    let position = |function: &KernelFn| {
        functions
            .iter()
            .position(|f| std::ptr::eq(f, function))
            .expect("a root is one of the kernel's functions")
    };
    let mut seen = BTreeSet::new();
    let mut reached = Vec::new();
    let mut queue: std::collections::VecDeque<(Vec<String>, usize)> = roots
        .iter()
        .map(|root| (vec![root.name.clone()], position(root)))
        .collect();
    while let Some((chain, at)) = queue.pop_front() {
        if !seen.insert(at) {
            continue;
        }
        let function = &functions[at];
        for call in calls_in(&function.body) {
            let (owner, name) = match &call {
                Call::Own(name) => (function.owner.as_deref(), name.as_str()),
                Call::Of(owner, name) => (Some(owner.as_str()), name.as_str()),
                Call::Free(name) => (None, name.as_str()),
            };
            for &next in index.get(&(owner, name)).into_iter().flatten() {
                let mut longer = chain.clone();
                longer.push(match owner {
                    Some(owner) => format!("{owner}::{name}"),
                    None => name.to_string(),
                });
                queue.push_back((longer, next));
            }
        }
        reached.push((chain, function));
    }
    reached
}

/// The witnesses a deterministic decision is taken under: the gate a
/// hard-limit call demands and the routing whose `gate()` mints one.
const DETERMINED_WITNESSES: [&str; 2] = ["DeterministicGate", "DeterministicRouting"];

/// The types of the parameters `signature` declares, `self` left out: the
/// text after each parameter's own `:`, split at the parameter list's
/// outermost commas. A generic list before the parameters is skipped, so
/// `fn f<F: Fn(u8)>(..)` is read from its parameters and not from `Fn(`.
fn parameter_types(signature: &str) -> Vec<String> {
    parameters(signature)
        .into_iter()
        .map(|(_, ty)| ty)
        .collect()
}

/// The parameters `signature` declares, `self` left out, each as its
/// pattern and its type — the text either side of the parameter's own `:`
/// — read as [`parameter_types`] describes.
fn parameters(signature: &str) -> Vec<(String, String)> {
    let Some(after_fn) = token_offsets(signature, "fn")
        .first()
        .map(|at| &signature[at + 2..])
    else {
        return Vec::new();
    };
    let mut depth = 0usize;
    let mut open = None;
    let mut previous = ' ';
    for (at, c) in after_fn.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if previous != '-' => depth = depth.saturating_sub(1),
            '(' if depth == 0 => {
                open = Some(at);
                break;
            }
            _ => {}
        }
        previous = c;
    }
    let Some(open) = open else {
        return Vec::new();
    };
    let list = &after_fn[open + 1..];
    let mut parameters = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut previous = ' ';
    for c in list.chars() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            '>' if previous == '-' => {}
            ')' | ']' | '}' | '>' if depth > 0 => depth -= 1,
            ')' if depth == 0 => break,
            ',' if depth == 0 => {
                parameters.push(std::mem::take(&mut current));
                previous = c;
                continue;
            }
            _ => {}
        }
        current.push(c);
        previous = c;
    }
    parameters.push(current);
    parameters
        .iter()
        .filter_map(|parameter| {
            // The parameter's own `:`, not one inside a `::` path.
            let bytes = parameter.as_bytes();
            let colon = (0..bytes.len()).find(|&at| {
                bytes[at] == b':'
                    && bytes.get(at + 1) != Some(&b':')
                    && (at == 0 || bytes[at - 1] != b':')
            })?;
            Some((
                parameter[..colon].trim().to_string(),
                parameter[colon + 1..].trim().to_string(),
            ))
        })
        .collect()
}

/// Whether `signature` takes a [`DETERMINED_WITNESSES`] value — by
/// reference, by value, under a path or inside another type — whatever the
/// parameter is called. A function that only returns one does not.
fn takes_a_determined_witness(signature: &str) -> bool {
    parameter_types(signature).iter().any(|ty| {
        DETERMINED_WITNESSES
            .iter()
            .any(|witness| !token_offsets(ty, witness).is_empty())
    })
}

/// The kernel production functions that take a deterministic gate or a
/// deterministic routing as a parameter, by its type and under any name:
/// the hard-limit decisions handed a witness by their caller.
///
/// Not every gated decision is a root. A function that mints its gate
/// inline, asking the router and calling `.gate()` itself before handing
/// it on, takes no witness parameter and is not walked from here:
/// `Platform::submit_released` (the desk's order path into
/// `OrderManager::submit`) and the breadth factory's `install_breadth`
/// (into `admit_ladder`). The advice, model-surface and field scans rooted
/// here therefore do not cover the order manager's pre-trade path. That is
/// a known gap, recorded on R12-RISK-3022, not a property these scans
/// hold.
///
/// The roots were once found by `gate: &DeterministicGate` alone, so
/// `Platform::observe_book_under_routing`, which takes the
/// `DeterministicRouting` by value and runs the risk monitor whose verdict
/// engages the kill switch, was never walked, and nor was any decision
/// whose gate parameter was given another name.
fn gated_kernel_functions(functions: &[KernelFn]) -> Vec<&KernelFn> {
    functions
        .iter()
        .filter(|function| takes_a_determined_witness(signature_of(&function.body)))
        .collect()
}

/// The roots are found by the witness's type, under any parameter name: a
/// routing taken by value, a gate under a path and another name, a routing
/// behind a generic list whose bound holds a `(`, and the gate under its
/// usual name are roots, and what the first reaches is walked; a function
/// that returns a routing, or takes a type whose name merely begins with
/// the witness's (`DeterministicGateRecord`), is not.
///
/// The failure this prevents: roots were found by `gate: &DeterministicGate`
/// only, so the kernel's one caller of `RiskMonitor::observe`,
/// `observe_book_under_routing(routing: DeterministicRouting, ..)`, was
/// outside both kernel walks.
///
/// Mutations: root by `demands_the_gate` again — the routing, renamed-gate
/// and generic roots are lost and the chain through `observe` is not
/// reported; match the witness anywhere in the signature rather than in
/// parameter types — the function returning a routing becomes a root;
/// match the witness as a substring rather than a delimited token —
/// `record` becomes a root.
#[test]
fn the_kernel_walk_roots_every_function_taking_a_gate_or_a_routing_by_type() {
    let fixture = "impl Platform {\n    \
                   fn observe(&mut self, routing: DeterministicRouting, now: Timestamp) {\n        \
                   self.helper(now);\n    }\n    \
                   fn helper(&self, now: Timestamp) {\n        \
                   let served = self.model_provider.serve(&artifact);\n    }\n    \
                   fn renamed(&mut self, witness: &qip_risk::gate::DeterministicGate) {}\n    \
                   fn generic<F: Fn(u8) -> u8>(&self, f: F, r: &mut DeterministicRouting) {}\n    \
                   fn usual(&mut self, gate: &DeterministicGate, now: Timestamp) {}\n    \
                   fn determine(&self, subject: &str) -> Result<DeterministicRouting> {}\n    \
                   fn record(&self, gate: &DeterministicGateRecord, now: Timestamp) {}\n}\n";
    assert_eq!(
        parameter_types("fn generic<F: Fn(u8) -> u8>(&self, f: F, r: &mut X<A, B>) {"),
        ["F", "&mut X<A, B>"],
        "premise: the parameter list is read past the generic list"
    );
    let sources = vec![("fixture.rs".to_string(), code_only(fixture))];
    let functions = kernel_functions(&sources);
    let roots: BTreeSet<&str> = gated_kernel_functions(&functions)
        .into_iter()
        .map(|root| root.name.as_str())
        .collect();
    assert_eq!(
        roots,
        BTreeSet::from(["observe", "renamed", "generic", "usual"]),
        "the roots are not every function taking a gate or a routing by type"
    );
    let reported: BTreeSet<String> = gated_model_reach(&functions).into_iter().collect();
    assert_eq!(
        reported,
        BTreeSet::from([
            "observe -> Platform::helper names [\"model_provider\", \"serve\"] (fixture.rs)"
                .to_string()
        ]),
        "a decision under a routing reached a model unseen"
    );
}

/// The calls the walk follows, and the ones it says it does not, on a
/// fixture: an own method, a `Self::` path, a named type's function, a
/// `crate::` path and a bare call are followed into the definition of that
/// name and owner; a method on another receiver and a same-named function of
/// another type are not.
///
/// Mutations: resolve every call by name alone (drop the owner from the
/// index key) — `Other::own` is reached; drop the `crate::` path arm of
/// `calls_in` — `free_leaf` is not reached. By name alone the kernel scans
/// below fail too: a `Self::new` in some `Default` impl reaches
/// `Platform::new`, which is why the owner is part of the key.
#[test]
fn the_kernel_walk_follows_calls_by_name_and_owner() {
    let fixture = "impl Platform {\n    fn root(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   self.own();\n        Self::path();\n        Book::make();\n        \
                   crate::module::free_leaf();\n        bare();\n        \
                   self.field.leaf();\n    }\n    fn own(&self) {}\n    fn path() {}\n}\n\
                   impl Book {\n    fn make() {}\n}\n\
                   impl Other {\n    fn leaf(&self) {}\n    fn own(&self) {}\n}\n\
                   fn free_leaf() {}\nfn bare() {}\n";
    let sources = vec![("fixture.rs".to_string(), code_only(fixture))];
    let functions = kernel_functions(&sources);
    let roots = gated_kernel_functions(&functions);
    assert_eq!(roots.len(), 1, "premise: one gated root");
    let reached: BTreeSet<(Option<String>, String)> = kernel_closure(&functions, &roots)
        .into_iter()
        .map(|(_, function)| (function.owner.clone(), function.name.clone()))
        .collect();
    let named = |owner: Option<&str>, name: &str| (owner.map(str::to_string), name.to_string());
    let expected: BTreeSet<_> = [
        named(Some("Platform"), "root"),
        named(Some("Platform"), "own"),
        named(Some("Platform"), "path"),
        named(Some("Book"), "make"),
        named(None, "free_leaf"),
        named(None, "bare"),
    ]
    .into_iter()
    .collect();
    assert_eq!(reached, expected);
    assert!(
        !reached.contains(&named(Some("Other"), "own"))
            && !reached.contains(&named(Some("Other"), "leaf")),
        "a same-named method of another type, or a method on another receiver, was followed"
    );
}

/// A call rustfmt wrapped across lines is followed as its one-line form is:
/// `self\n    .helper(..)` reaches `Platform::helper`, a wrapped chain
/// `self\n    .adjust(..)\n    .unwrap_or_default()` reaches `adjust`, and a
/// wrapped `self\n    .cost_router\n    .select(` is reported as the
/// router's model arm. A wrapped method on a field
/// (`self\n    .field\n    .leaf()`) is still not followed: joining the
/// lines must not turn another receiver into `self`.
///
/// The failure this prevents: the `self.` arm of [`calls_in`] stripped a
/// trailing `self` without trimming the line break before it, so none of
/// the kernel's twenty-seven wrapped own-method calls was followed and a
/// gated decision could reach a model through any of them unseen.
///
/// Mutations: make [`joined_chains`] return its input unchanged, or drop
/// its call from `calls_in` — the wrapped call reads as no call at all
/// (`left: {}`); drop its call from `model_surface_named` alone — the
/// router arm goes unreported.
#[test]
fn the_kernel_walk_follows_a_call_rustfmt_wrapped_across_lines() {
    let fixture = "impl Platform {\n    \
                   fn decide(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   let figures = self\n            .helper(now);\n        \
                   let lot = self\n            .adjust(now)\n            .unwrap_or_default();\n        \
                   self\n            .field\n            .leaf();\n    }\n    \
                   fn helper(&self, now: Timestamp) {}\n    \
                   fn adjust(&self, now: Timestamp) {}\n    \
                   fn leaf(&self) {}\n    \
                   fn judged(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   let placed = self\n            .cost_router\n            .select(&context);\n    }\n}\n";
    assert_eq!(
        calls_in("let figures = self.helper(now);"),
        BTreeSet::from([Call::Own("helper".to_string())]),
        "premise: the one-line form is followed"
    );
    assert_eq!(
        calls_in("let figures = self\n            .helper(now);"),
        calls_in("let figures = self.helper(now);"),
        "the wrapped form is not the same call as the one-line form"
    );
    let sources = vec![("fixture.rs".to_string(), code_only(fixture))];
    let functions = kernel_functions(&sources);
    let decide = functions
        .iter()
        .find(|function| function.name == "decide")
        .expect("premise: the fixture's root was split out");
    let roots: Vec<&KernelFn> = vec![decide];
    let reached: BTreeSet<String> = kernel_closure(&functions, &roots)
        .into_iter()
        .map(|(_, function)| function.name.clone())
        .collect();
    assert_eq!(
        reached,
        BTreeSet::from(["decide", "helper", "adjust"].map(str::to_string)),
        "a wrapped own-method call was not followed, or a wrapped field method was"
    );
    let reported: BTreeSet<String> = gated_model_reach(&functions).into_iter().collect();
    assert_eq!(
        reported,
        BTreeSet::from(["judged names [\"cost_router.select\"] (fixture.rs)".to_string()]),
        "a wrapped `self.cost_router.select(` went unreported"
    );
}

// --- the Risk Brain recommends (R12-RISK-3022) -----------------------------

/// What the Risk Brain hands the desk, by every name a function would have to
/// use to read it: the cycle's `RiskDiscovery` (`qip_kernel::cycle`), its
/// parts, the passes that compute it, and the risk control agent
/// (`qip_investment_agents::control::RiskControl`), whose finding reaches
/// the kernel only as one of the organisation's.
const RISK_BRAIN_ADVICE: [&str; 9] = [
    "RiskDiscovery",
    "risk_discovery",
    "last_risk_discovery",
    "discover_risk",
    "EmergingRisk",
    "ExposureFinding",
    "with_emerging_since",
    "market_exposure_findings",
    "RiskControl",
];

/// Which of [`RISK_BRAIN_ADVICE`] `code` names, plus `ids::RISK` — the risk
/// control's agent id, the one way to pick its finding out of a report.
fn advice_named(code: &str) -> Vec<String> {
    let mut named: Vec<String> = RISK_BRAIN_ADVICE
        .iter()
        .filter(|token| !token_offsets(code, token).is_empty())
        .map(|token| token.to_string())
        .collect();
    if token_offsets(code, "RISK")
        .into_iter()
        .any(|at| code[..at].ends_with("ids::"))
    {
        named.push("ids::RISK".to_string());
    }
    named
}

/// The crates that hold a hard limit outside the kernel, the production file
/// each limit lives in, and what that file must declare for the row to be
/// about the limit it names. Every crate-dependency check reads this list:
/// the advice reach below, the model reach, and the scratch-manifest proof
/// that a model or advice dependency added to any of them is seen.
///
/// `qip-survival` (`SurvivalGovernor::size` and `seed`, the survival
/// ceiling) and `qip-treasury` (`LeverageBook::check`, the leverage limit on
/// every financed requirement) were absent from it, so a `qip-ai` or
/// `qip-investment-agents` dependency added to either failed no test while
/// the kernel scans held the kernel's calls into both gated.
const HARD_LIMIT_CRATES: [(&str, &str, &[&str]); 5] = [
    (
        "qip-execution-engine",
        "backend/crates/services/qip-execution-engine/src/oms.rs",
        &["fn submit(", "fn replace(", "fn admit_ladder("],
    ),
    (
        "qip-risk-engine",
        "backend/crates/services/qip-risk-engine/src/pretrade.rs",
        &["impl PreTradeChecker", "fn check("],
    ),
    (
        "qip-risk-engine",
        "backend/crates/services/qip-risk-engine/src/autonomy.rs",
        &["fn trip_scope_under(", "fn trip_scope(", "fn trip_global("],
    ),
    (
        "qip-survival",
        "backend/crates/services/qip-survival/src/survival.rs",
        &["impl SurvivalGovernor", "pub fn size(", "pub fn seed("],
    ),
    (
        "qip-treasury",
        "backend/crates/services/qip-treasury/src/treasury.rs",
        &["impl LeverageBook", "pub fn check("],
    ),
];

/// The crates that own the Risk Brain's advice: the kernel (`RiskDiscovery`)
/// and the investment agents (`RiskControl`).
const ADVICE_OWNERS: [&str; 2] = ["qip-kernel", "qip-investment-agents"];

/// Every path by which a [`HARD_LIMIT_CRATES`] crate reaches a model crate
/// or a crate owning the Risk Brain's advice, through normal dependencies.
fn hard_limit_crate_reach(manifests: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    HARD_LIMIT_CRATES
        .iter()
        .flat_map(|(krate, _, _)| {
            let mut reached = crates_reached(manifests, krate, &MODEL_CRATES);
            reached.extend(crates_reached(manifests, krate, &ADVICE_OWNERS));
            reached
        })
        .collect()
}

/// A scratch copy of `qip-survival`'s and `qip-treasury`'s real manifests,
/// each given one `qip-ai` or one `qip-investment-agents` line, read back
/// through the same parser the workspace scan uses: each of the four
/// additions must be reported, as the direct edge from that crate.
///
/// The failure this prevents: neither crate was a root of any crate check,
/// so the survival ceiling or the leverage limit could have linked the
/// language-model crate or the risk control agent and failed nothing.
///
/// Mutations: drop the `qip-survival` row from [`HARD_LIMIT_CRATES`] — the
/// membership assertion fires; drop the [`MODEL_CRATES`] arm of
/// [`hard_limit_crate_reach`] — the `qip-ai` addition goes unseen; drop its
/// [`ADVICE_OWNERS`] arm — the agents addition goes unseen, though it still
/// reaches `qip-ai` two links down, because the direct edge is what is
/// asserted. On the real tree, `qip-ai.workspace = true` written into
/// `qip-survival`'s manifest fails the model check, and
/// `qip-investment-agents.workspace = true` in `qip-treasury`'s fails the
/// advice check and the model check both.
#[test]
fn a_model_or_advice_dependency_added_to_the_survival_or_treasury_crate_is_seen() {
    let root = std::fs::read_to_string(crates_root().join("../Cargo.toml"))
        .expect("the workspace manifest is readable");
    let inherited = workspace_packages(&root);
    let manifests = workspace_dependencies();
    assert!(
        hard_limit_crate_reach(&manifests).is_empty(),
        "premise: no hard-limit crate reaches a model or advice crate today"
    );
    for (krate, manifest) in [
        (
            "qip-survival",
            "backend/crates/services/qip-survival/Cargo.toml",
        ),
        (
            "qip-treasury",
            "backend/crates/services/qip-treasury/Cargo.toml",
        ),
    ] {
        assert!(
            HARD_LIMIT_CRATES.iter().any(|(name, _, _)| *name == krate),
            "{krate} holds a hard limit and is not a row of HARD_LIMIT_CRATES"
        );
        let text = qip_acceptance::read(manifest);
        for (line, added) in [
            ("qip-ai.workspace = true", "qip-ai"),
            (
                "agents = { path = \"../../agents/qip-investment-agents\", package = \
                 \"qip-investment-agents\" }",
                "qip-investment-agents",
            ),
        ] {
            let scratch_text =
                text.replacen("[dependencies]\n", &format!("[dependencies]\n{line}\n"), 1);
            assert_ne!(
                scratch_text, text,
                "premise: {manifest} has a [dependencies] table the scratch line joined"
            );
            let mut scratch = manifests.clone();
            scratch.insert(
                krate.to_string(),
                normal_dependencies(&scratch_text, &inherited),
            );
            let reached = hard_limit_crate_reach(&scratch);
            let edge = format!("{krate} -> {added}");
            assert!(
                reached.contains(&edge),
                "`{line}` added to {manifest} went unseen: {reached:?}"
            );
        }
    }
}

/// R12-RISK-3022 (b): "the Risk Brain only recommends", held for the crates
/// that make the hard-limit decisions outside the kernel. The order manager
/// (`submit`, `replace`, `admit_ladder`), `PreTradeChecker::check` and the
/// kill switch's trips live in crates that link neither `qip-kernel`, which
/// owns `RiskDiscovery`, nor `qip-investment-agents`, which owns
/// `RiskControl`, so none of them can name either value: the compiler holds
/// it, not a reviewer.
///
/// The failure this prevents: no test failed if a hard limit began
/// enforcing the brain's advice, so a discovery or a control finding wired
/// into a refusal would have been a second gate nobody had reviewed as one.
///
/// Mutation: the scratch copy below adds `qip-investment-agents` beneath
/// `qip-risk`, and the walk must find it.
#[test]
fn the_crates_holding_the_hard_limits_cannot_name_the_risk_brains_advice() {
    // Premise: each hard limit is where this says, and each piece of advice
    // is owned where this says.
    for (_, file, declarations) in HARD_LIMIT_CRATES {
        let source = qip_acceptance::read(file);
        for declaration in declarations {
            assert!(
                source.contains(declaration),
                "premise: {file} declares `{declaration}`"
            );
        }
    }
    assert!(
        qip_acceptance::read("backend/crates/runtime/qip-kernel/src/cycle.rs")
            .contains("pub struct RiskDiscovery {")
    );
    assert!(
        qip_acceptance::read("backend/crates/agents/qip-investment-agents/src/control.rs")
            .contains("pub struct RiskControl {")
    );

    let manifests = workspace_dependencies();
    for (krate, _, _) in HARD_LIMIT_CRATES {
        let reached = crates_reached(&manifests, krate, &ADVICE_OWNERS);
        assert!(
            reached.is_empty(),
            "{krate} holds a hard limit and can reach a crate that owns the Risk Brain's advice, \
             so the limit could read a RiskDiscovery or a RiskControl finding: {reached:?}"
        );
    }

    let mut scratch = manifests.clone();
    scratch
        .get_mut("qip-risk")
        .expect("qip-risk is a workspace crate")
        .insert("qip-investment-agents".to_string());
    let reached = crates_reached(&scratch, "qip-execution-engine", &ADVICE_OWNERS);
    assert!(
        reached
            .iter()
            .any(|path| path.ends_with("-> qip-risk -> qip-investment-agents")),
        "an advice edge added below the order manager went unseen: {reached:?}"
    );
}

/// R12-RISK-3022 (b) inside the kernel: no function that makes a hard-limit
/// decision under the deterministic gate — the survival ceiling and sizing,
/// sign-off, release, the central plane's halt, and whatever demands the
/// gate next — nor any kernel function it reaches by name, reads the cycle's
/// `RiskDiscovery` or the risk control's finding.
///
/// The roots are found by signature, not listed, so a gated decision added
/// later is covered without a row here; the premise pins the ones that exist.
///
/// Mutation: in `Platform::sign_off_drafts`, read
/// `self.last_risk_discovery.as_ref()` — this fails naming the chain
/// `sign_off_drafts` and `last_risk_discovery`.
#[test]
fn no_gated_kernel_decision_reads_the_risk_brains_advice() {
    let sources = kernel_sources();
    let functions = kernel_functions(&sources);
    let roots = gated_kernel_functions(&functions);
    let root_names: BTreeSet<&str> = roots.iter().map(|root| root.name.as_str()).collect();
    for (function, _, _, _) in KERNEL_HARD_LIMIT_CALLS {
        assert!(
            root_names.contains(function),
            "premise: `{function}` demands the gate and is a root: {root_names:?}"
        );
    }
    assert!(
        root_names.contains("ingest"),
        "premise: the plane's gated halt is a root"
    );
    assert!(
        root_names.contains("observe_book_under_routing"),
        "premise: the risk monitor's pass, taken under a routing, is a root"
    );
    let closure = kernel_closure(&functions, &roots);
    assert!(
        closure.len() > roots.len(),
        "premise: the walk followed the roots into what they call"
    );
    let readers: Vec<String> = closure
        .iter()
        .filter_map(|(chain, function)| {
            let named = advice_named(&function.body);
            (!named.is_empty())
                .then(|| format!("{} reads {named:?} ({})", chain.join(" -> "), function.file))
        })
        .collect();
    assert!(
        readers.is_empty(),
        "a gated hard-limit decision reads what the Risk Brain only recommends, so the advice \
         has become a second gate nobody reviewed as one: {readers:?}"
    );
}

/// Every kernel production function that names the Risk Brain's advice, and
/// what it does with it. Each is advisory: it derives the discovery, carries
/// it on a report, journals it, or starts the platform with none. No kernel
/// function names the risk control's type or id: its finding reaches the
/// kernel only inside the organisation's report, as one of the panel's.
const ADVICE_READERS: [(Option<&str>, &str, &str); 7] = [
    (
        Some("Platform"),
        "discover_risk",
        "derives it after every stage has run",
    ),
    (
        Some("Platform"),
        "run_cycle",
        "carries it on the cycle report",
    ),
    (
        Some("Platform"),
        "refuse_cycle_before",
        "carries an unmeasured one on a refusal",
    ),
    (
        Some("Platform"),
        "journal_cycle",
        "journals it with the cycle",
    ),
    (
        Some("Platform"),
        "with_model_provider",
        "starts the platform with none",
    ),
    (
        Some("RiskDiscovery"),
        "with_emerging_since",
        "derives what emerged",
    ),
    (
        None,
        "market_exposure_findings",
        "derives the basis and gap findings",
    ),
];

/// R12-RISK-3022 (b): every place the kernel consumes the Risk Brain's
/// advice is a row of [`ADVICE_READERS`] — derive, report, journal — and
/// nothing else names it. A new reader is a new row, so whoever adds one
/// states what it does with the advice, and a reader added inside a gate
/// fails both this and the closure test above.
///
/// Mutation: read `self.last_risk_discovery` in `size_survival` — it is not a
/// row and this fails naming it.
#[test]
fn every_kernel_reader_of_the_risk_brains_advice_derives_reports_or_journals_it() {
    let sources = kernel_sources();
    let functions = kernel_functions(&sources);
    let found: BTreeSet<(Option<String>, String)> = functions
        .iter()
        .filter(|function| !advice_named(&function.body).is_empty())
        .map(|function| (function.owner.clone(), function.name.clone()))
        .collect();
    let expected: BTreeSet<(Option<String>, String)> = ADVICE_READERS
        .iter()
        .map(|(owner, name, _)| (owner.map(str::to_string), name.to_string()))
        .collect();
    assert_eq!(
        found, expected,
        "the kernel functions naming the Risk Brain's advice are not the advisory readers this \
         suite lists; a reader that is not deriving, reporting or journaling it may be enforcing it"
    );
    assert!(
        functions
            .iter()
            .all(|function| !advice_named(&function.body)
                .iter()
                .any(|named| named == "RiskControl" || named == "ids::RISK")),
        "a kernel function names the risk control's type or id"
    );
}

// --- the advice laundered through a field (R12-RISK-3022) ------------------

/// The index of the bracket closing the `(`, `[` or `{` at `open` in `text`.
fn closing_bracket(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (at, byte) in text.bytes().enumerate().skip(open) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// `text` split at its outermost commas, each part trimmed and empty parts
/// dropped: outside every `()`, `[]` and `{}`, and outside every `<..>` when
/// `types` (a field list, where `->` closes nothing) or only a turbofish's
/// when not (an expression, where a `<` may be a comparison).
fn outermost_parts(text: &str, types: bool) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let (mut depth, mut angles, mut start) = (0usize, 0usize, 0);
    for (at, &byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' if types || angles > 0 || (at >= 2 && &bytes[at - 2..at] == b"::") => {
                angles += 1;
            }
            b'>' if angles > 0 && (at == 0 || bytes[at - 1] != b'-') => angles -= 1,
            b',' if depth == 0 && angles == 0 => {
                parts.push(text[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts.retain(|part| !part.is_empty());
    parts
}

/// `text` split at its first `:` that is not half of a `::`.
fn split_at_colon(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let colon = (0..bytes.len()).find(|&at| {
        bytes[at] == b':' && bytes.get(at + 1) != Some(&b':') && (at == 0 || bytes[at - 1] != b':')
    })?;
    Some((&text[..colon], &text[colon + 1..]))
}

/// Whether `text` ends with `word` as a whole identifier.
fn ends_with_word(text: &str, word: &str) -> bool {
    text.strip_suffix(word).is_some_and(|head| {
        !head
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Each field `declaration` (`pub struct Platform {`) opens in `code`, a
/// [`code_only`] text, as its name and its declared type, attributes and
/// visibility dropped.
fn struct_fields(code: &str, declaration: &str) -> Vec<(String, String)> {
    let Some(at) = code.find(declaration) else {
        return Vec::new();
    };
    let open = at + declaration.len() - 1;
    let Some(close) = closing_bracket(code, open) else {
        return Vec::new();
    };
    outermost_parts(&code[open + 1..close], true)
        .into_iter()
        .filter_map(|field| {
            let mut field = field;
            while field.starts_with("#[") {
                field = field[closing_bracket(field, 1)? + 1..].trim_start();
            }
            if let Some(rest) = field.strip_prefix("pub") {
                field = match rest.trim_start().strip_prefix('(') {
                    Some(scope) => scope.split_once(')')?.1,
                    None => rest,
                }
                .trim_start();
            }
            let (name, ty) = split_at_colon(field)?;
            Some((name.trim().to_string(), ty.trim().to_string()))
        })
        .collect()
}

/// Where the expression beginning at `start` in `text` ends: the first byte
/// of `stops` outside every bracket opened after `start`, a bracket closing
/// one opened before it, or — when `at_block` — the first `{` outside every
/// bracket, which opens the block of an `if let`, a `while let` or a `for`.
fn expression_end(text: &str, start: usize, stops: &[u8], at_block: bool) -> usize {
    let mut depth = 0usize;
    for (at, byte) in text.bytes().enumerate().skip(start) {
        match byte {
            b'{' if at_block && depth == 0 => return at,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => match depth.checked_sub(1) {
                Some(less) => depth = less,
                None => return at,
            },
            _ if depth == 0 && stops.contains(&byte) => return at,
            _ => {}
        }
    }
    text.len()
}

/// The names a pattern binds: each identifier that is not a keyword, a
/// type or variant (capitalised), a path segment, a call, or a struct
/// pattern's field name before its `:`.
fn pattern_bindings(pattern: &str) -> Vec<String> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut names = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = pattern[cursor..].find(|c: char| c.is_ascii_alphabetic() || c == '_') {
        let at = cursor + offset;
        let end = pattern[at..]
            .find(|c: char| !ident(c))
            .map_or(pattern.len(), |length| at + length);
        cursor = end;
        let name = &pattern[at..end];
        let before = &pattern[..at];
        let after = pattern[end..].trim_start();
        if before.chars().next_back().is_some_and(ident)
            || before.ends_with("::")
            || before.ends_with('.')
            || name.starts_with(|c: char| c.is_ascii_uppercase())
            || matches!(name, "_" | "mut" | "ref" | "box")
            || after.starts_with(['(', '{', '!'])
            || after.starts_with("::")
            || (after.starts_with(':') && !after.starts_with("::"))
        {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

/// Each binding `body` (a [`joined_chains`] text) makes, as the pattern and
/// the text it binds from: every `let` — plain, `if let`, `while let`,
/// let-else and a chained `&& let` — with its type annotation counted as
/// source, and every `for` loop's pattern with its iterator.
fn local_bindings(body: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for at in token_offsets(body, "let") {
        let head = body[..at].trim_end();
        let conditional =
            ends_with_word(head, "if") || ends_with_word(head, "while") || head.ends_with("&&");
        let start = at + "let".len();
        let bytes = body.as_bytes();
        let mut depth = 0usize;
        let mut equals = None;
        for index in start..bytes.len() {
            match bytes[index] {
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b'>' if bytes[index - 1] != b'-' => depth = depth.saturating_sub(1),
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b';' if depth == 0 => break,
                b'=' if depth == 0
                    && !matches!(bytes.get(index + 1), Some(b'=' | b'>'))
                    && !matches!(bytes[index - 1], b'=' | b'!' | b'<' | b'>') =>
                {
                    equals = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let Some(equals) = equals else {
            continue;
        };
        let end = expression_end(body, equals + 1, b";", conditional);
        let declared = &body[start..equals];
        let (pattern, ty) = split_at_colon(declared).unwrap_or((declared, ""));
        found.push((
            pattern.to_string(),
            format!("{ty} {}", &body[equals + 1..end]),
        ));
    }
    for at in token_offsets(body, "for") {
        let rest = &body[at + "for".len()..];
        let Some(&within) = token_offsets(rest, "in").first() else {
            continue;
        };
        let pattern = &rest[..within];
        if pattern.contains(['{', '}', ';', '=']) {
            continue;
        }
        let start = at + "for".len() + within + "in".len();
        let end = expression_end(body, start, b";", true);
        found.push((pattern.to_string(), body[start..end].to_string()));
    }
    found
}

/// Every offset in `text` (a [`joined_chains`] text) where the field
/// `field` of `self` is named — `self.field`, not `self.field(..)`, which
/// is a method, nor `x.self.field`.
fn self_field_mentions(text: &str, field: &str) -> Vec<usize> {
    token_offsets(text, field)
        .into_iter()
        .filter(|&at| {
            text[..at].strip_suffix("self.").is_some_and(|head| {
                !head
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            }) && !text[at + field.len()..].trim_start().starts_with('(')
        })
        .collect()
}

/// Whether `text` names the Risk Brain's advice ([`advice_named`]), one of
/// `locals` other than as a field of something, or `self.` one of `fields`.
fn names_held_advice(text: &str, locals: &BTreeSet<String>, fields: &BTreeSet<String>) -> bool {
    !advice_named(text).is_empty()
        || locals.iter().any(|local| {
            token_offsets(text, local)
                .into_iter()
                .any(|at| !text[..at].ends_with('.'))
        })
        || fields
            .iter()
            .any(|field| !self_field_mentions(text, field).is_empty())
}

/// The locals the function `body` (a [`joined_chains`] text) binds from the
/// Risk Brain's advice: each parameter whose type names it, then each
/// [`local_bindings`] pattern whose source names it, a local already bound
/// from it, or `self.` one of `fields` — to a fixed point.
fn advice_locals(body: &str, fields: &BTreeSet<String>) -> BTreeSet<String> {
    let mut locals: BTreeSet<String> = parameters(signature_of(body))
        .into_iter()
        .filter(|(_, ty)| !advice_named(ty).is_empty())
        .flat_map(|(pattern, _)| pattern_bindings(&pattern))
        .collect();
    let bindings = local_bindings(body);
    loop {
        let before = locals.len();
        for (pattern, source) in &bindings {
            if names_held_advice(source, &locals, fields) {
                locals.extend(pattern_bindings(pattern));
            }
        }
        if locals.len() == before {
            return locals;
        }
    }
}

/// Every field of `self` the function `body` (a [`joined_chains`] text)
/// assigns, with the text assigned: `self.f = ..`, `self.f += ..` and the
/// other compound forms, `self.f.g = ..` and `self.f[i] = ..` (as `f`), and
/// each `f: ..` or shorthand `f` of a `Self { .. }` or `Platform { .. }`
/// literal. A mutating method — `self.f.push(..)`, `self.f.insert(..)` — is
/// not an assignment here; see the limits on
/// [`no_gated_kernel_decision_reads_a_field_the_risk_brains_advice_was_copied_into`].
fn field_assignments(body: &str) -> Vec<(String, String)> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut found = Vec::new();
    for at in token_offsets(body, "self") {
        let Some(rest) = body[at + "self".len()..].strip_prefix('.') else {
            continue;
        };
        let field: String = rest.chars().take_while(|c| ident(*c)).collect();
        if field.is_empty() || body[..at].ends_with('.') {
            continue;
        }
        let mut cursor = at + "self.".len() + field.len();
        if body[cursor..].trim_start().starts_with('(') {
            continue;
        }
        loop {
            let tail = &body[cursor..];
            if let Some(after) = tail.strip_prefix('.') {
                let length = after.chars().take_while(|c| ident(*c)).count();
                let next = after[length..].trim_start();
                if length == 0 || next.starts_with('(') || next.starts_with("::") {
                    break;
                }
                cursor += 1 + length;
            } else if tail.starts_with('[') {
                let Some(close) = closing_bracket(body, cursor) else {
                    break;
                };
                cursor = close + 1;
            } else {
                break;
            }
        }
        let after = body[cursor..].trim_start();
        let operator = ["<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "|=", "&=", "^="]
            .iter()
            .find(|operator| after.starts_with(**operator))
            .map(|operator| operator.len())
            .or_else(|| {
                (after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>"))
                    .then_some(1)
            });
        let Some(length) = operator else {
            continue;
        };
        let start = body.len() - after.len() + length;
        let end = expression_end(body, start, b";,", false);
        found.push((field, body[start..end].trim().to_string()));
    }
    for name in ["Self", "Platform"] {
        for at in token_offsets(body, name) {
            let head = body[..at].trim_end();
            let trimmed = body[at + name.len()..].trim_start();
            if !trimmed.starts_with('{')
                || head.ends_with("->")
                || head.ends_with("::")
                || ends_with_word(head, "impl")
                || ends_with_word(head, "for")
                || ends_with_word(head, "struct")
            {
                continue;
            }
            let open = body.len() - trimmed.len();
            let Some(close) = closing_bracket(body, open) else {
                continue;
            };
            for part in outermost_parts(&body[open + 1..close], false) {
                if part.starts_with("..") {
                    continue;
                }
                match split_at_colon(part) {
                    Some((field, source)) => {
                        found.push((field.trim().to_string(), source.trim().to_string()));
                    }
                    None if part.chars().all(ident) => {
                        found.push((part.to_string(), part.to_string()));
                    }
                    None => {}
                }
            }
        }
    }
    found
}

/// The fields of `Platform` (`fields`, as [`struct_fields`] reads them) the
/// Risk Brain's advice can be held in, each with how: its declared type
/// names the advice, or a `Platform` method assigns or initialises it from
/// text naming the advice, a local bound from it, or a field already held —
/// to a fixed point, so a copy of a copy is held too.
fn advice_fields(fields: &[(String, String)], functions: &[KernelFn]) -> BTreeMap<String, String> {
    let declared: BTreeSet<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
    let mut held: BTreeMap<String, String> = fields
        .iter()
        .filter(|(_, ty)| !advice_named(ty).is_empty())
        .map(|(name, ty)| (name.clone(), format!("declared `{ty}`")))
        .collect();
    let methods: Vec<(&str, String)> = functions
        .iter()
        .filter(|function| function.owner.as_deref() == Some("Platform"))
        .map(|function| (function.name.as_str(), joined_chains(&function.body)))
        .collect();
    loop {
        let before = held.len();
        let names: BTreeSet<String> = held.keys().cloned().collect();
        for (method, body) in &methods {
            let locals = advice_locals(body, &names);
            for (field, source) in field_assignments(body) {
                if declared.contains(field.as_str())
                    && !held.contains_key(&field)
                    && names_held_advice(&source, &locals, &names)
                {
                    held.insert(field, format!("assigned in `{method}` from `{source}`"));
                }
            }
        }
        if held.len() == before {
            return held;
        }
    }
}

/// Every chain from a gated kernel function to a `Platform` method in its
/// closure that names `self.` a field [`advice_fields`] holds, as
/// `root -> .. -> method reads self.field, <how> (file)`.
fn gated_advice_field_reads(fields: &[(String, String)], functions: &[KernelFn]) -> Vec<String> {
    let held = advice_fields(fields, functions);
    let roots = gated_kernel_functions(functions);
    kernel_closure(functions, &roots)
        .into_iter()
        .filter(|(_, function)| function.owner.as_deref() == Some("Platform"))
        .flat_map(|(chain, function)| {
            let body = joined_chains(&function.body);
            held.iter()
                .filter(|(field, _)| !self_field_mentions(&body, field).is_empty())
                .map(|(field, how)| {
                    format!(
                        "{} reads self.{field}, {how} ({})",
                        chain.join(" -> "),
                        function.file
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The field scan on a fixture: `run_cycle` copies a part of the cycle's
/// `RiskDiscovery` into a plain `Option<Decimal>` through an `if let`, a
/// `for` loop copies a finding into a `Vec` slot, a second method copies
/// the first copy on, and a constructor taking a `&RiskDiscovery` seeds a
/// field in a `Self { .. }` literal; the gated `size_survival` and the
/// helper it calls read the four copies. None of it names the advice inside
/// the gate, so the existing closure scan passes it — asserted first — and
/// this scan reports each read and nothing else: not the honest field, not
/// a field only ever given `BTreeMap::new()`.
///
/// The failure this prevents: advice laundered through a field passed every
/// scan. `run_cycle` may name the discovery (it is an advisory reader), and
/// `size_survival` names only `self.concentration_hint`.
///
/// Mutations: drop the `self.f = ..` arm of [`field_assignments`] — the
/// `run_cycle` copies are lost; make [`advice_locals`] bind nothing — the
/// copy through `drivers` is lost; stop [`advice_fields`] after one pass —
/// the copy of a copy (`echo`) is lost; drop the struct-literal arm — the
/// seeded field is lost, and the real-tree assignment premise fires too;
/// drop the declared-type arm — the two declared fields are lost; read
/// another type's methods in [`gated_advice_field_reads`] — no read is
/// reported.
#[test]
fn the_field_scan_reports_a_gated_read_of_a_field_the_advice_was_copied_into() {
    let fixture = "pub struct Platform {\n    \
                   #[allow(dead_code)]\n    \
                   pub(crate) last_risk_discovery: Option<RiskDiscovery>,\n    \
                   held_control: Option<Box<qip_investment_agents::control::RiskControl>>,\n    \
                   concentration_hint: Option<Decimal>,\n    \
                   per_driver: Vec<Decimal>,\n    \
                   echo: Option<Decimal>,\n    \
                   seeded: BTreeMap<String, Decimal>,\n    \
                   blank: BTreeMap<String, Decimal>,\n    \
                   honest: Decimal,\n}\n\
                   impl Platform {\n    \
                   fn new(config: Config) -> Self {\n        \
                   Self {\n            last_risk_discovery: None,\n            held_control: None,\n            \
                   concentration_hint: None,\n            per_driver: Vec::new(),\n            \
                   echo: None,\n            seeded: BTreeMap::new(),\n            \
                   blank: BTreeMap::new(),\n            honest: Decimal::ZERO,\n        }\n    }\n    \
                   fn seeded_from(config: Config, prior: &RiskDiscovery) -> Self {\n        \
                   let base = Self::new(config);\n        \
                   Self {\n            seeded: prior.by_name.clone(),\n            ..base\n        }\n    }\n    \
                   fn run_cycle(&mut self, now: Timestamp) {\n        \
                   let discovery = self.discover_risk();\n        \
                   if let Some(drivers) = discovery.causal_drivers.as_ref() {\n            \
                   self.concentration_hint = Some(drivers.total);\n        }\n        \
                   for finding in discovery.market_exposure.iter() {\n            \
                   self.per_driver[0] = finding.gap;\n        }\n        \
                   self.honest = self.capital.equity();\n    }\n    \
                   fn mirror(&mut self) {\n        self.echo = self.concentration_hint;\n    }\n    \
                   fn size_survival(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   let ceiling = self\n            .concentration_hint\n            .unwrap_or_default();\n        \
                   let equity = self.honest + self.blank.len();\n        \
                   self.helper(now);\n    }\n    \
                   fn helper(&self, now: Timestamp) -> Decimal {\n        \
                   self.echo.unwrap_or_default() + self.per_driver[0] + self.seeded.len()\n    }\n}\n";
    let code = code_only(fixture);
    let sources = vec![("fixture.rs".to_string(), code.clone())];
    let functions = kernel_functions(&sources);
    let roots = gated_kernel_functions(&functions);
    let closure = kernel_closure(&functions, &roots);
    assert_eq!(
        closure
            .iter()
            .map(|(_, function)| function.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["size_survival", "helper"]),
        "premise: the gated closure is the decision and its helper"
    );
    assert!(
        closure
            .iter()
            .all(|(_, function)| advice_named(&function.body).is_empty()),
        "premise: the gated closure names no advice, so the advice scan passes the laundering"
    );
    let fields = struct_fields(&code, "pub struct Platform {");
    assert_eq!(
        fields.len(),
        8,
        "premise: every field was read past its attribute, visibility and generics: {fields:?}"
    );
    let held: BTreeSet<String> = advice_fields(&fields, &functions).into_keys().collect();
    assert_eq!(
        held,
        BTreeSet::from(
            [
                "last_risk_discovery",
                "held_control",
                "concentration_hint",
                "per_driver",
                "echo",
                "seeded",
            ]
            .map(str::to_string)
        ),
        "the fields the advice can be held in are not exactly the declared and the copied ones"
    );
    let reads: BTreeSet<String> = gated_advice_field_reads(&fields, &functions)
        .into_iter()
        .map(|read| read.split(',').next().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reads,
        BTreeSet::from(
            [
                "size_survival reads self.concentration_hint",
                "size_survival -> Platform::helper reads self.echo",
                "size_survival -> Platform::helper reads self.per_driver",
                "size_survival -> Platform::helper reads self.seeded",
            ]
            .map(str::to_string)
        ),
        "the gated reads of a field holding the advice are not exactly the four copies"
    );
}

/// R12-RISK-3022 (b), through a field: no field of `Platform` that can hold
/// the Risk Brain's advice — declared with a type naming `RiskDiscovery`,
/// `EmergingRisk`, `ExposureFinding` or `RiskControl`, or assigned or
/// initialised by any `Platform` method from text naming the advice, a
/// local bound from it, or another such field — is named `self.` inside a
/// `Platform` method the gated kernel walk roots or reaches.
///
/// The failure this prevents: `run_cycle` may name the discovery, so a
/// line there copying a part of it into a plain field, read by
/// `size_survival`, made the advice a second gate while naming it nowhere
/// the closure scan looks.
///
/// What a name scan still cannot see: the advice carried through a helper's
/// return (`self.f = self.summary()` where `summary` reads the discovery)
/// or a collection's mutator (`self.f.push(d)`, `self.f.insert(k, d)`), a
/// `match` arm's or a closure's binding, a `std::mem::replace`, a write
/// through `&mut self.f` handed to a function, a field read by a free
/// function or another type's method given the `Platform`, and a field of
/// a type other than `Platform`.
///
/// Mutation: write `self.concentration_hint = Some(dec!(0.1))` beside the
/// discovery in `run_cycle`, with the field declared, and read it in
/// `size_survival` — this fails naming the chain; both existing advice
/// scans pass it.
#[test]
fn no_gated_kernel_decision_reads_a_field_the_risk_brains_advice_was_copied_into() {
    let sources = kernel_sources();
    let functions = kernel_functions(&sources);
    let platform = sources
        .iter()
        .find(|(file, _)| file.ends_with("qip-kernel/src/platform.rs"))
        .map(|(_, code)| code.as_str())
        .expect("premise: the kernel has a platform.rs");
    let fields = struct_fields(platform, "pub struct Platform {");
    assert!(
        fields.len() > 50,
        "premise: the Platform's fields were read: {}",
        fields.len()
    );
    assert!(
        fields
            .iter()
            .any(|(name, ty)| name == "last_risk_discovery" && ty == "Option<RiskDiscovery>"),
        "premise: the Platform holds the last discovery under its declared type"
    );
    let assigned: usize = functions
        .iter()
        .filter(|function| function.owner.as_deref() == Some("Platform"))
        .map(|function| field_assignments(&joined_chains(&function.body)).len())
        .sum();
    assert!(
        assigned > 100,
        "premise: the Platform's own field assignments were read: {assigned}"
    );
    let held = advice_fields(&fields, &functions);
    assert!(
        held.contains_key("last_risk_discovery"),
        "premise: the field holding the discovery is held: {held:?}"
    );
    let roots: BTreeSet<&str> = gated_kernel_functions(&functions)
        .into_iter()
        .map(|root| root.name.as_str())
        .collect();
    for (function, _, _, _) in KERNEL_HARD_LIMIT_CALLS {
        assert!(
            roots.contains(function),
            "premise: `{function}` is a root: {roots:?}"
        );
    }
    let reads = gated_advice_field_reads(&fields, &functions);
    assert!(
        reads.is_empty(),
        "a gated hard-limit decision reads a field the Risk Brain's advice was copied into, so \
         the advice has become a second gate under another name: {reads:?}"
    );
}

/// R12-RISK-3022 (b), behaviourally, through the platform. A book whose one
/// position sits downstream of one claimed cause: the cycle's `RiskDiscovery`
/// names that driver at 10% of equity, and emerging. With no limit on the
/// axis, a further order in the same name is admitted — the discovery
/// refused nothing by itself. With the deterministic cap on the same axis at
/// 5%, the same order on the same book is refused, and the refusal names the
/// cap.
///
/// Mutations: in `Platform::submit_released`, refuse while
/// `last_risk_discovery` names an emerging risk — the first half fails with
/// the refusal; build the capped desk without its cap — the second half
/// fails with "the deterministic cap admitted the order".
#[test]
fn a_risk_discovery_naming_a_concentration_refuses_nothing_and_the_limit_on_it_does() {
    use qip_execution_engine::order::Side;
    use qip_kernel::{Platform, PlatformConfig};
    use qip_risk::shared_cause::CAUSAL_DRIVER_AXIS;

    let start = t(1_760_000_000);
    let equity = Decimal::from_int(10_000_000);
    let desk = |cap: Option<f64>| -> Platform {
        let config = PlatformConfig::default().with_initial_equity(equity);
        let (context, _clock) = qip_core::Context::deterministic(start, config.seed);
        let mut universe = qip_financial::universe::Universe::new();
        universe
            .insert(
                qip_financial::object::FinancialObject::builder(
                    ObjectId::from_string("obj-AAA"),
                    "AAA",
                    qip_financial::asset_class::InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
                )
                .venue("XNYS")
                .sector(qip_financial::asset_class::Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(qip_financial::quality::Provenance::synthetic(
                    "risk-advice",
                    start,
                ))
                .build(start)
                .expect("a valid instrument"),
            )
            .expect("insertable");
        let mut limits = LimitSet::new("risk-advice").with(Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional {
                limit: Decimal::from_int(5_000_000),
            },
        ));
        if let Some(cap) = cap {
            limits = limits.with(Limit::new(
                "causal-driver-concentration",
                LimitKind::MaxAxisWeight {
                    axis: CAUSAL_DRIVER_AXIS.to_string(),
                    limit: cap,
                },
            ));
        }
        let mut platform = Platform::new(
            config,
            context,
            qip_observability::Telemetry::silent(),
            universe,
            limits,
        )
        .expect("the platform assembles");
        // One position of 1,000,000 — 10% of equity — then a claimed cause
        // the position sits downstream of.
        let first = platform
            .order_for(
                ObjectId::from_string("obj-AAA"),
                Side::Buy,
                dec!("10000"),
                dec!("100"),
                "prop-risk-advice",
                vec!["hyp-risk-advice".to_string()],
                start,
            )
            .expect("an order");
        platform
            .submit_order(first, start)
            .expect("the first order, before any cause is claimed, is admitted");
        platform
            .claim_causal(
                qip_world_model::causal::CausalEdge::new(
                    "funding-stress",
                    "obj-AAA",
                    qip_world_model::causal::Mechanism::DiscountRate,
                    1.0,
                    Duration::from_days(1),
                    start.saturating_sub(Duration::from_days(1)),
                )
                .expect("a strength in [0, 1]")
                .with_confidence(1.0)
                .expect("a confidence in [0, 1]"),
            )
            .expect("an edge naming both ends");
        platform
    };
    let second = |platform: &mut Platform| {
        let order = platform
            .order_for(
                ObjectId::from_string("obj-AAA"),
                Side::Buy,
                dec!("1000"),
                dec!("100"),
                "prop-risk-advice",
                vec!["hyp-risk-advice".to_string()],
                start,
            )
            .expect("an order");
        platform.submit_order(order, start)
    };

    let mut advised = desk(None);
    let report = advised.run_cycle(start);
    let driven = report
        .risk_discovery
        .causal_drivers
        .as_ref()
        .and_then(|drivers| drivers.get("funding-stress"))
        .copied();
    // Premise: the discovery names the concentration the cap below refuses,
    // above that cap, and as emerging.
    assert!(
        driven.is_some_and(|gross| gross.to_f64() / equity.to_f64() > 0.05),
        "premise: the discovery names the driver above 5% of equity: {:?}",
        report.risk_discovery
    );
    assert!(
        report
            .risk_discovery
            .emerging
            .iter()
            .any(|risk| risk.axis == CAUSAL_DRIVER_AXIS && risk.bucket == "funding-stress"),
        "premise: the driver is reported as emerging: {:?}",
        report.risk_discovery.emerging
    );
    let admitted = second(&mut advised);
    assert!(
        admitted.is_ok(),
        "an order was refused on a book whose only finding is the Risk Brain's advice: {admitted:?}"
    );

    let mut capped = desk(Some(0.05));
    let refused = second(&mut capped).expect_err("the deterministic cap admitted the order");
    assert!(
        refused.message().contains("causal-driver-concentration"),
        "refused, but not by the cap on the axis the discovery named: {}",
        refused.message()
    );
}

// --- no model inside a gated decision (R12-RISK-3022) ----------------------

/// Every name by which the kernel reaches a model for inference, found by
/// reading what it imports and calls (`qip_ai::serving`, `qip_ai::language`):
/// the serving types and the provider the platform holds, the language model
/// it hands the organisation, the methods that serve or forecast from a
/// model card, and the organisation itself, whose agents call the language
/// model. `ModelPort` and `forecast_with` were looked for and are named
/// nowhere in the tree; `qip-language-providers` is not a kernel dependency
/// and is listed so that a new edge to it cannot arrive unseen.
const MODEL_SURFACE: [&str; 20] = [
    "qip_ai::serving",
    "qip_ai::language",
    "qip_language_providers",
    "ModelProvider",
    "ServedModel",
    "ModelArtifact",
    "NoProvider",
    "LanguageModel",
    "DeterministicModel",
    "JudgedRouting",
    "model_provider",
    "language_model",
    "serve",
    "serve_model",
    "stage_card_forecasts",
    "standing_card_forecasts",
    "complete",
    "complete_structured",
    "organisation",
    "Organisation",
];

/// The cost router's arms that place a decision on a model rung, by the
/// receiver the kernel holds the router under: `select` answers a
/// determinism-not-required decision with a `JudgedRouting`, `escalate`
/// climbs it, `route` is the router's general entry. A gated decision asks
/// `determine` and nothing else.
const MODEL_ROUTES: [&str; 3] = ["select", "escalate", "route"];

/// Which of [`MODEL_SURFACE`] and `cost_router.`[`MODEL_ROUTES`] `code`
/// names, the router's arms read through [`joined_chains`] so that
/// `self\n    .cost_router\n    .select(` is the router's `select` too.
fn model_surface_named(code: &str) -> Vec<String> {
    let code = joined_chains(code);
    let code = code.as_str();
    let mut named: Vec<String> = MODEL_SURFACE
        .iter()
        .filter(|token| !token_offsets(code, token).is_empty())
        .map(|token| token.to_string())
        .collect();
    for route in MODEL_ROUTES {
        if token_offsets(code, route)
            .into_iter()
            .any(|at| code[..at].trim_end().ends_with("cost_router."))
        {
            named.push(format!("cost_router.{route}"));
        }
    }
    named
}

/// Every chain from a gated kernel function to a kernel function that names
/// a model-serving surface, as `root -> .. -> callee names [..] (file)`.
fn gated_model_reach(functions: &[KernelFn]) -> Vec<String> {
    let roots = gated_kernel_functions(functions);
    kernel_closure(functions, &roots)
        .into_iter()
        .filter_map(|(chain, function)| {
            let named = model_surface_named(&function.body);
            (!named.is_empty())
                .then(|| format!("{} names {named:?} ({})", chain.join(" -> "), function.file))
        })
        .collect()
}

/// The model-surface scan on a fixture: a gated decision that serves a
/// model two calls down, through its own method and a free function, and a
/// gated decision that asks the router to `select` a rung, are both
/// reported with the chain; an ungated function that serves a model, and a
/// gated one that only asks the router to `determine`, are not.
///
/// Mutations: drop `"model_provider"` and `"serve"` from [`MODEL_SURFACE`] —
/// the first chain is not reported; drop the `cost_router.` arm of
/// [`model_surface_named`] — the second is not.
#[test]
fn the_model_surface_scan_reports_a_gated_decision_that_reaches_a_model() {
    let fixture = "impl Platform {\n    \
                   fn decide(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   self.helper(now);\n    }\n    \
                   fn helper(&self, now: Timestamp) {\n        crate::sizing::score_it(now);\n    }\n    \
                   fn judged(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   let placed = self.cost_router.select(&context);\n    }\n    \
                   fn routed(&mut self, gate: &DeterministicGate, now: Timestamp) {\n        \
                   let routing = self.cost_router.determine(subject, conditions);\n    }\n    \
                   fn research(&self, artifact: &Artifact) {\n        \
                   self.model_provider.serve(artifact);\n    }\n}\n\
                   fn score_it(now: Timestamp) {\n    \
                   let served = PLATFORM.model_provider.serve(&artifact);\n}\n";
    let sources = vec![("fixture.rs".to_string(), code_only(fixture))];
    let functions = kernel_functions(&sources);
    let reached: BTreeSet<String> = gated_model_reach(&functions).into_iter().collect();
    assert_eq!(
        reached,
        BTreeSet::from([
            "decide -> Platform::helper -> score_it names [\"model_provider\", \"serve\"] \
             (fixture.rs)"
                .to_string(),
            "judged names [\"cost_router.select\"] (fixture.rs)".to_string(),
        ]),
        "the scan did not report exactly the gated decisions that reach a model"
    );
}

/// R12-RISK-3022 (d): no kernel function that makes a hard-limit decision
/// under the deterministic gate, nor any kernel function it reaches by name
/// and owner, names a model-serving surface. `qip-kernel` links `qip-ai`, so
/// the compiler cannot hold this for the kernel the way it holds it for the
/// risk engine; this scan is what does.
///
/// The failure this prevents: a model call inside sizing, sign-off,
/// release, the plane's halt or the leverage decision failed no test, so a
/// gated decision could have become a judged one while still carrying the
/// gate that says it was determined.
///
/// Mutation: in `Platform::survival_figures`, read
/// `self.model_provider.name()` — this fails naming the chain
/// `seed_survival_ceiling -> Platform::survival_figures`.
#[test]
fn no_gated_kernel_decision_reaches_a_model_serving_surface() {
    let sources = kernel_sources();
    let functions = kernel_functions(&sources);
    // Premise: the names are the kernel's real inference surface, each
    // named somewhere in its production source, so a clean result is the
    // gated closure not reaching them rather than a list of names nobody
    // uses.
    let named_anywhere: BTreeSet<String> = functions
        .iter()
        .flat_map(|function| model_surface_named(&function.body))
        .collect();
    for surface in [
        "model_provider",
        "serve",
        "serve_model",
        "stage_card_forecasts",
        "language_model",
        "organisation",
        "cost_router.select",
    ] {
        assert!(
            named_anywhere.contains(surface),
            "premise: the kernel names `{surface}` somewhere: {named_anywhere:?}"
        );
    }
    let roots: BTreeSet<&str> = gated_kernel_functions(&functions)
        .into_iter()
        .map(|root| root.name.as_str())
        .collect();
    for (function, _, _, _) in KERNEL_HARD_LIMIT_CALLS {
        assert!(
            roots.contains(function),
            "premise: `{function}` is a root: {roots:?}"
        );
    }
    assert!(
        roots.contains("observe_book_under_routing"),
        "premise: the risk monitor's pass, taken under a routing, is a root: {roots:?}"
    );
    let reached = gated_model_reach(&functions);
    assert!(
        reached.is_empty(),
        "a gated hard-limit decision reaches a model-serving surface, so the gate it carries \
         no longer says the decision was determined: {reached:?}"
    );
}
