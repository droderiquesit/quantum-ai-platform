//! Control reaching the decision thread only at a pass boundary, and the
//! file reads that used to sit on that thread moved off it (ADR 0100 §6).
//!
//! Red-team M5: `StrategyInstaller::install`, `HaltFlag::poll` and the
//! region wire's `poll` each did a blocking `std::fs` read on the thread that
//! runs passes. A file on a hung mount therefore stopped trading by freezing
//! it — orders resting, none withdrawn at its time to live — which is not
//! the same thing as halting, and which no import scan of the pass loop
//! would show. The tests that prove the reads have moved use a FIFO nobody
//! ever opens for writing: opening it for reading blocks forever, exactly as
//! a read on a hung mount does, so a read left on the decision thread stops
//! the pass cadence and the test sees it stop.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PlanDigest, PolicyPayload, Slot};
use qip_contracts::replay::ControlPosition;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{EventId, ObjectId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::cell::{CellConfig, PolledHalt, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::region::RegionOutlook;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::control::{
    Applied, ControlKind, Delivery, Handoff, Outcome, PlanCompiler, Poller, Provenance,
};
use qip_edge_node::dark::DarkRegionWire;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::halt::HaltFlag;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::strategies::{CompiledPlan, StrategyInstaller, StrategyPlan};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration as StdDuration, Instant};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"control-test-envelope-key";
const STREAM: &str = "control.london-1";

/// How long a test waits for one turn of a decision loop before calling the
/// cadence stopped. Generous against a loaded machine: a healthy turn here
/// takes milliseconds, and a stopped one never ends at all.
const TURN_DEADLINE: StdDuration = StdDuration::from_secs(10);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn position(offset: u64) -> ControlPosition {
    ControlPosition::new(
        STREAM,
        0,
        offset,
        EventId::from_string(format!("evt-control-{offset}")),
    )
}

/// A fresh directory for one test, so tests running in parallel never read
/// each other's files.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-node-control-{}-{test}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A FIFO at `path`: opening it for reading blocks until somebody opens it
/// for writing, which in these tests nobody does until the end.
fn fifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(status.success(), "mkfifo {} failed", path.display());
}

/// Open each FIFO for writing once and close it, so a reader blocked on it
/// reads end-of-file and its thread ends with the test rather than after.
fn release(fifos: &[PathBuf]) {
    for path in fifos {
        let path = path.clone();
        std::thread::spawn(move || drop(fs::OpenOptions::new().write(true).open(path)));
    }
}

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

fn plan_file(dir: &Path, specs: &[StrategySpec]) -> (PathBuf, String) {
    let json = serde_json::json!({ "strategies": specs });
    let bytes = serde_json::to_vec(&json).expect("a plan serialises");
    let path = dir.join("plan.json");
    fs::write(&path, &bytes).expect("the plan is written");
    (path, StrategyPlan::digest_of(&bytes))
}

fn grant_until(strategy: &str, expires_at: Timestamp) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
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
    let signed = build(&sign_payload(KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, KEY, CELL, t(1))
}

/// A payload with nothing produced but its sequence.
fn bare_policy(sequence: u64, issued_at: Timestamp) -> Result<VerifiedPolicy> {
    let payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

/// A payload naming the plan by digest and funding the grant for each of
/// `grants`, as the centre ships one once a region's grant is partitioned.
fn funded_policy(
    sequence: u64,
    issued_at: Timestamp,
    digest: &str,
    strategies: u64,
    grants: &[&VerifiedEnvelope],
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: digest.to_string(),
            strategies,
        },
        issued_at,
    );
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants
                .iter()
                .map(|grant| grant.signature().to_string())
                .collect(),
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

fn node_with_feed() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    Ok((node, gateway, feed))
}

/// Receive one turn's report from a decision loop running on its own
/// thread, or fail naming the turn at which the cadence stopped.
fn next_turn<T>(turns: &mpsc::Receiver<T>, turn: usize, what: &str) -> T {
    match turns.recv_timeout(TURN_DEADLINE) {
        Ok(report) => report,
        Err(error) => panic!(
            "the pass cadence stopped at turn {turn}: no turn completed within {TURN_DEADLINE:?} \
             ({error}). {what}"
        ),
    }
}

#[test]
fn a_verified_value_handed_across_is_applied_exactly_once_at_a_pass_boundary() -> Result<()> {
    // The failure this prevents is a value that changes the cell somewhere
    // other than between passes — so a pass runs under two policies — or
    // that changes it twice, so a boundary re-asks the cell a question it
    // has already answered and journals the refusal on every pass.
    let (mut node, mut gateway, mut feed) = node_with_feed()?;
    let (sender, mut handoff) = Handoff::bounded(8)?;
    let mut stats = PassStats::default();

    sender.send(Delivery::policy(position(7), bare_policy(1, t(10))?))?;
    // A pass between the send and the boundary runs under the control in
    // force when it began: none.
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(11),
    )?;
    assert!(
        matches!(outcome, PassOutcome::Ran { .. }),
        "the premise is a pass that ran: {outcome:?}"
    );
    assert_eq!(
        node.cell.policy_sequence(),
        None,
        "a value handed across changed the cell before any boundary"
    );

    let first = handoff.boundary(&mut node.cell, None, t(12));
    assert_eq!(
        first.applied,
        vec![Applied {
            kind: ControlKind::Policy,
            provenance: Provenance::Fabric(position(7)),
            outcome: Outcome::Applied,
        }],
        "the boundary did not apply the one value handed across, with its position"
    );
    assert_eq!(first.positions(), vec![position(7)]);
    assert!(!first.producers_gone, "the producer is still alive");
    assert_eq!(node.cell.policy_sequence(), Some(1));

    // The next boundary has nothing new and applies nothing: the value is
    // consumed, not retained and re-offered.
    let second = handoff.boundary(&mut node.cell, None, t(13));
    assert!(
        second.applied.is_empty(),
        "a value was applied at a second boundary: {:?}",
        second.applied
    );

    // A newer value lands at the boundary after it was sent, alone.
    sender.send(Delivery::policy(position(8), bare_policy(2, t(14))?))?;
    let third = handoff.boundary(&mut node.cell, None, t(15));
    assert_eq!(
        third.applied,
        vec![Applied {
            kind: ControlKind::Policy,
            provenance: Provenance::Fabric(position(8)),
            outcome: Outcome::Applied,
        }],
        "the third boundary did not apply exactly the value sent since the second"
    );
    assert_eq!(node.cell.policy_sequence(), Some(2));
    Ok(())
}

#[test]
fn a_package_is_read_and_compiled_off_the_decision_thread_and_activated_at_a_pass_boundary()
-> Result<()> {
    // Half one: the payload names a plan whose file is a FIFO nobody opens
    // for writing. The compile must wait on it forever *somewhere else*;
    // the decision loop keeps turning. Compiled on the decision thread, the
    // first turn that asked for the plan would never end.
    const TURNS: usize = 20;
    let hung = scratch("plan-hung");
    let hung_plan = hung.join("plan.json");
    fifo(&hung_plan);
    let (turns_tx, turns) = mpsc::channel();
    let loop_plan = hung_plan.clone();
    std::thread::spawn(move || -> Result<()> {
        let (mut node, mut gateway, mut feed) = node_with_feed()?;
        let (sender, mut handoff) = Handoff::bounded(8)?;
        let mut installer = StrategyInstaller::new(None, Some(PricingPolicy::Marketable));
        let mut compiler = PlanCompiler::spawn(loop_plan, sender.clone())?;
        let digest = "0".repeat(64);
        let payload = funded_policy(1, t(10), &digest, 1, &[])?;
        sender.send(Delivery::policy(position(1), payload))?;
        let mut stats = PassStats::default();
        for turn in 0..TURNS {
            let now = t(20 + i64::try_from(turn).unwrap_or(0));
            let boundary = handoff.boundary(&mut node.cell, Some(&mut installer), now);
            let asked = match node.cell.compiled_plan(now).cloned() {
                Some(named) => compiler.ensure(&named)?,
                None => false,
            };
            run_pass(
                &mut node.cell,
                &mut gateway,
                &mut feed,
                None,
                &mut stats,
                now,
            )?;
            let described = boundary.plan.map(|plan| plan.describe());
            if turns_tx.send((asked, described)).is_err() {
                break;
            }
        }
        Ok(())
    });
    let mut asked_at_least_once = false;
    let mut last = None;
    for turn in 0..TURNS {
        let (asked, described) = next_turn(
            &turns,
            turn,
            "a plan read on the decision thread blocks every pass behind the mount",
        );
        asked_at_least_once |= asked;
        last = described;
    }
    assert!(
        asked_at_least_once,
        "the premise is a loop that asked for the hung plan to be compiled"
    );
    let last = last.unwrap_or_default();
    assert!(
        last.contains("no compiled plan has been handed across yet"),
        "a hung plan read should leave the installer waiting for the compiler, not deploying: \
         {last}"
    );
    release(&[hung_plan]);

    // Half two: a readable plan, compiled on the compiler's thread, is
    // deployed at the first boundary after it is handed across and not
    // before — and the boundary records the path and digest it came from.
    let dir = scratch("plan-ready");
    let (path, digest) = plan_file(&dir, &[spec(STRATEGY, "10")]);
    let grant = grant_until(STRATEGY, t(3600))?;
    let (mut node, mut gateway, mut feed) = node_with_feed()?;
    let (sender, mut handoff) = Handoff::bounded(8)?;
    let mut installer = StrategyInstaller::new(None, Some(PricingPolicy::Marketable));
    let mut compiler = PlanCompiler::spawn(path.clone(), sender.clone())?;
    sender.send(Delivery::envelope(position(1), grant.clone()))?;
    sender.send(Delivery::policy(
        position(2),
        funded_policy(1, t(10), &digest, 1, &[&grant])?,
    ))?;
    let first = handoff.boundary(&mut node.cell, Some(&mut installer), t(20));
    assert_eq!(
        first.applied.iter().map(|a| &a.outcome).collect::<Vec<_>>(),
        vec![&Outcome::Held, &Outcome::Applied],
        "the premise is a grant held for the plan and the payload naming it applied: {:?}",
        first.applied
    );
    assert!(
        node.cell.deployed_strategies().is_empty(),
        "nothing may deploy before the compiled plan crosses"
    );
    let named = node
        .cell
        .compiled_plan(t(20))
        .cloned()
        .expect("the payload names a fresh plan");
    assert!(compiler.ensure(&named)?, "the compile was not queued");

    let started = Instant::now();
    let activation = loop {
        let now = t(21);
        let boundary = handoff.boundary(&mut node.cell, Some(&mut installer), now);
        if boundary.applied.iter().any(|a| a.kind == ControlKind::Plan) {
            break boundary;
        }
        assert!(
            node.cell.deployed_strategies().is_empty(),
            "the strategy deployed at a boundary the compiled plan had not reached"
        );
        assert!(
            started.elapsed() < TURN_DEADLINE,
            "the compiled plan never crossed: {:?}",
            compiler.last_refusal()
        );
        std::thread::sleep(StdDuration::from_millis(5));
    };
    assert_eq!(
        activation.applied,
        vec![Applied {
            kind: ControlKind::Plan,
            provenance: Provenance::Plan {
                path: path.clone(),
                digest: digest.clone(),
            },
            outcome: Outcome::Applied,
        }]
    );
    let installed = activation.plan.expect("the installer reported");
    assert_eq!(
        installed.deployed,
        vec![STRATEGY.to_string()],
        "{}",
        installed.describe()
    );
    assert_eq!(installer.compiled_digest(), Some(digest.as_str()));

    let mut stats = PassStats::default();
    let pass = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(22),
    )?;
    let PassOutcome::Ran { report, .. } = pass else {
        panic!("the node halted on the pass after activation: {pass:?}");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "the activated plan did not trade: {:?}",
        report.refusals
    );
    Ok(())
}

#[test]
fn halt_and_region_wires_whose_files_block_forever_do_not_change_the_pass_cadence() -> Result<()> {
    // Both wires are FIFOs nobody writes to. The readers block forever on
    // their own threads; the decision loop keeps turning, and because no
    // reading was ever published it turns *halted*, with every mirror
    // suspended — a wire whose state is unknown reads as engaged.
    const TURNS: usize = 25;
    let dir = scratch("wires-hung");
    let flag_path = dir.join("halt");
    fifo(&flag_path);
    let flag = HaltFlag::at(&flag_path)?;
    let wire = DarkRegionWire::beside(flag.path()).expect("the flag names a directory");
    fifo(wire.path());
    let fifos = vec![flag_path.clone(), wire.path().to_path_buf()];

    let (turns_tx, turns) = mpsc::channel();
    std::thread::spawn(move || -> Result<()> {
        let (mut node, mut gateway, mut feed) = node_with_feed()?;
        let poller = Poller::spawn(
            flag,
            Some(wire),
            StdDuration::from_millis(5),
            StdDuration::from_millis(200),
            Arc::new(SystemClock),
        )?;
        let mut stats = PassStats::default();
        for turn in 0..TURNS {
            let now = t(20 + i64::try_from(turn).unwrap_or(0));
            let readings = poller.apply(&mut node.cell, now);
            let outcome = run_pass(
                &mut node.cell,
                &mut gateway,
                &mut feed,
                None,
                &mut stats,
                now,
            )?;
            let halted_pass = matches!(outcome, PassOutcome::Halted { .. });
            if turns_tx
                .send((readings, node.cell.is_halted(), halted_pass))
                .is_err()
            {
                break;
            }
        }
        Ok(())
    });
    let mut last = None;
    for turn in 0..TURNS {
        last = Some(next_turn(
            &turns,
            turn,
            "a wire read on the decision thread blocks every pass behind the mount",
        ));
    }
    release(&fifos);
    let (readings, halted, halted_pass) = last.expect("the loop turned");
    assert!(
        matches!(&readings.halt.value, PolledHalt::Unreadable(reason) if reason.contains("has not published")),
        "a halt flag that was never read must apply as unreadable: {:?}",
        readings.halt
    );
    assert!(
        matches!(&readings.region, Some(region) if matches!(&region.value, RegionOutlook::Unreadable(_))),
        "a region wire that was never read must apply as unreadable: {:?}",
        readings.region
    );
    assert!(halted, "the cell ran on a halt flag nobody could read");
    assert!(halted_pass, "the pass ran rather than turning halted");
    Ok(())
}

#[test]
fn a_poller_that_stops_publishing_reads_as_an_unreadable_halt_flag() -> Result<()> {
    // The failure: a reader thread that stops — its file hung, its thread
    // died — and a decision thread that keeps applying the last thing it
    // said. If that was "released", the kill switch is off for as long as
    // the reader is gone, which is exactly when the operator needs it.
    let dir = scratch("poller-stops");
    let flag_path = dir.join("halt");
    let flag = HaltFlag::at(&flag_path)?;
    let (mut node, _gateway, _feed) = node_with_feed()?;
    let bound = StdDuration::from_millis(150);
    let poller = Poller::spawn(
        flag,
        None,
        StdDuration::from_millis(10),
        bound,
        Arc::new(SystemClock),
    )?;

    let started = Instant::now();
    while poller.latest().halt.value != PolledHalt::Absent {
        assert!(
            started.elapsed() < TURN_DEADLINE,
            "the premise is a poller that published the absent flag: {:?}",
            poller.latest()
        );
        std::thread::sleep(StdDuration::from_millis(5));
    }
    let healthy = poller.apply(&mut node.cell, t(20));
    assert_eq!(healthy.halt.value, PolledHalt::Absent);
    assert!(
        !node.cell.is_halted(),
        "the premise is a cell running on an absent flag"
    );

    // The flag's path becomes a file whose read never returns: the reader
    // thread blocks inside its next read and publishes nothing more.
    fifo(&flag_path);
    let started = Instant::now();
    let stale = loop {
        let reading = poller.latest();
        if matches!(reading.halt.value, PolledHalt::Unreadable(_)) {
            break reading;
        }
        assert!(
            started.elapsed() < TURN_DEADLINE,
            "the poller stopped publishing and the flag still reads {:?} after {:?}",
            reading.halt.value,
            started.elapsed()
        );
        std::thread::sleep(StdDuration::from_millis(10));
    };
    assert!(
        started.elapsed() >= bound.saturating_sub(StdDuration::from_millis(20)),
        "the reading went unreadable before the bound could have elapsed"
    );
    let PolledHalt::Unreadable(reason) = &stale.halt.value else {
        panic!("matched above");
    };
    assert!(
        reason.contains("last published"),
        "the reason should say the reader went silent: {reason}"
    );
    assert!(
        stale.halt.read_at.is_some(),
        "a stale reading keeps the instant of the last publication"
    );
    poller.apply(&mut node.cell, t(21));
    assert!(
        node.cell.is_halted(),
        "a poller that stopped publishing left the cell running"
    );
    release(&[flag_path]);
    Ok(())
}

#[test]
fn with_nothing_delivered_the_cell_runs_on_its_last_valid_envelope_until_it_expires() -> Result<()>
{
    // RES-003/060: the fabric goes down and nothing more is delivered. ADR
    // 0008 says a cell keeps working within its envelope; the envelope's own
    // expiry is the backstop, and that is what stops it — not the silence.
    let dir = scratch("fabric-down");
    let (path, digest) = plan_file(&dir, &[spec(STRATEGY, "10")]);
    // The whole timeline sits inside the simulated session's thirty seconds,
    // because nothing in the pass loop answers the venue's heartbeat and the
    // venue answering is not what is under test.
    let expiry = t(20);
    let grant = grant_until(STRATEGY, expiry)?;
    let (mut node, mut gateway, mut feed) = node_with_feed()?;
    let (sender, mut handoff) = Handoff::bounded(8)?;
    let mut installer = StrategyInstaller::new(None, Some(PricingPolicy::Marketable));
    let payload = funded_policy(1, t(2), &digest, 1, &[&grant])?;
    let named = PlanDigest {
        digest: digest.clone(),
        strategies: 1,
    };
    sender.send(Delivery::envelope(position(1), grant))?;
    sender.send(Delivery::policy(position(2), payload))?;
    sender.send(Delivery::plan(CompiledPlan::read_and_compile(
        &path, &named,
    )?))?;
    let armed = handoff.boundary(&mut node.cell, Some(&mut installer), t(5));
    assert_eq!(
        armed.plan.as_ref().map(|plan| plan.deployed.clone()),
        Some(vec![STRATEGY.to_string()]),
        "the premise is a deployed strategy: {:?}",
        armed
    );

    // The fabric goes down: every producer is gone.
    drop(sender);
    let mut stats = PassStats::default();
    for now in [t(6), t(10), t(15), t(19)] {
        let boundary = handoff.boundary(&mut node.cell, Some(&mut installer), now);
        assert!(
            boundary.producers_gone,
            "the premise is a fabric that is down"
        );
        assert!(boundary.applied.is_empty(), "nothing was delivered");
        let pass = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            None,
            &mut stats,
            now,
        )?;
        let PassOutcome::Ran { report, .. } = pass else {
            panic!("an empty handoff halted the cell at {now}: {pass:?}");
        };
        assert!(
            !report.orders.is_empty(),
            "the cell stopped trading on a live envelope at {now}: {:?}",
            report.refusals
        );
    }

    // Past the envelope's expiry, and still inside the payload's own
    // validity, the envelope — not the fabric — stops the strategy.
    let now = t(25);
    let boundary = handoff.boundary(&mut node.cell, Some(&mut installer), now);
    assert!(boundary.applied.is_empty());
    let pass = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        now,
    )?;
    let PassOutcome::Ran { report, .. } = pass else {
        panic!("an expired envelope halted the cell rather than refusing: {pass:?}");
    };
    assert!(report.orders.is_empty(), "an expired envelope traded");
    assert!(
        report
            .refusals
            .iter()
            .any(|(gate, _)| gate == "envelope_expiry"),
        "the refusal did not name the envelope's expiry: {:?}",
        report.refusals
    );
    assert!(
        !report
            .refusals
            .iter()
            .any(|(gate, reason)| gate.contains("fabric") || reason.contains("fabric")),
        "a refusal named the fabric: {:?}",
        report.refusals
    );
    assert!(!node.cell.is_halted());
    Ok(())
}
