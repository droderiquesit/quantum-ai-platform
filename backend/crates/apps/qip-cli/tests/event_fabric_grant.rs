//! `qip event-fabric grant`: the labelled fixture that signs slice control.
//!
//! ADR 0100's "What would make this wrong" names the fixture grant path
//! surviving and becoming how grants are issued. Each test here pins one of
//! the fences that keep it a fixture — loopback only, labelled, signed as the
//! fixture — and one pins that what it signs is something the cell actually
//! accepts, through the cell's own downlinks rather than a re-implementation
//! of their check. The routing test runs the built binary, because the
//! property it guards — `main.rs` routing the whole family — is invisible to
//! a test that calls the library.
//!
//! Since ADR 0110 and 0112 the command signs under the `envelope` and
//! `policy` class keys and reaches its broker over mutual TLS. The tests
//! below pin both: the cell verifying against a trust bundle (and refusing
//! an HMAC by construction), the shared key confined to a `local`
//! deployment, the fixture label on every asymmetric key, the signer proven
//! before it signs, the link chosen from the deployment, and — once — the
//! whole run against a real mutual-TLS listener.
//!
//! Every key, certificate and token is generated at run time and written to
//! scratch files reached through a lookup the test supplies; no test reads
//! the process environment. Only the last test opens a socket, a TLS
//! listener on loopback it owns; the others publish to a recording broker.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "../../../libs/qip-transport/tests/support/certs.rs"]
mod certs;

use qip_api::signing::{ARTEFACT_SIGNER_VARIABLE, KMS_KEY_VERSIONS_VARIABLE};
use qip_api::signing::{LOCAL_SIGNING_KEYS_VARIABLE, TRUST_BUNDLE_VARIABLE};
use qip_cli::event_fabric::grant::{self, FIXTURE_KEY_LABEL, KEY_VARIABLE, TOKEN_VARIABLE};
use qip_cli::event_fabric::{Environment, Lookup, SUBCOMMANDS};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::trust::{ArtefactClass, ArtefactSignature, TrustBundle, TrustedKey};
use qip_core::deployment::DEPLOYMENT_VARIABLE;
use qip_core::error::{Error, Result};
use qip_core::hash::{from_hex, sha256_hex, to_hex};
use qip_core::time::{Duration, ManualClock, Timestamp};
use qip_edge::TrustRoot;
use qip_edge::envelope::VerifiedEnvelope;
use qip_edge::mesh::{CapitalDownlink, PolicyDownlink, ScriptedFrames};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, PayloadCodec};
use qip_events::{AnyEvent, Topic};
use qip_transport::event_fabric::auth::{self, BearerToken};
use qip_transport::event_fabric::protocol::{
    Metadata, ProduceAck, ProducerInitResponse, Request, Response, Route, decode_request,
    encode_response,
};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::RecordingSleeper;
use qip_transport::server::{
    Handler, Request as ServerRequest, Response as ServerResponse, Server, ServerLimits,
};
use qip_transport::tls::{
    LinkSecurity, PeerNames, TLS_CERTIFICATE_CHAIN_VARIABLE, TLS_MODE_VARIABLE,
    TLS_PRIVATE_KEY_VARIABLE, TLS_TRUST_ROOTS_VARIABLE, TlsServer, WorkloadIdentity,
};
use qip_transport::trust::{BundleVerifier, LocalSigner, generate_pkcs8};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// The signatory, written out rather than taken from the crate's constant:
/// a test that compared against the constant would pass whatever the
/// constant was changed to.
const SIGNATORY: &str = "fixture:qip-event-fabric-grant";
const CELL: &str = "london-1";
const STREAM: &str = "control.local";
const LOOPBACK: &str = "127.0.0.1:7100";
/// The fixture key versions, written out for the same reason.
const ENVELOPE_VERSION: &str = "qip-fixture-envelope/1";
const POLICY_VERSION: &str = "qip-fixture-policy/1";

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
    /// Whether each connection was opened over mutual TLS.
    links: Arc<Mutex<Vec<bool>>>,
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

    fn links(&self) -> Vec<bool> {
        self.links.lock().expect("lock").clone()
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

impl RecordingBroker {
    fn answer(&mut self, request: Request) -> Result<Response> {
        match request {
            Request::ProducerInit(_) => Ok(Response::ProducerInit(ProducerInitResponse {
                producer_epoch: 1,
                next_sequence: 0,
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

impl FabricTransport for RecordingBroker {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        self.answer(request)
    }
}

/// The shared key material a test writes: labelled, and derived from the
/// test's name so no literal key sits in the source.
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

/// One class key a test signs under: its version name and PKCS#8 document.
struct ClassKey {
    version: String,
    pkcs8: Vec<u8>,
}

impl ClassKey {
    fn generate(version: &str) -> Self {
        Self {
            version: version.to_string(),
            pkcs8: generate_pkcs8().expect("a fresh key"),
        }
    }

    fn public_key_hex(&self, class: ArtefactClass) -> String {
        LocalSigner::from_pkcs8(class, &self.version, &self.pkcs8)
            .expect("the key just generated loads")
            .public_key_hex()
    }

    fn trusted(&self, class: ArtefactClass) -> TrustedKey {
        TrustedKey {
            class,
            key_version: self.version.clone(),
            public_key: self.public_key_hex(class),
            not_after: Timestamp::from_secs(4_000_000_000),
        }
    }
}

/// Everything one run of the command is given, before it is turned into an
/// [`Environment`]: the variables, the files they name, and the trust root
/// the cell would hold.
struct Wiring {
    dir: PathBuf,
    test: String,
    variables: BTreeMap<String, String>,
    bundle: TrustBundle,
}

impl Wiring {
    /// A `local` deployment over a plaintext link — the workstation run the
    /// command exists for — signing under fixture-labelled local keys that a
    /// trust bundle lists.
    fn new(test: &str) -> Self {
        let dir = scratch(test);
        let envelope = ClassKey::generate(ENVELOPE_VERSION);
        let policy = ClassKey::generate(POLICY_VERSION);
        let bundle = TrustBundle::new(
            vec![
                envelope.trusted(ArtefactClass::Envelope),
                policy.trusted(ArtefactClass::Policy),
            ],
            BTreeSet::new(),
        )
        .expect("a bundle of generated keys");
        let mut wiring = Self {
            dir,
            test: test.to_string(),
            variables: BTreeMap::new(),
            bundle,
        };
        wiring.write_bundle(&wiring.bundle.clone());
        wiring.write_local_keys(&[
            ("envelope", &envelope.version, &envelope.pkcs8),
            ("policy", &policy.version, &policy.pkcs8),
        ]);
        let token_path = wiring.dir.join("release-controller.token");
        std::fs::write(&token_path, token(test)).expect("the token file");
        wiring.set(
            &format!("{TOKEN_VARIABLE}_FILE"),
            &token_path.display().to_string(),
        );
        wiring.set(DEPLOYMENT_VARIABLE, "local");
        wiring.set(TLS_MODE_VARIABLE, "plaintext");
        wiring.set(ARTEFACT_SIGNER_VARIABLE, "local");
        wiring
    }

    fn set(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    fn unset(&mut self, name: &str) {
        self.variables.remove(name);
    }

    fn write_bundle(&mut self, bundle: &TrustBundle) {
        let path = self.dir.join("trust-bundle.json");
        std::fs::write(&path, serde_json::to_vec(bundle).expect("serialises")).expect("bundle");
        self.set(TRUST_BUNDLE_VARIABLE, &path.display().to_string());
    }

    /// The API's local keys file shape, for `(class, version, pkcs8)`.
    fn write_local_keys(&mut self, keys: &[(&str, &str, &[u8])]) {
        let mut file = serde_json::Map::new();
        for (class, version, pkcs8) in keys {
            file.insert(
                (*class).to_string(),
                serde_json::json!({ "key_version": version, "pkcs8_hex": to_hex(pkcs8) }),
            );
        }
        let path = self.dir.join("local-signing-keys.json");
        std::fs::write(&path, serde_json::to_vec(&file).expect("serialises")).expect("keys");
        self.set(LOCAL_SIGNING_KEYS_VARIABLE, &path.display().to_string());
    }

    /// Sign with the shared HMAC fixture key instead: no asymmetric signer,
    /// and the key file the cell's shared-key root is configured from.
    /// Returns the bytes that root holds.
    fn use_shared_key(&mut self) -> Vec<u8> {
        self.unset(ARTEFACT_SIGNER_VARIABLE);
        self.unset(TRUST_BUNDLE_VARIABLE);
        self.unset(LOCAL_SIGNING_KEYS_VARIABLE);
        let key_path = self.dir.join("fixture.key");
        // A trailing newline, as `echo` writes one: the secret resolver trims
        // it, and the cell is configured from the trimmed value.
        std::fs::write(&key_path, format!("{}\n", key_material(&self.test))).expect("key file");
        self.set(
            &format!("{KEY_VARIABLE}_FILE"),
            &key_path.display().to_string(),
        );
        key_material(&self.test).into_bytes()
    }

    /// Make the link mutual TLS under `leaf`, a certificate `ca` issued.
    fn use_tls(&mut self, ca: &certs::Minted, leaf: &certs::Minted) {
        let chain = self.dir.join("client-chain.pem");
        let key = self.dir.join("client-key.pem");
        let roots = self.dir.join("roots.pem");
        std::fs::write(&chain, leaf.certificate_pem()).expect("chain");
        std::fs::write(&key, leaf.key_pem()).expect("key");
        std::fs::write(&roots, ca.certificate_pem()).expect("roots");
        self.unset(TLS_MODE_VARIABLE);
        self.set(TLS_CERTIFICATE_CHAIN_VARIABLE, &chain.display().to_string());
        self.set(TLS_PRIVATE_KEY_VARIABLE, &key.display().to_string());
        self.set(TLS_TRUST_ROOTS_VARIABLE, &roots.display().to_string());
    }

    /// The root a cell holding this run's trust bundle verifies with.
    fn root(&self) -> TrustRoot {
        TrustRoot::bundle(BundleVerifier::new(self.bundle.clone()))
    }

    fn lookup(&self, seen: &Seen) -> Lookup {
        let variables = self.variables.clone();
        let lookups = Arc::clone(&seen.lookups);
        Box::new(move |name| {
            lookups.lock().expect("lock").push(name.to_string());
            variables.get(name).cloned()
        })
    }

    /// The environment, publishing to the recording broker.
    fn environment(&self) -> (Environment, Seen) {
        let seen = Seen::default();
        let connects = Arc::clone(&seen.connects);
        let identities = Arc::clone(&seen.identities);
        let links = Arc::clone(&seen.links);
        let published = Arc::clone(&seen.published);
        let environment = Environment::new(
            self.lookup(&seen),
            Arc::new(ManualClock::new(now())),
            Arc::new(RecordingSleeper::new()),
            Box::new(|_, _| panic!("the grant opened a link without saying how it is secured")),
        )
        .with_link_connector(Box::new(
            move |link: &LinkSecurity, peer, identity: BearerToken| {
                connects.lock().expect("lock").push(peer);
                links.lock().expect("lock").push(link.is_mutual_tls());
                // Kept only to compare against the token file; never printed.
                identities
                    .lock()
                    .expect("lock")
                    .push(identity.header_value());
                let transport: Box<dyn FabricTransport + Send> = Box::new(RecordingBroker {
                    published: Arc::clone(&published),
                    written: 0,
                });
                Ok(transport)
            },
        ));
        (environment, seen)
    }
}

/// The default run: local, plaintext, fixture-labelled local keys. Returns
/// what the broker saw and the root the cell would verify with.
fn wired(test: &str) -> (Environment, Seen, TrustRoot) {
    let wiring = Wiring::new(test);
    let (environment, seen) = wiring.environment();
    (environment, seen, wiring.root())
}

fn arguments(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

fn fixture_arguments(fixture: &Path, peer: &str) -> Vec<String> {
    arguments(&["--fixture", &fixture.display().to_string(), "--peer", peer])
}

fn frames_of(batches: &[String]) -> Vec<AnyEvent> {
    let mut frames = Vec::new();
    for batch_hex in batches {
        let bytes = from_hex(batch_hex).expect("the batch is hex");
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

/// Every record the broker received, decoded back to the frames a consumer
/// of the control stream would read.
fn published_frames(seen: &Seen) -> Vec<AnyEvent> {
    let batches: Vec<String> = seen
        .published()
        .into_iter()
        .map(|published| published.batch_hex)
        .collect();
    frames_of(&batches)
}

fn scripted(frames: &[AnyEvent]) -> ScriptedFrames {
    let mut source = ScriptedFrames::new(16).expect("a scripted source");
    for frame in frames {
        source.push(frame.clone()).expect("room for the frame");
    }
    source
}

/// Run `frames` through the cell's capital and policy downlinks under
/// `root`, asserting both accept exactly one artefact, and return the grant.
fn accepted_by_the_cell(frames: &[AnyEvent], root: &TrustRoot) -> VerifiedEnvelope {
    let at = now().saturating_add(Duration::from_secs(1));
    let mut capital =
        CapitalDownlink::from_source_trusted(CELL, root.clone(), 16, Box::new(scripted(frames)))
            .expect("a capital downlink");
    let granted = capital.poll(at).expect("the capital downlink polls");
    assert!(
        granted.refused.is_empty(),
        "the cell refused the fixture grant: {:?}",
        granted.refused
    );
    assert_eq!(granted.verified.len(), 1, "{granted:?}");
    let grant = granted.verified[0].clone();
    assert_eq!(grant.cell(), CELL);
    assert_eq!(grant.strategy().as_str(), "microprice-guard");

    let mut policy =
        PolicyDownlink::from_source_trusted(CELL, root.clone(), Box::new(scripted(frames)));
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
    // R12-REFLEX-5030: beside the plan, the opportunity manifest naming its
    // entry rule by the definition the node will compile, so the node
    // activates the slice strategy rather than reading it as unnamed.
    // Mutation: drop the slot from the fixture payload.
    let opportunities = payload
        .opportunity_definitions
        .value()
        .expect("the opportunity slot is produced");
    assert_eq!(
        opportunities.opportunities,
        qip_cli::event_fabric::grant::plan_opportunities(
            &String::from_utf8(plan_bytes.clone()).expect("the plan is text")
        )
        .expect("the plan digests"),
    );
    assert!(
        opportunities
            .opportunities
            .contains_key("microprice-guard/microprice-defined"),
        "{opportunities:?}"
    );
    let manifest = payload
        .capital_grants
        .value()
        .expect("the grants slot is produced");
    assert_eq!(manifest.live_grants, vec![grant.signature().to_string()]);
    grant
}

// --- the grant is one the cell accepts ------------------------------------

/// What the command publishes must pass the cell's own checks: the capital
/// downlink and the policy downlink, built from a `FrameSource` exactly as
/// the node's control consumer builds them, verifying against the trust
/// bundle the cell holds. A grant that only verified against a copy of the
/// check written in this test would prove the copy.
///
/// The grant is signed under the `envelope` key and the payload under the
/// `policy` key, each named as a fixture key — which the cell's own refusal
/// proves: a bundle root refuses an HMAC by construction.
///
/// Mutation: sign the payload under the envelope signer in `grant::sign` —
/// `sign_artefact` refuses the class mismatch and the run fails.
#[test]
fn a_fixture_grant_verifies_through_the_cells_own_verify_path() {
    let (environment, seen, root) = wired("verify");
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

    let grant = accepted_by_the_cell(&frames, &root);
    // Signed under the fixture envelope key, by name.
    let signature = ArtefactSignature::parse(grant.signature()).expect("an asymmetric signature");
    assert_eq!(signature.key_version(), ENVELOPE_VERSION);
}

// --- loopback only ----------------------------------------------------------

/// A peer that is not loopback is refused before anything is read or sent.
/// The fixture reaching a regional fabric would make it a grant path, which
/// is the failure ADR 0100 names; and no key, signer, deployment or link
/// variable is even looked up, so a run that could not have gone anywhere
/// safe never touches key material.
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
        let lookups = seen.lookups();
        for untouched in [
            format!("{KEY_VARIABLE}_FILE"),
            LOCAL_SIGNING_KEYS_VARIABLE.to_string(),
            ARTEFACT_SIGNER_VARIABLE.to_string(),
            DEPLOYMENT_VARIABLE.to_string(),
            TLS_PRIVATE_KEY_VARIABLE.to_string(),
        ] {
            assert!(
                !lookups.contains(&untouched),
                "{untouched} was looked up for a run refusing {peer}"
            );
        }
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

/// Every asymmetric key the command signs under must be named as a fixture
/// key, checked before it signs — the asymmetric form of the shared key's
/// label, and for the same reason: a production envelope key configured
/// here by mistake must never mint a grant. For Cloud KMS the label is the
/// `cryptoKeys/` name, not anywhere in the path: a production key in a key
/// ring someone named `qip-fixture-…` is still a production key.
///
/// The KMS case is refused before anything is sent to Google: no TLS roots
/// for Google are configured, so a run that got past the label would be
/// refused for them instead, naming the wrong thing.
///
/// Mutations: make `is_fixture_key_version` return `true` and the first
/// negative predicate assertion fails; leave the predicate alone and make
/// `require_fixture_key` admit everything, and the unlabelled local key
/// publishes, so its `expect_err` fires.
#[test]
fn a_signing_key_not_named_as_a_fixture_key_is_refused_before_it_signs() {
    // The predicate, on the names a deployment would actually hold.
    assert!(grant::is_fixture_key_version(ENVELOPE_VERSION));
    assert!(grant::is_fixture_key_version(
        "projects/p/locations/l/keyRings/r/cryptoKeys/qip-fixture-envelope/cryptoKeyVersions/1"
    ));
    assert!(!grant::is_fixture_key_version("envelope/1"));
    assert!(!grant::is_fixture_key_version(
        "projects/p/locations/l/keyRings/r/cryptoKeys/envelope/cryptoKeyVersions/1"
    ));
    assert!(!grant::is_fixture_key_version(
        "projects/p/locations/l/keyRings/qip-fixture-ring/cryptoKeys/envelope/cryptoKeyVersions/1"
    ));

    // A local key the bundle trusts, but not named as a fixture key: the
    // signature would verify, so only the label can refuse it.
    let mut wiring = Wiring::new("label-local-key");
    let production = ClassKey::generate("envelope/1");
    let policy = ClassKey::generate(POLICY_VERSION);
    let bundle = TrustBundle::new(
        vec![
            production.trusted(ArtefactClass::Envelope),
            policy.trusted(ArtefactClass::Policy),
        ],
        BTreeSet::new(),
    )
    .expect("a bundle");
    wiring.write_bundle(&bundle);
    wiring.write_local_keys(&[
        ("envelope", &production.version, &production.pkcs8),
        ("policy", &policy.version, &policy.pkcs8),
    ]);
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("an unlabelled envelope key signed a fixture grant");
    assert!(
        error
            .message()
            .contains("\"envelope/1\" is not named as a fixture key"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.published().is_empty());

    // A KMS envelope version in a production key: refused before a signer
    // is built.
    let mut wiring = Wiring::new("label-kms-key");
    let versions = wiring.dir.join("kms-key-versions.json");
    std::fs::write(
        &versions,
        serde_json::to_vec(&serde_json::json!({
            "envelope": "projects/p/locations/l/keyRings/r/cryptoKeys/envelope/cryptoKeyVersions/1",
            "policy": "projects/p/locations/l/keyRings/r/cryptoKeys/qip-fixture-policy/cryptoKeyVersions/1",
        }))
        .expect("serialises"),
    )
    .expect("the versions file");
    wiring.set(ARTEFACT_SIGNER_VARIABLE, "kms");
    wiring.unset(LOCAL_SIGNING_KEYS_VARIABLE);
    wiring.set(KMS_KEY_VERSIONS_VARIABLE, &versions.display().to_string());
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("a production KMS envelope key was accepted");
    assert!(
        error.message().contains("is not named as a fixture key"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());
}

// --- the signer --------------------------------------------------------------

/// The shared HMAC key signs only in a `local` deployment. There it still
/// works, verified by a shared-key cell, so a workstation mid-migration is
/// not broken; anywhere else it is refused, naming the asymmetric signer to
/// use — a key that verifies can also sign, so every holder could mint the
/// grant.
///
/// Mutation: delete the `permits_reproducible_keys` refusal in the shared
/// arm of `grant::read_signer` — the `dev` run publishes and the
/// `expect_err` fires.
#[test]
fn the_shared_fixture_key_signs_only_in_a_local_deployment() {
    // Premise: in `local` the shared key's grant is accepted by a cell
    // holding the same key.
    let mut wiring = Wiring::new("shared-local");
    let key = wiring.use_shared_key();
    let (environment, seen) = wiring.environment();
    grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("the shared key signs on a workstation");
    let frames = published_frames(&seen);
    accepted_by_the_cell(&frames, &TrustRoot::shared_key(&key).expect("a key"));
    // And a bundle cell refuses it: an HMAC is not an asymmetric signature.
    let bundle_cell = Wiring::new("shared-local-bundle").root();
    let mut capital =
        CapitalDownlink::from_source_trusted(CELL, bundle_cell, 16, Box::new(scripted(&frames)))
            .expect("a capital downlink");
    let polled = capital
        .poll(now().saturating_add(Duration::from_secs(1)))
        .expect("polls");
    assert!(
        polled.verified.is_empty(),
        "a bundle cell accepted an HMAC grant"
    );

    // In `dev`, over a link that is otherwise acceptable, it is refused.
    let ca = certs::authority("qip grant test pool");
    let client = certs::leaf(&ca, 3, "release-controller.test.qip.internal");
    let mut wiring = Wiring::new("shared-dev");
    wiring.use_shared_key();
    wiring.use_tls(&ca, &client);
    wiring.set(DEPLOYMENT_VARIABLE, "dev");
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("the shared key signed in dev");
    assert!(
        error
            .message()
            .contains("The shared key is admitted only where QIP_DEPLOYMENT=local"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());
    assert!(seen.published().is_empty());
}

/// `QIP_DEPLOYMENT=local` on a machine that can see it is deployed is
/// refused before any key, token or link is read: `local` is what admits
/// the plaintext link and the shared key, and on Cloud Run it would be one
/// unreviewed environment edit away.
///
/// Mutation: pass `&DeploymentMarkers::none()` to `parse_observed` in
/// `grant::run` and the Cloud Run run publishes; the `expect_err` fires.
#[test]
fn a_local_deployment_is_refused_where_the_process_can_see_it_is_deployed() {
    // Premise: the same local wiring publishes with no marker.
    let (environment, seen, _) = wired("markers-premise");
    grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("a local run with no marker publishes");
    assert!(!seen.published().is_empty());

    let mut wiring = Wiring::new("markers-cloud-run");
    wiring.set("K_SERVICE", "qip-api");
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("local was admitted on Cloud Run");
    assert!(
        error.message().contains("K_SERVICE"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());
    assert!(seen.published().is_empty());
}

/// An asymmetric signer is proven against the trust bundle before anything
/// is signed, and the API's other refusals hold: a local signer outside a
/// `local` deployment, and the shared key mounted beside an asymmetric
/// signer. Each is refused with nothing published.
///
/// The first is the one that would otherwise ship: a bundle that lists the
/// right version under the wrong public key would make every cell refuse
/// the grant, and the command would have reported it published.
///
/// Mutation: drop the `prove_signer` call in `grant::read_signer`'s `take` —
/// the mismatched run publishes and its `expect_err` fires.
#[test]
fn a_signer_the_trust_bundle_does_not_verify_is_refused_and_nothing_is_published() {
    // The bundle lists the policy version under somebody else's key.
    let mut wiring = Wiring::new("unproven");
    let envelope = ClassKey::generate(ENVELOPE_VERSION);
    let policy = ClassKey::generate(POLICY_VERSION);
    let impostor = ClassKey::generate(POLICY_VERSION);
    let bundle = TrustBundle::new(
        vec![
            envelope.trusted(ArtefactClass::Envelope),
            impostor.trusted(ArtefactClass::Policy),
        ],
        BTreeSet::new(),
    )
    .expect("a bundle");
    wiring.write_bundle(&bundle);
    wiring.write_local_keys(&[
        ("envelope", &envelope.version, &envelope.pkcs8),
        ("policy", &policy.version, &policy.pkcs8),
    ]);
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("a signer the bundle does not verify was used");
    assert!(
        error.message().contains(
            "the policy signer (qip-fixture-policy/1) does not verify under the trust bundle"
        ),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());
    assert!(seen.published().is_empty());

    // A local signer in `dev`, over an acceptable link.
    let ca = certs::authority("qip grant test pool");
    let client = certs::leaf(&ca, 3, "release-controller.test.qip.internal");
    let mut wiring = Wiring::new("local-in-dev");
    wiring.use_tls(&ca, &client);
    wiring.set(DEPLOYMENT_VARIABLE, "dev");
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("a local signer signed in dev");
    assert!(
        error
            .message()
            .contains("QIP_ARTEFACT_SIGNER=local in the dev deployment"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.published().is_empty());

    // The shared key mounted beside an asymmetric signer.
    let mut wiring = Wiring::new("both-keys");
    let key_path = wiring.dir.join("fixture.key");
    std::fs::write(&key_path, key_material("both-keys")).expect("key file");
    wiring.set(
        &format!("{KEY_VARIABLE}_FILE"),
        &key_path.display().to_string(),
    );
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("an asymmetric run held the shared key too");
    assert!(
        error.message().contains("are both configured"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.published().is_empty());
}

// --- the link ------------------------------------------------------------------

/// The link to the broker is mutual TLS unless the deployment is `local`
/// and `QIP_TLS_MODE=plaintext` says so (ADR 0112). Plaintext outside
/// `local` is refused before any key is read; an absent deployment is
/// refused, not defaulted; omitting the mode on a workstation is mutual TLS,
/// not plaintext; and a configured certificate reaches the connector as a
/// mutual-TLS link.
///
/// Mutation: in `grant::run`, replace `LinkSecurity::from_lookup(…)` with
/// `LinkSecurity::plaintext_for_tests()` — the `dev` plaintext run is
/// refused for its local signer instead, and the TLS run reaches the
/// connector as plaintext.
#[test]
fn the_fabric_link_is_mutual_tls_unless_a_local_deployment_chooses_plaintext() {
    // Premise: local + plaintext is admitted, and the connector is told so.
    let (environment, seen, _) = wired("link-plaintext");
    grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("a workstation may choose plaintext");
    assert_eq!(seen.links(), vec![false]);

    let mut wiring = Wiring::new("link-dev-plaintext");
    wiring.set(DEPLOYMENT_VARIABLE, "dev");
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("plaintext in dev");
    assert!(
        error
            .message()
            .contains("QIP_TLS_MODE=plaintext in the dev deployment"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());
    assert!(
        !seen
            .lookups()
            .contains(&LOCAL_SIGNING_KEYS_VARIABLE.to_string()),
        "a key was read for a run whose link was refused"
    );

    let mut wiring = Wiring::new("link-no-deployment");
    wiring.unset(DEPLOYMENT_VARIABLE);
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("a run that named no deployment");
    assert!(
        error.message().contains("QIP_DEPLOYMENT is not set"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());

    let mut wiring = Wiring::new("link-no-mode");
    wiring.unset(TLS_MODE_VARIABLE);
    let (environment, seen) = wiring.environment();
    let error = grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect_err("an omitted mode came up in plaintext");
    assert!(
        error
            .message()
            .contains("There is no fallback to plaintext"),
        "refused for the wrong reason: {}",
        error.message()
    );
    assert!(seen.connects().is_empty());

    let ca = certs::authority("qip grant test pool");
    let client = certs::leaf(&ca, 3, "release-controller.test.qip.internal");
    let mut wiring = Wiring::new("link-tls");
    wiring.use_tls(&ca, &client);
    let (environment, seen) = wiring.environment();
    grant::run(
        &fixture_arguments(&committed_fixture(), LOOPBACK),
        &environment,
    )
    .expect("a configured certificate is a mutual-TLS link");
    assert_eq!(seen.links(), vec![true]);
    assert_eq!(seen.published().len(), 1);
}

/// A broker on the far side of a real mutual-TLS listener: the protocol's
/// own codec, the recording broker's answers.
struct TlsBroker {
    broker: Mutex<RecordingBroker>,
    tokens: Mutex<Vec<String>>,
}

impl Handler for TlsBroker {
    fn handle(&self, request: &ServerRequest) -> ServerResponse {
        let Some(route) = Route::from_path(&request.path) else {
            return ServerResponse::text(404, "no such route");
        };
        if let Some(token) = request.header(auth::HEADER) {
            self.tokens.lock().expect("lock").push(token.to_string());
        }
        let answer = decode_request(route, &request.body)
            .and_then(|decoded| self.broker.lock().expect("lock").answer(decoded))
            .and_then(|response| encode_response(&response));
        match answer {
            Ok(body) => ServerResponse::new(200, "application/json", body),
            Err(error) => ServerResponse::text(500, error.message().to_string()),
        }
    }
}

/// The whole run through the production connector — `HttpTransport::over`,
/// as `qip-edge-node`'s outbox builds it — against a listener that requires
/// a client certificate: the command presents its workload certificate,
/// verifies the broker's against the loopback address it checked, publishes
/// as the release controller, and the cell accepts what arrived.
///
/// Mutation: make `grant::fabric_transport` ignore the link and build
/// `HttpTransport::new("http://…")` — the plaintext request never reaches
/// the TLS broker's handler and the run fails.
#[test]
fn a_fixture_grant_reaches_a_mutual_tls_broker_through_the_production_connector() {
    let ca = certs::authority("qip grant test pool");
    // Valid for the address the command dials, because it dials the checked
    // address and never the name.
    let server = certs::leaf_with_ip(&ca, 2, "localhost", [127, 0, 0, 1]);
    let client = certs::leaf(&ca, 3, "release-controller.test.qip.internal");

    let published = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(TlsBroker {
        broker: Mutex::new(RecordingBroker {
            published: Arc::clone(&published),
            written: 0,
        }),
        tokens: Mutex::new(Vec::new()),
    });
    let listener = Server::bind_tls(
        "127.0.0.1:0",
        Arc::clone(&handler) as Arc<dyn Handler>,
        ServerLimits::default(),
        TlsServer {
            config: WorkloadIdentity::from_pem(
                server.certificate_pem().as_bytes(),
                server.key_pem().as_bytes(),
                ca.certificate_pem().as_bytes(),
            )
            .expect("the broker's identity")
            .server_config()
            .expect("a server config"),
            peers: PeerNames::default(),
        },
    )
    .expect("binds an ephemeral port");
    let address = listener.local_address().expect("an address");
    let stop = listener.shutdown_handle();
    let thread = std::thread::spawn(move || {
        let _ = listener.serve();
    });

    let mut wiring = Wiring::new("tls-broker");
    wiring.use_tls(&ca, &client);
    let root = wiring.root();
    let seen = Seen::default();
    // The production link connector: `Environment::new` installs it.
    let environment = Environment::new(
        wiring.lookup(&seen),
        Arc::new(ManualClock::new(now())),
        Arc::new(RecordingSleeper::new()),
        Box::new(|_, _| panic!("the grant opened a link without saying how it is secured")),
    );
    let outcome = grant::run(
        &fixture_arguments(&committed_fixture(), &address),
        &environment,
    );

    stop.store(true, Ordering::SeqCst);
    let _ = std::net::TcpStream::connect(&address);
    thread.join().expect("the broker thread");
    let outcome = outcome.expect("the grant is published over mutual TLS");
    assert!(
        outcome.lines.iter().any(|line| line.contains("mutual TLS")),
        "{:?}",
        outcome.lines
    );

    let batches: Vec<String> = published
        .lock()
        .expect("lock")
        .iter()
        .map(|published| published.batch_hex.clone())
        .collect();
    assert_eq!(batches.len(), 1, "one batch reached the TLS broker");
    assert!(
        handler
            .tokens
            .lock()
            .expect("lock")
            .iter()
            .all(|header| *header == format!("Bearer {}", token("tls-broker"))),
        "a call carried another token"
    );
    accepted_by_the_cell(&frames_of(&batches), &root);
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
