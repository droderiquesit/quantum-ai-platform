//! `qip event-fabric grant`: the labelled fixture that signs slice control.
//!
//! ADR 0100's "What would make this wrong" names the fixture grant path
//! surviving and becoming how grants are issued. Each test here pins one of
//! the fences that keep it a fixture — loopback only, labelled, signed as the
//! fixture — and one pins that what it signs is something the cell actually
//! accepts, through the cell's own downlinks rather than a re-implementation
//! of their check. The last runs the built binary, because the property it
//! guards — `main.rs` routing the whole family — is invisible to a test that
//! calls the library.
//!
//! No test here reads the process environment or opens a socket. The key and
//! token are written to scratch files at run time and reached through a
//! lookup the test supplies, and the broker is a recording transport.

use qip_cli::event_fabric::grant::{self, FIXTURE_KEY_LABEL, KEY_VARIABLE, TOKEN_VARIABLE};
use qip_cli::event_fabric::{Environment, SUBCOMMANDS};
use qip_contracts::capital::CapitalEnvelope;
use qip_core::error::{Error, Result};
use qip_core::hash::{from_hex, sha256_hex};
use qip_core::time::{Duration, ManualClock, Timestamp};
use qip_edge::mesh::{CapitalDownlink, PolicyDownlink, ScriptedFrames};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, PayloadCodec};
use qip_events::{AnyEvent, Topic};
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::protocol::{
    Metadata, ProduceAck, ProducerInitResponse, Request, Response,
};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::RecordingSleeper;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The signatory, written out rather than taken from the crate's constant:
/// a test that compared against the constant would pass whatever the
/// constant was changed to.
const SIGNATORY: &str = "fixture:qip-event-fabric-grant";
const CELL: &str = "london-1";
const STREAM: &str = "control.local";
const LOOPBACK: &str = "127.0.0.1:7100";

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn committed_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/event-fabric-grant.fixture.json")
}

fn slice_plan() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../qip-edge-node/fixtures/slice-strategy-plan.json")
}

/// A fresh directory for one test, so tests running in parallel never read
/// each other's files.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-cli-event-fabric-grant-{}-{test}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// One published batch, as the broker received it.
#[derive(Clone, Debug)]
struct Published {
    stream: String,
    partition: u32,
    batch_hex: String,
}

/// What the recording broker saw: every connection and every batch.
#[derive(Clone, Debug, Default)]
struct Seen {
    connects: Arc<Mutex<Vec<SocketAddr>>>,
    published: Arc<Mutex<Vec<Published>>>,
    lookups: Arc<Mutex<Vec<String>>>,
    /// The header value each connection was opened with.
    identities: Arc<Mutex<Vec<String>>>,
}

impl Seen {
    fn connects(&self) -> Vec<SocketAddr> {
        self.connects.lock().expect("lock").clone()
    }

    fn published(&self) -> Vec<Published> {
        self.published.lock().expect("lock").clone()
    }

    fn lookups(&self) -> Vec<String> {
        self.lookups.lock().expect("lock").clone()
    }

    fn identities(&self) -> Vec<String> {
        self.identities.lock().expect("lock").clone()
    }
}

/// A broker that accepts every produce and answers as a healthy single node:
/// the batch is archived as soon as it is written, so a quorum producer is
/// satisfied by the acknowledgement itself.
#[derive(Debug)]
struct RecordingBroker {
    published: Arc<Mutex<Vec<Published>>>,
    written: u64,
}

impl FabricTransport for RecordingBroker {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        match request {
            Request::ProducerInit(_) => Ok(Response::ProducerInit(ProducerInitResponse {
                producer_epoch: 1,
            })),
            Request::Produce(produce) => {
                let base = self.written;
                self.written += 1;
                self.published.lock().expect("lock").push(Published {
                    stream: produce.stream().to_string(),
                    partition: produce.partition(),
                    batch_hex: produce.batch().to_string(),
                });
                Ok(Response::Produce(ProduceAck::new(
                    produce.stream(),
                    produce.partition(),
                    base,
                    self.written,
                    self.written,
                )?))
            }
            Request::Metadata(metadata) => Ok(Response::Metadata(Metadata::new(
                metadata.stream,
                metadata.partition,
                1,
                self.written,
                self.written,
            )?)),
            other => Err(Error::invalid(format!(
                "the recording broker does not answer {:?}",
                other.route()
            ))),
        }
    }
}

/// The key material a test writes: labelled, and derived from the test's
/// name so no literal key sits in the source.
fn key_material(test: &str) -> String {
    format!(
        "{FIXTURE_KEY_LABEL}{}",
        sha256_hex(format!("key-{test}").as_bytes())
    )
}

/// The token a test writes, derived from its name for the same reason.
fn token(test: &str) -> String {
    sha256_hex(format!("token-{test}").as_bytes())
}

/// A complete environment for `test`: a key file and a token file in its
/// own scratch directory, reached through the `_FILE` variables only, a
/// manual clock, and the recording broker. Returns what the broker saw and
/// the key bytes a cell would be configured with.
fn wired(test: &str) -> (Environment, Seen, Vec<u8>) {
    let dir = scratch(test);
    let key_path = dir.join("fixture.key");
    // A trailing newline, as `echo` writes one: the secret resolver trims it,
    // and the cell is configured from the trimmed value.
    std::fs::write(&key_path, format!("{}\n", key_material(test))).expect("the key file");
    let token_path = dir.join("release-controller.token");
    std::fs::write(&token_path, token(test)).expect("the token file");

    let mut variables = BTreeMap::new();
    variables.insert(
        format!("{KEY_VARIABLE}_FILE"),
        key_path.display().to_string(),
    );
    variables.insert(
        format!("{TOKEN_VARIABLE}_FILE"),
        token_path.display().to_string(),
    );

    let seen = Seen::default();
    let lookups = Arc::clone(&seen.lookups);
    let connects = Arc::clone(&seen.connects);
    let identities = Arc::clone(&seen.identities);
    let published = Arc::clone(&seen.published);
    let environment = Environment::new(
        Box::new(move |name| {
            lookups.lock().expect("lock").push(name.to_string());
            variables.get(name).cloned()
        }),
        Arc::new(ManualClock::new(now())),
        Arc::new(RecordingSleeper::new()),
        Box::new(move |peer, identity: BearerToken| {
            connects.lock().expect("lock").push(peer);
            // Kept only to compare against the token file; never printed.
            identities
                .lock()
                .expect("lock")
                .push(identity.header_value());
            Box::new(RecordingBroker {
                published: Arc::clone(&published),
                written: 0,
            })
        }),
    );
    (environment, seen, key_material(test).into_bytes())
}

fn arguments(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

fn fixture_arguments(fixture: &Path, peer: &str) -> Vec<String> {
    arguments(&["--fixture", &fixture.display().to_string(), "--peer", peer])
}

/// Every record the broker received, decoded back to the frames a consumer
/// of the control stream would read.
fn published_frames(seen: &Seen) -> Vec<AnyEvent> {
    let mut frames = Vec::new();
    for published in seen.published() {
        let bytes = from_hex(&published.batch_hex).expect("the batch is hex");
        let DecodeOutcome::Complete(batch) = Batch::decode(&bytes).expect("the batch decodes")
        else {
            panic!("the published batch is torn");
        };
        for record in &batch.records {
            frames.push(
                record
                    .decode_payload(PayloadCodec::CanonicalJson)
                    .expect("a record decodes to a frame"),
            );
        }
    }
    frames
}

fn scripted(frames: &[AnyEvent]) -> ScriptedFrames {
    let mut source = ScriptedFrames::new(16).expect("a scripted source");
    for frame in frames {
        source.push(frame.clone()).expect("room for the frame");
    }
    source
}

// --- the grant is one the cell accepts ------------------------------------

/// What the command publishes must pass the cell's own checks: the capital
/// downlink and the policy downlink, built from a `FrameSource` exactly as
/// the node's control consumer builds them, verifying against the key the
/// cell is configured with. A grant that only verified against a copy of
/// the check written in this test would prove the copy.
#[test]
fn a_fixture_grant_verifies_through_the_cells_own_verify_path() {
    let (environment, seen, key) = wired("verify");
    let outcome = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("the committed fixture is granted on loopback");
    assert_eq!(outcome.code, 0);

    // Premise: exactly one batch reached the P0 stream the fixture names,
    // carrying a grant and a payload.
    let published = seen.published();
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(published[0].stream, STREAM);
    assert_eq!(published[0].partition, 0);
    // Published as the release controller, with the token from its file.
    assert_eq!(
        seen.identities(),
        vec![format!("Bearer {}", token("verify"))]
    );
    let frames = published_frames(&seen);
    let topics: Vec<Topic> = frames.iter().map(|frame| frame.topic).collect();
    assert_eq!(
        topics,
        vec![Topic::RiskApproved, Topic::PolicyDistributed],
        "a grant then the payload naming it"
    );

    let at = now().saturating_add(Duration::from_secs(1));
    let mut capital = CapitalDownlink::from_source(CELL, &key, 16, Box::new(scripted(&frames)))
        .expect("a capital downlink");
    let granted = capital.poll(at).expect("the capital downlink polls");
    assert!(
        granted.refused.is_empty(),
        "the cell refused the fixture grant: {:?}",
        granted.refused
    );
    assert_eq!(granted.verified.len(), 1, "{granted:?}");
    let grant = &granted.verified[0];
    assert_eq!(grant.cell(), CELL);
    assert_eq!(grant.strategy().as_str(), "microprice-guard");

    let mut policy = PolicyDownlink::from_source(CELL, &key, Box::new(scripted(&frames)))
        .expect("a policy downlink");
    let applied = policy.poll(at).expect("the policy downlink polls");
    assert!(
        applied.refused.is_empty(),
        "the cell refused the fixture payload: {:?}",
        applied.refused
    );
    assert_eq!(applied.verified.len(), 1, "{applied:?}");
    let payload = applied.verified[0].payload();
    // The payload names the slice plan by the digest of its bytes on disk,
    // and names the grant the cell just verified, so the share it funds is
    // summed over a grant the cell holds.
    let plan_bytes = std::fs::read(slice_plan()).expect("the slice plan");
    let plan = payload
        .compiled_plan
        .value()
        .expect("the plan slot is produced");
    assert_eq!(plan.digest, sha256_hex(&plan_bytes));
    assert_eq!(plan.strategies, 1);
    let manifest = payload
        .capital_grants
        .value()
        .expect("the grants slot is produced");
    assert_eq!(manifest.live_grants, vec![grant.signature().to_string()]);
}

// --- loopback only ----------------------------------------------------------

/// A peer that is not loopback is refused before anything is read or sent.
/// The fixture reaching a regional fabric would make it a grant path, which
/// is the failure ADR 0100 names; and the key is not even looked up, so a
/// run that could not have gone anywhere safe never touches key material.
#[test]
fn the_grant_command_refuses_a_peer_that_is_not_loopback() {
    // Premise: the same fixture and environment are granted on loopback, over
    // both address families, so a refusal below is the peer's doing.
    for peer in [LOOPBACK, "[::1]:7100"] {
        let (environment, seen, _) = wired("loopback-premise");
        grant::run(&fixture_arguments(&committed_fixture(), peer), &environment)
            .unwrap_or_else(|error| panic!("{peer} was refused: {}", error.message()));
        assert_eq!(seen.published().len(), 1, "{peer} published nothing");
        assert_eq!(
            seen.connects(),
            vec![peer.parse::<SocketAddr>().expect("an address")],
            "the transport was not opened to the checked address"
        );
    }

    let outside = [
        "10.0.0.7:7100",
        "0.0.0.0:7100",
        "203.0.113.9:7100",
        "[2001:db8::1]:7100",
        "[::]:7100",
    ];
    for peer in outside {
        let (environment, seen, _) = wired("loopback-refused");
        let error = grant::run(&fixture_arguments(&committed_fixture(), peer), &environment)
            .expect_err(&format!("{peer} was accepted as a peer"));
        assert!(
            error.message().contains("which is not loopback"),
            "{peer} was refused for the wrong reason: {}",
            error.message()
        );
        assert!(seen.connects().is_empty(), "{peer} was connected to");
        assert!(seen.published().is_empty(), "{peer} was published to");
        let key_file = format!("{KEY_VARIABLE}_FILE");
        assert!(
            !seen.lookups().contains(&key_file),
            "the key was looked up for a run refusing {peer}"
        );
    }
}

// --- signed as the fixture --------------------------------------------------

/// Every envelope the command signs names the fixture as its signatory, so
/// a journal holding one says where it came from. "Every" is both frames on
/// the wire — the grant and the payload — by their lineage, and the grant by
/// its approver, which is the field the cell reports a grant under.
#[test]
fn every_envelope_the_fixture_command_signs_names_the_fixture_as_its_signatory() {
    let (environment, seen, _) = wired("signatory");
    grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("the committed fixture is granted on loopback");

    let frames = published_frames(&seen);
    // Premise: both kinds the command signs are on the wire, so the loop
    // below cannot pass by inspecting nothing.
    let topics: Vec<Topic> = frames.iter().map(|frame| frame.topic).collect();
    assert!(topics.contains(&Topic::RiskApproved), "{topics:?}");
    assert!(topics.contains(&Topic::PolicyDistributed), "{topics:?}");

    for frame in &frames {
        assert_eq!(
            frame.lineage.producer, SIGNATORY,
            "the {:?} frame names {:?} as its signatory",
            frame.topic, frame.lineage.producer
        );
    }
    let grant_frame = frames
        .iter()
        .find(|frame| frame.topic == Topic::RiskApproved)
        .expect("the grant frame");
    let grant: CapitalEnvelope =
        serde_json::from_value(grant_frame.payload.clone()).expect("the grant decodes");
    assert_eq!(grant.approver(), SIGNATORY);
}

// --- labelled -----------------------------------------------------------------

/// No `--fixture`, no grant; and a file without the fixture's label is not
/// signed either. Without the first, the command has a default input and a
/// shell history that shows no fixture at all; without the second, any JSON
/// file with the right fields becomes a grant.
#[test]
fn the_grant_command_refuses_to_run_without_the_fixture_label() {
    // Premise: the labelled fixture is granted in this environment.
    let (environment, seen, _) = wired("label-premise");
    grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("the labelled fixture is granted");
    assert_eq!(seen.published().len(), 1);

    // No --fixture at all.
    let (environment, seen, _) = wired("label-absent");
    let error = grant::run(&arguments(&["--peer", LOOPBACK]), &environment)
        .expect_err("the command ran without --fixture");
    assert!(
        error
            .message()
            .contains("will not run without --fixture <path>"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());
    assert!(seen.published().is_empty());

    // A file carrying the right fields but not the label, and one labelled
    // as something else. Written beside the committed fixture's own plan
    // path, so the only difference is the label.
    let committed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(committed_fixture()).expect("the committed fixture"),
    )
    .expect("the committed fixture is JSON");
    let dir = scratch("label-files");
    for (name, label) in [
        ("unlabelled", None),
        ("mislabelled", Some("fixture:something-else")),
    ] {
        let mut fixture = committed.clone();
        let fields = fixture.as_object_mut().expect("an object");
        fields.insert(
            "plan".to_string(),
            serde_json::Value::String(slice_plan().display().to_string()),
        );
        match label {
            None => {
                fields.remove("label");
            }
            Some(label) => {
                fields.insert(
                    "label".to_string(),
                    serde_json::Value::String(label.to_string()),
                );
            }
        }
        let path = dir.join(format!("{name}.json"));
        std::fs::write(&path, serde_json::to_vec(&fixture).expect("serialises"))
            .expect("the fixture file");

        let (environment, seen, _) = wired("label-files-run");
        let error = grant::run(&fixture_arguments(&path, LOOPBACK), &environment)
            .expect_err(&format!("the {name} fixture was granted"));
        assert!(
            error
                .message()
                .contains("Only a file labelled as the grant fixture is signed"),
            "the {name} fixture was refused for the wrong reason: {}",
            error.message()
        );
        assert!(
            seen.published().is_empty(),
            "the {name} fixture was published"
        );
    }
}

// --- the family is routed whole ----------------------------------------------

/// `main.rs` routes every `event-fabric` subcommand to the family, so a
/// subcommand the library answers is one the binary answers, and an unknown
/// one is refused by the family naming its list rather than by main's
/// generic "unknown command". Runs the built binary: routing in `main.rs` is
/// exactly what a library-level test cannot see.
#[test]
fn every_event_fabric_subcommand_reaches_the_family_dispatcher() {
    let qip = env!("CARGO_BIN_EXE_qip");
    let run = |parts: &[&str]| {
        let output = std::process::Command::new(qip)
            .args(parts)
            .env_clear()
            .output()
            .expect("the binary runs");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    };

    // Premise: main's own refusal of an unknown command is recognisable, so
    // the assertion that the family answered instead can fail.
    let (code, stderr) = run(&["no-such-command"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("qip: unknown command: no-such-command"),
        "{stderr}"
    );

    assert!(!SUBCOMMANDS.is_empty());
    for subcommand in SUBCOMMANDS {
        // Every subcommand, given no arguments, is refused by its own
        // argument handling — which it can only reach through the family.
        let (code, stderr) = run(&["event-fabric", subcommand]);
        assert_eq!(code, Some(1), "{subcommand}: {stderr}");
        assert!(
            !stderr.contains("unknown command"),
            "`qip event-fabric {subcommand}` was refused by main, not the family: {stderr}"
        );
        assert!(
            !stderr.contains("unknown event-fabric subcommand"),
            "`qip event-fabric {subcommand}` is listed but not dispatched: {stderr}"
        );
    }

    let (code, stderr) = run(&["event-fabric", "frobnicate"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "unknown event-fabric subcommand \"frobnicate\"; the family is: {}",
            SUBCOMMANDS.join(", ")
        )),
        "`qip event-fabric frobnicate` was not refused by the family: {stderr}"
    );
    assert!(!stderr.contains("unknown command"), "{stderr}");
}
