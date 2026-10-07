//! A node keeps deciding and executing when the centre stops answering
//! (REASON-034).
//!
//! Every reasoning service the platform has — the reasoning engine, the
//! world model, the optimiser — runs in the central binaries, and a node
//! reaches the centre over the mesh link and nothing else. `architecture.rs`
//! holds the static half: no edge crate depends on a reasoning crate. This is
//! the run-time half, which a dependency graph cannot show: a node could link
//! none of them and still wait on the centre before it works a pass.
//!
//! So the same node is run twice over the same passes against the simulated
//! venue. In one run the centre answers throughout. In the other the centre is
//! stopped part-way through — the listener is gone, every later tick fails —
//! and the node's pass count, its refusals and the orders it sent are required
//! to be the same, pass for pass.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Decimal, ManualClock, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::mesh::{MeshLink, MeshSettings};
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_transport::{MeshEndpoint, MeshInbox, Method, RecordingSleeper};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"outage-test-envelope-key";
/// Passes each run works, and the pass before which the centre is stopped.
const PASSES: usize = 7;
const CENTRE_STOPS_BEFORE: usize = 3;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

// --- the centre, as a loopback listener that can be stopped ------------------

struct Centre {
    address: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Centre {
    fn start() -> Result<Self> {
        let endpoint = MeshEndpoint::new(MeshInbox::new("central", 64, 256)?);
        let io = |error: std::io::Error| Error::io(error.to_string());
        let listener = TcpListener::bind("127.0.0.1:0").map_err(io)?;
        let address = listener.local_addr().map_err(io)?.to_string();
        listener.set_nonblocking(true).map_err(io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        answer(stream, &endpoint);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            handle: Some(handle),
        })
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }
}

/// Dropping the centre stops it: the thread ends and the listener closes, so
/// the address refuses every connection made afterwards.
impl Drop for Centre {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn answer(mut stream: TcpStream, endpoint: &MeshEndpoint) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let Some(method) = parts.next().and_then(Method::parse) else {
        return;
    };
    let Some(target) = parts.next().map(str::to_string) else {
        return;
    };
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let response = endpoint.handle(method, &target, &body);
    let mut out = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.status,
        qip_transport::EndpointResponse::CONTENT_TYPE,
        response.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&response.body);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

// --- the node ----------------------------------------------------------------

/// A grant small enough that the strategy runs out of it within the run, so
/// the passes after the centre stops include refusals as well as orders.
fn grant() -> Result<VerifiedEnvelope> {
    let lineage = Lineage::root(CorrelationId::from_string(STRATEGY), "reasoning_outage");
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1300"),
            dec!("1300"),
            dec!("1300"),
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
            lineage.clone(),
        )
    };
    let unsigned = build("unsigned")?;
    let signed = build(&sign_payload(KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let lineage = Lineage::root(CorrelationId::from_string(STRATEGY), "reasoning_outage");
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5), lineage);
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, t(5))
}

/// What one pass did, in the terms the requirement names.
#[derive(Debug, PartialEq)]
struct PassRecord {
    /// `(order id, quantity, price)` for every order the venue accepted.
    orders: Vec<(String, Decimal, Decimal)>,
    /// `(gate, reason)` for every refusal.
    refusals: Vec<(String, String)>,
}

#[derive(Debug)]
struct Run {
    passes: Vec<PassRecord>,
    stats: PassStats,
    /// Orders the venue itself counted, independently of the cell's report.
    submitted: u64,
    /// Ticks on which the node could not reach the centre.
    unanswered: usize,
    halted: bool,
}

/// Work [`PASSES`] passes, a mesh tick after each. With `centre_stops` the
/// centre is stopped before pass [`CENTRE_STOPS_BEFORE`] and never returns.
fn run(centre_stops: bool) -> Result<Run> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;

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
    node.cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        grant()?,
        PricingPolicy::Marketable,
    )?;
    let named = grant()?.signature().to_string();
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;

    let mut centre = Some(Centre::start()?);
    let peer = centre.as_ref().map(Centre::url).unwrap_or_default();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let mut link = MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer,
            seed: 3,
        },
        KEY,
        clock,
        Arc::new(RecordingSleeper::new()),
    )?;

    let mut out = Run {
        passes: Vec::new(),
        stats: PassStats::default(),
        submitted: 0,
        unanswered: 0,
        halted: false,
    };
    for pass in 0..PASSES {
        // Two seconds apart, so the whole run sits inside the thirty seconds
        // the simulated venue's session lasts with nothing answering its
        // heartbeat — which nothing in the pass loop does.
        let now = t(10 + 2 * pass as i64);
        if pass == 1 {
            // The venue lists the instrument from the second pass, so the
            // first refuses for want of a book and the rest have one.
            gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), now)?;
            gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), now)?;
        }
        if centre_stops && pass == CENTRE_STOPS_BEFORE {
            centre = None;
        }
        let outcome = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            None,
            &mut out.stats,
            now,
        )?;
        // A halted turn is recorded and the run goes on ticking, so that a
        // node which stops when the centre does is reported as having
        // stopped rather than as having lost the centre for fewer ticks.
        let report = match outcome {
            PassOutcome::Ran { report, .. } => {
                out.passes.push(PassRecord {
                    orders: report
                        .orders
                        .iter()
                        .map(|o| (o.order_id.clone(), o.quantity, o.price))
                        .collect(),
                    refusals: report.refusals.clone(),
                });
                *report
            }
            PassOutcome::Halted { .. } => {
                out.halted = true;
                WorkReport::default()
            }
        };
        let tick = link.exchange(&mut node.cell, &report, now);
        // Answered means all three legs: both polls returned and the delta
        // was delivered. Once the breaker opens a poll is skipped rather than
        // failed, so a poll error alone would undercount the outage.
        let answered = tick.poll_error.is_none()
            && tick.policy_poll_error.is_none()
            && tick.delta.as_deref() == Some("delivered");
        out.unanswered += usize::from(!answered);
    }
    out.submitted = gateway.submitted_count();
    out.halted |= node.cell.is_halted();
    drop(centre);
    Ok(out)
}

#[test]
fn a_node_works_the_same_passes_sends_the_same_orders_and_refuses_the_same_when_the_centre_stops_answering()
-> Result<()> {
    // The failure this prevents: a hot path that waits on, or is stopped by,
    // the loss of the services that reason. A cell that halted, sized
    // differently or held its orders when the centre went away would turn an
    // outage of the slow lane into an outage of the fast one.
    let answered = run(false)?;
    let stopped = run(true)?;

    // The premise, both halves. With the centre up every tick was answered;
    // with it stopped, every tick from that pass on failed — the node really
    // did lose the centre, it did not merely have nothing to hear.
    assert_eq!(answered.unanswered, 0, "{answered:?}");
    assert_eq!(
        stopped.unanswered,
        PASSES - CENTRE_STOPS_BEFORE,
        "{stopped:?}"
    );
    // Losing the centre stopped neither run.
    assert!(
        !answered.halted && !stopped.halted,
        "a node halted: with the centre up {}, with it stopped {}",
        answered.halted,
        stopped.halted
    );
    // And the passes after the stop are not idle ones: orders were sent and
    // refusals were made on both sides of it, so "unchanged" below is said of
    // a node that was deciding and executing throughout.
    assert_eq!(answered.passes.len(), PASSES, "{answered:?}");
    let (before, after) = answered.passes.split_at(CENTRE_STOPS_BEFORE);
    for (side, passes) in [("before", before), ("after", after)] {
        assert!(
            passes.iter().any(|p| !p.orders.is_empty()),
            "no order was sent {side} the stop: {passes:?}"
        );
        assert!(
            passes.iter().any(|p| !p.refusals.is_empty()),
            "nothing was refused {side} the stop: {passes:?}"
        );
    }

    // Pass count, refusals and order flow: unchanged.
    assert_eq!(stopped.stats.passes, PASSES as u64);
    assert_eq!(stopped.stats, answered.stats);
    assert_eq!(stopped.passes, answered.passes);
    assert_eq!(stopped.submitted, answered.submitted);
    Ok(())
}
