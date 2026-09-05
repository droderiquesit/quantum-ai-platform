//! The mesh backbone, proven at the API's own seam over real sockets.
//!
//! Everything a deployed cell does here is done by the *cell's* types from
//! `qip-edge` — the uplink that publishes a genuine `CellStateDelta`, the
//! downlink that polls and verifies a grant — against listeners the API
//! bound. That is the point of the suite: `qip_mesh::delta` mirrors the wire
//! shape rather than sharing a declaration with the edge crate, and only a
//! frame the edge crate itself produced can prove the mirror right.
//!
//! The properties under test are the ones the diagram-gap audit said could
//! never fire in a deployment:
//!
//! * a delta a cell publishes is drained and ingested by a serving binary,
//!   and delivered twice it is ingested **once**;
//! * a reconciliation break crossing the wire trips the platform's kill
//!   switch, scoped to the reporting cell;
//! * a capital envelope the centre dispatches survives the durable spool,
//!   reaches a real downlink, and verifies under the operator key the trust
//!   module installed.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test
// the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::cells::CellRegistry;
use qip_api::http::{Method, Request, Response};
use qip_api::mesh::{CellAddress, MeshBackbone, MeshSettings, PendingGrant};
use qip_api::routes::Api;
use qip_api::{Handler, harden_central};
use qip_contracts::capital::{CapitalEnvelope, Utilisation};
use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::{FillRecord, FillShare};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Decimal, ManualClock, ObjectId};
use qip_edge::envelope::sign_payload;
use qip_edge::mesh::{
    CapitalDownlink, CellStateDelta, CellUplink, DeltaOrder, DownlinkConfig, StrategyUtilisation,
    UplinkConfig,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_risk::AggregateFigures;
use qip_storage::ChainArchive;
use qip_storage::kv::MemoryKeyValueStore;
use qip_transport::retry::{Sleeper, ThreadSleeper};
use qip_transport::{ClientLimits, MemoryDeadLetters, MeshConfig, RetryPolicy};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

const CELL: &str = "london-1";
const REGION: &str = "eu-west";
const STRATEGY: &str = "mean-reversion-1";
/// Exactly the plane's 32-byte floor, so `harden_central` installs it.
const ENVELOPE_KEY: &str = "a-real-key-from-the-secret-store";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn at(offset: Duration) -> Timestamp {
    start().saturating_add(offset)
}

fn sleeper() -> Arc<dyn Sleeper> {
    Arc::new(ThreadSleeper)
}

/// Tight ladders and timeouts: every peer here is a loopback in this
/// process, so a retry that waits seconds would only slow the suite down
/// without proving anything a millisecond ladder does not.
fn mesh_config(name: &str, peer: &str) -> MeshConfig {
    MeshConfig::new(name, peer)
        .with_retry(RetryPolicy {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(4),
            multiplier: 2,
            jitter_basis_points: 0,
        })
        .with_limits(ClientLimits {
            connect_timeout: StdDuration::from_millis(500),
            read_timeout: StdDuration::from_secs(1),
            write_timeout: StdDuration::from_millis(500),
            ..ClientLimits::default()
        })
}

struct Rig {
    api: Api,
    platform: Arc<Mutex<Platform>>,
    mesh: Arc<Mutex<MeshBackbone>>,
    /// Where the cell's `QIP_MESH_PEER` would point, read from the bound
    /// socket rather than assumed.
    cell_peer: String,
}

/// Assemble a serving API with the mesh backbone on ephemeral loopback
/// ports, the way `main` does it from the environment.
fn rig(inbox_capacity: usize) -> Result<Rig> {
    rig_with(inbox_capacity, PlatformConfig::default())
}

/// The same rig under a configuration the test states — the arbitrage policy
/// `main` would have read from `QIP_ARBITRAGE_POLICY_PATH` — set before the
/// trust root is installed, as `main` orders it, so the plane
/// `harden_central` rebuilds carries the policy too.
fn rig_with(inbox_capacity: usize, config: PlatformConfig) -> Result<Rig> {
    rig_full(inbox_capacity, config, None, None)
}

/// The same rig with the chain archive `main` wires through
/// `Api::with_archive`, so a test can read what `POST /cycle` handed to the
/// store and not only what the platform still holds in memory.
fn rig_full(
    inbox_capacity: usize,
    config: PlatformConfig,
    archive: Option<Arc<ChainArchive>>,
    regions: Option<RegionMembership>,
) -> Result<Rig> {
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        qip_observability::Telemetry::silent(),
        qip_financial::universe::Universe::new(),
        qip_risk::limits::LimitSet::conservative_default(),
    )?;
    // The trust root first, exactly as `main` orders it: the key installed
    // here is the key the downlink in these tests verifies against.
    harden_central(&mut platform, Some(ENVELOPE_KEY))?;

    let settings = MeshSettings {
        cells: vec![CellAddress {
            cell: CELL.to_string(),
            address: "127.0.0.1:0".to_string(),
        }],
        inbox_capacity,
        spool_capacity: 64,
        regions,
    };
    let mesh = MeshBackbone::open(
        &settings,
        Arc::new(MemoryKeyValueStore::new()),
        clock.clone() as Arc<dyn Clock>,
        // The same trust root the plane was hardened with above — one key,
        // one rotation, and the downlinks in these tests verify policy and
        // capital against the same secret, exactly as a deployment does.
        Some(ENVELOPE_KEY.as_bytes().to_vec()),
    )?;
    let cell_peer = mesh
        .cell_address(CELL)
        .ok_or_else(|| Error::not_found("the bound cell address"))?
        .to_string();
    let mesh = Arc::new(Mutex::new(mesh));

    let platform = Arc::new(Mutex::new(platform));
    let authenticator = Arc::new(Authenticator::new(vec![Credential::from_token(
        "operator@example.com",
        Role::Operator,
        "operator-token".to_string(),
        start(),
        start().saturating_add(Duration::from_days(30)),
    )]));
    let api = Api::new(
        platform.clone(),
        authenticator,
        Arc::new(RateLimiter::new(Duration::from_secs(60), 1_000)),
        clock.clone(),
    )
    .with_cells(Arc::new(CellRegistry::default()))
    .with_mesh(mesh.clone());
    let api = match archive {
        Some(archive) => api.with_archive(archive),
        None => api,
    };

    Ok(Rig {
        api,
        platform,
        mesh,
        cell_peer,
    })
}

fn request(method: Method, path: &str) -> Request {
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        "Bearer operator-token".to_string(),
    );
    Request {
        method,
        path: path.to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    }
}

fn body_json(response: &Response) -> Result<serde_json::Value> {
    serde_json::from_slice(&response.body).map_err(|error| {
        Error::schema(format!(
            "the response body is not JSON: {error}: {}",
            String::from_utf8_lossy(&response.body)
        ))
    })
}

fn run_cycle(rig: &Rig) -> Result<serde_json::Value> {
    let response = rig.api.handle(&request(Method::Post, "/api/v1/cycle"));
    assert_eq!(response.status, 202, "the cycle request was refused");
    body_json(&response)
}

fn mesh_status(rig: &Rig) -> Result<serde_json::Value> {
    let response = rig.api.handle(&request(Method::Get, "/api/v1/mesh"));
    assert_eq!(response.status, 200);
    body_json(&response)
}

fn uplink(rig: &Rig, name: &str) -> Result<CellUplink> {
    CellUplink::connect(
        UplinkConfig::new(
            CELL,
            REGION,
            mesh_config(name, &format!("http://{}", rig.cell_peer)),
        ),
        Arc::new(ManualClock::new(start())) as Arc<dyn Clock>,
        sleeper(),
        Box::new(MemoryDeadLetters::new(16)),
    )
}

fn delta(utilisation_orders_sent: u64) -> CellStateDelta {
    CellStateDelta {
        // Stamped by the uplink, which refuses a delta naming another cell.
        cell: String::new(),
        region: String::new(),
        sequence: 0,
        at: start(),
        halted: false,
        utilisation: vec![StrategyUtilisation {
            strategy: StrategyId::new(STRATEGY),
            utilisation: Utilisation {
                gross_committed: Decimal::from_int(250_000),
                realised_loss: Decimal::from_int(0),
                orders_sent: utilisation_orders_sent,
            },
            envelope_expires_at: at(Duration::from_hours(8)),
        }],
        // A real netted order, not an empty vector. This suite exists because
        // "only a frame the edge crate itself produced can prove the mirror
        // right", and a delta carrying no order proves nothing about
        // `DeltaOrder` — the half of the mirror most likely to drift, since it
        // is the half that gained a field.
        orders: vec![DeltaOrder {
            order_id: "london-1-1".to_string(),
            strategy: StrategyId::new(STRATEGY),
            object_id: ObjectId::from_string("ACME"),
            venue: VenueId::new("XLON"),
            // Net +60 is a buy, and a buy takes the ask: the side and the
            // contributor signs agree the way the cell now writes them.
            side: BookSide::Ask,
            quantity: Decimal::from_int(60),
            price: Decimal::from_int(100),
            simulated: true,
            contributors: vec![
                Contributor {
                    strategy: StrategyId::new(STRATEGY),
                    signed_size: Decimal::from_int(100),
                    inputs: vec![("book_pressure{levels=5}".to_string(), 11)],
                },
                Contributor {
                    strategy: StrategyId::new("momentum-2"),
                    signed_size: Decimal::from_int(-40),
                    inputs: vec![("momentum{}".to_string(), 9)],
                },
            ],
        }],
        // The venue's report on that order, as the cell attributes it. This
        // is what the centre bills; the order above is only what was sent.
        fills: vec![FillRecord {
            order_id: "london-1-1".to_string(),
            object_id: ObjectId::from_string("ACME"),
            venue: VenueId::new("XLON"),
            side: BookSide::Ask,
            quantity: Decimal::from_int(60),
            price: Decimal::from_int(100),
            simulated: true,
            at: start(),
            shares: vec![FillShare {
                strategy: StrategyId::new(STRATEGY),
                quantity: Decimal::from_int(60),
            }],
        }],
        fills_omitted: 0,
        refusals: Vec::new(),
        refusals_omitted: 0,
        reconciliation_breaks: Vec::new(),
        reconciliation_breaks_omitted: 0,
        crosses: Vec::new(),
        crosses_omitted: 0,
    }
}

/// The same delta with its order still resting: sent, accepted, unfilled.
fn unfilled_delta(utilisation_orders_sent: u64) -> CellStateDelta {
    let mut delta = delta(utilisation_orders_sent);
    delta.fills.clear();
    delta
}

fn signed_grant() -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            Decimal::from_int(1_000_000),
            Decimal::from_int(100_000),
            Decimal::from_int(50_000),
            vec![VenueId::new("XLON")],
            start(),
            at(Duration::from_hours(8)),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&sign_payload(
        ENVELOPE_KEY.as_bytes(),
        &unsigned.signing_payload(),
    ))
}

#[test]
fn a_delta_published_twice_by_a_restarted_cell_is_one_ingestion() -> Result<()> {
    let rig = rig(64)?;

    // Two uplinks with one identity: a cell that restarted after publishing
    // re-derives the same sequence and re-sends the same delta. Both sends
    // are real sockets against the API's own listener.
    let sent = uplink(&rig, "uplink")?.publish(delta(14), start())?;
    assert!(
        sent.is_delivered(),
        "the first delta did not reach the API's inbox: {sent:?}"
    );
    let resent = uplink(&rig, "uplink-restarted")?.publish(delta(14), start())?;
    assert!(
        resent.is_delivered(),
        "the redelivery was refused rather than absorbed: {resent:?}"
    );

    // The premise, before the drain: the plane knows nothing about the cell,
    // so whatever it knows afterwards came over the wire.
    {
        let platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        assert!(
            platform
                .central()
                .utilisation(CELL, &StrategyId::new(STRATEGY))
                .is_none(),
            "the plane already held this cell's utilisation before anything crossed"
        );
    }

    let cycle = run_cycle(&rig)?;
    assert_eq!(
        cycle["mesh"]["drained"]["absorbed"], 1,
        "one delta in two deliveries must be one absorption: {cycle}"
    );

    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    let absorbed = platform
        .central()
        .utilisation(CELL, &StrategyId::new(STRATEGY))
        .ok_or_else(|| Error::not_found("the utilisation the delta carried"))?;
    assert_eq!(
        absorbed.orders_sent, 14,
        "the utilisation the plane holds is not the one the cell reported"
    );
    drop(platform);

    // A later cycle finds nothing new: the ingestion happened once, not once
    // per cycle that could see the frame.
    let again = run_cycle(&rig)?;
    assert_eq!(again["mesh"]["drained"]["absorbed"], 0, "{again}");
    let status = mesh_status(&rig)?;
    assert_eq!(
        status["counters"]["reports_ingested"], 1,
        "two deliveries produced two ingestions: {status}"
    );
    // The order the delta carried was decoded at the centre, over real
    // sockets, out of bytes the edge crate's own serializer produced. Until
    // the fixture carried an order this counter was zero and `DeltaOrder` —
    // the half of the mirror that gained a field — crossed the wire in no test
    // anywhere.
    assert_eq!(
        status["counters"]["orders_reported"], 1,
        "the netted order did not survive the decode at the centre: {status}"
    );
    Ok(())
}

#[test]
fn a_redelivery_the_inbox_has_forgotten_is_still_one_ingestion_at_the_drain() -> Result<()> {
    // A small inbox whose dedup window forgets quickly, so the redelivery
    // below is *accepted onto the inbox again* and only the receiver's own
    // absorption memory stands between it and a second ingestion. This is
    // the layer the transport documentation says must exist, proven at the
    // API's drain.
    let rig = rig(8)?;

    let first = uplink(&rig, "uplink")?.publish(delta(14), start())?;
    assert!(first.is_delivered());
    let drained = run_cycle(&rig)?;
    assert_eq!(drained["mesh"]["drained"]["absorbed"], 1);

    // A restarted cell's uplink re-derives sequences from one, so this
    // chatter is sequences 1 through 8: the first is recognised by the inbox
    // window, the rest are new. Cycles interleave so the bounded inbox never
    // overflows — the point is to age the first key out of the window, not
    // to fill the queue.
    let mut chatter = uplink(&rig, "uplink")?;
    for _ in 0..8 {
        let sent = chatter.publish(delta(15), start())?;
        assert!(sent.is_delivered(), "a chatter delta was refused: {sent:?}");
    }
    let drained = run_cycle(&rig)?;
    assert_eq!(
        drained["mesh"]["drained"]["absorbed"], 7,
        "sequences two to eight: the restated sequence one is the inbox window's to absorb: \
         {drained}"
    );
    // The ninth key is what pushes sequence one out of the eight-key window.
    let sent = chatter.publish(delta(15), start())?;
    assert!(sent.is_delivered());
    let drained = run_cycle(&rig)?;
    assert_eq!(drained["mesh"]["drained"]["absorbed"], 1, "{drained}");

    // The redelivery of the very first delta, from a rebuilt uplink. The
    // inbox has forgotten its key, so it is queued again in earnest.
    let redelivered = uplink(&rig, "uplink-rebuilt")?.publish(delta(14), start())?;
    assert!(redelivered.is_delivered());

    let cycle = run_cycle(&rig)?;
    let status = mesh_status(&rig)?;
    assert!(
        status["receiver"]["duplicates"].as_u64().unwrap_or(0) >= 1,
        "the receiver never saw the redelivery, so this test lost its premise: {status} / {cycle}"
    );
    // Sequence 1 was ingested exactly once, however many times it arrived:
    // nine distinct deltas crossed (sequences 1..=9), and nothing more.
    assert_eq!(
        status["counters"]["reports_ingested"], 9,
        "a redelivery past the inbox window was ingested twice: {status}"
    );
    Ok(())
}

#[test]
fn a_reconciliation_break_crossing_the_wire_halts_that_cell_and_only_that_cell() -> Result<()> {
    let rig = rig(64)?;

    let mut broken = delta(14);
    broken
        .reconciliation_breaks
        .push("OBJEQUITY1: the cell holds 100 and the venue confirms 60".to_string());
    let sent = uplink(&rig, "uplink")?.publish(broken, start())?;
    assert!(sent.is_delivered());

    let cycle = run_cycle(&rig)?;
    assert_eq!(
        cycle["mesh"]["drained"]["halted_cells"][0], CELL,
        "the ingestion did not report the halt: {cycle}"
    );

    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    let switch = platform.autonomy().kill_switch();
    assert!(
        switch.halted_scopes().contains(&CELL),
        "the reconciliation break crossed the wire and tripped nothing"
    );
    assert!(
        !switch.is_globally_tripped(),
        "one cell's bookkeeping failure must not halt the whole platform"
    );
    Ok(())
}

#[test]
fn a_dispatched_envelope_survives_the_spool_and_verifies_at_a_real_downlink() -> Result<()> {
    let rig = rig(64)?;
    let grant = signed_grant()?;

    let mut mesh = rig
        .mesh
        .lock()
        .map_err(|_| Error::invalid("the mesh lock is poisoned"))?;
    let dispatched = mesh.dispatch(
        vec![PendingGrant {
            cell: CELL.to_string(),
            envelope: grant.clone(),
        }],
        at(Duration::from_secs(30)),
    );
    assert_eq!(
        dispatched.delivered, 1,
        "the grant did not reach the cell's capital inbox: {dispatched:?}"
    );

    // Dispatching the same grant again is a no-op, not a second spool entry:
    // the plane holds its envelopes across cycles, and every cycle re-offers
    // them.
    let again = mesh.dispatch(
        vec![PendingGrant {
            cell: CELL.to_string(),
            envelope: grant.clone(),
        }],
        at(Duration::from_secs(31)),
    );
    assert_eq!(
        (again.delivered, again.held, again.deferred),
        (0, 0, 0),
        "a grant already dispatched was dispatched again: {again:?}"
    );

    // An envelope for a cell this process does not serve is a configuration
    // gap the summary must name, not a silent skip.
    let unserved = mesh.dispatch(
        vec![PendingGrant {
            cell: "tokyo-9".to_string(),
            envelope: signed_grant()?,
        }],
        at(Duration::from_secs(32)),
    );
    assert_eq!(unserved.unserved_cells, vec!["tokyo-9".to_string()]);
    drop(mesh);

    // The cell's half, unmodified from the deployable: poll the same address
    // a deployed cell's QIP_MESH_PEER names, and verify under the same key
    // the trust module installed at the centre.
    let mut downlink = CapitalDownlink::connect(
        DownlinkConfig::new(
            CELL,
            mesh_config("downlink", &format!("http://{}", rig.cell_peer)),
        ),
        ENVELOPE_KEY.as_bytes(),
        Arc::new(ManualClock::new(start())) as Arc<dyn Clock>,
        sleeper(),
    )?;
    let batch = downlink.poll(at(Duration::from_secs(60)))?;
    assert_eq!(
        batch.verified.len(),
        1,
        "the grant the centre dispatched did not verify at the cell: {:?}",
        batch.refused
    );
    assert_eq!(batch.verified[0].strategy().as_str(), STRATEGY);
    assert_eq!(
        batch.verified[0].expires_at(),
        at(Duration::from_hours(8)),
        "the expiry the cell verified is not the one the centre granted"
    );

    let status = mesh_status(&rig)?;
    assert_eq!(status["counters"]["envelopes_dispatched"], 1, "{status}");
    assert_eq!(
        status["counters"]["envelopes_unserved"], 1,
        "the unserved envelope left no trace an operator could read: {status}"
    );
    Ok(())
}

#[test]
fn an_api_without_mesh_configuration_serves_no_mesh_and_says_so() -> Result<()> {
    // Assembled without `with_mesh`, exactly what `main` builds when
    // QIP_MESH_CELLS is unset.
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock.clone(), config.seed);
    let platform = Platform::new(
        config,
        context,
        qip_observability::Telemetry::silent(),
        qip_financial::universe::Universe::new(),
        qip_risk::limits::LimitSet::conservative_default(),
    )?;
    let api = Api::new(
        Arc::new(Mutex::new(platform)),
        Arc::new(Authenticator::new(vec![Credential::from_token(
            "operator@example.com",
            Role::Operator,
            "operator-token".to_string(),
            start(),
            start().saturating_add(Duration::from_days(30)),
        )])),
        Arc::new(RateLimiter::new(Duration::from_secs(60), 1_000)),
        clock,
    );

    let mesh = api.handle(&request(Method::Get, "/api/v1/mesh"));
    assert_eq!(mesh.status, 200);
    let body = body_json(&mesh)?;
    assert_eq!(body["available"], false);
    assert!(
        body["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("QIP_MESH_CELLS"),
        "the refusal does not name the variable that would change it: {body}"
    );

    let status = api.handle(&request(Method::Get, "/api/v1/system/status"));
    let body = body_json(&status)?;
    assert_eq!(
        body["mesh"]["served"], false,
        "the status page does not state that no mesh is served: {body}"
    );

    // And the cycle report carries no mesh key at all: an absent mesh is not
    // a mesh that did nothing.
    let cycle = api.handle(&request(Method::Post, "/api/v1/cycle"));
    let body = body_json(&cycle)?;
    assert!(
        body.get("mesh").is_none(),
        "an unconfigured mesh appeared in the cycle report: {body}"
    );
    Ok(())
}

// --- policy and halt, end to end over real sockets ---------------------------

use qip_edge::mesh::PolicyDownlink;

fn policy_downlink(rig: &Rig, name: &str) -> Result<PolicyDownlink> {
    PolicyDownlink::connect(
        DownlinkConfig::new(
            CELL,
            mesh_config(name, &format!("http://{}", rig.cell_peer)),
        ),
        ENVELOPE_KEY.as_bytes(),
        Arc::new(ManualClock::new(start())) as Arc<dyn Clock>,
        sleeper(),
    )
}

#[test]
fn a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it() -> Result<()> {
    // The whole loop the integration pass reported broken: policy travelling
    // down, and an operator's one action stopping the regions. Real sockets,
    // real signatures, the same key on both ends.
    let rig = rig(64)?;
    let mut downlink = policy_downlink(&rig, "policy-e2e")?;

    run_cycle(&rig)?;
    let batch = downlink.poll(at(Duration::from_secs(5)))?;
    assert!(
        batch.refused.is_empty(),
        "the cycle shipped something this cell refused: {:?}",
        batch.refused
    );
    let payload = batch
        .verified
        .first()
        .expect("the cycle shipped a payload this cell verified");
    assert!(!payload.halted(), "nothing tripped, yet the payload halts");
    // The grant manifest slot is produced — the platform really fills it —
    // and the cognition slots are unproduced, because nothing produces them.
    // Both halves are the design: what exists ships, what does not narrows.
    assert!(
        payload.payload().capital_grants.value().is_some(),
        "the grant manifest slot shipped unproduced"
    );
    assert!(
        payload.payload().risk_envelope.value().is_some(),
        "the risk envelope slot shipped unproduced"
    );
    assert!(
        payload.payload().belief_priors.value().is_none(),
        "a belief slot shipped produced, and nothing here produces beliefs"
    );

    // The operator trips the switch; the same action must reach the region.
    let response = rig
        .api
        .handle(&request(Method::Post, "/api/v1/kill-switch"));
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&response.body)?;
    assert_eq!(
        body["broadcast"]["sent"].as_u64(),
        Some(1),
        "the trip was not broadcast to the one configured cell: {body}"
    );

    let batch = downlink.poll(at(Duration::from_secs(10)))?;
    let halt = batch
        .halts
        .first()
        .expect("the broadcast halt arrived and verified");
    assert!(!halt.reason().is_empty());
    Ok(())
}

/// The sink used to build the report from the standing alone and drop the
/// interval, so the centre attributed no fill and settled no cross however
/// much the cell traded: every strategy book at the centre stayed flat while
/// the cell's own journal showed the orders. Then it carried the orders and
/// the centre billed *those*, resting or not. The delta now carries the fill
/// the venue confirmed, and it is the fill that reaches the book. The
/// premise is asserted first — no lot for the contributor before the drain
/// — so a plane that already knew the position could not make this pass.
#[test]
fn the_fills_a_cell_reports_reach_the_centres_strategy_books() -> Result<()> {
    let rig = rig(64)?;
    let sent = uplink(&rig, "uplink")?.publish(delta(1), start())?;
    assert!(
        sent.is_delivered(),
        "the delta did not reach the inbox: {sent:?}"
    );

    {
        let platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        assert!(
            platform
                .central()
                .strategy_lot(CELL, &StrategyId::new(STRATEGY), "ACME")
                .is_none(),
            "the centre held a lot for the strategy before anything crossed"
        );
    }

    let cycle = run_cycle(&rig)?;
    assert_eq!(cycle["mesh"]["drained"]["absorbed"], 1, "{cycle}");

    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    let lot = platform
        .central()
        .strategy_lot(CELL, &StrategyId::new(STRATEGY), "ACME")
        .expect("the fill the cell reported was not attributed to its contributor");
    assert_eq!(
        lot.quantity,
        Decimal::from_int(60),
        "the whole fill belongs to the one strategy the cell's share names"
    );
    Ok(())
}

/// The defect this slice closes, at the seam a deployment uses: a delta
/// carrying a sent order and no fill used to be billed as a fill of the
/// order's full size — a strategy book, a risk aggregate and a position for
/// an order still resting at the venue. Premise first: the delta genuinely
/// carries the order and no fill, and the centre holds nothing for the
/// strategy before the drain.
#[test]
fn an_order_a_cell_reports_sent_and_unfilled_reaches_no_book_and_charges_nothing() -> Result<()> {
    let rig = rig(64)?;
    let resting = unfilled_delta(1);
    assert_eq!(resting.orders.len(), 1, "the premise is a sent order");
    assert!(
        resting.fills.is_empty(),
        "the premise is that nothing filled"
    );
    let sent = uplink(&rig, "uplink")?.publish(resting, start())?;
    assert!(
        sent.is_delivered(),
        "the delta did not reach the inbox: {sent:?}"
    );
    {
        let platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        assert!(
            platform
                .central()
                .strategy_lot(CELL, &StrategyId::new(STRATEGY), "ACME")
                .is_none()
        );
        assert!(platform.risk_figures().strategy_gross(CELL).is_zero());
    }

    let cycle = run_cycle(&rig)?;
    assert_eq!(
        cycle["mesh"]["drained"]["absorbed"], 1,
        "the delta was not absorbed, so nothing below was tested: {cycle}"
    );

    {
        // Scoped: the status read below takes the same lock.
        let platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        assert!(
            platform
                .central()
                .strategy_lot(CELL, &StrategyId::new(STRATEGY), "ACME")
                .is_none(),
            "a resting order was booked as a position"
        );
        assert!(
            platform.risk_figures().strategy_gross(CELL).is_zero(),
            "a resting order was charged to the risk aggregate as a fill"
        );
    }
    // And the cell was not halted for it: an open order is not a break.
    let status = mesh_status(&rig)?;
    assert_eq!(status["counters"]["orders_reported"], 1, "{status}");
    assert_eq!(status["counters"]["fills_reported"], 0, "{status}");
    assert_eq!(status["counters"]["cell_halts"], 0, "{status}");
    Ok(())
}

// --- the cycle whitelist, from a policy and a grant, over real sockets -------
//
// Slot 8 shipped unproduced from every deployed centre until `pending_policy`
// called the producer, so the desk the node can install installed never.
// These tests drive the seam a deployment uses — `POST /cycle` under the
// platform lock, the signed payload over the wire, the cell's own downlink
// verifying it — and read the slot the way the node's installer does.
//
// The grant is issued through the plane's own door, because it is the only
// door: `CentralPlane::issue` refuses a strategy the ladder does not say
// holds capital, so the fixture below compiles a strategy, attaches evidence
// that passes every gate, walks it to pilot under dual approval, and issues.
// Mirrored from the kernel's own suite rather than shared with it — a
// fixture crate would be a dependency, and there is nowhere below both to
// put one.

use qip_capital::allocation::StrategyProposal;
use qip_capital::capacity::CapacityModel;
use qip_compliance::approval::OperatorCredential;
use qip_contracts::feature::FeatureKey;
use qip_contracts::gate::GateStage;
use qip_contracts::governance::Approval;
use qip_contracts::policy::{CycleWhitelist, GrantManifest};
use qip_contracts::signal::SignalKind;
use qip_contracts::venue::VenueClass;
use qip_core::dec;
use qip_core::rng::{Rng, Xoshiro256};
use qip_events::{EventFilter, Topic};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_kernel::central::{
    ArbitragePolicy, CentralConfig, CentralPlane, RegionMembership, StrategyCandidate,
    WhitelistIssue, WhitelistedMarket, WhitelistedVenue, capital_subject,
};
use qip_lifecycle::evidence::{
    CrossValidationRun, FeatureTiming, HoldoutEvidence, KillCondition, LeakageAudit, PaperEvidence,
    PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence, StrategyEvidence,
};
use qip_lifecycle::trials::StrategyFamily;
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;

/// The desk's strategy: the one the policy names and the one the grant funds.
const DESK: &str = "arb-desk";
/// The venue the grant permits. The policy trades here, or, in the refusal
/// test, somewhere the grant does not reach.
const DESK_VENUE: &str = "XNYS";
/// The most conversions the node walks per pass — `qip-edge-node`'s
/// `MAX_CONVERSIONS`, restated because an app cannot link another app; the
/// kernel's `MAX_MARKETS` is half of it for the same reason.
const NODE_MAX_CONVERSIONS: usize = 256;

fn desk() -> StrategyId {
    StrategyId::new(DESK)
}

fn desk_venue() -> VenueId {
    VenueId::new(DESK_VENUE)
}

/// Ninety days of pilot plus a month, so the scaled gate's duration bar is met.
fn scaled_at() -> Timestamp {
    start().saturating_add(Duration::from_days(120))
}

fn compile(id: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = ObjectId::from_string(format!("obj-{id}"));
    let pressure = FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;
    let spec = StrategySpec::new(StrategyId::new(id), subject, Duration::from_millis(250))
        .with_rule(Rule::new(
            "enter",
            SignalKind::Enter,
            Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
            Expr::Exact(Decimal::from_int(100)),
            Expr::Statistic(0.62),
            500,
        ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn good_returns(seed: u64, n: usize, drift: f64) -> Vec<f64> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..n)
        .map(|_| {
            let u = rng.next_f64() + rng.next_f64() - 1.0;
            drift + u * 0.01
        })
        .collect()
}

fn honest_cross_validation(observations: usize) -> Result<CrossValidationRun> {
    let (folds, label_horizon, embargo) = (5, 10, 5);
    let splits = PurgedSplit::new(folds, label_horizon, embargo)?.split(observations)?;
    Ok(CrossValidationRun {
        folds,
        label_horizon,
        embargo,
        observations,
        purged: splits.iter().map(|s| s.purged).sum(),
        embargoed: splits.iter().map(|s| s.embargoed).sum(),
    })
}

fn dual_approval(subject: &str, at: Timestamp, rationale: &str) -> Result<Approval> {
    Approval::new(subject, "alice.chen", at, rationale)?.countersigned_by("bram.oduya")
}

fn credentials(at: Timestamp) -> Result<Vec<OperatorCredential>> {
    Ok(vec![
        OperatorCredential::verified("alice.chen", "webauthn", at)?,
        OperatorCredential::verified("bram.oduya", "webauthn", at)?,
    ])
}

fn full_evidence(id: &StrategyId, cell: &str) -> Result<StrategyEvidence> {
    let observations = 400;
    let holdout = HoldoutEvidence {
        holdout_returns: good_returns(1, observations, 0.0018),
        in_sample_folds: (0..5).map(|f| good_returns(10 + f, 80, 0.0020)).collect(),
        out_of_sample_folds: (0..5).map(|f| good_returns(20 + f, 80, 0.0018)).collect(),
        trials: 12,
        periods_per_year: 252.0,
        cross_validation: honest_cross_validation(observations)?,
        leakage: LeakageAudit {
            timings: (0..8)
                .map(|i| FeatureTiming {
                    feature: format!("feature-{i}"),
                    known_at: start(),
                    used_at: at(Duration::from_hours(1)),
                })
                .collect(),
            restated_without_snapshots: Vec::new(),
        },
    };
    let paper = PaperEvidence {
        against_live_data: true,
        assumed_cost_bps: 8.0,
        realised_cost_bps: (0..400).map(|i| 7.0 + f64::from(i % 5) * 0.2).collect(),
        peak_participation: 0.04,
        modelled_participation_limit: 0.10,
        unfillable_orders: 4,
        filled_orders: 400,
    };
    let shadow = ShadowEvidence {
        decisions: (0..400)
            .map(|i| ShadowDecision {
                at: at(Duration::from_mins(i)),
                object_id: ObjectId::from_string(format!("obj-{}", i % 20)),
                live: SignalKind::Enter,
                predicted: SignalKind::Enter,
                live_quantity: dec!("100"),
                predicted_quantity: dec!("100"),
            })
            .collect(),
        orders_reached_a_venue: false,
        decision_latency_p99: Duration::from_millis(40),
    };
    let proposed = CapitalEnvelope::new(
        id.clone(),
        cell,
        dec!("250000"),
        dec!("250000"),
        dec!("250000"),
        vec![desk_venue()],
        start(),
        at(Duration::from_days(14)),
        "alice.chen",
        "proposed-not-issued",
    )?;
    let pilot = PilotEvidence {
        approval: Some(dual_approval(
            &format!("{id} pilot"),
            start(),
            "shadow agreement held at 100% over 400 decisions",
        )?),
        envelope: Some(proposed),
        kill_conditions: vec![
            KillCondition::RealisedLoss(dec!("25000")),
            KillCondition::Drawdown(0.08),
            KillCondition::ConsecutiveLosingDays(5),
        ],
    };
    let scaled = ScaledEvidence {
        pilot_returns: good_returns(99, 120, 0.0030),
        pilot_started_at: start(),
        pilot_utilisation: Utilisation {
            gross_committed: dec!("180000"),
            realised_loss: dec!("0"),
            orders_sent: 5_400,
        },
        proposed_notional: dec!("1000000"),
        modelled_capacity: dec!("4000000"),
        pilot_approval: Some(dual_approval(
            &format!("{id} pilot"),
            start(),
            "shadow agreement held at 100% over 400 decisions",
        )?),
        scaling_approval: Some(dual_approval(
            &format!("{id} scaling"),
            scaled_at(),
            "ninety days at pilot returned a 0.7 Sharpe inside a quarter of capacity",
        )?),
    };
    Ok(StrategyEvidence::new()
        .with_holdout(holdout)
        .with_paper(paper)
        .with_shadow(shadow)
        .with_pilot(pilot)
        .with_scaled(scaled))
}

fn proposal(id: &StrategyId, cell: &str) -> Result<StrategyProposal> {
    Ok(StrategyProposal {
        strategy: id.clone(),
        cell: cell.to_string(),
        venue: desk_venue(),
        expected_sharpe: 1.8,
        sharpe_standard_error: 0.05,
        capacity: CapacityModel::new(
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 4.0),
            TransactionCostModel::listed(4.0),
            45.0,
            dec!("100"),
            0.5,
        )?,
        capacity_uncertainty: 0.2,
    })
}

/// Register the desk's strategy with evidence that passes every gate, walk it
/// to pilot, and issue its grant at `CELL`. Returns the envelope the plane
/// now holds — the one fact the whitelist is sized against.
fn grant_the_desk(plane: &mut CentralPlane) -> Result<CapitalEnvelope> {
    let id = desk();
    let (compiled, program) = compile(DESK)?;
    let candidate = StrategyCandidate::new(
        compiled,
        program,
        StrategyFamily::new("mesh-tests")?,
        CELL,
        desk_venue(),
        start(),
    )?
    .with_evidence(full_evidence(&id, CELL)?)
    .with_model("microprice-distilled@3")
    .with_evidence_artifacts(vec![
        format!("sha256:holdout-{id}"),
        format!("sha256:shadow-{id}"),
    ]);
    plane.factory_mut().register(candidate)?;
    plane.set_proposal(proposal(&id, CELL)?);
    for rung in [
        GateStage::Holdout,
        GateStage::Paper,
        GateStage::Shadow,
        GateStage::Pilot,
    ] {
        let approval = if rung.requires_human_approval() {
            Some(dual_approval(
                DESK,
                start(),
                "every gate check passed with the evidence attached",
            )?)
        } else {
            None
        };
        plane
            .factory_mut()
            .promote(&id, approval, "the gate passed", start())?;
    }
    let approval = dual_approval(
        &capital_subject(&id, CELL),
        start(),
        "the pilot gate passed and the allocator sized it inside the budget",
    )?;
    plane.issue(
        &id,
        "research.desk",
        &approval,
        &credentials(start())?,
        0.0,
        start(),
    )?;
    plane
        .envelope(CELL, &id)
        .cloned()
        .ok_or_else(|| Error::not_found("the grant the plane just issued"))
}

/// A policy trading one book at each named venue, funded in USD.
fn arbitrage_policy(venues: &[&str]) -> ArbitragePolicy {
    ArbitragePolicy {
        strategy: desk(),
        funding_instrument: "USD".to_string(),
        venues: venues
            .iter()
            .map(|venue| {
                (
                    venue.to_string(),
                    WhitelistedVenue {
                        class: VenueClass::Exchange,
                        taker_cost: dec!("0.0005"),
                    },
                )
            })
            .collect(),
        markets: venues
            .iter()
            .map(|venue| WhitelistedMarket {
                venue: venue.to_string(),
                market: format!("AAA-USD@{venue}"),
                base: "AAA".to_string(),
                quote: "USD".to_string(),
            })
            .collect(),
        start_sizes: BTreeMap::from([("AAA".to_string(), dec!("100"))]),
    }
}

fn config_with(policy: ArbitragePolicy) -> PlatformConfig {
    PlatformConfig::default().with_central(CentralConfig {
        arbitrage: Some(policy),
        ..CentralConfig::default()
    })
}

/// The whitelist lines the cycle response carries for the one cell.
fn whitelist_lines(cycle: &serde_json::Value) -> Vec<String> {
    cycle["mesh"]["policy"]["whitelist"]
        .as_array()
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn share_lines(cycle: &serde_json::Value) -> Vec<String> {
    cycle["mesh"]["policy"]["shares"]
        .as_array()
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The grant manifest the cell's own downlink verified, read the way the
/// node reads it before deriving its region share.
fn shipped_manifest(rig: &Rig, name: &str) -> Result<Option<GrantManifest>> {
    let mut downlink = policy_downlink(rig, name)?;
    let batch = downlink.poll(at(Duration::from_secs(5)))?;
    let payload = batch
        .verified
        .first()
        .ok_or_else(|| Error::not_found("a payload the cell verified"))?;
    Ok(payload.payload().capital_grants.value().cloned())
}

/// ADR 0039 at the seam a deployment runs: with a membership declared, the
/// `capital_grants` slot carries the centre's share of the region's grant
/// for this cell — the grant the ladder issued, named by signature — and the
/// cycle says so; a second cell under the same grant would be partitioned
/// against the same plan, which the kernel's own suite proves. Until the
/// seam called `grant_manifests`, no deployed centre withheld a manifest,
/// whatever the membership said.
#[test]
fn with_a_membership_declared_the_cycle_ships_the_cell_its_share_of_the_regions_grant() -> Result<()>
{
    let membership = RegionMembership::parse(&format!("europe-west2=5000000:{CELL}"))?;
    let rig = rig_full(
        64,
        config_with(arbitrage_policy(&[DESK_VENUE])),
        None,
        Some(membership),
    )?;
    let grant = {
        let mut platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        grant_the_desk(platform.central_mut())?
    };
    // Premise: the grant is live, so a manifest that does not name it is
    // the seam's omission and not the fixture's.
    assert!(grant.is_live(at(Duration::from_secs(5))));

    let cycle = run_cycle(&rig)?;
    let lines = share_lines(&cycle);
    assert_eq!(lines.len(), 1, "one cell, one share line: {cycle}");
    assert!(
        lines[0].contains(&format!("region share for {CELL}"))
            && lines[0].contains("europe-west2")
            && !lines[0].contains("not shipped"),
        "the cycle does not account for the share it shipped: {}",
        lines[0]
    );

    let manifest = shipped_manifest(&rig, "region-share")?
        .ok_or_else(|| Error::not_found("a produced grant manifest"))?;
    assert_eq!(
        manifest.live_grants,
        vec![grant.signature().to_string()],
        "the share's manifest does not name exactly the grant the ladder issued"
    );
    Ok(())
}

/// The undeclared case says what it is. Every live grant shipping to every
/// cell is the one-cell-per-region shape ADR 0039 grows out of; a deployment
/// still in it reads that in the cycle rather than discovering it when two
/// nodes under one grant each spend it.
#[test]
fn without_a_membership_the_cycle_says_every_live_grant_shipped_and_names_the_variable()
-> Result<()> {
    let rig = rig_with(64, config_with(arbitrage_policy(&[DESK_VENUE])))?;
    {
        let mut platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        grant_the_desk(platform.central_mut())?;
    }
    let cycle = run_cycle(&rig)?;
    let lines = share_lines(&cycle);
    assert_eq!(lines.len(), 1, "one cell, one share line: {cycle}");
    assert!(
        lines[0].contains("no QIP_MESH_REGIONS declared") && lines[0].contains("1 grant(s)"),
        "the undeclared membership is not said out loud: {}",
        lines[0]
    );
    Ok(())
}

/// Read the slot the way the node's installer does: the payload the cell's
/// own downlink verified, and slot 8 on it.
fn shipped_whitelist(rig: &Rig, name: &str) -> Result<Option<CycleWhitelist>> {
    let mut downlink = policy_downlink(rig, name)?;
    let batch = downlink.poll(at(Duration::from_secs(5)))?;
    assert!(
        batch.refused.is_empty(),
        "the cycle shipped something this cell refused: {:?}",
        batch.refused
    );
    let payload = batch
        .verified
        .first()
        .ok_or_else(|| Error::not_found("a payload the cell verified"))?;
    // The rest of the payload ships whatever slot 8 does: the grant manifest
    // is produced on every path below.
    assert!(
        payload.payload().capital_grants.value().is_some(),
        "the grant manifest slot shipped unproduced"
    );
    Ok(payload.payload().cycle_whitelist.value().cloned())
}

/// The whole path a deployment runs: a policy the operator stated, a grant
/// the ladder issued, one `POST /cycle`, and a whitelist at the cell's
/// downlink that the node's `graph_from_whitelist` would accept — every
/// venue permitted by the grant, no self-conversion, every cost in [0, 1),
/// one class per venue, within the node's walk bound — sized from the
/// grant's own order limit. Until the seam called the producer, the slot
/// this reads was unproduced in every payload any centre ever shipped.
#[test]
fn a_cycle_ships_the_desk_a_live_grant_funds_as_a_whitelist_the_cell_verifies() -> Result<()> {
    let rig = rig_with(64, config_with(arbitrage_policy(&[DESK_VENUE])))?;
    let grant = {
        let mut platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        // Premise: nothing has been distributed before the cycle, so the
        // record found afterwards was written by the shipping seam.
        let distributed = EventFilter::new().topic(Topic::PolicyDistributed);
        assert!(platform.replay_journal(&distributed)?.is_empty());
        grant_the_desk(platform.central_mut())?
    };
    // Premise: the grant is live and permits the policy's venue, so a
    // refusal below would be the seam's and not the fixture's.
    assert!(grant.is_live(at(Duration::from_secs(5))));
    assert!(grant.permits_venue(&desk_venue()));
    assert!(grant.order_limit().is_positive());

    let cycle = run_cycle(&rig)?;
    let lines = whitelist_lines(&cycle);
    assert_eq!(lines.len(), 1, "one cell, one line: {cycle}");
    assert!(
        lines[0].contains("2 trade edge(s)") && lines[0].contains(grant.signature()),
        "the cycle does not say what it shipped and against which grant: {}",
        lines[0]
    );

    let whitelist = shipped_whitelist(&rig, "policy-whitelist")?
        .ok_or_else(|| Error::not_found("a produced cycle whitelist on the verified payload"))?;
    // One book, two edges: buying the base out of the quote, selling it back.
    assert_eq!(whitelist.conversions.len(), 2, "{whitelist:?}");
    assert!(whitelist.conversions.len() <= NODE_MAX_CONVERSIONS);
    let mut classes = BTreeMap::new();
    for conversion in &whitelist.conversions {
        assert!(
            grant.permits_venue(&VenueId::new(conversion.venue.as_str())),
            "the whitelist names {}, which the grant does not permit; the node refuses it whole",
            conversion.venue
        );
        assert_ne!(conversion.from, conversion.to, "a self-conversion");
        assert!(
            !conversion.cost_fraction.is_negative() && conversion.cost_fraction < Decimal::ONE,
            "cost {} is outside [0, 1)",
            conversion.cost_fraction
        );
        if let Some(previous) = classes.insert(conversion.venue.clone(), conversion.venue_class) {
            assert_eq!(previous, conversion.venue_class, "one venue, two classes");
        }
    }
    assert!(
        whitelist
            .conversions
            .iter()
            .any(|c| c.from == "USD" && c.to == "AAA")
            && whitelist
                .conversions
                .iter()
                .any(|c| c.from == "AAA" && c.to == "USD"),
        "the book did not become an edge in each direction: {:?}",
        whitelist.conversions
    );
    // Sized from the one authority on how much may be committed.
    assert_eq!(
        whitelist.start_sizes.get("USD"),
        Some(&grant.order_limit()),
        "the funding instrument is not sized by the grant's order limit"
    );
    assert_eq!(whitelist.start_sizes.get("AAA"), Some(&dec!("100")));

    // And the record: the whitelist that reached the cell is the one the
    // journal holds, so the permission is reproducible from the log alone.
    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    let recorded = platform.replay_journal(&EventFilter::new().topic(Topic::PolicyDistributed))?;
    assert_eq!(recorded.len(), 1, "one cycle, one issue for the one cell");
    let issue = recorded[0].decode::<WhitelistIssue>()?.body;
    assert_eq!(issue.cell, CELL);
    assert_eq!(
        issue.whitelist, whitelist,
        "the wire and the journal disagree"
    );
    Ok(())
}

/// The record of what a cycle shipped is in the store before the cycle
/// answers.
///
/// Not hypothetical. `POST /cycle` archived the log first and issued the
/// whitelist second, so the `policy_distributed` record a cycle journals
/// reached the store only when the *next* cycle archived. Against a running
/// `qip-api` serving one cell, `/system` reported five events logged while
/// the store held four, and the missing one was the last cycle's whitelist —
/// a permission the cell had already been sent, with no durable record of it.
/// This process has no signal handler, so "the next cycle will archive it" is
/// a promise a `SIGTERM` between cycles breaks; the whitelist is journaled
/// precisely so it is never a permission reproducible from nothing.
#[test]
fn the_whitelist_a_cycle_ships_is_in_the_archive_before_the_cycle_answers() -> Result<()> {
    let archive = Arc::new(ChainArchive::open(Arc::new(MemoryKeyValueStore::new()))?);
    let rig = rig_full(64, PlatformConfig::default(), Some(archive.clone()), None)?;
    // Premise: nothing has been archived before the cycle, so every record
    // found afterwards was handed over by the route.
    assert_eq!(archive.len()?, 0);

    run_cycle(&rig)?;

    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    // Premise: the cycle journaled one whitelist for the one served cell. If
    // it had not, an archive with no such record would prove nothing.
    let journaled = platform
        .replay_journal(&EventFilter::new().topic(Topic::PolicyDistributed))?
        .len();
    assert_eq!(journaled, 1, "one cell, one whitelist issued");

    let archived_whitelists = archive
        .records()?
        .iter()
        .filter(|entry| entry.record.event.topic == Topic::PolicyDistributed)
        .count();
    assert_eq!(
        archived_whitelists, 1,
        "the whitelist this cycle shipped is not in the archive it answered from; it would \
         reach the store only when a later cycle archived, and never if the process stopped \
         first"
    );
    // And nothing the log holds is still waiting for a later cycle.
    assert_eq!(
        archive.len()?,
        platform.event_log().len(),
        "the archive lags the log after the cycle answered"
    );
    Ok(())
}

/// No policy is the deployed default, and it must read as what it is at both
/// ends: the slot ships produced and empty — what the producer returned and
/// what the journal says was distributed — so the installer declines with
/// `EmptyWhitelist` rather than `NoWhitelist`, and the cycle says the policy
/// is unset rather than leaving an operator to infer it from a desk that
/// never appears.
#[test]
fn without_a_policy_the_whitelist_ships_empty_and_the_cycle_says_the_policy_is_unset() -> Result<()>
{
    let rig = rig(64)?;
    {
        let platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        // Premise: the default configuration really carries no policy.
        assert!(platform.central().config().arbitrage.is_none());
    }

    let cycle = run_cycle(&rig)?;
    let lines = whitelist_lines(&cycle);
    assert_eq!(lines.len(), 1, "one cell, one line: {cycle}");
    assert!(
        lines[0].contains("CentralConfig::arbitrage is unset"),
        "the cycle does not say why the whitelist is empty: {}",
        lines[0]
    );

    let whitelist = shipped_whitelist(&rig, "policy-no-policy")?
        .ok_or_else(|| Error::not_found("a produced, empty cycle whitelist"))?;
    assert!(
        whitelist.conversions.is_empty() && whitelist.start_sizes.is_empty(),
        "an unset policy shipped a whitelist with something in it: {whitelist:?}"
    );
    Ok(())
}

/// The producer's refusal, at the seam. A policy trading somewhere the desk's
/// grant does not reach is a whitelist the cell would refuse whole; the
/// centre refuses it first, ships the slot unproduced — the cell narrows,
/// and never receives a guessed whitelist — and names the venue where the
/// policy was shipped from, not only in a cell's delta stream.
#[test]
fn a_policy_venue_the_grant_does_not_permit_ships_the_slot_unproduced_and_names_the_venue()
-> Result<()> {
    let rig = rig_with(64, config_with(arbitrage_policy(&[DESK_VENUE, "XOTHER"])))?;
    let grant = {
        let mut platform = rig
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
        grant_the_desk(platform.central_mut())?
    };
    // Premise: the grant is live and it is the second venue it does not
    // permit, so the refusal below is the venue check and nothing else.
    assert!(grant.is_live(at(Duration::from_secs(5))));
    assert!(grant.permits_venue(&desk_venue()));
    assert!(!grant.permits_venue(&VenueId::new("XOTHER")));

    let cycle = run_cycle(&rig)?;
    let lines = whitelist_lines(&cycle);
    assert_eq!(lines.len(), 1, "one cell, one line: {cycle}");
    assert!(
        lines[0].contains("not shipped")
            && lines[0].contains("XOTHER")
            && lines[0].contains("does not permit"),
        "the refusal does not name the venue or say nothing shipped: {}",
        lines[0]
    );

    assert!(
        shipped_whitelist(&rig, "policy-refused")?.is_none(),
        "a refused whitelist reached the cell as a produced slot"
    );
    // Nothing was distributed, so nothing is recorded as distributed.
    let platform = rig
        .platform
        .lock()
        .map_err(|_| Error::invalid("the platform lock is poisoned"))?;
    assert!(
        platform
            .replay_journal(&EventFilter::new().topic(Topic::PolicyDistributed))?
            .is_empty(),
        "a refusal was journaled as a distribution"
    );
    Ok(())
}
