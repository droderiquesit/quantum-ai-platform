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
