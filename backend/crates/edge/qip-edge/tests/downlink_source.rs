//! The downlinks' frame source, and the one verify path behind it.
//!
//! ADR 0100 §8 moves control down onto the event fabric "through the SAME
//! verify code as the mesh downlink". The failure that sentence exists to
//! prevent is a second verification path: a fabric adapter that decodes a
//! grant itself, or trusts the fabric's own integrity and skips the frame
//! checks, is the route by which an unsigned or edited grant reaches a cell
//! while the mesh path still looks perfectly guarded (FABRIC-081).
//!
//! So every property here is stated as a comparison with the mesh, over a
//! real socket, rather than as a list of what a scripted source does on its
//! own. A test that only checked the scripted source refuses a forgery would
//! pass against a fabric path with its own, weaker checks, so long as those
//! checks happened to catch that forgery; asserting the scripted batch *is*
//! the mesh batch, refusal by refusal and reason by reason, is what makes a
//! divergence in either direction visible.
//!
//! Two non-mesh routes are exercised wherever the property allows, because
//! both exist: a downlink built over a source (`from_source`) and a mesh
//! downlink taking a second source alongside its own (`poll_from`), which is
//! the shape a cell has while control moves from one wire to the other.
//!
//! A frame put on the mesh here goes into the inbox through
//! `MeshInbox::accept` rather than through a publisher. That is deliberate:
//! the endpoint's publish-time hash check would refuse an edited frame at the
//! door, and the frame this cell must refuse is one edited *after* the inbox
//! took it — on the pull path the endpoint's check never covered.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{HaltCommand, PolicyPayload};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Clock, CorrelationId, Decimal, Duration, Id, Lineage, ManualClock, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{
    CapitalDownlink, CapitalGrantTopic, DEFAULT_GRANT_MEMORY, DownlinkBatch, DownlinkConfig,
    HaltTopic, PolicyBatch, PolicyDownlink, PolicyPayloadTopic, ScriptedFrames,
};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_transport::{
    Admission, ClientLimits, MeshConfig, MeshEndpoint, MeshInbox, MeshMessage, Method,
    RecordingSleeper, RetryPolicy, Sleeper,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const OTHER_KEY: &[u8] = b"a-key-this-cell-has-never-held";
const CELL: &str = "london-1";
const REGION: &str = "eu-west";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

// --- a loopback server that serves a real mesh endpoint ---------------------

/// A mesh endpoint on a real port, in its own thread — the same harness
/// `tests/mesh.rs` uses, so "the mesh" below is the wire and not an object
/// standing in for it.
struct MeshServer {
    address: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MeshServer {
    fn spawn(endpoint: MeshEndpoint) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| qip_core::Error::io(error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| qip_core::Error::io(error.to_string()))?
            .to_string();
        listener
            .set_nonblocking(true)
            .map_err(|error| qip_core::Error::io(error.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve_one(stream, &endpoint);
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

impl Drop for MeshServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_one(mut stream: TcpStream, endpoint: &MeshEndpoint) {
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
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
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

// --- fixtures ---------------------------------------------------------------

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
            read_timeout: std::time::Duration::from_millis(500),
            connect_timeout: std::time::Duration::from_millis(500),
            ..ClientLimits::default()
        })
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(t(0)))
}

fn sleeper() -> Arc<dyn Sleeper> {
    Arc::new(RecordingSleeper::new())
}

/// The capital downlink exactly as `qip-edge-node` builds it: over the mesh.
fn capital_over_mesh(peer: &str, name: &str) -> Result<CapitalDownlink> {
    CapitalDownlink::connect(
        DownlinkConfig::new(CELL, mesh_config(name, peer)),
        KEY,
        clock(),
        sleeper(),
    )
}

/// The policy downlink exactly as `qip-edge-node` builds it: over the mesh.
fn policy_over_mesh(peer: &str, name: &str) -> Result<PolicyDownlink> {
    PolicyDownlink::connect(
        DownlinkConfig::new(CELL, mesh_config(name, peer)),
        KEY,
        clock(),
        sleeper(),
    )
}

/// A scripted source holding `frames`, in order — the shape an adapter over
/// the fabric's control records hands the cell.
fn scripted(frames: &[AnyEvent]) -> Result<ScriptedFrames> {
    let mut source = ScriptedFrames::new(64)?;
    for frame in frames {
        source.push(frame.clone())?;
    }
    Ok(source)
}

/// Put frames on the mesh inbox as they would sit there after a publish,
/// asserting each was taken — a frame the inbox absorbed as a duplicate would
/// make the mesh side of every comparison below quietly shorter.
fn put_on_mesh(inbox: &MeshInbox, frames: &[AnyEvent]) {
    for frame in frames {
        let admission = inbox.accept(&MeshMessage {
            key: frame.event_id.as_str().to_string(),
            attempt: 1,
            frame: frame.clone(),
        });
        assert!(
            matches!(admission, Admission::Accepted(_)),
            "the inbox did not take {}: {admission:?}",
            frame.event_id
        );
    }
}

/// An envelope signed the way the central allocator signs one.
fn grant(gross: Decimal, key: &[u8]) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("mean-reversion-1"),
            CELL,
            gross,
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(7_200),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&sign_payload(key, &unsigned.signing_payload()))
}

/// The centre's side of each frame kind, written here rather than imported
/// from `qip-mesh` — that crate is a service and this is an edge library, so
/// the two ends share the vocabulary and the topic constants, not a type.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantBody(CapitalEnvelope);

impl EventBody for GrantBody {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct PolicyBody(PolicyPayload);

impl EventBody for PolicyBody {
    const TOPIC: Topic = PolicyPayloadTopic::TOPIC;
    const SCHEMA_VERSION: u32 = PolicyPayloadTopic::SCHEMA_VERSION;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct HaltBody(HaltCommand);

impl EventBody for HaltBody {
    const TOPIC: Topic = HaltTopic::TOPIC;
    const SCHEMA_VERSION: u32 = HaltTopic::SCHEMA_VERSION;
}

fn frame<B: EventBody>(body: B, event_id: &str, at: Timestamp) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-edge-tests",
        ),
        body,
    )
    .erase()
}

/// A frame whose payload was edited after its hash was taken.
fn tampered(mut frame: AnyEvent, field: &str, value: serde_json::Value) -> AnyEvent {
    frame.payload[field] = value;
    frame
}

/// A frame written by a newer schema than this cell understands, and
/// otherwise genuine: the hash covers the payload, not the version, so only
/// the schema ceiling stands between it and the cell.
fn from_newer_schema(mut frame: AnyEvent) -> AnyEvent {
    frame.schema_version += 1;
    frame
}

/// A cell with nothing deployed. What is under test is what reaches it, and
/// a halt needs no strategy to stop.
fn bare_cell() -> Result<Cell> {
    Cell::new(
        CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON")),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
}

/// A cell with the strategy the fixtures' grants fund deployed under a first
/// grant, so a renewal is something the cell can act on.
fn funded_cell() -> Result<Cell> {
    use qip_contracts::signal::SignalKind;
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(
        StrategyId::new("mean-reversion-1"),
        qip_core::ObjectId::from_string("obj-ACME"),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "never",
        SignalKind::Enter,
        Expr::Flag(false),
        Expr::Exact(dec!("1")),
        Expr::Statistic(0.5),
        100,
    ));
    let strategy = compiler.compile(&spec)?;
    let program = compiler.into_program();
    let mut cell = bare_cell()?;
    let first = VerifiedEnvelope::verify(grant(dec!("1000"), KEY)?, KEY, CELL, t(10))?;
    cell.deploy(strategy, program, first)?;
    Ok(cell)
}

// --- the comparisons --------------------------------------------------------

/// One refusal as a scripted route and the mesh each recorded it, by event id.
///
/// Matched on the frame rather than on position, and each of the mesh's
/// refusals looked up before any count is compared, so a divergence is
/// reported against the frame it happened on — the edited frame refused for
/// another reason, or not refused at all — rather than as two lists of
/// different lengths a reader then has to diff by eye.
fn assert_refusals_match<'a>(
    route: &str,
    mesh: impl Iterator<Item = (&'a str, &'a str)>,
    scripted: &[(&'a str, &'a str)],
) {
    let mut compared = 0;
    for (event_id, reason) in mesh {
        let theirs = scripted
            .iter()
            .find(|(id, _)| *id == event_id)
            .map(|(_, reason)| *reason);
        assert_eq!(
            theirs,
            Some(reason),
            "{route}: the scripted source refused {event_id} differently from the mesh"
        );
        compared += 1;
    }
    assert_eq!(
        scripted.len(),
        compared,
        "{route}: the scripted source refused frames the mesh did not: {scripted:?}"
    );
}

fn assert_capital_route_matches(route: &str, mesh: &DownlinkBatch, scripted: &DownlinkBatch) {
    let ours: Vec<(&str, &str)> = scripted
        .refused
        .iter()
        .map(|refusal| (refusal.event_id.as_str(), refusal.reason.as_str()))
        .collect();
    assert_refusals_match(
        route,
        mesh.refused
            .iter()
            .map(|refusal| (refusal.event_id.as_str(), refusal.reason.as_str())),
        &ours,
    );
    assert_eq!(
        scripted, mesh,
        "{route}: the scripted source delivered a different capital batch from the mesh"
    );
}

fn assert_policy_route_matches(route: &str, mesh: &PolicyBatch, scripted: &PolicyBatch) {
    let ours: Vec<(&str, &str)> = scripted
        .refused
        .iter()
        .map(|refusal| (refusal.event_id.as_str(), refusal.reason.as_str()))
        .collect();
    assert_refusals_match(
        route,
        mesh.refused
            .iter()
            .map(|refusal| (refusal.event_id.as_str(), refusal.reason.as_str())),
        &ours,
    );
    assert_eq!(
        scripted, mesh,
        "{route}: the scripted source delivered a different policy batch from the mesh"
    );
}

#[test]
fn frames_from_a_scripted_source_verify_and_refuse_exactly_as_mesh_frames_do() -> Result<()> {
    // Twelve frames: for each of the three things the centre sends down — a
    // grant, a policy payload, a halt — one genuine, one edited after it was
    // hashed, one signed with a key this cell has never held, and one written
    // by a schema newer than the cell understands. Each downlink ignores the
    // other's topics, as it does on the shared inbox `qip-edge-node` polls.
    let halting_policy = {
        let mut payload = PolicyPayload::unproduced(8, CELL, t(10));
        payload.halted = true;
        payload.signed(KEY)?
    };
    let frames = vec![
        frame(GrantBody(grant(dec!("2000"), KEY)?), "EVT-GRANT", t(11))?,
        // An inflated grant: the edit a forger wants most.
        tampered(
            frame(
                GrantBody(grant(dec!("1500"), KEY)?),
                "EVT-GRANT-TAMPERED",
                t(11),
            )?,
            "gross_limit",
            serde_json::to_value(dec!("9000000"))?,
        ),
        frame(
            GrantBody(grant(dec!("1600"), OTHER_KEY)?),
            "EVT-GRANT-WRONG-KEY",
            t(11),
        )?,
        from_newer_schema(frame(
            GrantBody(grant(dec!("1700"), KEY)?),
            "EVT-GRANT-NEWER",
            t(11),
        )?),
        frame(
            PolicyBody(PolicyPayload::unproduced(7, CELL, t(10)).signed(KEY)?),
            "EVT-POLICY",
            t(12),
        )?,
        // A halting payload edited to release: the edit that un-halts a cell.
        tampered(
            frame(PolicyBody(halting_policy), "EVT-POLICY-TAMPERED", t(12))?,
            "halted",
            serde_json::Value::Bool(false),
        ),
        frame(
            PolicyBody(PolicyPayload::unproduced(9, CELL, t(10)).signed(OTHER_KEY)?),
            "EVT-POLICY-WRONG-KEY",
            t(12),
        )?,
        from_newer_schema(frame(
            PolicyBody(PolicyPayload::unproduced(10, CELL, t(10)).signed(KEY)?),
            "EVT-POLICY-NEWER",
            t(12),
        )?),
        frame(
            HaltBody(HaltCommand::new(CELL, t(13), "operator halt").signed(KEY)?),
            "EVT-HALT",
            t(13),
        )?,
        tampered(
            frame(
                HaltBody(HaltCommand::new(CELL, t(13), "operator halt").signed(KEY)?),
                "EVT-HALT-TAMPERED",
                t(13),
            )?,
            "reason",
            serde_json::Value::String("nothing to see".to_string()),
        ),
        frame(
            HaltBody(HaltCommand::new(CELL, t(13), "operator halt").signed(OTHER_KEY)?),
            "EVT-HALT-WRONG-KEY",
            t(13),
        )?,
        from_newer_schema(frame(
            HaltBody(HaltCommand::new(CELL, t(13), "operator halt").signed(KEY)?),
            "EVT-HALT-NEWER",
            t(13),
        )?),
    ];

    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    put_on_mesh(&inbox, &frames);
    let now = t(60);

    // --- the mesh, and its premise ----------------------------------------
    let mut mesh_capital = capital_over_mesh(&server.url(), "downlink:london-1")?;
    let mesh_grants = mesh_capital.poll(now)?;
    let mut mesh_policy = policy_over_mesh(&server.url(), "policy:london-1")?;
    let mesh_control = mesh_policy.poll(now)?;

    // The comparison is only worth something if the mesh itself refused each
    // bad frame for the reason that frame was built to trip. Otherwise two
    // routes that both waved a forgery through would agree perfectly.
    assert_eq!(mesh_grants.verified.len(), 1, "{mesh_grants:?}");
    let expected_capital = [
        ("EVT-GRANT-TAMPERED", "no longer matches the hash"),
        (
            "EVT-GRANT-WRONG-KEY",
            "does not verify against this cell's key",
        ),
        ("EVT-GRANT-NEWER", "schema version 2"),
    ];
    assert_eq!(mesh_grants.refused.len(), expected_capital.len());
    for (refusal, (event_id, reason)) in mesh_grants.refused.iter().zip(expected_capital) {
        assert_eq!(refusal.event_id, event_id);
        assert!(
            refusal.reason.contains(reason),
            "the mesh refused {event_id} for the wrong reason: {}",
            refusal.reason
        );
    }
    assert_eq!(mesh_control.verified.len(), 1, "{mesh_control:?}");
    assert_eq!(mesh_control.halts.len(), 1, "{mesh_control:?}");
    let expected_control = [
        ("EVT-POLICY-TAMPERED", "no longer matches the hash"),
        (
            "EVT-POLICY-WRONG-KEY",
            "does not verify against this cell's key",
        ),
        ("EVT-POLICY-NEWER", "schema version 2"),
        ("EVT-HALT-TAMPERED", "no longer matches the hash"),
        (
            "EVT-HALT-WRONG-KEY",
            "does not verify against this cell's key",
        ),
        ("EVT-HALT-NEWER", "schema version 2"),
    ];
    assert_eq!(mesh_control.refused.len(), expected_control.len());
    for (refusal, (event_id, reason)) in mesh_control.refused.iter().zip(expected_control) {
        assert_eq!(refusal.event_id, event_id);
        assert!(
            refusal.reason.contains(reason),
            "the mesh refused {event_id} for the wrong reason: {}",
            refusal.reason
        );
    }

    // --- a downlink whose only source is scripted --------------------------
    let mut built_on_script = CapitalDownlink::from_source(
        CELL,
        KEY,
        DEFAULT_GRANT_MEMORY,
        Box::new(scripted(&frames)?),
    )?;
    assert_capital_route_matches("from_source", &mesh_grants, &built_on_script.poll(now)?);
    assert_eq!(built_on_script.stats(), mesh_capital.stats());

    let mut policy_on_script =
        PolicyDownlink::from_source(CELL, KEY, Box::new(scripted(&frames)?))?;
    assert_policy_route_matches("from_source", &mesh_control, &policy_on_script.poll(now)?);
    assert_eq!(policy_on_script.stats(), mesh_policy.stats());

    // --- a mesh downlink taking a scripted source alongside its own ---------
    // Fresh downlinks, so no grant memory from the polls above stands between
    // a frame and its checks.
    let mut alongside = capital_over_mesh(&server.url(), "downlink:alongside")?;
    let mut script = scripted(&frames)?;
    assert_capital_route_matches(
        "poll_from",
        &mesh_grants,
        &alongside.poll_from(&mut script, now)?,
    );
    assert_eq!(alongside.stats(), mesh_capital.stats());

    let mut policy_alongside = policy_over_mesh(&server.url(), "policy:alongside")?;
    let mut script = scripted(&frames)?;
    assert_policy_route_matches(
        "poll_from",
        &mesh_control,
        &policy_alongside.poll_from(&mut script, now)?,
    );
    assert_eq!(policy_alongside.stats(), mesh_policy.stats());
    Ok(())
}

#[test]
fn a_grant_seen_twice_through_either_source_is_applied_once() -> Result<()> {
    // A centre moving control from the mesh to the fabric publishes on both
    // for a while, and a cell listening on both hears every grant twice under
    // two event ids. The memory that absorbs that is keyed on the grant's
    // signature; if each wire had its own, the second copy would verify
    // afresh and be applied again, and nothing in the two frames would say
    // they were one grant.
    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    let mut link = capital_over_mesh(&server.url(), "downlink:london-1")?;
    let mut cell = funded_cell()?;
    let mut applied = Vec::new();

    let first = grant(dec!("2000"), KEY)?;
    let second = grant(dec!("3000"), KEY)?;
    assert_ne!(
        first.signature(),
        second.signature(),
        "the two grants are one grant, so this test could not tell a duplicate from a renewal"
    );

    // Mesh first, then the fabric.
    put_on_mesh(
        &inbox,
        &[frame(GrantBody(first.clone()), "EVT-FIRST-MESH", t(30))?],
    );
    let over_mesh = link.poll(t(60))?;
    assert_eq!(
        over_mesh.verified.len(),
        1,
        "the grant never arrived over the mesh, so there is nothing for the fabric to repeat: \
         {over_mesh:?}"
    );
    applied.extend(over_mesh.verified);

    // The fabric repeats the first grant and carries the second before the
    // mesh does.
    let mut fabric = scripted(&[
        frame(GrantBody(first.clone()), "EVT-FIRST-FABRIC", t(40))?,
        frame(GrantBody(second.clone()), "EVT-SECOND-FABRIC", t(41))?,
    ])?;
    let over_fabric = link.poll_from(&mut fabric, t(60))?;
    assert_eq!(
        over_fabric.duplicates.len(),
        1,
        "the grant the mesh delivered was not recognised when the fabric delivered it: \
         {over_fabric:?}"
    );
    assert_eq!(
        over_fabric.verified.len(),
        1,
        "the fabric's copy of a grant the mesh already delivered was verified again: \
         {over_fabric:?}"
    );
    assert_eq!(over_fabric.verified[0].gross_limit(), dec!("3000"));
    applied.extend(over_fabric.verified);

    // Fabric first, then the mesh.
    put_on_mesh(
        &inbox,
        &[frame(GrantBody(second.clone()), "EVT-SECOND-MESH", t(70))?],
    );
    let late_mesh = link.poll(t(90))?;
    assert_eq!(
        late_mesh.duplicates.len(),
        1,
        "the grant the fabric delivered was not recognised when the mesh delivered it: \
         {late_mesh:?}"
    );
    assert!(
        late_mesh.verified.is_empty(),
        "the mesh's copy of a grant the fabric already delivered was verified again: \
         {late_mesh:?}"
    );
    applied.extend(late_mesh.verified);

    assert!(link.has_applied(&first) && link.has_applied(&second));
    assert_eq!(link.stats().verified, 2);
    assert_eq!(link.stats().duplicates, 2);

    // And the effect on the cell: two grants, each heard twice, two renewals.
    for envelope in applied {
        cell.renew_capital(envelope, t(90))?;
    }
    assert_eq!(
        cell.journal().tally().get("capital_renewed"),
        Some(&2),
        "four deliveries of two grants did not produce exactly two renewals"
    );
    Ok(())
}

#[test]
fn a_halt_frame_from_a_scripted_source_engages_the_halt_the_mesh_would() -> Result<()> {
    // The halt is the frame whose loss matters most, and it rides its own
    // topic so it can arrive when the payload path is wedged. A source that
    // handed it to the payload arm would have it refused as a malformed
    // payload — counted, reasoned, and stopping nothing.
    let halt = frame(
        HaltBody(HaltCommand::new(CELL, t(20), "operator halt").signed(KEY)?),
        "EVT-HALT",
        t(20),
    )?;
    let now = t(60);

    let inbox = MeshInbox::new("london-1-inbox", 64, 256)?;
    let server = MeshServer::spawn(MeshEndpoint::new(inbox.clone()))?;
    put_on_mesh(&inbox, std::slice::from_ref(&halt));

    let mut over_mesh = policy_over_mesh(&server.url(), "policy:london-1")?;
    let mesh_batch = over_mesh.poll(now)?;
    let mut built_on_script =
        PolicyDownlink::from_source(CELL, KEY, Box::new(scripted(std::slice::from_ref(&halt))?))?;
    let source_batch = built_on_script.poll(now)?;
    let mut alongside = policy_over_mesh(&server.url(), "policy:alongside")?;
    let alongside_batch = alongside.poll_from(&mut scripted(&[halt])?, now)?;

    let mut halted = Vec::new();
    for (route, batch) in [
        ("mesh", mesh_batch),
        ("from_source", source_batch),
        ("poll_from", alongside_batch),
    ] {
        assert_eq!(
            batch.halts.len(),
            1,
            "{route}: the halt was not delivered as a halt: {batch:?}"
        );
        assert!(
            batch.refused.is_empty() && batch.verified.is_empty(),
            "{route}: the halt was read as something else: {batch:?}"
        );
        let mut cell = bare_cell()?;
        assert!(
            !cell.is_halted(),
            "{route}: the cell was halted before any halt arrived"
        );
        for verified in batch.halts {
            cell.apply_halt(verified, now);
        }
        assert!(
            cell.is_halted(),
            "{route}: a verified halt did not stop the cell"
        );
        halted.push((route, cell));
    }

    // Not merely halted: halted identically. The journal is hash-chained, so
    // equal entries mean the same decision, at the same instant, for the same
    // reason — the record an examiner reads is indifferent to the wire.
    let (_, mesh_cell) = &halted[0];
    assert_eq!(mesh_cell.journal().tally().get("halt_changed"), Some(&1));
    for (route, cell) in &halted[1..] {
        assert_eq!(
            cell.journal().entries(),
            mesh_cell.journal().entries(),
            "{route}: the scripted halt was journalled differently from the mesh's"
        );
    }
    Ok(())
}
