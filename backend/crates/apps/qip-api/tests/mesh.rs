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
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Decimal, ManualClock};
use qip_edge::envelope::sign_payload;
use qip_edge::mesh::{
    CapitalDownlink, CellStateDelta, CellUplink, DownlinkConfig, StrategyUtilisation, UplinkConfig,
};
use qip_kernel::{Platform, PlatformConfig};
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
    let config = PlatformConfig::default();
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
    };
    let mesh = MeshBackbone::open(
        &settings,
        Arc::new(MemoryKeyValueStore::new()),
        clock.clone() as Arc<dyn Clock>,
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
        orders: Vec::new(),
        refusals: Vec::new(),
        refusals_omitted: 0,
        reconciliation_breaks: Vec::new(),
        reconciliation_breaks_omitted: 0,
    }
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
